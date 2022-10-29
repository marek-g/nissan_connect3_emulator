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

pub fn sched_setscheduler(
    unicorn: &mut Unicorn<Context>,
    pid: u32,
    policy: u32,
    param_addr: u32,
) -> u32 {
    let res = 0u32;

    log::trace!(
        "{:#x}: [SYSCALL] sched_setscheduler(pid = {:#x}, policy = {:#x}, param_addr = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        pid,
        policy,
        param_addr,
        res
    );

    res
}

pub fn clone(
    unicorn: &mut Unicorn<Context>,
    flags: u32,
    child_stack: u32,
    parent_tid_ptr: u32,
    child_tid_ptr: u32,
    new_tls: u32,
) -> u32 {
    let res = 2u32;

    log::trace!(
        "{:#x}: [SYSCALL] clone(flags = {:#x}, child_stack: {:#x}, parent_tid_ptr: {:#x}, child_tid_ptr: {:#x}, new_tls: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        flags,
        child_stack,
        parent_tid_ptr,
        child_tid_ptr,
        new_tls,
        res
    );

    res
}
