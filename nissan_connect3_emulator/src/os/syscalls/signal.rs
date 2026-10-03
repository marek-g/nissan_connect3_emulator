use crate::emulator::context::Context;
use crate::emulator::thread::{BlockReason, ThreadAction};
use std::time::{Duration, Instant};
use unicorn_engine::{RegisterARM, Unicorn};

// ---- ARM rt_sigframe layout (arch/arm/kernel/signal.c + asm/ucontext.h) ----
//
// struct rt_sigframe {
//     struct siginfo  info;      // @ 0x00, 128 bytes
//     struct sigframe sig;        // @ 0x80
// };
// struct sigframe { struct ucontext uc; unsigned long retcode[2]; };
//
// struct ucontext (glibc ABI, kernel pads to match):
//   uc_flags      @ +0x00
//   uc_link       @ +0x04
//   uc_stack      @ +0x08  (12 bytes)
//   uc_mcontext   @ +0x14  (struct sigcontext, 84 bytes)
//   uc_sigmask    @ +0x68  (128 bytes in glibc)
//   uc_regspace[] @ +0xe8
//   sizeof(ucontext) = 0x2e8
const UC_OFFSET: u32 = 0x80;
const UC_MCONTEXT: u32 = UC_OFFSET + 0x14; // 0x94
const UC_SIGMASK: u32 = UC_OFFSET + 0x68; // 0xe8
const RETCODE_OFFSET: u32 = UC_OFFSET + 0x2e8; // 0x368
const RT_SIGFRAME_SIZE: u32 = RETCODE_OFFSET + 8; // 0x370 = 880

// offsets within struct sigcontext (uc_mcontext)
const SC_R0: u32 = 0x0c;
const SC_PC: u32 = 0x48;
const SC_LR: u32 = 0x44;
const SC_SP: u32 = 0x40;
const SC_CPSR: u32 = 0x4c;
const SC_FAULT_ADDR: u32 = 0x50;

// siginfo_t fields (SIGSEGV)
const SI_SIGNO: u32 = 0x00;
const SI_ERRNO: u32 = 0x04;
const SI_CODE: u32 = 0x08;
const SI_ADDR: u32 = 0x0c;

const SIGSEGV: u32 = 11;
// KERN_SIGRETURN_CODE = CONFIG_VECTORS_BASE + 0x500 (vectors at 0xffff0000)
pub const KERN_SIGRETURN_CODE: u32 = 0xffff_0500;

const SA_RESTORER: u32 = 0x0400_0000;
const SA_NODEFER: u32 = 0x4000_0000;
const PSR_F: u32 = 0x1f;

// how many times we will re-deliver SIGSEGV for the same faulting PC before giving up
// (prevents an infinite loop if a handler returns to the faulting instruction)
const MAX_REFALT: u32 = 4;

/// Per-signal disposition, mirroring kernel `struct sigaction` (ARM):
/// sa_handler@0, sa_flags@4, sa_restorer@8, sa_mask(low word)@12.
#[derive(Clone, Copy, Default)]
pub struct SigAction {
    pub handler: u32, // 0 = SIG_DFL, 1 = SIG_IGN, else code address
    pub flags: u32,
    pub restorer: u32,
    pub mask: u32,
}

#[derive(Default)]
pub struct SignalState {
    pub actions: [SigAction; 32],
    /// current blocked signal set (low word) - restored on sigreturn
    pub blocked: u32,
    // re-fault guard
    refault_pc: u32,
    refault_count: u32,
}

impl SignalState {
    pub fn new() -> Self {
        Self::default()
    }
}

fn rd(unicorn: &Unicorn<'_, Context>, addr: u32) -> Option<u32> {
    let mut b = [0u8; 4];
    unicorn.mem_read(addr as u64, &mut b).ok()?;
    Some(u32::from_le_bytes(b))
}

fn wr(unicorn: &mut Unicorn<'_, Context>, addr: u32, v: u32) -> bool {
    unicorn.mem_write(addr as u64, &v.to_le_bytes()).is_ok()
}

