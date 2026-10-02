use crate::emulator::context::Context;
use crate::emulator::thread::{BlockReason, ThreadAction, ThreadStatus};
use crate::emulator::utils::unpack_u32;
use std::time::{Duration, Instant};
use unicorn_engine::{RegisterARM, Unicorn};

pub fn set_robust_list(unicorn: &mut Unicorn<'_, Context>, head: u32, len: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_robust_list(head = {:#x}, len: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        head,
        len,
    );

    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_robust_list => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        res
    );

    res
}

pub fn futex(
    unicorn: &mut Unicorn<'_, Context>,
    uaddr: u32,
    futex_op: u32,
    val: u32,
    timeout: u32,
    _uaddr2: u32,
    _val3: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] futex(uaddr = {:#x}, futex_op: {:#x}, val: {:#x}, timeout: {:#x}, uaddr2: {:#x}, val3: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        uaddr,
        futex_op,
        val,
        timeout,
        _uaddr2,
        _val3,
    );

    if futex_op & 0x80 == 0 {
        // FUTEX_PRIVATE_FLAG - if not set synchronization between processes is needed
        log::error!("futex without FUTEX_PRIVATE_FLAG not implemented");
    }

    let res = match futex_op & 0x7F {
        0x00 | 0x09 => {
            // FUTEX_WAIT / FUTEX_WAIT_BITSET
            let mut buf = [0u8; 4];
            unicorn.mem_read(uaddr as u64, &mut buf).unwrap();
            let val_read = unpack_u32(&buf);

            if val_read != val {
                -11i32 as u32 // EAGAIN
            } else {
                // wait - block the current guest thread until woken by FUTEX_WAKE
                // or by the timeout (whichever comes first)
                log::trace!(
                    "{:#x}: [{}] [SYSCALL] futex - wait",
                    unicorn.reg_read(RegisterARM::PC).unwrap(),
                    unicorn.get_data().thread_id()
                );

                let deadline = read_timeout_deadline(unicorn, timeout);
                let data = unicorn.get_data();
                let thread_id = data.thread_id();
                {
                    let state = &mut data.inner.sys_calls_state.lock().unwrap();
                    state
                        .futex_waiters
                        .entry(uaddr)
                        .or_insert(Vec::new())
                        .push(thread_id);
                }

                unicorn.get_data().set_action(ThreadAction::Block(BlockReason::FutexWait {
                    addr: uaddr,
                    deadline,
                }));

                log::trace!(
                    "{:#x}: [{}] [SYSCALL] futex - woken up",
                    unicorn.reg_read(RegisterARM::PC).unwrap(),
                    unicorn.get_data().thread_id()
                );

                0u32
            }
        }
        0x01 | 0x0A | 0x05 => {
            // FUTEX_WAKE / FUTEX_WAKE_BITSET / FUTEX_WAKE_OP (op semantics not
            // evaluated - a plain wake of `val` waiters covers the common cases)
            wake_waiters(unicorn, uaddr, val)
        }
        op => {
            log::error!("unsupported futex operation: {}", op);
            -38i32 as u32 // ENOSYS
        }
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] futex => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        res
    );

    res
}

/// wake at most `val` blocked waiters of the futex at `uaddr`
pub(crate) fn wake_waiters(unicorn: &mut Unicorn<'_, Context>, uaddr: u32, val: u32) -> u32 {
    let data = unicorn.get_data();

    // pop up to `val` waiters (LIFO)
    let woken_candidates: Vec<u32> = {
        let mut state = data.sys_calls_state.lock().unwrap();
        let list = state.futex_waiters.entry(uaddr).or_insert(Vec::new());
        (0..val.min(list.len() as u32))
            .rev()
            .filter_map(|_| list.pop())
            .collect()
    };

    let mut count = 0;
    {
        let mut threads = data.threads.lock().unwrap();
        for tid in woken_candidates {
            if let Some(thread) = threads.iter_mut().find(|t| t.id == tid) {
                // only wake if the thread is still waiting on this futex
                // (it may have timed out and moved on in the meantime)
                if matches!(
                    &thread.status,
                    ThreadStatus::Blocked(BlockReason::FutexWait { addr, .. }) if *addr == uaddr
                ) {
                    thread.status = ThreadStatus::Runnable;
                    count += 1;
                }
            }
        }
    }

    count as u32
}

/// read an optional `struct timespec` timeout from guest memory
fn read_timeout_deadline(unicorn: &Unicorn<'_, Context>, timeout: u32) -> Option<Instant> {
    if timeout == 0 {
        return None;
    }

    let mut buf = [0u8; 4];
    unicorn.mem_read(timeout as u64, &mut buf).unwrap();
    let seconds = unpack_u32(&buf) as u64;
    unicorn.mem_read(timeout as u64 + 4, &mut buf).unwrap();
    let nanoseconds = unpack_u32(&buf) as u64;

    Some(Instant::now() + Duration::new(seconds, nanoseconds as u32))
}
