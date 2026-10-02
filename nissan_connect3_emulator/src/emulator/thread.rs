use crate::emulator::context::Context;
use crate::emulator::memory_map::{GET_TLS_ADDR, MQ_NOTIFY_EXIT_STUB};
use crate::emulator::print::{disasm, print_mmu, print_stack};
use std::time::Instant;
use unicorn_engine::unicorn_const::{MemType, Prot};
use unicorn_engine::{RegisterARM, Unicorn};
use unicorn_engine::Context as CpuContext;

/// Why a guest thread is blocked (waiting to be woken by the scheduler).
#[derive(Clone, Copy, PartialEq)]
pub enum BlockReason {
    FutexWait { addr: u32, deadline: Option<Instant> },
    SleepUntil(Instant),
    /// waiting in mq_timedsend for a free slot on the queue
    MqSend {
        queue_id: u32,
        msg_ptr: u32,
        msg_len: u32,
        priority: u32,
        deadline: Option<Instant>,
    },
    /// waiting in mq_receive/mq_timedreceive for a message on the queue
    MqReceive {
        queue_id: u32,
        msg_ptr: u32,
        msg_len: u32,
        prio_ptr: u32,
        deadline: Option<Instant>,
    },
}

/// Action requested by a syscall handler; consumed by the syscall hook wrapper.
#[derive(Clone, Copy, PartialEq)]
pub enum ThreadAction {
    None,
    /// block the current guest thread (scheduler switches to another one)
    Block(BlockReason),
    /// terminate the current guest thread
    ExitThread(i32),
    /// terminate the whole process
    ExitProcess(i32),
}

#[derive(Clone, Copy, PartialEq)]
pub enum ThreadStatus {
    Runnable,
    Running,
    Blocked(BlockReason),
    Exited(i32),
}

/// A guest thread: saved CPU state + scheduling status. All guest threads share
/// the single Unicorn VM (one address space); only the CPU context is per-thread.
pub struct GuestThread {
    pub id: u32,
    pub status: ThreadStatus,

    /// saved CPU state (None while Running)
    pub cpu_context: Option<CpuContext>,
    /// PC to start from when switching in (valid when not Running)
    pub pc: u32,
    /// syscall result to install into R0 on the next switch-in (set by the code
    /// that completes a blocked operation, e.g. a woken mq receiver)
    pub pending_result: Option<u32>,
}

pub fn block_current_thread(unicorn: &mut Unicorn<'_, Context>, reason: BlockReason) {
    let (tid, threads) = {
        let data = unicorn.get_data();
        (data.thread_id(), data.threads.clone())
    };
    if let Some(thread) = threads.lock().unwrap().iter_mut().find(|t| t.id == tid) {
        thread.status = ThreadStatus::Blocked(reason);
    }
    unicorn.emu_stop().unwrap();
}

pub fn exit_current_thread(unicorn: &mut Unicorn<'_, Context>, code: i32) {
    let (tid, threads) = {
        let data = unicorn.get_data();
        (data.thread_id(), data.threads.clone())
    };
    if let Some(thread) = threads.lock().unwrap().iter_mut().find(|t| t.id == tid) {
        thread.status = ThreadStatus::Exited(code);
    }
    unicorn.emu_stop().unwrap();
}

pub fn exit_process(unicorn: &mut Unicorn<'_, Context>, code: i32) {
    let (tid, threads) = {
        let data = unicorn.get_data();
        data.set_process_exit_code(code);
        (data.thread_id(), data.threads.clone())
    };
    if let Some(thread) = threads.lock().unwrap().iter_mut().find(|t| t.id == tid) {
        thread.status = ThreadStatus::Exited(code);
    }
    unicorn.emu_stop().unwrap();
}

