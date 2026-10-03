//! POSIX per-process timers (`timer_create` & friends).
//!
//! The head-unit processes create monotonic/realtime timers for periodic and
//! one-shot events. Delivering timer signals correctly is not needed to get far
//! enough to reach the graphics path, so the timers here are inert: `timer_create`
//! hands out an id and the rest succeed as no-ops. A process that blocks until a
//! timer fires will simply wait for its own timeout, which is strictly better than
//! the previous `-ENOSYS`.

use crate::emulator::context::Context;
use crate::emulator::utils::pack_u32;
use std::sync::atomic::{AtomicU32, Ordering};
use unicorn_engine::Unicorn;

static NEXT_TIMER_ID: AtomicU32 = AtomicU32::new(1);

/// timer_create(clockid, evp, timerid_ptr): write a fresh id to *timerid_ptr.
pub fn timer_create(
    unicorn: &mut Unicorn<'_, Context>,
    _clock_id: u32,
    _evp: u32,
    timerid_ptr: u32,
) -> u32 {
    let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed);
    unicorn
        .mem_write(timerid_ptr as u64, &pack_u32(id))
        .unwrap();
    log::trace!(
        "{:#x}: [{}] [SYSCALL] timer_create => id {}",
        unicorn.reg_read(unicorn_engine::RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        id
    );
    0u32
}

/// timer_settime(timerid, flags, new_value, old_value): accept but do not arm.
pub fn timer_settime(
    unicorn: &mut Unicorn<'_, Context>,
    _timerid: u32,
    _flags: u32,
    _new_value: u32,
    old_value: u32,
) -> u32 {
    if old_value != 0 {
        // zero the returned itimerspec (currently disarmed)
        unicorn.mem_write(old_value as u64, &[0u8; 32]).unwrap();
    }
    0u32
}

/// timer_gettime(timerid, curr_value): report a disarmed timer.
pub fn timer_gettime(unicorn: &mut Unicorn<'_, Context>, _timerid: u32, curr_value: u32) -> u32 {
    if curr_value != 0 {
        unicorn.mem_write(curr_value as u64, &[0u8; 32]).unwrap();
    }
    0u32
}

/// timer_getoverrun(timerid): never overran.
pub fn timer_getoverrun(_unicorn: &mut Unicorn<'_, Context>, _timerid: u32) -> u32 {
    0u32
}

/// timer_delete(timerid): always succeeds (timers hold no resources here).
pub fn timer_delete(_unicorn: &mut Unicorn<'_, Context>, _timerid: u32) -> u32 {
    0u32
}
