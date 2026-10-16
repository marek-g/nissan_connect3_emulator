use crate::emulator::context::Context;
use crate::emulator::elf_loader::load_elf;
use crate::emulator::thread::{
    dump_context, enable_vfp, set_kernel_traps, BlockReason, GuestThread, ThreadStatus, Wake,
};
use crate::emulator::utils::load_binary;
use std::error::Error;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use unicorn_engine::unicorn_const::uc_error;
use unicorn_engine::{RegisterARM, Unicorn};

fn map_uc_error(error: uc_error) -> Box<dyn Error + Send + Sync + 'static> {
    format!("Unicorn error: {:?}", error).into()
}

/// instructions to run per guest thread when more than one is runnable in a
/// process, so a busy thread cannot starve its siblings on the same host thread
const TIMESLICE_INSTRUCTIONS: usize = 10_000;

/// upper bound on how long a host thread parks before re-checking the shared
/// IPC objects. Cross-process wakeups are also pushed via doorbells, so this is
/// only a safety net that bounds the latency of any missed wake.
const POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Load an ELF into a fresh VM and register its main guest thread. Called once
/// per process before the cooperative scheduling loop begins. The returned
/// Unicorn owns its Context (and through it this process' mmu/threads/namespace).
pub fn setup_process(
    unicorn: &mut Unicorn<'_, Context>,
    elf_filepath: &str,
    program_args: Vec<String>,
    program_envs: Vec<(String, String)>,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let buf = load_binary(unicorn, elf_filepath)?;

    let (interp_entry_point, elf_entry, stack_ptr) =
        load_elf(unicorn, elf_filepath, &buf, &program_args, &program_envs)?;

    unicorn
        .reg_write(RegisterARM::SP as i32, stack_ptr as u64)
        .unwrap();

    set_kernel_traps(unicorn);
    enable_vfp(unicorn);

    log::info!(
        "========== Start program (interp_entry_point: {:#x}, elf_entry_point: {:#x}) ==========",
        interp_entry_point,
        elf_entry
    );

    // register the main guest thread
    {
        let data = unicorn.get_data();
        let main_id = data.next_thread_id.fetch_add(1, Ordering::Relaxed);
        data.set_thread_id(main_id);
        let cpu_context = unicorn.context_init().map_err(map_uc_error)?;
        data.threads.lock().unwrap().push(GuestThread {
            id: main_id,
            rtos_task: false,
            status: ThreadStatus::Runnable,
            cpu_context: Some(cpu_context),
            pc: interp_entry_point,
            pending_result: None,
            clear_child_tid: None,
        });
    }

    Ok(())
}

/// Wall-clock budget per quantum in microseconds, from `EMU_QUANTUM_TIMEOUT_US`
/// (0 disables it). Without a budget, a lone thread runs until it blocks or exits, so
/// a user-space spin loop is invisible from the outside.
fn spin_watch_timeout() -> u64 {
    use std::sync::OnceLock;
    static VALUE: OnceLock<u64> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("EMU_QUANTUM_TIMEOUT_US")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    })
}

