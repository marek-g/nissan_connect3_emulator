use crate::emulator::context::Context;
use crate::emulator::elf_loader::load_elf;
use crate::emulator::thread::{
    dump_context, enable_vfp, set_kernel_traps, BlockReason, GuestThread, ThreadStatus,
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

/// instructions to run per timeslice when multiple guest threads are runnable
const TIMESLICE_INSTRUCTIONS: usize = 10_000;

/// host sleep tick when all guest threads are blocked without a deadline
const IDLE_TICK: Duration = Duration::from_millis(500);

/// Runs the single Unicorn VM, cooperatively scheduling all guest threads on it.
pub fn run(
    unicorn: &mut Unicorn<'_, Context>,
    elf_filepath: &str,
    program_args: Vec<String>,
    program_envs: Vec<(String, String)>,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let buf = load_binary(unicorn, elf_filepath);

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
        });
    }

    loop {
        wake_expired(unicorn);

        if unicorn.get_data().process_exit_code().is_some() {
            break;
        }

        let next_id = pick_next_runnable(unicorn);
        let Some(next_id) = next_id else {
            // no runnable thread - either everything is blocked or exited
            if all_exited(unicorn) {
                break;
            }
            sleep_until_next_wakeup(unicorn);
            continue;
        };

        // ---- switch in: restore the thread's CPU state onto the shared vCPU
        let (cpu_context, pc) = {
            let data = unicorn.get_data();
            let mut threads = data.threads.lock().unwrap();
            let thread = threads.iter_mut().find(|t| t.id == next_id).unwrap();
            thread.status = ThreadStatus::Running;
            (thread.cpu_context.take().unwrap(), thread.pc)
        };
        unicorn.get_data().set_thread_id(next_id);
        unicorn.context_restore(&cpu_context).map_err(map_uc_error)?;

        // timeslice only when there is another runnable thread to switch to
        let count = if other_runnable_exists(unicorn) {
            TIMESLICE_INSTRUCTIONS
        } else {
            0
        };

        let res = unicorn.emu_start(pc as u64, 0, 0, count);

        // ---- switch out: save the CPU state back to the thread record
        let mut saved_context = unicorn.context_alloc().map_err(map_uc_error)?;
        unicorn.context_save(&mut saved_context).map_err(map_uc_error)?;
        let pc = unicorn.reg_read(RegisterARM::PC).unwrap() as u32;
        {
            let data = unicorn.get_data();
            let mut threads = data.threads.lock().unwrap();
            if let Some(thread) = threads.iter_mut().find(|t| t.id == next_id) {
                // a syscall handler may have already changed the status
                // (blocked / exited); otherwise the timeslice simply expired
                if thread.status == ThreadStatus::Running {
                    thread.status = ThreadStatus::Runnable;
                }
                thread.cpu_context = Some(saved_context);
                thread.pc = pc;
            }
        }

        if let Err(error) = res {
            // the VM is stopped now, so dumping memory is safe
            log::error!(
                "{:#x}: [{}] Execution error: {:?}",
                unicorn.reg_read(RegisterARM::PC).unwrap(),
                next_id,
                error
            );
            dump_context(unicorn);

            let data = unicorn.get_data();
            let mut threads = data.threads.lock().unwrap();
            if let Some(thread) = threads.iter_mut().find(|t| t.id == next_id) {
                if !matches!(thread.status, ThreadStatus::Blocked(_) | ThreadStatus::Exited(_)) {
                    thread.status = ThreadStatus::Exited(1);
                }
            }
        }
    }

    log::info!("========== Program done ==========");
    Ok(())
}

/// wake blocked threads whose deadline has passed
fn wake_expired(unicorn: &Unicorn<'_, Context>) {
    let now = Instant::now();
    let data = unicorn.get_data();

    let mut expired_futex_waiters: Vec<(u32, u32)> = Vec::new(); // (addr, tid)
    {
        let mut threads = data.threads.lock().unwrap();
        for thread in threads.iter_mut() {
            if let ThreadStatus::Blocked(reason) = &thread.status {
                let expired = match reason {
                    BlockReason::FutexWait { deadline, .. } => {
                        deadline.map(|d| d <= now).unwrap_or(false)
                    }
                    BlockReason::SleepUntil(until) => *until <= now,
                };
                if expired {
                    if let ThreadStatus::Blocked(BlockReason::FutexWait { addr, .. }) =
                        &thread.status
                    {
                        expired_futex_waiters.push((*addr, thread.id));
                    }
                    thread.status = ThreadStatus::Runnable;
                }
            }
        }
    }

    if !expired_futex_waiters.is_empty() {
        let mut state = data.sys_calls_state.lock().unwrap();
        for (addr, tid) in expired_futex_waiters {
            if let Some(list) = state.futex_waiters.get_mut(&addr) {
                list.retain(|&waiter| waiter != tid);
            }
        }
    }
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

fn other_runnable_exists(unicorn: &Unicorn<'_, Context>) -> bool {
    let data = unicorn.get_data();
    let threads = data.threads.lock().unwrap();
    threads.iter().any(|t| t.status == ThreadStatus::Runnable)
}

fn all_exited(unicorn: &Unicorn<'_, Context>) -> bool {
    let data = unicorn.get_data();
    let threads = data.threads.lock().unwrap();
    !threads.is_empty() && threads.iter().all(|t| matches!(t.status, ThreadStatus::Exited(_)))
}

/// sleep the host until the nearest blocked-thread deadline (or a short tick)
fn sleep_until_next_wakeup(unicorn: &Unicorn<'_, Context>) {
    let now = Instant::now();
    let data = unicorn.get_data();
    let threads = data.threads.lock().unwrap();
    let next_deadline = threads.iter().find_map(|t| match &t.status {
        ThreadStatus::Blocked(BlockReason::FutexWait { deadline, .. }) => *deadline,
        ThreadStatus::Blocked(BlockReason::SleepUntil(until)) => Some(*until),
        _ => None,
    });

    drop(threads);

    let duration = match next_deadline {
        Some(deadline) if deadline > now => deadline.duration_since(now),
        _ => IDLE_TICK,
    };
    std::thread::sleep(duration);
}