// If the compiler for the target does not provides some primitives for some
// reasons (e.g. target limitations), the kernel is responsible to assist
// with these operations.
//
// The following is some `kuser` helpers, which can be found here:
// https://elixir.bootlin.com/linux/latest/source/arch/arm/kernel/entry-armv.S#L899
pub fn set_kernel_traps(unicorn: &mut Unicorn<'_, Context>) {
    // allocate memory directly by unicorn (not mmu object)
    unicorn
        .mem_map(
            0xFFFF0000u64,
            0x1000u64,
            Prot::READ | Prot::EXEC,
        )
        .unwrap();

    // memory_barrier
    log::debug!("Set kernel trap: memory_barrier at 0xFFFF0FA0");
    unicorn
        .mem_write(
            0xFFFF0FA0,
            // mcr   p15, 0, r0, c7, c10, 5
            // nop
            // mov   pc, lr
            &[
                0xBA, 0x0F, 0x07, 0xEE, 0x00, 0xF0, 0x20, 0xE3, 0x0E, 0xF0, 0xA0, 0xE1,
            ],
        )
        .unwrap();

    // cmpxchg
    log::debug!("Set kernel trap: cmpxchg at 0xFFFF0FC0");
    unicorn
        .mem_write(
            0xFFFF0FC0,
            // ldr   r3, [r2]
            // subs  r3, r3, r0
            // streq r1, [r2]
            // rsbs  r0, r3, #0
            // mov   pc, lr
            &[
                0x00, 0x30, 0x92, 0xE5, 0x00, 0x30, 0x53, 0xE0, 0x00, 0x10, 0x82, 0x05, 0x00, 0x00,
                0x73, 0xE2, 0x0E, 0xF0, 0xA0, 0xE1,
            ],
        )
        .unwrap();

    // mq-notify exit stub - a SIGEV_THREAD notification thread returns to here
    // after its handler function returns and exits (exit syscall), mirroring
    // how glibc's rt notify thread terminates the spawned thread
    log::debug!(
        "Set kernel trap: mq-notify exit stub at {:#X}",
        MQ_NOTIFY_EXIT_STUB
    );
    unicorn
        .mem_write(
            MQ_NOTIFY_EXIT_STUB as u64,
            // mov   r7, #1     ; exit
            // swi   #0
            &[
                0x01, 0x70, 0xA0, 0xE3, 0x00, 0x00, 0x00, 0xEF,
            ],
        )
        .unwrap();

    // get_tls - reads the per-thread TLS register (C13_C0_3), which is saved and
    // restored with each thread's CPU context by the scheduler
    log::debug!("Set kernel trap: get_tls at {:#X}", GET_TLS_ADDR);
    unicorn
        .mem_write(
            GET_TLS_ADDR as u64,
            // mrc   p15, 0, r0, c13, c0, 3
            // mov   pc, lr
            &[0x70, 0x0F, 0x1D, 0xEE, 0x0E, 0xF0, 0xA0, 0xE1],
        )
        .unwrap();
}

pub fn enable_vfp(unicorn: &mut Unicorn<'_, Context>) {
    // other version? https://github.com/AeonLucid/AndroidNativeEmu/blob/40b89c8095b2aeb4a918ba9a85332afdb3d1b1/src/androidemu/emulator.py

    // https://github.com/qilingframework/qiling/blob/master/qiling/arch/arm.py
    let c1_c0_2 = unicorn.reg_read(RegisterARM::C1_C0_2).unwrap();
    unicorn
        .reg_write(RegisterARM::C1_C0_2, c1_c0_2 | (0b11 << 20) | (0b11 << 22))
        .unwrap();
    unicorn.reg_write(RegisterARM::FPEXC, 1 << 30).unwrap();
}

/// Memory fault callback. Must stay lightweight: it runs inside the emulator
/// while the vCPU is mid-translation, so no memory reads (dumping happens in
/// the scheduler after `emu_start` returns, when the VM is stopped).
fn on_mem_fault(
    unicorn: &mut Unicorn<'_, Context>,
    memtype: MemType,
    address: u64,
    size: usize,
    value: i64,
) -> bool {
    log::error!(
        "{:#x}: [{}] memory fault {:?} - address {:#x}, size: {:#x}, value: {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        memtype,
        address,
        size,
        value
    );

    false
}

pub fn add_mem_fault_hooks(unicorn: &mut Unicorn<'_, Context>) {
    use unicorn_engine::unicorn_const::HookType;
    unicorn
        .add_mem_hook(HookType::MEM_FETCH_UNMAPPED, 1, 0, on_mem_fault)
        .unwrap();
    unicorn
        .add_mem_hook(HookType::MEM_READ_UNMAPPED, 1, 0, on_mem_fault)
        .unwrap();
    unicorn
        .add_mem_hook(HookType::MEM_WRITE_UNMAPPED, 1, 0, on_mem_fault)
        .unwrap();
    unicorn
        .add_mem_hook(HookType::MEM_WRITE_PROT, 1, 0, on_mem_fault)
        .unwrap();
}

/// Dump the full context of the current thread. Only call when the VM is stopped
/// (i.e. not from inside a hook callback).
pub fn dump_context(unicorn: &Unicorn<'_, Context>) {
    println!(
        "PC: {:#10x}, LR (return code): {:#10x}, SP: {:#10x}, FP: {:#10x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.reg_read(RegisterARM::LR).unwrap(),
        unicorn.reg_read(RegisterARM::SP).unwrap(),
        unicorn.reg_read(RegisterARM::FP).unwrap()
    );
    print_mmu(unicorn);

    let pc = unicorn.reg_read(RegisterARM::PC).unwrap() as u32;
    let lr = unicorn.reg_read(RegisterARM::LR).unwrap() as u32;
    if let Some(pc_start) = pc.checked_sub(100) {
        disasm(unicorn, pc_start, 200);
    }
    if let Some(lr_start) = lr.checked_sub(100) {
        disasm(unicorn, lr_start, 200);
    }

    print_stack(unicorn);
}