pub fn rt_sigaction(
    unicorn: &mut Unicorn<'_, Context>,
    signum: u32,
    action: u32,
    old_action: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigaction(signum = {:#x}, action: {:#x}, old_action: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        signum,
        action,
        old_action,
    );

    let mut res = 0u32;
    if signum < 32 {
        // copy the old action out before touching guest memory (the lock borrows `unicorn`)
        let old = unicorn
            .get_data()
            .sys_calls_state
            .lock()
            .unwrap()
            .signals
            .actions[signum as usize];
        if old_action != 0 {
            // kernel struct sigaction: handler@0, flags@4, restorer@8, mask@12
            wr(unicorn, old_action, old.handler);
            wr(unicorn, old_action + 4, old.flags);
            wr(unicorn, old_action + 8, old.restorer);
            wr(unicorn, old_action + 12, old.mask);
        }
        if action != 0 {
            let handler = rd(unicorn, action).unwrap_or(0);
            let flags = rd(unicorn, action + 4).unwrap_or(0);
            let restorer = rd(unicorn, action + 8).unwrap_or(0);
            let mask = rd(unicorn, action + 12).unwrap_or(0);
            log::info!(
                "rt_sigaction: sig {:#x} -> handler {:#x}, flags {:#x}, restorer {:#x}, mask {:#x}",
                signum,
                handler,
                flags,
                restorer,
                mask
            );
            unicorn
                .get_data()
                .sys_calls_state
                .lock()
                .unwrap()
                .signals
                .actions[signum as usize] = SigAction {
                    handler,
                    flags,
                    restorer,
                    mask,
                };
        }
    } else {
        res = 0xffff_f5e8; // -EINVAL
    }

    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigaction => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

/// rt_sigreturn (#173): restore registers from the rt_sigframe at SP.
pub fn rt_sigreturn(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let sp = unicorn.reg_read(RegisterARM::SP).unwrap() as u32;
    match do_sigreturn_core(unicorn, sp) {
        Ok(r0) => {
            // hand control back to the scheduler so it re-drives from the restored PC
            let _ = unicorn.emu_stop();
            r0
        }
        Err(()) => {
            log::error!("rt_sigreturn: bad frame at sp {:#x}", sp);
            0
        }
    }
}

/// Restore registers from a rt_sigframe whose base is `frame`.
fn do_sigreturn_core(unicorn: &mut Unicorn<'_, Context>, frame: u32) -> Result<u32, ()> {
    if frame & 7 != 0 {
        return Err(());
    }
    let mc = UC_MCONTEXT + frame;
    let mut regs = [0u32; 11]; // r0..r10
    for i in 0..11 {
        regs[i] = rd(unicorn, mc + SC_R0 + i as u32 * 4).ok_or(())?;
    }
    let r11 = rd(unicorn, mc + 0x38).ok_or(())?;
    let r12 = rd(unicorn, mc + 0x3c).ok_or(())?;
    let sp = rd(unicorn, mc + SC_SP).ok_or(())?;
    let lr = rd(unicorn, mc + SC_LR).ok_or(())?;
    let pc = rd(unicorn, mc + SC_PC).ok_or(())?;
    let cpsr = rd(unicorn, mc + SC_CPSR).ok_or(())?;
    let r0 = regs[0];

    for (i, reg) in [
        RegisterARM::R0,
        RegisterARM::R1,
        RegisterARM::R2,
        RegisterARM::R3,
        RegisterARM::R4,
        RegisterARM::R5,
        RegisterARM::R6,
        RegisterARM::R7,
        RegisterARM::R8,
        RegisterARM::R9,
        RegisterARM::R10,
    ]
    .iter()
    .enumerate()
    {
        unicorn.reg_write(*reg as i32, regs[i] as u64).map_err(|_| ())?;
    }
    unicorn
        .reg_write(RegisterARM::FP as i32, r11 as u64)
        .map_err(|_| ())?;
    unicorn
        .reg_write(RegisterARM::IP as i32, r12 as u64)
        .map_err(|_| ())?;
    unicorn
        .reg_write(RegisterARM::SP as i32, sp as u64)
        .map_err(|_| ())?;
    unicorn
        .reg_write(RegisterARM::LR as i32, lr as u64)
        .map_err(|_| ())?;
    unicorn
        .reg_write(RegisterARM::CPSR as i32, cpsr as u64)
        .map_err(|_| ())?;
    unicorn
        .reg_write(RegisterARM::PC as i32, pc as u64)
        .map_err(|_| ())?;

    // restore the blocked signal set from uc_sigmask
    if let Some(mask) = rd(unicorn, UC_SIGMASK + frame) {
        unicorn.get_data().sys_calls_state.lock().unwrap().signals.blocked = mask;
    }
    log::trace!(
        "rt_sigreturn: restored pc={:#x} sp={:#x} lr={:#x} r0={:#x}",
        pc,
        sp,
        lr,
        r0
    );
    Ok(r0)
}

/// Handle a memory fault (called from the scheduler with the VM stopped).
/// Returns true if the fault was consumed (delivered to a handler or a sigreturn),
/// false if the process should die.
pub fn handle_mem_fault(unicorn: &mut Unicorn<'_, Context>, addr: u32, is_fetch: bool) -> bool {
    // execution landing on the kernel sig-return high page means the signal handler
    // returned via `bx lr` (no SA_RESTORER 32-bit path) -> do a sigreturn
    if is_fetch && addr >= KERN_SIGRETURN_CODE && addr < KERN_SIGRETURN_CODE + 0x10 {
        let sp = unicorn.reg_read(RegisterARM::SP).unwrap() as u32;
        return do_sigreturn_core(unicorn, sp).is_ok();
    }

    deliver_sigsegv(unicorn, addr)
}

/// Build an rt_sigframe on the guest stack and jump to the SIGSEGV handler.
fn deliver_sigsegv(unicorn: &mut Unicorn<'_, Context>, fault_addr: u32) -> bool {
    let (handler, flags, restorer, allowed) = {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        let act = state.signals.actions[SIGSEGV as usize];
        if act.handler == 0 || act.handler == 1 {
            // SIG_DFL / SIG_IGN -> default action is to terminate
            log::error!(
                "{:#x}: [{}] unhandled SIGSEGV at addr {:#x} (no handler) -> terminating",
                unicorn.reg_read(RegisterARM::PC).unwrap() as u32,
                unicorn.get_data().inner.thread_id(),
                fault_addr
            );
            return false;
        }
        // re-fault guard: same faulting PC too many times in a row -> give up
        let pc = unicorn.reg_read(RegisterARM::PC).unwrap() as u32;
        if state.signals.refault_pc == pc {
            state.signals.refault_count += 1;
        } else {
            state.signals.refault_pc = pc;
            state.signals.refault_count = 1;
        }
        let allowed = state.signals.refault_count <= MAX_REFALT;
        if allowed {
            // block the signal during handler execution unless SA_NODEFER
            let new_blocked = if act.flags & SA_NODEFER != 0 {
                state.signals.blocked
            } else {
                state.signals.blocked | (1 << SIGSEGV)
            };
            state.signals.blocked = new_blocked;
        }
        (act.handler, act.flags, act.restorer, allowed)
    };

    if !allowed {
        log::error!(
            "SIGSEGV re-faulted at pc {:#x} {} times; terminating",
            unicorn.reg_read(RegisterARM::PC).unwrap(),
            MAX_REFALT
        );
        return false;
    }

    // capture the current register set
    let sp = unicorn.reg_read(RegisterARM::SP).unwrap() as u32;
    let frame = (sp - RT_SIGFRAME_SIZE) & !7;

    // siginfo_t
    let si_code = if fault_addr < 0x1000 { 1 } else { 2 }; // SEGV_MAPERR / SEGV_ACCERR
    wr(unicorn, frame + SI_SIGNO, SIGSEGV);
    wr(unicorn, frame + SI_ERRNO, 0);
    wr(unicorn, frame + SI_CODE, si_code);
    wr(unicorn, frame + SI_ADDR, fault_addr);

    // ucontext header
    wr(unicorn, UC_OFFSET + frame, 0); // uc_flags
    wr(unicorn, UC_OFFSET + 4 + frame, 0); // uc_link
    for i in 0..3u32 {
        wr(unicorn, UC_OFFSET + 8 + i * 4 + frame, 0); // uc_stack (zeroed)
    }

    // uc_mcontext: save registers
    let mc = UC_MCONTEXT + frame;
    // read all registers into locals first (the closure would hold an immutable borrow
    // of `unicorn` and conflict with the mutable writes below)
    let regs = [
        unicorn.reg_read(RegisterARM::R0).unwrap() as u32,
        unicorn.reg_read(RegisterARM::R1).unwrap() as u32,
        unicorn.reg_read(RegisterARM::R2).unwrap() as u32,
        unicorn.reg_read(RegisterARM::R3).unwrap() as u32,
        unicorn.reg_read(RegisterARM::R4).unwrap() as u32,
        unicorn.reg_read(RegisterARM::R5).unwrap() as u32,
        unicorn.reg_read(RegisterARM::R6).unwrap() as u32,
        unicorn.reg_read(RegisterARM::R7).unwrap() as u32,
        unicorn.reg_read(RegisterARM::R8).unwrap() as u32,
        unicorn.reg_read(RegisterARM::R9).unwrap() as u32,
        unicorn.reg_read(RegisterARM::R10).unwrap() as u32,
    ];
    let r11 = unicorn.reg_read(RegisterARM::FP).unwrap() as u32;
    let r12 = unicorn.reg_read(RegisterARM::IP).unwrap() as u32;
    let lr = unicorn.reg_read(RegisterARM::LR).unwrap() as u32;
    let pc = unicorn.reg_read(RegisterARM::PC).unwrap() as u32;
    let cpsr = unicorn.reg_read(RegisterARM::CPSR).unwrap() as u32;

    for (i, v) in regs.iter().enumerate() {
        wr(unicorn, mc + SC_R0 + i as u32 * 4, *v);
    }
    wr(unicorn, mc + 0x38, r11); // fp (r11)
    wr(unicorn, mc + 0x3c, r12); // ip (r12)
    wr(unicorn, mc + SC_SP, sp);
    wr(unicorn, mc + SC_LR, lr);
    wr(unicorn, mc + SC_PC, pc);
    wr(unicorn, mc + SC_CPSR, cpsr);
    wr(unicorn, mc + SC_FAULT_ADDR, fault_addr);

    // uc_sigmask (new blocked set) - low word at UC_SIGMASK, rest zeroed
    let blocked = unicorn.get_data().sys_calls_state.lock().unwrap().signals.blocked;
    wr(unicorn, UC_SIGMASK + frame, blocked);

    // retcode[]
    let lr = if flags & SA_RESTORER != 0 {
        restorer
    } else {
        KERN_SIGRETURN_CODE + 12 // SA_SIGINFO, non-thumb (idx=3)
    };
    wr(unicorn, RETCODE_OFFSET + frame, lr);
    wr(unicorn, RETCODE_OFFSET + 4 + frame, 0);

    // install the handler entry: r0 = signal number, sp = frame, pc = handler, lr = retcode
    unicorn.reg_write(RegisterARM::R0 as i32, SIGSEGV as u64).unwrap();
    unicorn.reg_write(RegisterARM::SP as i32, frame as u64).unwrap();
    unicorn.reg_write(RegisterARM::LR as i32, lr as u64).unwrap();
    unicorn
        .reg_write(RegisterARM::CPSR as i32, (cpsr & !PSR_F) as u64)
        .unwrap();
    unicorn
        .reg_write(RegisterARM::PC as i32, handler as u64)
        .unwrap();

    log::info!(
        "delivering SIGSEGV: fault_addr {:#x} -> handler {:#x} (frame {:#x}, lr {:#x})",
        fault_addr,
        handler,
        frame,
        lr
    );
    true
}

pub fn rt_sigprocmask(
    unicorn: &mut Unicorn<'_, Context>,
    how: u32,
    set: u32,
    old_set: u32,
    sig_set_size: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigprocmask(how: {:#x}, set: {:#x}, old_set: {:#x}, sig_set_size: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        how,
        set,
        old_set,
        sig_set_size,
    );

    let mut res = 0u32;
    let old_blocked = {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        let old = state.signals.blocked;
        match how {
            0 => state.signals.blocked = old | set, // SIG_BLOCK
            1 => state.signals.blocked = old & !set, // SIG_UNBLOCK
            2 => state.signals.blocked = set, // SIG_SETMASK
            _ => res = 0xffff_f5e8,
        }
        old
    };
    if old_set != 0 {
        wr(unicorn, old_set, old_blocked);
    }

    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigprocmask => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

pub fn sigaltstack(unicorn: &mut Unicorn<'_, Context>, ss: u32, old_ss: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sigaltstack(ss: {:#x}, old_ss: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        ss,
        old_ss
    );

    if ss != 0 {
        let mut mem = vec![0u8; 12];
        unicorn.mem_read(ss as u64, &mut mem).unwrap();
        let ss_sp = u32::from_le_bytes(mem[0..4].try_into().unwrap());
        let ss_flags = u32::from_le_bytes(mem[4..8].try_into().unwrap());
        let ss_size = u32::from_le_bytes(mem[8..12].try_into().unwrap());
        log::trace!(
            "ss_sp: {:#x}, ss_flags: {:#x}, ss_size: {:#x}",
            ss_sp,
            ss_flags,
            ss_size
        );
    }

    let res = 0i32 as u32;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] sigaltstack => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}
pub fn rt_sigtimedwait(
    unicorn: &mut Unicorn<'_, Context>,
    set: u32,
    info: u32,
    timeout: u32,
    sig_set_size: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigtimedwait(set: {:#x}, info: {:#x}, timeout: {:#x}, sig_set_size: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        set,
        info,
        timeout,
        sig_set_size,
    );

    // No per-thread pending-signal delivery yet, so a wait for a signal finds
    // none. Rather than return EAGAIN immediately (which makes OSAL's
    // sigtimedwait-based wait loops busy-spin at full emulation speed and starve
    // the cooperative scheduler), block briefly so other threads make progress;
    // the guest then re-polls. This approximates a blocking wait for an idle
    // thread without risking a deadlock from never being woken.
    let res = -11i32 as u32; // EAGAIN
    unicorn.get_data().set_action(ThreadAction::Block(BlockReason::SleepUntil(
        Instant::now() + Duration::from_millis(1),
    )));

    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigtimedwait => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}
