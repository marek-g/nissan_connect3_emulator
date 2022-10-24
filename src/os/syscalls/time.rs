use crate::emulator::context::Context;
use crate::emulator::utils::pack_u64;
use std::time::SystemTime;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn clock_gettime(unicorn: &mut Unicorn<Context>, clock_id: u32, time_spec: u32) -> u32 {
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
        "{:#x}: [SYSCALL] clock_gettime(clock_id = {:#x}, time_spec: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        clock_id,
        time_spec,
        0
    );

    0
}
