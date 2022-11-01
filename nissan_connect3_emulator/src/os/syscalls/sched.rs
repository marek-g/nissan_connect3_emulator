use crate::emulator::context::Context;
use crate::emulator::utils::pack_u32;
use std::sync::atomic::Ordering;
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
    child_tls: u32,
    child_tid_ptr: u32,
    regs: u32,
) -> u32 {
    let parent_tid = unicorn.get_data().thread_id;
    let child_tid = unicorn
        .get_data()
        .next_thread_id
        .fetch_add(1, Ordering::Relaxed)
        + 1;

    /*print_stack(unicorn);
    mem_dump(unicorn, regs, 128);
    mem_dump(unicorn, child_stack, 128);
    disasm(
        unicorn,
        (unicorn.reg_read(RegisterARM::PC).unwrap() - 100) as u32,
        200,
    );
    let mut new_addr = vec![0u8; 4];
    unicorn.mem_read(child_stack as u64, &mut new_addr).unwrap();
    disasm(unicorn, unpack_u32(&new_addr), 200);*/

    unicorn
        .mem_write(parent_tid_ptr as u64, &pack_u32(parent_tid))
        .unwrap();
    unicorn
        .mem_write(child_tid_ptr as u64, &pack_u32(child_tid))
        .unwrap();

    //let (new_thread, join_handle) = Thread::clone(unicorn, child_tid, child_tls, child_stack);
    //unicorn.get_data_mut().threads

    let res = child_tid as u32;

    log::trace!(
        "{:#x}: [SYSCALL] clone(flags = {:#x}, child_stack: {:#x}, parent_tid_ptr: {:#x}, child_tls: {:#x}, child_tid_ptr: {:#x}, regs: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        flags,
        child_stack,
        parent_tid_ptr,
        child_tls,
        child_tid_ptr,
        regs,
        res
    );

    res
}
