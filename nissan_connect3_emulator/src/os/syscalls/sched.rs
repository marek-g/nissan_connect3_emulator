use crate::emulator::context::Context;
use crate::emulator::thread::Thread;
use crate::emulator::utils::pack_u32;
use std::sync::atomic::Ordering;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn sched_get_priority_min(unicorn: &mut Unicorn<Context>, policy: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sched_get_priority_min(policy = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        policy,
    );

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
        "{:#x}: [{}] [SYSCALL] sched_get_priority_min => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}

pub fn sched_get_priority_max(unicorn: &mut Unicorn<Context>, policy: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sched_get_priority_min(policy = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        policy,
    );

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
        "{:#x}: [{}] [SYSCALL] sched_get_priority_min => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
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
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sched_setscheduler(pid = {:#x}, policy = {:#x}, param_addr = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        pid,
        policy,
        param_addr,
    );

    let res = 0u32;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] sched_setscheduler => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
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
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] clone(flags = {:#x}, child_stack: {:#x}, parent_tid_ptr: {:#x}, child_tls: {:#x}, child_tid_ptr: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        flags,
        child_stack,
        parent_tid_ptr,
        child_tls,
        child_tid_ptr,
    );

    let child_tid = unicorn
        .get_data()
        .inner
        .next_thread_id
        .fetch_add(1, Ordering::Relaxed);

    if flags & 0x00200000 != 0 {
        // CLONE_CHILD_CLEARTID
        // Erase child thread ID at location child_tidptr in child memory when the child exits,
        // and do a wakeup on the futex at that address.
        log::warn!("clone() - CLONE_CHILD_CLEARTID not implemented");
    }

    if flags & 0x00100000 != 0 {
        // CLONE_PARENT_SETTID
        // Store child thread ID at location parent_tid_ptr in parent and child memory
        unicorn
            .mem_write(parent_tid_ptr as u64, &pack_u32(child_tid))
            .unwrap();
    }
    if flags & 0x01000000 != 0 {
        // CLONE_CHILD_SETTID
        // Store child thread ID at location child_tidptr in child memory
        unicorn
            .mem_write(child_tid_ptr as u64, &pack_u32(child_tid))
            .unwrap();
    }

    let (new_thread, _) = Thread::clone(unicorn, child_tid, child_tls, child_stack).unwrap();
    if let Some(threads) = unicorn.get_data().inner.threads.upgrade() {
        threads.lock().unwrap().push(new_thread);
    }

    let res = child_tid as u32;
    log::trace!(
        "{:#x}: [{}] [SYSCALL] clone => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    //std::thread::sleep(Duration::from_secs(3));

    res
}
