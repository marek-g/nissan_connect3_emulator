use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn sched_get_priority_min(unicorn: &mut Unicorn<Context>, policy: u32) -> u32 {
    let res = match policy {
        0 => 0u32, // SCHED_NORMAL,
        1 => 1u32, // SCHED_FIFO,
        2 => 1u32, // SCHED_RR,
        3 => 0u32, // SCHED_BATCH,
        5 => 0u32, // SCHED_IDLE,
        6 => 0u32, // SCHED_DEADLINE,
        _ => -1i32 as u32,
    };

    log::trace!(
        "{:#x}: [SYSCALL] sched_get_priority_min(policy = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        policy,
        res
    );

    res
}

pub fn sched_get_priority_max(unicorn: &mut Unicorn<Context>, policy: u32) -> u32 {
    let res = match policy {
        0 => 0u32,  // SCHED_NORMAL,
        1 => 99u32, // SCHED_FIFO,
        2 => 99u32, // SCHED_RR,
        3 => 0u32,  // SCHED_BATCH,
        5 => 0u32,  // SCHED_IDLE,
        6 => 0u32,  // SCHED_DEADLINE,
        _ => -1i32 as u32,
    };

    log::trace!(
        "{:#x}: [SYSCALL] sched_get_priority_min(policy = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        policy,
        res
    );

    res
}
