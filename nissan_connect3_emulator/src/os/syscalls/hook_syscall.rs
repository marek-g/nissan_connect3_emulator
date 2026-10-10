use crate::emulator::context::Context;
use crate::emulator::memory_map::MQ_NOTIFY_EXIT_STUB;
use crate::emulator::thread::{
    block_current_thread, exit_current_thread, exit_process, BlockReason, GuestThread,
    ThreadAction, ThreadStatus,
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
        // Who is supposed to answer this request? On the real unit the request is
        // served by the monitor's message pump, so a `svc #6` that nobody wakes is
        // fatal no matter how the resume frame is built. Dump the whole thread list the
        // first time we see one, to see whether a serving thread exists in-process.
        if call == 6 {
            static DUMPED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !DUMPED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                let listing = unicorn
                    .get_data()
                    .threads
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|thread| {
                        let state = match &thread.status {
                            ThreadStatus::Runnable => "runnable".to_string(),
                            ThreadStatus::Running => "running".to_string(),
                            ThreadStatus::Blocked(reason) =>
                                format!("blocked on {:?}", std::mem::discriminant(reason)),
                            ThreadStatus::Exited(code) => format!("exited {}", code),
                        };
                        format!("[{}] {} pc={:#x}", thread.id, state, thread.pc)
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                log::info!("LX: threads when [{}] yields: {}", unicorn.get_data().thread_id(), listing);
            }
        }
        // The `svc` vector belongs to the Triton monitor, so the first service call of a
        // process maps it into this address space; everything below is served by host
        // code until the monitor's own handlers can be dispatched.
        crate::rtos::monitor::attach(unicorn);
        // A monitor service that is handed an entry point has to run it in the
        // caller's context and return the callee's result: only the callee fills the
        // link slot the shim pops next, so skipping the call sends PROCNAV into the
        // NX stack.
        lx_log_handoff(unicorn, args[0], args[2]);
        restore_callee_saved_regs(unicorn, saved_callee_saved);
        if lx_coroutine_yield(unicorn, args) {
            return;
        }
        let scratch = lx_scratch_page(unicorn);
        unicorn.reg_write(RegisterARM::R0, scratch as u64).unwrap();
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

const LX_HANDOFF_STACK_SIZE: u32 = 0x10000;

/// `ldr sp, [pc, #4] ; b <stub> ; nop ; <table>` - point the shared resume stub at a
/// specific save table. The branch is PC-relative, so this must be assembled for the
/// address it will actually run from.
fn lx_trampoline(at: u32, stub: u32, table: u32) -> Vec<u8> {
    let branch =
        0xea00_0000u32 | ((((stub as i32) - (at as i32 + 12)) >> 2) as u32 & 0x00ff_ffff);
    let mut code = Vec::with_capacity(16);
    for word in [0xe59f_f004u32, branch, 0xe320_f000, table] {
        code.extend_from_slice(&word.to_le_bytes());
    }
    code
}

/// Stub that finishes a monitor callback: it stores the callee's result into the
/// saved R0 slot, then restores the caller's registers plus SP and resume address,
/// so the detour through the callback is invisible apart from the result.
static LX_CALL_STUB: std::sync::LazyLock<std::sync::Mutex<Option<u32>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

fn lx_call_stub(unicorn: &mut Unicorn<'_, Context>) -> Option<u32> {
    if let Some(stub) = *LX_CALL_STUB.lock().unwrap() {
        return Some(stub);
    }
    let mmu_arc = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    let addr = mmu_arc.lock().unwrap().heap_alloc(
        unicorn,
        0x20,
        unicorn_engine::unicorn_const::Prot::READ
            | unicorn_engine::unicorn_const::Prot::WRITE
            | unicorn_engine::unicorn_const::Prot::EXEC,
        "[lx-monitor-call-stub]",
    );
    let code: [u8; 16] = [
        0x00, 0x00, 0x8d, 0xe5, // str r0, [sp]
        0x34, 0xc0, 0x8d, 0xe2, // add r12, sp, #0x34
        0xff, 0x4f, 0xbd, 0xe8, // pop {r0-r11, lr}
        0x00, 0xa0, 0xbc, 0xe8, // ldmia r12, {sp, pc}
    ];
    unicorn.mem_write(addr as u64, &code).ok()?;
    log::info!("LX monitor call stub at {:#x}", addr);
    *LX_CALL_STUB.lock().unwrap() = Some(addr);
    Some(addr)
}

/// Log the handoff control block the wrapper builds (`mov r0, sp`) and the frame
/// geometry the entry expects, decoded from the entry's own code, so the resume can be
/// driven from the record instead of from whatever the stack slot happens to hold.
fn lx_log_handoff(unicorn: &mut Unicorn<'_, Context>, block: u32, entry: u32) {
    let mut words = [0u8; 32];
    if unicorn.mem_read(block.wrapping_sub(8) as u64, &mut words).is_ok() {
        let cells: Vec<String> = words
            .chunks(4)
            .map(|c| format!("{:#010x}", u32::from_le_bytes(c.try_into().unwrap())))
            .collect();
        log::info!(
            "LX handoff block {:#x} (r0={:#x}): {}",
            block.wrapping_sub(8),
            block,
            cells.join(" ")
        );
    }

    let mut code = [0u8; 64];
    if unicorn.mem_read(entry as u64, &mut code).is_err() {
        return;
    }
    use capstone::arch::arm::ArchMode;
    use capstone::prelude::*;
    use capstone::{Capstone, Endian};

    let Ok(cs) = Capstone::new().arm().mode(ArchMode::Arm).endian(Endian::Little).build()
    else {
        return;
    };
    let Ok(insns) = cs.disasm_all(&code, entry as u64) else {
        return;
    };
    for insn in insns.iter().take(10) {
        log::info!(
            "  LX entry {:#x}: {} {}",
            insn.address(),
            insn.mnemonic().unwrap_or(""),
            insn.op_str().unwrap_or("")
        );
    }
}

/// The entry point handed to the monitor starts with `cmp r0, #0`, so it is a
/// completion path that reads the service result rather than a function to call with
/// fresh arguments. Resume there in place, reporting success.
fn lx_resume_at_continuation(unicorn: &mut Unicorn<'_, Context>, continuation: u32) -> bool {
    if continuation == 0 {
        return false;
    }
    let mut word = [0u8; 4];
    if unicorn.mem_read(continuation as u64, &mut word).is_err() {
        return false;
    }

    let sp = unicorn.reg_read_i32(RegisterARM::R13).unwrap_or(0) as u32;
    // The body resumes mid-function and its epilogue pops its saved frame from the
    // caller's own stack, so the link it will branch to is already in memory. Check it
    // now: reporting the handoff that cannot return is far cheaper than letting the
    // thread branch into it blind.
    let mut frame = [0u8; 8];
    if let (Ok(_), Ok(_)) = (
        unicorn.mem_read(sp as u64 + 0x10, &mut frame[0..4]),
        unicorn.mem_read(sp as u64 + 0x14, &mut frame[4..8]),
    ) {
        let popped_r4 = u32::from_le_bytes(frame[0..4].try_into().unwrap());
        let popped_pc = u32::from_le_bytes(frame[4..8].try_into().unwrap());
        log::info!(
            "LX continuation {:#x} resumes with sp={:#x}: pops r4={:#x} pc={:#x}",
            continuation,
            sp,
            popped_r4,
            popped_pc
        );
    }

    let _ = unicorn.reg_write(RegisterARM::R0, 0);
    let _ = unicorn.reg_write(RegisterARM::PC, continuation as u64);
    true
}

/// `svc #6` hands control to a coroutine entry and suspends the caller. The entry is an
/// address inside the yielding function (or a sibling body) whose epilogue pops a frame
/// that only exists once *another* coroutine resumes it, so it can neither be called inline
/// nor resumed in place. Map it onto what the emulator already schedules: the continuation
/// becomes a guest thread, the caller parks until that thread returns, and the thread's
/// final R0 is the result the caller's shim pops next.
/// Offset from the argument block to the slot a completion pops its return address from.
/// The shim hands the block over in R0 and the completion ends by unwinding its own frame
/// and popping a register list that ends in PC, e.g.
///
/// ```text
/// 60d680: add sp, sp, #0x10
/// 60d684: movge r0, #0
/// 60d688: pop {r4, pc}      ; return slot = block + 0x10 + 4
/// ```
///
/// which is where the monitor puts the address the coroutine lands on when it is done.
/// Getting this wrong makes the completion branch into whatever the app left in an
/// unrelated slot - seen as a branch into a data page or a null PC.
fn lx_completion_return_offset(unicorn: &mut Unicorn<'_, Context>, resume: u32) -> u32 {
    use capstone::arch::arm::ArchMode;
    use capstone::prelude::*;
    use capstone::{Capstone, Endian};

    let mut code = [0u8; 192];
    if unicorn.mem_read(resume as u64, &mut code).is_err() {
        return 0x14;
    }
    let Ok(cs) = Capstone::new().arm().mode(ArchMode::Arm).endian(Endian::Little).build()
    else {
        return 0x14;
    };
    let Ok(insns) = cs.disasm_all(&code, resume as u64) else {
        return 0x14;
    };

    let mut unwound = 0u32;
    for insn in insns.iter() {
        let mnemonic = insn.mnemonic().unwrap_or("");
        let ops = insn.op_str().unwrap_or("");
        if mnemonic.starts_with("add") && ops.starts_with("sp, sp, #") {
            unwound += parse_arm_immediate(ops.split("#").nth(1).unwrap_or("0"));
            continue;
        }
        // `pop {..., pc}`, `ldmia sp!, {..., pc}` and `ldr pc, [sp], #4` all take the
        // return address off the stack; anything else that ends the function (a branch
        // through a register) does not, so the block is not the frame here.
        let pops_pc = (matches!(mnemonic, "pop" | "ldm") && ops.contains("pc"))
            || (mnemonic == "ldr" && ops.starts_with("pc,"));
        if pops_pc {
            let skipped = ops
                .trim_matches(|c| c == '{' || c == '}')
                .split(',')
                .take_while(|item| item.trim() != "pc")
                .count() as u32;
            return unwound + 4 * skipped;
        }
        if matches!(mnemonic, "bx" | "mov") && ops.contains("pc") {
            break;
        }
    }
    0x14
}

/// ARM immediates are printed as decimal or hex depending on the value.
fn parse_arm_immediate(text: &str) -> u32 {
    let text = text.trim();
    match text.strip_prefix("#0x").or_else(|| text.strip_prefix("0x")) {
        Some(hex) => u32::from_str_radix(hex, 16).unwrap_or(0),
        None => text.strip_prefix('#').unwrap_or(text).parse().unwrap_or(0),
    }
}

fn lx_coroutine_yield(unicorn: &mut Unicorn<'_, Context>, args: [u32; 4]) -> bool {
    let (block, id, resume) = (args[0], args[1], args[2]);
    if block < 0x1000 || resume < 0x1000 || resume & 1 != 0 {
        log::warn!(
            "LX: completion {:#x} or argument block {:#x} unusable",
            resume,
            block
        );
        return false;
    }

    let mut words = [0u32; 8];
    let mut raw = [0u8; 4];
    for (index, word) in words.iter_mut().enumerate() {
        if unicorn
            .mem_read((block + 4 * index as u32) as u64, &mut raw)
            .is_ok()
        {
            *word = u32::from_le_bytes(raw);
        }
    }

    // Build the resume frame the monitor would leave at `block + 0x10`: the completion's
    // epilogue is `add sp, #0x10 ; pop {r4, pc}`, so it pops its saved r4 and its return
    // address from there. The return address is the thread-exit trap, because finishing a
    // completion ends that coroutine - returning would unwind into the shim that yielded
    // and hand the very same completion back again.
    let slot = lx_completion_return_offset(unicorn, resume);
    if unicorn
        .mem_write((block + slot) as u64, &MQ_NOTIFY_EXIT_STUB.to_le_bytes())
        .is_err()
    {
        log::warn!(
            "LX: cannot build the resume frame at {:#x} for completion {:#x}",
            block + slot,
            resume
        );
        return false;
    }

    // The completion runs on the yielding frame: it reads its result and out-parameter
    // through the same SP (`cmp r0, #0` / `ldrge r0, [sp, #4]`) and it needs the
    // yielder's registers, so it starts from a full copy of them. R0 carries the service
    // status, which the consuming code branches on.
    // Measure before mutating: where SP and LR were, and what the app itself left in the
    // slot we are about to overwrite. That tells whether the decoded slot is the epilogue's
    // pop slot (then the word there is a code address) or someone else's data.
    let sp_at_svc = unicorn.reg_read_i32(RegisterARM::R13).unwrap_or(0) as u32;
    let lr_at_svc = unicorn.reg_read_i32(RegisterARM::R14).unwrap_or(0) as u32;
    let mut slot_word = 0u32;
    if unicorn
        .mem_read((block + slot) as u64, &mut raw)
        .is_ok()
    {
        slot_word = u32::from_le_bytes(raw);
    }
    let mut window = Vec::with_capacity(24);
    for index in 0..24u32 {
        let at = sp_at_svc.wrapping_sub(0x20) + 4 * index;
        if unicorn.mem_read(at as u64, &mut raw).is_ok() {
            window.push(format!("{:#x}", u32::from_le_bytes(raw)));
        }
    }

    let Ok(parent_context) = unicorn.context_init() else {
        log::warn!("LX: cannot snapshot the yielding context");
        return false;
    };
    for (register, value) in [
        (RegisterARM::R13, block),
        (RegisterARM::R0, 0u32),
        (RegisterARM::R14, MQ_NOTIFY_EXIT_STUB),
        (RegisterARM::PC, resume),
    ] {
        if unicorn.reg_write(register as i32, value as u64).is_err() {
            return false;
        }
    }
    let Ok(child_context) = unicorn.context_init() else {
        log::warn!("LX: cannot snapshot the completion context");
        return false;
    };
    if unicorn.context_restore(&parent_context).is_err() {
        return false;
    }

    let tid = unicorn
        .get_data()
        .next_thread_id
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let threads = unicorn.get_data().threads.clone();
    threads.lock().unwrap().push(GuestThread {
        id: tid,
        status: ThreadStatus::Runnable,
        cpu_context: Some(child_context),
        pc: resume,
        pending_result: None,
        clear_child_tid: None,
    });

    let yielder = unicorn.get_data().thread_id();
    log::info!(
        "LX: service {:#x} block={:#x} sp={:#x} lr={:#x} slot={:#x} slot_was={:#x} stack[{:#x}..]={} words={} -> completion [{}] at {:#x}, yielder [{}] parks",
        id,
        block,
        sp_at_svc,
        lr_at_svc,
        block + slot,
        slot_word,
        sp_at_svc.wrapping_sub(0x20),
        window.join(" "),
        words
            .iter()
            .map(|w| format!("{:#x}", w))
            .collect::<Vec<_>>()
            .join(" "),
        tid,
        resume,
        yielder
    );
    block_current_thread(
        unicorn,
        BlockReason::CoroutineExit {
            child: tid,
            deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(2)),
        },
    );
    true
}

