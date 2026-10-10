use crate::emulator::context::Context;
use crate::emulator::thread::{
    block_current_thread, exit_current_thread, exit_process, ThreadAction,
};
use crate::os::syscalls::{
    fcntl, futex, ioctl, linux, mman, mqueue, prctl, resource, sched, signal, socket, stat, time,
    timer, uio, unistd, utsname,
};
use unicorn_engine::{RegisterARM, Unicorn};

pub fn hook_syscall(unicorn: &mut Unicorn<'_, Context>, int_no: u32) {
    // Guest C code expects ARM callee-saved registers to survive OSAL stubs and
    // Linux syscalls. Some host stub/syscall paths use R4-R10 as scratch space, so
    // preserve them unless the handler is rt_sigreturn, which intentionally restores
    // a complete saved CPU context.
    let saved_callee_saved = save_callee_saved_regs(unicorn);

    // A stubbed function entry was patched to `svc #0`. When it executes the intr
    // hook fires with PC already advanced past the svc, so the entry address is
    // PC - 4. If this interrupt came from one of our stubs, run its handler, return
    // the value in R0 and jump back to the caller (PC = LR), skipping the original
    // body (which never runs). Otherwise fall through to a real syscall.
    let pc = unicorn.reg_read(RegisterARM::PC).unwrap() as u32;
    let stub = {
        let map = unicorn.get_data().code_stubs.lock().unwrap();
        map.get(&pc.wrapping_sub(4)).copied()
    };
    if let Some(stub) = stub {
        let tid = unicorn.get_data().thread_id();
        log::trace!(
            "{:#x}: [{}] [{} HOOK] {}() [IN]",
            pc,
            tid,
            stub.lib,
            stub.name
        );
        crate::os::code_stub::set_current_stub_name(stub.name);
        let res = (stub.handler)(unicorn);
        crate::os::code_stub::set_current_stub_name("");
        restore_callee_saved_regs(unicorn, saved_callee_saved);
        log::trace!(
            "{:#x}: [{}] [{} HOOK] {}() => {}",
            pc,
            tid,
            stub.lib,
            stub.name,
            res
        );
        unicorn.reg_write(RegisterARM::R0, res as u64).unwrap();
        unicorn
            .reg_write(RegisterARM::PC, unicorn.reg_read(RegisterARM::LR).unwrap())
            .unwrap();
        return;
    }

    // Statically linked navigation processes (PROCNAV) reach the LX monitor
    // through generated shims instead of a shared library:
    //
    //     push {r4, lr}
    //     svc  #<call id>     <- the id is the instruction immediate
    //     pop  {r4, pc}
    //
    // Linux code (glibc and our own stubs) always executes `svc #0` and keeps the
    // number in R7, so a non-zero immediate with `push {r4, lr}` in front of it
    // identifies these calls without touching normal syscall handling. PROCNAV
    // exits with code 1 as soon as one of them is unanswered.
    if let Some(call) = lx_monitor_call(unicorn, pc) {
        let args = [
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ];
        // Whatever the previous call handed back, PROCNAV has written through it
        // by now; dumping it here is what reveals the expected result structure.
        let scratch = LX_SCRATCH.lock().unwrap().unwrap_or(0);
        if scratch != 0 {
            let mut page = [0u8; 64];
            if unicorn.mem_read(scratch as u64, &mut page).is_ok() {
                log::info!(
                    "[{}] LX scratch at {:#x} before call #{}: {}",
                    unicorn.get_data().thread_id(),
                    scratch,
                    call,
                    page.iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            }
        }
        log::info!(
            "[{}] LX monitor call #{} args {:#x} {:#x} {:#x} {:#x} (pc={:#x})",
            unicorn.get_data().thread_id(),
            call,
            args[0],
            args[1],
            args[2],
            args[3],
            pc
        );
        // Returning 0 makes PROCNAV exit with code 1 right after the last call and
        // returning 1 makes it write through the result (WRITE_UNMAPPED at
        // 0x5ea818), so the result has to be a usable address.
        let scratch = lx_scratch_page(unicorn);
        unicorn.reg_write(RegisterARM::R0, scratch as u64).unwrap();
        restore_callee_saved_regs(unicorn, saved_callee_saved);
        return;
    }

    // PROCNAV is statically linked, so none of the library-level traces cover it
    // and it looks silent while it is really sitting in a syscall we do not log.
    // Trace every syscall of that one process instead of guessing.
    if unicorn.get_data().elf_path.contains("/navbin/") {
        log::info!(
            "[{}] NAVBIN syscall #{} args {:#x} {:#x} {:#x} (pc={:#x})",
            unicorn.get_data().thread_id(),
            unicorn.get_syscall_number(),
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            pc
        );
    }

    // table:
    // - https://marcin.juszkiewicz.com.pl/download/tables/syscalls.html
    // - https://github.com/qilingframework/qiling/blob/master/qiling/os/linux/map_syscall.py
    //
    // sample implementations:
    // - https://github.com/zeropointdynamics/zelos/blob/master/src/zelos/ext/platforms/linux/syscalls/syscalls.py
    // - https://github.com/qilingframework/qiling/tree/master/qiling/os/posix/syscall
    let syscall = unicorn.get_syscall_number();
    let res = match syscall {
        1 => unistd::exit(unicorn, unicorn.get_u32_arg(0)),
        3 => unistd::read(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        4 => unistd::write(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        5 => fcntl::open(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        6 => unistd::close(unicorn, unicorn.get_u32_arg(0)),
        9 => unistd::link(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        10 => unistd::unlink(unicorn, unicorn.get_u32_arg(0)),
        19 => unistd::lseek(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        20 => unistd::get_pid(unicorn),
        33 => unistd::access(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        39 => stat::mkdir(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        41 => unistd::dup(unicorn, unicorn.get_u32_arg(0)),
        63 => unistd::dup2(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        45 => unistd::brk(unicorn, unicorn.get_u32_arg(0)),
        54 => ioctl::ioctl(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        60 => stat::umask(unicorn, unicorn.get_u32_arg(0)),
        78 => time::gettimeofday(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        83 => unistd::symlink(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        88 => unistd::reboot(unicorn, unicorn.get_u32_arg(0)),
        90 => mman::mmap(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
            unicorn.get_u32_arg(5),
        ),
        91 => mman::munmap(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        93 => unistd::ftruncate(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        97 => resource::set_priority(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        99 => stat::statfs(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        120 => sched::clone(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
        ),
        122 => utsname::uname(unicorn, unicorn.get_u32_arg(0)),
        125 => mman::mprotect(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        140 => unistd::_llseek(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
        ),
        141 => unistd::getdents(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        146 => uio::writev(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        155 => sched::sched_getparam(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        156 => sched::sched_setscheduler(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        157 => sched::sched_getscheduler(unicorn, unicorn.get_u32_arg(0)),
        159 => sched::sched_get_priority_max(unicorn, unicorn.get_u32_arg(0)),
        160 => sched::sched_get_priority_min(unicorn, unicorn.get_u32_arg(0)),
        162 => time::nanosleep(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        172 => prctl::prctl(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
        ),
        173 => signal::rt_sigreturn(unicorn),
        174 => signal::rt_sigaction(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        175 => signal::rt_sigprocmask(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        177 => signal::rt_sigtimedwait(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        186 => signal::sigaltstack(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        191 => resource::ugetrlimit(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        192 => mman::mmap2(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
            unicorn.get_u32_arg(5),
        ),
        195 => stat::stat64(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        196 => stat::lstat64(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        197 => stat::fstat64(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        217 => unistd::getdents64(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        219 => mman::mincore(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        221 => fcntl::fcntl64(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        224 => unistd::get_tid(unicorn),
        240 => futex::futex(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
            unicorn.get_u32_arg(5),
        ),
        238 => signal::tkill(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        248 => unistd::exit_group(unicorn, unicorn.get_u32_arg(0)),
        256 => unistd::set_tid_address(unicorn, unicorn.get_u32_arg(0)),
        263 => time::clock_gettime(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        268 => signal::tgkill(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        257 => timer::timer_create(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        258 => timer::timer_settime(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        259 => timer::timer_gettime(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        260 => timer::timer_getoverrun(unicorn, unicorn.get_u32_arg(0)),
        261 => timer::timer_delete(unicorn, unicorn.get_u32_arg(0)),
        274 => mqueue::mq_open(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        275 => mqueue::mq_unlink(unicorn, unicorn.get_u32_arg(0)),
        276 => mqueue::mq_timedsend(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
        ),
        277 => mqueue::mq_timedreceive(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
        ),
        278 => mqueue::mq_notify(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        279 => mqueue::mq_getsetattr(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        281 => socket::socket(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        282 => socket::bind(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        283 => socket::connect(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        284 => socket::listen(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        285 => socket::accept(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        286 => socket::getsockname(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        287 => socket::getpeername(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        288 => socket::socketpair(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        289 => socket::send(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        290 => socket::sendto(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
            unicorn.get_u32_arg(5),
        ),
        291 => socket::recv(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        292 => socket::recvfrom(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
            unicorn.get_u32_arg(5),
        ),
        293 => socket::shutdown(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        294 => socket::setsockopt(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
        ),
        295 => socket::getsockopt(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
        ),
        296 => socket::sendmsg(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        297 => socket::recvmsg(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        220 => {
            log::debug!(
                "{:#x}: [{}] no-op ARM syscall #220, args: {:#x}, {:#x}, {:#x}, ...",
                unicorn.reg_read(RegisterARM::PC).unwrap(),
                unicorn.get_data().thread_id(),
                unicorn.get_u32_arg(0),
                unicorn.get_u32_arg(1),
                unicorn.get_u32_arg(2),
            );
            0
        }
        316 => unistd::inotify_init(unicorn, 0),
        317 => unistd::inotify_add_watch(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        318 => unistd::inotify_rm_watch(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        322 => fcntl::openat(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        327 => stat::fstatat64(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        338 => futex::set_robust_list(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        983045 => linux::set_tls(unicorn, unicorn.get_u32_arg(0)),
        x => {
            log::error!(
                "{:#x}: [{}] not implemented syscall #{} (int {}), args: {:#x}, {:#x}, {:#x}, ...",
                unicorn.reg_read(RegisterARM::PC).unwrap(),
                unicorn.get_data().thread_id(),
                x,
                int_no,
                unicorn.get_u32_arg(0),
                unicorn.get_u32_arg(1),
                unicorn.get_u32_arg(2),
            );
            -38i32 as u32 // ENOSYS
        }
    };
    unicorn.set_u32_result(res);

    if syscall != 173 {
        restore_callee_saved_regs(unicorn, saved_callee_saved);
    }

    // a syscall handler may have requested a scheduling action
    match unicorn.get_data().take_action() {
        ThreadAction::None => {}
        ThreadAction::Block(reason) => block_current_thread(unicorn, reason),
        ThreadAction::ExitThread(code) => exit_current_thread(unicorn, code),
        ThreadAction::ExitProcess(code) => exit_process(unicorn, code),
    }
}

/// Guest page handed back as the result of an LX monitor call, so the caller has
/// somewhere to write what it expects the monitor to fill in.
static LX_SCRATCH: std::sync::LazyLock<std::sync::Mutex<Option<u32>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

fn lx_scratch_page(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    if let Some(page) = *LX_SCRATCH.lock().unwrap() {
        return page;
    }
    let mmu_arc = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    let page = mmu_arc.lock().unwrap().heap_alloc(
        unicorn,
        0x1000,
        unicorn_engine::unicorn_const::Prot::READ | unicorn_engine::unicorn_const::Prot::WRITE,
        "[lx-monitor-scratch]",
    );
    log::info!("LX monitor scratch page at {:#x}", page);
    *LX_SCRATCH.lock().unwrap() = Some(page);
    page
}

/// Call id of the LX monitor shim whose `svc` instruction ends just before `pc`,
/// or `None` when the interrupt did not come from such a shim.
fn lx_monitor_call(unicorn: &mut Unicorn<'_, Context>, pc: u32) -> Option<u32> {
    let mut words = [0u8; 8];
    unicorn.mem_read((pc - 8) as u64, &mut words).ok()?;
    let push = u32::from_le_bytes(words[0..4].try_into().ok()?);
    let svc = u32::from_le_bytes(words[4..8].try_into().ok()?);
    // ARM: push {r4, lr} = 0xE92D4010, svc #imm24 = 0xEF000000 | imm
    if push == 0xE92D_4010 && svc >> 24 == 0xEF {
        let id = svc & 0x00FF_FFFF;
        if id != 0 {
            return Some(id);
        }
    }
    None
}

fn save_callee_saved_regs(unicorn: &mut Unicorn<'_, Context>) -> [u64; 7] {
    [
        unicorn.reg_read(RegisterARM::R4).unwrap_or(0),
        unicorn.reg_read(RegisterARM::R5).unwrap_or(0),
        unicorn.reg_read(RegisterARM::R6).unwrap_or(0),
        unicorn.reg_read(RegisterARM::R7).unwrap_or(0),
        unicorn.reg_read(RegisterARM::R8).unwrap_or(0),
        unicorn.reg_read(RegisterARM::R9).unwrap_or(0),
        unicorn.reg_read(RegisterARM::R10).unwrap_or(0),
    ]
}

fn restore_callee_saved_regs(unicorn: &mut Unicorn<'_, Context>, values: [u64; 7]) {
    for (register, value) in [
        (RegisterARM::R4, values[0]),
        (RegisterARM::R5, values[1]),
        (RegisterARM::R6, values[2]),
        (RegisterARM::R7, values[3]),
        (RegisterARM::R8, values[4]),
        (RegisterARM::R9, values[5]),
        (RegisterARM::R10, values[6]),
    ] {
        let _ = unicorn.reg_write(register, value);
    }
}

trait Args {
    fn get_syscall_number(&self) -> u32;
    fn get_u32_arg(&self, num: i32) -> u32;
    fn set_u32_result(&mut self, res: u32);
}

impl<'a> Args for Unicorn<'a, Context> {
    fn get_syscall_number(&self) -> u32 {
        self.reg_read_i32(RegisterARM::R7).unwrap() as u32
    }

    fn get_u32_arg(&self, num: i32) -> u32 {
        match num {
            0 => self.reg_read_i32(RegisterARM::R0).unwrap() as u32,
            1 => self.reg_read_i32(RegisterARM::R1).unwrap() as u32,
            2 => self.reg_read_i32(RegisterARM::R2).unwrap() as u32,
            3 => self.reg_read_i32(RegisterARM::R3).unwrap() as u32,
            4 => self.reg_read_i32(RegisterARM::R4).unwrap() as u32,
            5 => self.reg_read_i32(RegisterARM::R5).unwrap() as u32,
            6 => self.reg_read_i32(RegisterARM::R6).unwrap() as u32,
            _ => panic!("wrong argument number"),
        }
    }

    fn set_u32_result(&mut self, res: u32) {
        self.reg_write(RegisterARM::R0 as i32, res as u64).unwrap();
    }
}
