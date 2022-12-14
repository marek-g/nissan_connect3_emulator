use crate::emulator::context::Context;
use crate::emulator::utils::{pack_u32, pack_u64, unpack_u32};
use std::time::{Duration, SystemTime};
use unicorn_engine::{RegisterARM, Unicorn};

pub fn clock_gettime(unicorn: &mut Unicorn<Context>, clock_id: u32, time_spec: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] clock_gettime(clock_id = {:#x}, time_spec: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        clock_id,
        time_spec,
    );

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();

    unicorn
        .mem_write(time_spec as u64, &pack_u64(now.as_secs()))
        .unwrap();

    unicorn
        .mem_write(
            (time_spec + 8) as u64,
            &pack_u64((now.as_nanos() % 1000000000u128) as u64),
        )
        .unwrap();

    log::trace!(
        "{:#x}: [{}] [SYSCALL] clock_gettime => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        0
    );

    0
}

pub fn gettimeofday(unicorn: &mut Unicorn<Context>, time_val: u32, time_zone: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] gettimeofday(time_val = {:#x}, time_zone: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
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
        unicorn.get_data().inner.thread_id,
        0
    );

    0
}

pub fn nanosleep(unicorn: &mut Unicorn<Context>, req: u32, rem: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] nanosleep(req = {:#x}, rem: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        req,
        rem,
    );

    let mut buf = vec![0u8; 4];
    unicorn.mem_read(req as u64, &mut buf).unwrap();
    let seconds = unpack_u32(&buf);
    unicorn.mem_read(req as u64 + 4, &mut buf).unwrap();
    let nanoseconds = unpack_u32(&buf);

    std::thread::sleep(Duration::from_nanos(
        (seconds as u64) * 1000000000u64 + nanoseconds as u64,
    ));

    log::trace!(
        "{:#x}: [{}] [SYSCALL] nanosleep => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        0
    );

    0
}
