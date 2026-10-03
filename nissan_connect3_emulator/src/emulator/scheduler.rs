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
            status: ThreadStatus::Runnable,
            cpu_context: Some(cpu_context),
            pc: interp_entry_point,
            pending_result: None,
            clear_child_tid: None,
        });
    }

    Ok(())
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
    unicorn.context_restore(&cpu_context).map_err(map_uc_error)?;

    // a completed blocked syscall (e.g. woken mq receive) parks its result
    // here instead of in the saved context
    if let Some(result) = pending_result {
        unicorn
            .reg_write(RegisterARM::R0 as i32, result as u64)
            .unwrap();
    }

    let res = unicorn.emu_start(pc as u64, 0, 0, count);

    // a memory fault captured by the mem hook? With the VM stopped we can now read
    // registers and write the signal frame. Delivery either jumps to the guest's
    // SIGSEGV handler (or performs a sigreturn), leaving the thread runnable at the
    // new PC; otherwise the default action applies (terminate).
    let (fault_handled, had_fault) = match unicorn.get_data().take_pending_fault() {
        Some(fault) => {
            let handled = crate::os::syscalls::signal::handle_mem_fault(
                unicorn,
                fault.addr,
                fault.is_fetch,
            );
            (handled, true)
        }
        None => (false, false),
    };

    // ---- switch out: save the CPU state back to the thread record
    let mut saved_context = unicorn.context_alloc().map_err(map_uc_error)?;
    unicorn.context_save(&mut saved_context).map_err(map_uc_error)?;
    let pc = unicorn.reg_read(RegisterARM::PC).unwrap() as u32;
    {
        let data = unicorn.get_data();
        let mut threads = data.threads.lock().unwrap();
        if let Some(thread) = threads.iter_mut().find(|t| t.id == next_id) {
            // a syscall handler may have already changed the status
            // (blocked / exited); otherwise decide based on the run result
            if thread.status == ThreadStatus::Running {
                if fault_handled {
                    // resumed at the signal handler / post-sigreturn PC
                    thread.status = ThreadStatus::Runnable;
                } else if had_fault {
                    // unhandled SIGSEGV -> default action: terminate (128 + SIGSEGV)
                    thread.status = ThreadStatus::Exited(139);
                } else if res.is_err() {
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
            }
            thread.cpu_context = Some(saved_context);
            thread.pc = pc;
        }
    }

    if res.is_err() && !fault_handled {
        // the VM is stopped now, so dumping memory is safe
        log::error!(
            "{:#x}: [{}] Execution error: {:?}",
            unicorn.reg_read(RegisterARM::PC).unwrap(),
            next_id,
            res.as_ref().err()
        );
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
    loop {
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
            BlockReason::FutexWaitShared { id, deadline } => {
                // a shared-memory futex (e.g. a POSIX named semaphore): woken by
                // any process' FUTEX_WAKE (signalled in the global registry), or
                // timed out. The registry entry is reaped here, in this owner.
                let outcome = {
                    let mut ns = namespace.lock().unwrap();
                    match ns.futex_poll_shared(id) {
                        crate::os::syscalls::namespace::SharedFutexPoll::Woken => Some(0u32),
                        crate::os::syscalls::namespace::SharedFutexPoll::Waiting => {
                            if deadline.map(|d| d <= now).unwrap_or(false) {
                                ns.futex_remove_shared(id);
                                Some(-110i32 as u32) // -ETIMEDOUT
                            } else {
                                None
                            }
                        }
                        crate::os::syscalls::namespace::SharedFutexPoll::Gone => None,
                    }
                };
                if let Some(result) = outcome {
                    set_runnable(unicorn, tid, Some(result));
                }
            }
            BlockReason::MqSend { .. } | BlockReason::MqReceive { .. } => {
                // completes if the queue allows it, else times out if the deadline
                // passed, else leaves the thread blocked (deadline-aware internally)
                crate::os::syscalls::mqueue::finish_mq_wait(unicorn, tid, now);
            }
            BlockReason::IoscMutex { .. }
            | BlockReason::IoscEvent { .. }
            | BlockReason::IoscSemaphore { .. } => {
                crate::os::dev::iosc::finish_iosc_wait(unicorn, tid, now);
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

    let next = threads.iter().find_map(|t| match &t.status {
        ThreadStatus::Blocked(BlockReason::FutexWait { deadline, .. }) => *deadline,
        ThreadStatus::Blocked(BlockReason::FutexWaitShared { deadline, .. }) => *deadline,
        ThreadStatus::Blocked(BlockReason::SleepUntil(until)) => Some(*until),
        ThreadStatus::Blocked(BlockReason::MqSend { deadline, .. } | BlockReason::MqReceive { deadline, .. }) => {
            *deadline
        }
        ThreadStatus::Blocked(
            BlockReason::IoscMutex { deadline, .. }
            | BlockReason::IoscEvent { deadline, .. }
            | BlockReason::IoscSemaphore { deadline, .. },
        ) => *deadline,
        _ => None,
    });

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
    !threads.is_empty() && threads.iter().all(|t| matches!(t.status, ThreadStatus::Exited(_)))
}
