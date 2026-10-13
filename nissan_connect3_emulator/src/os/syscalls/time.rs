use crate::emulator::context::Context;
use crate::emulator::thread::{BlockReason, ThreadAction};
use crate::emulator::utils::{pack_u32, unpack_u32};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};
use unicorn_engine::{RegisterARM, Unicorn};

/// The clocks the unit counts from power-on (`CLOCK_MONOTONIC` and its raw
/// variant): the emulator's own start is its boot time.  Reporting host wall
/// clock here would make the guest's `tv_sec * 1000` millisecond conversion wrap
/// inside `u32`, so every elapsed-time comparison in every process would jump.
fn clock_value(clock_id: u32) -> Duration {
    if matches!(clock_id, 1 | 4) {
        static BOOT: OnceLock<Instant> = OnceLock::new();
        BOOT.get_or_init(Instant::now).elapsed()
    } else {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
    }
}

pub fn clock_gettime(unicorn: &mut Unicorn<'_, Context>, clock_id: u32, time_spec: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] clock_gettime(clock_id = {:#x}, time_spec: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        clock_id,
        time_spec,
    );

    let now = clock_value(clock_id);

    // ARM EABI `struct timespec` is `{ time_t tv_sec; long tv_nsec; }` with both
    // members 32 bits wide, so the two fields are written as separate words: the
    // 64-bit form used to overwrite `tv_nsec` with the (zero) high half of
    // `tv_sec`, which quantised every sub-second elapsed-time reading in the
    // guest to whole seconds (`OSAL_ClockGetElapsedTime` reported 0 for up to a
    // second at a time and the map engine kept re-pointing its camera because of
    // it).
    unicorn
        .mem_write(time_spec as u64, &pack_u32(now.as_secs() as u32))
        .unwrap();

    unicorn
        .mem_write(
            (time_spec + 4) as u64,
            &pack_u32((now.as_nanos() % 1000000000u128) as u32),
        )
        .unwrap();

    log::trace!(
        "{:#x}: [{}] [SYSCALL] clock_gettime => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        0
    );

    0
}

pub fn gettimeofday(unicorn: &mut Unicorn<'_, Context>, time_val: u32, time_zone: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] gettimeofday(time_val = {:#x}, time_zone: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        time_val,
        time_zone,
    );

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();

    if time_val != 0 {
        unicorn
            .mem_write(time_val as u64, &pack_u32(now.as_secs() as u32))
            .unwrap();

        unicorn
            .mem_write(
                (time_val + 4) as u64,
                &pack_u32((now.as_nanos() % 1000000000u128) as u32),
            )
            .unwrap();
    }

    if time_zone != 0 {
        let buf = vec![0u8; 8];
        unicorn.mem_write(time_zone as u64, &buf).unwrap();
    }

    log::trace!(
        "{:#x}: [{}] [SYSCALL] gettimeofday => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        0
    );

    0
}

pub fn nanosleep(unicorn: &mut Unicorn<'_, Context>, req: u32, rem: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] nanosleep(req = {:#x}, rem: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        req,
        rem,
    );

    let mut buf = vec![0u8; 4];
    unicorn.mem_read(req as u64, &mut buf).unwrap();
    let seconds = unpack_u32(&buf);
    unicorn.mem_read(req as u64 + 4, &mut buf).unwrap();
    let nanoseconds = unpack_u32(&buf);

    // block the current guest thread (other threads keep running) until the deadline
    let duration = Duration::new(seconds as u64, nanoseconds as u32);
    unicorn
        .get_data()
        .set_action(ThreadAction::Block(BlockReason::SleepUntil(
            Instant::now() + duration,
        )));

    log::trace!(
        "{:#x}: [{}] [SYSCALL] nanosleep => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        0
    );

    0
}