/// Run one scheduling quantum on a single process' VM: switch in its next
/// runnable guest thread, execute up to `count` instructions (0 = until the
/// thread blocks or exits), handle any memory fault, then switch out. Returns
/// true if a thread was run.
fn run_quantum(
    unicorn: &mut Unicorn<'_, Context>,
    count: usize,
) -> Result<bool, Box<dyn Error + Send + Sync + 'static>> {
    let next_id = match pick_next_runnable(unicorn) {
        Some(id) => id,
        None => return Ok(false),
    };

    // ---- switch in: restore the thread's CPU state onto the shared vCPU
    let (cpu_context, pc, pending_result) = {
        let data = unicorn.get_data();
        let mut threads = data.threads.lock().unwrap();
        let thread = threads.iter_mut().find(|t| t.id == next_id).unwrap();
        thread.status = ThreadStatus::Running;
        (
            thread.cpu_context.take().unwrap(),
            thread.pc,
            thread.pending_result.take(),
        )
    };
    unicorn.get_data().set_thread_id(next_id);
    unicorn
        .context_restore(&cpu_context)
        .map_err(map_uc_error)?;

    // a completed blocked syscall (e.g. woken mq receive) parks its result
    // here instead of in the saved context
    if let Some(result) = pending_result {
        unicorn
            .reg_write(RegisterARM::R0 as i32, result as u64)
            .unwrap();
    }

    // A thread whose saved PC is a null branch cannot run at all: `emu_start(0, 0)`
    // returns immediately (begin == until), which would livelock this loop forever.
    // Linux would deliver SIGSEGV here, so terminate with the same default action.
    if pc < 0x1000 {
        let rtos_task = unicorn
            .get_data()
            .threads
            .lock()
            .unwrap()
            .iter()
            .any(|t| t.id == next_id && t.rtos_task);
        if rtos_task {
            // Fabricated RTOS task whose body unwound into its null teardown
            // continuation (see `GuestThread::rtos_task`): the real kernel
            // illtraps this task the same way; it is not an emulator error.
            log::info!(
                "[{}] rtos task unwound to its null teardown continuation (pc={:#x}), killed like the real kernel would",
                next_id,
                pc
            );
        } else {
            log::error!(
                "[{}] cannot resume at pc={:#x} (lr={:#x} sp={:#x}), terminating",
                next_id,
                pc,
                unicorn.reg_read(RegisterARM::R14).unwrap_or(0),
                unicorn.reg_read(RegisterARM::R13).unwrap_or(0)
            );
        }
        let data = unicorn.get_data();
        if let Some(thread) = data
            .threads
            .lock()
            .unwrap()
            .iter_mut()
            .find(|t| t.id == next_id)
        {
            thread.pc = pc;
            thread.status = ThreadStatus::Exited(139);
        }
        return Ok(true);
    }

    // Trace quantum starts for one process family: a livelock shows up as quanta that
    // keep starting at (almost) the same PC and returning immediately.
    {
        static QUANTA: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let watched = unicorn.get_data().elf_path.contains("navbin");
        let tick = QUANTA.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if watched {
            log::info!(
                "[{}] quantum {}: pc={:#x} lr={:#x} sp={:#x}",
                next_id,
                tick,
                pc,
                unicorn.reg_read(RegisterARM::R14).unwrap_or(0),
                unicorn.reg_read(RegisterARM::R13).unwrap_or(0)
            );
        }
    }

    let budget = spin_watch_timeout();
    // `uc_emu_start(begin)` also picks the mode: an even `begin` forces ARM even though the
    // saved context says THUMB, so every odd (THUMB) thread entry would decode as garbage
    // ARM. Hand the restored CPSR's T bit to `emu_start` as the address's low bit.
    let cpsr = unicorn.reg_read(RegisterARM::CPSR).unwrap_or(0) as u32;
    let begin = pc as u64 | u64::from((cpsr >> 5) & 1);
    let res = unicorn.emu_start(begin, 0, budget, count);

    // A spin watch can only stop the run from the outside, which is not a fault and
    // not an exit: sample where the thread actually is and let it run again.
    let spin_stop = budget != 0
        && res
            .as_ref()
            .err()
            .map_or(false, |err| format!("{:?}", err).to_uppercase().contains("TIMEOUT"));
    if spin_stop {
            log::warn!(
                "[{}] spin watch: pc={:#x} lr={:#x} sp={:#x}",
                unicorn.get_data().thread_id(),
                unicorn.reg_read(RegisterARM::PC).unwrap_or(0),
                unicorn.reg_read(RegisterARM::R14).unwrap_or(0),
                unicorn.reg_read(RegisterARM::R13).unwrap_or(0)
            );
    }

    // a memory fault captured by the mem hook? With the VM stopped we can now read
    // registers and write the signal frame. Delivery either jumps to the guest's
    // SIGSEGV handler (or performs a sigreturn), leaving the thread runnable at the
    // new PC; otherwise the default action applies (terminate).
    let (fault_handled, had_fault) = match unicorn.get_data().take_pending_fault() {
        Some(fault) => {
            let handled =
                crate::os::syscalls::signal::handle_mem_fault(unicorn, fault.addr, fault.is_fetch);
            (handled, true)
        }
        None => (false, false),
    };

    // ---- switch out: save the CPU state back to the thread record
    let mut exited_with: Option<i32> = None;
    let mut saved_context = unicorn.context_alloc().map_err(map_uc_error)?;
    unicorn
        .context_save(&mut saved_context)
        .map_err(map_uc_error)?;
    let pc = unicorn.reg_read(RegisterARM::PC).unwrap() as u32;
    {
        let data = unicorn.get_data();
        let mut threads = data.threads.lock().unwrap();
        if let Some(thread) = threads.iter_mut().find(|t| t.id == next_id) {
            // a syscall handler may have already changed the status
            // (blocked / exited); otherwise decide based on the run result
            let was_running = thread.status == ThreadStatus::Running;
            if thread.status == ThreadStatus::Running {
                if fault_handled {
                    // resumed at the signal handler / post-sigreturn PC
                    thread.status = ThreadStatus::Runnable;
                } else if had_fault {
                    // unhandled SIGSEGV -> default action: terminate (128 + SIGSEGV)
                    thread.status = ThreadStatus::Exited(139);
                } else if res.is_err() && !spin_stop {
                    thread.status = ThreadStatus::Exited(1);
                } else {
                    // timeslice simply expired
                    thread.status = ThreadStatus::Runnable;
                }
            }
            // surface silent thread death (an unhandled fault / exit leaves no other
            // trace, which makes a vanished process very hard to diagnose)
            if let ThreadStatus::Exited(code) = &thread.status {
                log::warn!("[{}] thread exited with code {}", next_id, code);
                if was_running {
                    exited_with = Some(*code);
                }
            }
            thread.cpu_context = Some(saved_context);
            thread.pc = pc;
        }
    }

    if let Some(code) = exited_with {
        // A terminated LX coroutine hands its final R0 back to whoever yielded into it.
        let parents: Vec<u32> = {
            let data = unicorn.get_data();
            let threads = data.threads.lock().unwrap();
            threads
                .iter()
                .filter_map(|t| match t.status {
                    ThreadStatus::Blocked(BlockReason::CoroutineExit { child, .. })
                        if child == next_id =>
                    {
                        Some(t.id)
                    }
                    _ => None,
                })
                .collect()
        };
        for parent in parents {
            log::info!(
                "LX: coroutine [{}] finished with {:#x}, resuming yielder [{}]",
                next_id,
                code as u32,
                parent
            );
            set_runnable(unicorn, parent, Some(code as u32));
        }
    }

    if res.is_err() && !fault_handled && !spin_stop {
        // the VM is stopped now, so dumping memory is safe
        log::error!(
            "{:#x}: [{}] Execution error: {:?}",
            unicorn.reg_read(RegisterARM::PC).unwrap(),
            next_id,
            res.as_ref().err()
        );
        if let Some((site, site_addr)) = crate::os::code_stub::last_site() {
            log::error!("last trapped site before fault: {site} at {site_addr:#x}");
            crate::os::code_stub::dump_site_ring();
        }
        dump_context(unicorn);
    }

    Ok(true)
}