/// Run `entry` the way the monitor would: the caller's state is saved aside, the
/// entry runs on its own frame, and when it returns the caller resumes at `resume`
/// with the callee's result in R0.
fn lx_invoke_monitor_callback(
    unicorn: &mut Unicorn<'_, Context>,
    resume: u32,
    entry: u32,
    args: [u32; 4],
) -> bool {
    if entry == 0 {
        return false;
    }
    if entry & 1 != 0 {
        log::warn!(
            "LX monitor callback entry {:#x} looks like Thumb, not handled",
            entry
        );
        return false;
    }
    let Some(stub) = lx_call_stub(unicorn) else {
        return false;
    };
    let sp = unicorn.reg_read_i32(RegisterARM::R13).unwrap_or(0) as u32;
    if sp < 0x1000 {
        log::warn!("LX monitor callback refused, bad SP {:#x}", sp);
        return false;
    }

    // One block per handoff, because a handoff body can itself hand off. The three
    // areas must not be adjacent: the body walks its own stack (`add sp, #0x10`) and
    // would otherwise pop our bookkeeping as its saved frame, which is exactly how a
    // null PC used to be produced.
    let mmu_arc = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    let mut alloc = mmu_arc.lock().unwrap();
    let block = alloc.heap_alloc(
        unicorn,
        0x100,
        unicorn_engine::unicorn_const::Prot::READ
            | unicorn_engine::unicorn_const::Prot::WRITE
            | unicorn_engine::unicorn_const::Prot::EXEC,
        "[lx-monitor-handoff]",
    );
    // The body runs calls and can yield again, so it needs a stack of its own - with a
    // single page the nested handoff ended up popping past the end of our block.
    let body_stack = alloc.heap_alloc(
        unicorn,
        LX_HANDOFF_STACK_SIZE,
        unicorn_engine::unicorn_const::Prot::READ
            | unicorn_engine::unicorn_const::Prot::WRITE
            | unicorn_engine::unicorn_const::Prot::EXEC,
        "[lx-monitor-stack]",
    );
    drop(alloc);

    let tramp = block;
    let locals = 0x10u32;
    let body_sp =
        (body_stack + LX_HANDOFF_STACK_SIZE - 0x100 - locals) & !7;
    let table = block + 0x40;

    // Save table in the layout the shared stub expects: R0..R11, R14, SP, resume PC.
    let mut saved = [0u32; 16];
    for (slot, register) in [
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
        RegisterARM::R11,
        RegisterARM::R14,
    ]
    .iter()
    .enumerate()
    {
        saved[slot] = unicorn.reg_read(*register).unwrap_or(0) as u32;
    }
    saved[13] = sp;
    saved[14] = resume;
    for (slot, value) in saved.iter().enumerate() {
        if unicorn
            .mem_write((table + (slot as u32 * 4)) as u64, &value.to_le_bytes())
            .is_err()
        {
            log::warn!(
                "LX monitor callback refused, cannot save state at {:#x}",
                table
            );
            return false;
        }
    }

    // `ldr sp, [pc, #4] ; b <stub> ; nop ; <table>`: point the shared stub at
    // this handoff's save table.
    if unicorn.mem_write(tramp as u64, &lx_trampoline(tramp, stub, table)).is_err() {
        return false;
    }
    // Same stub at the bottom of the coroutine stack: the epilogues that end a
    // coroutine branch to a landing address on that stack, and it must hand control
    // back to the yielder instead of faulting. The branch is relative, so the copy has
    // to be assembled for its own address.
    let landing = lx_trampoline(body_stack, stub, table);
    if unicorn.mem_write(body_stack as u64, &landing).is_err() {
        return false;
    }

    // The entry is a mid-function address, so give it the frame its prologue implies:
    // locals below a saved r4 and a return link, entered with SP at the locals.
    // Locals, then the saved r4 and the link the epilogue pops. With no frame at all
    // (a return through lr), the link has to be in LR instead.
    let mut body = vec![0u32; (locals / 4) as usize + 2];
    for (slot, value) in args.iter().take((locals / 4) as usize).enumerate() {
        body[slot] = *value;
    }
    body[(locals / 4) as usize] = saved[4]; // r4
    body[(locals / 4) as usize + 1] = tramp;
    for (slot, value) in body.iter().enumerate() {
        if unicorn
            .mem_write(
                (body_sp + (slot as u32 * 4)) as u64,
                &value.to_le_bytes(),
            )
            .is_err()
        {
            return false;
        }
    }
    log::info!(
        "LX handoff entry {:#x} runs with locals={:#x} sp={:#x} link={:#x}",
        entry,
        locals,
        body_sp,
        tramp
    );

    let _ = unicorn.reg_write(RegisterARM::R13, body_sp as u64);
    for (register, value) in [
        (RegisterARM::R0, args[0]),
        (RegisterARM::R1, args[1]),
        (RegisterARM::R2, args[2]),
        (RegisterARM::R3, args[3]),
        (RegisterARM::R14, tramp),
    ] {
        let _ = unicorn.reg_write(register, value as u64);
    }
    let _ = unicorn.reg_write(RegisterARM::PC, entry as u64);
    true
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
