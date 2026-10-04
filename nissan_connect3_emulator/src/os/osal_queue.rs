use crate::common::osal_queues::OsalQueueService;
use crate::emulator::context::Context;
use crate::emulator::thread::{BlockReason, ThreadStatus};
use crate::emulator::utils::pack_u32;
use std::time::Instant;
use unicorn_engine::Unicorn;

/// Re-evaluate a guest thread blocked inside an intercepted OSAL queue wait.
///
/// This is the Unicorn/threading glue around the pure OSAL queue model: the
/// scheduler stops the VM, so guest-memory delivery happens in the owning
/// process' own address space, exactly like completed POSIX mqueue waits.
pub(crate) fn finish_guest_wait(unicorn: &mut Unicorn<'_, Context>, tid: u32, now: Instant) {
    let reason = {
        let threads = unicorn.get_data().threads.lock().unwrap();
        threads.iter().find(|thread| thread.id == tid).and_then(|thread| match thread.status {
            ThreadStatus::Blocked(reason) => Some(reason),
            _ => None,
        })
    };

    let Some(BlockReason::OsalQueueReceive {
        queue_id,
        msg_ptr,
        msg_len,
        prio_ptr,
        deadline,
    }) = reason
    else {
        return;
    };

    let message = {
        let mut state = unicorn.get_data().namespace.lock().unwrap();
        let message = OsalQueueService::pop_guest_message(&mut state.mq, queue_id, msg_len as usize);
        if message.is_some() || deadline.map(|deadline| deadline <= now).unwrap_or(false) {
            state.mq.remove_waiter(queue_id, tid);
        }
        message
    };

    match message {
        Some(message) => {
            if !message.data.is_empty() {
                unicorn.mem_write(msg_ptr as u64, &message.data).unwrap();
            }
            if prio_ptr != 0 {
                unicorn.mem_write(prio_ptr as u64, &pack_u32(message.priority)).unwrap();
            }
            set_runnable_with_result(unicorn, tid, message.data.len() as u32);
        }
        None if deadline.map(|deadline| deadline <= now).unwrap_or(false) => {
            set_runnable_with_result(unicorn, tid, 0)
        }
        None => {}
    }
}

fn set_runnable_with_result(unicorn: &mut Unicorn<'_, Context>, tid: u32, result: u32) {
    let mut threads = unicorn.get_data().threads.lock().unwrap();
    if let Some(thread) = threads.iter_mut().find(|thread| thread.id == tid) {
        if matches!(thread.status, ThreadStatus::Blocked(_)) {
            thread.status = ThreadStatus::Runnable;
            thread.pending_result = Some(result);
        }
    }
}