/// A process is finished when it has requested exit or all its threads exited.
fn process_is_done(unicorn: &Unicorn<'_, Context>) -> bool {
    unicorn.get_data().process_exit_code().is_some() || all_exited(unicorn)
}

/// Run one guest process on the calling (host) thread until it exits. This is
/// the per-process entry point for the parallel model: every process has its own
/// host thread and its own VM, so guest threads of *different* processes run
/// truly in parallel, while the guest threads *within* a process still
/// cooperate round-robin on this one host thread. `wake` is this process'
/// doorbell - parked on when nothing can run, rung by peers on IPC activity.
pub fn run_process_loop(
    unicorn: &mut Unicorn<'_, Context>,
    wake: &Wake,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    // A process that never calls mmap() (a static executable whose segments were all
    // mapped by the loader) would never get its library hooks installed, because the only
    // other call site is mmap.
    {
        let mmu_arc = unicorn.get_data().mmu.clone();
        mmu_arc.lock().unwrap().update_library_hooks(unicorn);
    }
    // Deferred code observers that library registration armed (it runs with
    // the MMU mutex held and cannot allocate its code caves): try the install
    // once here, deterministically - the ELF text is fully loaded, the MMU
    // mutex is free, and the guest has not executed a single instruction yet.
    // The call self-gates to the process it belongs to.
    crate::libs::procmapengine::install_pending_observers(unicorn);
    crate::os::code_stub::flush_trap_sites(unicorn);

    let mut last_park_report: Option<String> = None;
    let mut passes: u64 = 0;
    loop {
        passes += 1;
        if passes % 1024 == 0 {
            log::info!(
                "SCHED: heartbeat {} passes={}",
                unicorn.get_data().elf_path,
                passes
            );
        }
        crate::gpu::tick(unicorn);
        crate::libs::prochmi::tick(unicorn);

        // memory-leak watchdog: guest heap/brk growth per process
        {
            static LOOP_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if LOOP_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 8192 == 0 {
                let mmu = unicorn.get_data().mmu.lock().unwrap();
                log::info!(
                    "SCHED: heap telemetry process={} heap_end={:#x} brk_end={:#x} regions={}",
                    unicorn.get_data().elf_path,
                    mmu.heap_mem_end,
                    mmu.brk_mem_end,
                    mmu.region_count(),
                );
            }
        }

        // re-evaluate blocked guest threads: complete the ones whose IPC object is
        // now ready or whose deadline passed (memory work happens here, in this
        // process' own VM, so cross-process delivery never touches foreign memory)
        advance_blocked(unicorn);

        if process_is_done(unicorn) {
            break;
        }

        // give each runnable guest thread a slice only when there is contention,
        // so a lone thread runs until it blocks/exits instead of being preempted
        let timeslice = if count_runnable(unicorn) > 1 {
            TIMESLICE_INSTRUCTIONS
        } else {
            0
        };

        if run_quantum(unicorn, timeslice)? {
            continue;
        }

        // Report why the process has nothing to run, but only when the situation
        // changed, so a permanently stuck process does not flood the log.
        let thread_state: String = {
            let data = unicorn.get_data();
            data.threads
                .lock()
                .unwrap()
                .iter()
                .map(|t| match t.status {
                    ThreadStatus::Blocked(reason) => format!("{}:blocked({})", t.id, reason.name()),
                    ThreadStatus::Exited(code) => format!("{}:exited({})", t.id, code),
                    ThreadStatus::Runnable => format!("{}:runnable", t.id),
                    ThreadStatus::Running => format!("{}:running", t.id),
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        // The report doubles as the dedup key: keep them identical so a steady
        // state prints exactly one line (deadline ticks live inside `reason.name()`
        // only for non-parked states, which are short-lived anyway).
        let park_report = thread_state;
        if last_park_report.as_deref() != Some(park_report.as_str()) {
            log::info!(
                "SCHED: parked {}, threads: {}",
                unicorn.get_data().elf_path,
                park_report
            );
            last_park_report = Some(park_report);
        }

        // nothing runnable: park until a peer rings our doorbell, the nearest
        // blocked-thread deadline elapses, or the poll tick fires
        wake.wait_timeout(park_duration(unicorn));
    }

    log::info!("========== Process done ==========");
    Ok(())
}

/// wake blocked threads whose deadline has passed and re-check the ones waiting
/// on shared IPC objects. Called from the process' own host thread with the VM
/// stopped, so the completion memory reads/writes target this process' memory.
fn advance_blocked(unicorn: &mut Unicorn<'_, Context>) {
    let now = Instant::now();

    // take the Arcs we need and release the borrow on `unicorn`, so the
    // completion helpers (which need &mut Unicorn) can run
    let (threads, sys_calls_state, namespace) = {
        let data = unicorn.get_data();
        (
            data.threads.clone(),
            data.sys_calls_state.clone(),
            data.namespace.clone(),
        )
    };

    // collect the blocked threads first (finish_* re-locks the thread list)
    let blocked: Vec<(u32, BlockReason)> = threads
        .lock()
        .unwrap()
        .iter()
        .filter_map(|t| match t.status {
            ThreadStatus::Blocked(reason) => Some((t.id, reason)),
            _ => None,
        })
        .collect();

    let mut expired_futex: Vec<(u32, u32)> = Vec::new(); // (addr, tid)

    for (tid, reason) in blocked {
        match reason {
            BlockReason::MonitorYield {
                service,
                block,
                deadline,
                ..
            } => {
                // a TSE wait wakes on its condition (semaphore count / flag pattern),
                // which any other thread's sig_sem/set_flg just updated; the deadline
                // below is only its µITRON timeout
                if let Some(status) = crate::rtos::monitor::poll_tse_wait(unicorn, service, block) {
                    set_runnable(unicorn, tid, Some(status));
                    continue;
                }
                // an OSAL queue wait wakes the moment a message lands in its host queue
                if let Some(status) = crate::rtos::monitor::poll_osal_mbx_wait(unicorn, service, block) {
                    set_runnable(unicorn, tid, Some(status));
                    continue;
                }
                if deadline.map(|until| until <= now).unwrap_or(false) {
                    let status = crate::rtos::monitor::complete(unicorn, service, block);
                    log::debug!(
                        "[{}] monitor: yield on service {:#x} ended with {:#x}",
                        tid,
                        service,
                        status
                    );
                    set_runnable(unicorn, tid, Some(status));
                }
            }
            BlockReason::SleepUntil(until) => {
                if until <= now {
                    set_runnable(unicorn, tid, None);
                }
            }
            BlockReason::FutexWait { addr, deadline } => {
                if deadline.map(|d| d <= now).unwrap_or(false) {
                    expired_futex.push((addr, tid));
                    set_runnable(unicorn, tid, Some(-110i32 as u32)); // -ETIMEDOUT
                }
            }
            BlockReason::FutexWaitShared { id, deadline, watchdog } => {
                // a shared-memory futex (e.g. a POSIX named semaphore): woken by
                // any process' FUTEX_WAKE (signalled in the global registry), or
                // timed out. The registry entry is reaped here, in this owner.
                let outcome = {
                    let mut ns = namespace.lock().unwrap();
                    match ns.futex_poll_shared(id) {
                        crate::os::syscalls::namespace::SharedFutexPoll::Woken => Some(Ok(0u32)),
                        crate::os::syscalls::namespace::SharedFutexPoll::Waiting => {
                            if deadline.map(|d| d <= now).unwrap_or(false) {
                                if watchdog {
                                    // Dead-owner repair: the guest asked for an
                                    // open-ended wait, so the deadline is the
                                    // emulator watchdog. Zero the mutex word in
                                    // the shared host buffer (this owner's
                                    // mapping is host-ptr backed, so the write
                                    // is coherent for every process) and wake
                                    // with success; the guest's lock retry then
                                    // takes the uncontended word.
                                    ns.futex_steal_shared(id).map(Err).or(Some(Ok(0u32)))
                                } else {
                                    ns.futex_remove_shared(id);
                                    Some(Ok(-110i32 as u32)) // -ETIMEDOUT
                                }
                            } else {
                                None
                            }
                        }
                        crate::os::syscalls::namespace::SharedFutexPoll::Gone => None,
                    }
                };
                match outcome {
                    Some(Ok(result)) => set_runnable(unicorn, tid, Some(result)),
                    Some(Err(uaddr)) => {
                        // `uaddr` is this owner's VA into the host-backed shm
                        // region; zeroing it clears the lost unlock globally.
                        if unicorn.mem_write(uaddr as u64, &[0u8; 4]).is_err() {
                            log::warn!(
                                "[{tid}] shared-futex watchdog: failed to zero word {uaddr:#x}"
                            );
                        } else {
                            log::warn!(
                                "[{tid}] shared-futex watchdog: zeroed stuck word {uaddr:#x} after 20 s grace (dead-owner repair)"
                            );
                        }
                        set_runnable(unicorn, tid, Some(0u32));
                    }
                    None => {}
                }
            }
            BlockReason::MqSend { .. } | BlockReason::MqReceive { .. } => {
                // completes if the queue allows it, else times out if the deadline
                // passed, else leaves the thread blocked (deadline-aware internally)
                crate::os::syscalls::mqueue::finish_mq_wait(unicorn, tid, now);
            }
            BlockReason::OsalQueueReceive { .. } => {
                crate::os::osal_queue::finish_guest_wait(unicorn, tid, now);
            }
            BlockReason::IoscMutex { .. }
            | BlockReason::IoscEvent { .. }
            | BlockReason::IoscSemaphore { .. } => {
                crate::os::dev::iosc::finish_iosc_wait(unicorn, tid, now);
            }
            BlockReason::CoroutineExit { child, deadline } => {
                let child_done = threads.lock().unwrap().iter().any(|t| {
                    t.id == child && matches!(t.status, ThreadStatus::Exited(_))
                });
                if child_done {
                    // the exit hook above resumes it with the child's result; this only
                    // catches a child that was already gone before this thread parked
                    set_runnable(unicorn, tid, Some(0));
                } else if deadline.map(|d| d <= now).unwrap_or(false) {
                    log::warn!(
                        "LX: coroutine [{}] yielder [{}] timed out waiting",
                        child,
                        tid
                    );
                    set_runnable(unicorn, tid, Some(0));
                }
            }
            BlockReason::InotifyRead { deadline, .. } => {
                if deadline.map(|d| d <= now).unwrap_or(false) {
                    set_runnable(unicorn, tid, Some(-11i32 as u32)); // -EAGAIN
                }
            }
            BlockReason::SocketRead { deadline, .. } => {
                if deadline.map(|d| d <= now).unwrap_or(false) {
                    set_runnable(unicorn, tid, Some(-110i32 as u32)); // -ETIMEDOUT
                }
            }
            BlockReason::SignalWait { .. } => {
                crate::os::syscalls::signal::finish_signal_wait(unicorn, tid, now);
            }
        }
    }

    if !expired_futex.is_empty() {
        let mut state = sys_calls_state.lock().unwrap();
        for (addr, tid) in expired_futex {
            if let Some(list) = state.futex_waiters.get_mut(&addr) {
                list.retain(|&waiter| waiter != tid);
            }
        }
    }
}

fn set_runnable(unicorn: &mut Unicorn<'_, Context>, tid: u32, result: Option<u32>) {
    let mut threads = unicorn.get_data().threads.lock().unwrap();
    if let Some(thread) = threads.iter_mut().find(|t| t.id == tid) {
        if matches!(thread.status, ThreadStatus::Blocked(_)) {
            thread.status = ThreadStatus::Runnable;
            thread.pending_result = result;
        }
    }
}

/// how long to park: the nearest blocked-thread deadline (clamped to
/// `POLL_INTERVAL` so a missed cross-process wake costs at most one tick)
fn park_duration(unicorn: &Unicorn<'_, Context>) -> Duration {
    let now = Instant::now();
    let data = unicorn.get_data();
    let threads = data.threads.lock().unwrap();

    let next = threads
        .iter()
        .filter_map(|t| match &t.status {
            ThreadStatus::Blocked(BlockReason::CoroutineExit { deadline, .. }) => *deadline,
            ThreadStatus::Blocked(BlockReason::FutexWait { deadline, .. }) => *deadline,
            ThreadStatus::Blocked(BlockReason::FutexWaitShared { deadline, .. }) => *deadline,
            ThreadStatus::Blocked(BlockReason::SleepUntil(until)) => Some(*until),
            ThreadStatus::Blocked(
                BlockReason::MqSend { deadline, .. } | BlockReason::MqReceive { deadline, .. },
            ) => *deadline,
            ThreadStatus::Blocked(BlockReason::OsalQueueReceive { deadline, .. }) => *deadline,
            ThreadStatus::Blocked(
                BlockReason::IoscMutex { deadline, .. }
                | BlockReason::IoscEvent { deadline, .. }
                | BlockReason::IoscSemaphore { deadline, .. }
                | BlockReason::MonitorYield { deadline, .. }
                | BlockReason::InotifyRead { deadline, .. }
                | BlockReason::SocketRead { deadline, .. }
                | BlockReason::SignalWait { deadline, .. },
            ) => *deadline,
            _ => None,
        })
        .min();

    match next {
        Some(deadline) if deadline > now => deadline.duration_since(now).min(POLL_INTERVAL),
        Some(_) => Duration::ZERO,
        None => POLL_INTERVAL,
    }
}

/// number of guest threads in this process that can run right now
fn count_runnable(unicorn: &Unicorn<'_, Context>) -> usize {
    let data = unicorn.get_data();
    let threads = data.threads.lock().unwrap();
    threads
        .iter()
        .filter(|t| t.status == ThreadStatus::Runnable)
        .count()
}

/// round-robin pick of the next runnable thread
fn pick_next_runnable(unicorn: &Unicorn<'_, Context>) -> Option<u32> {
    let data = unicorn.get_data();
    let picked = {
        let threads = data.threads.lock().unwrap();
        let count = threads.len();
        if count == 0 {
            return None;
        }

        let start = data.last_run_index() % count;
        (0..count)
            .find(|&offset| threads[(start + offset) % count].status == ThreadStatus::Runnable)
            .map(|offset| {
                let index = (start + offset) % count;
                (index, threads[index].id)
            })
    };

    let Some((index, id)) = picked else {
        return None;
    };
    data.set_last_run_index(index + 1);
    Some(id)
}

fn all_exited(unicorn: &Unicorn<'_, Context>) -> bool {
    let data = unicorn.get_data();
    let threads = data.threads.lock().unwrap();
    !threads.is_empty()
        && threads
            .iter()
            .all(|t| matches!(t.status, ThreadStatus::Exited(_)))
}
