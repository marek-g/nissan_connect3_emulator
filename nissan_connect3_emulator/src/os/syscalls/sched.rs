use crate::emulator::context::Context;
use crate::emulator::memory_map::{STACK_BASE, STACK_SIZE};
use crate::emulator::thread::{GuestThread, ThreadStatus};
use crate::emulator::utils::pack_u32;
use std::sync::atomic::Ordering;
use unicorn_engine::unicorn_const::Prot;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn sched_get_priority_min(unicorn: &mut Unicorn<'_, Context>, policy: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sched_get_priority_min(policy = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
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
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn sched_get_priority_max(unicorn: &mut Unicorn<'_, Context>, policy: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sched_get_priority_min(policy = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
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
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

/// sched_getparam(pid, param) - kernel/sched.c: sys_sched_getparam
/// copies the thread's sched_param (priority 0 for SCHED_NORMAL) to user space
pub fn sched_getparam(unicorn: &mut Unicorn<'_, Context>, pid: u32, param_addr: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sched_getparam(pid = {:#x}, param_addr = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        pid,
        param_addr,
    );

    let exists = {
        let data = unicorn.get_data();
        let threads = data.threads.lock().unwrap();
        threads.iter().any(|t| t.id == pid)
    };

    let res: u32 = if !exists {
        -3i32 as u32 // ESRCH
    } else {
        match unicorn.mem_write(param_addr as u64, &pack_u32(0)) {
            Ok(()) => 0u32,
            Err(_) => -14i32 as u32, // EFAULT
        }
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] sched_getparam => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        res
    );

    res
}

/// sched_getscheduler(pid) - kernel/sched.c: sys_sched_getscheduler
/// returns the scheduling policy of the thread (SCHED_NORMAL for all guest threads)
pub fn sched_getscheduler(unicorn: &mut Unicorn<'_, Context>, pid: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sched_getscheduler(pid = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        pid,
    );

    let exists = {
        let data = unicorn.get_data();
        let threads = data.threads.lock().unwrap();
        threads.iter().any(|t| t.id == pid)
    };

    let res: u32 = if !exists {
        -3i32 as u32 // ESRCH
    } else {
        0u32 // SCHED_NORMAL
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] sched_getscheduler => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        res
    );

    res
}

pub fn sched_setscheduler(
    unicorn: &mut Unicorn<'_, Context>,
    pid: u32,
    policy: u32,
    param_addr: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sched_setscheduler(pid = {:#x}, policy = {:#x}, param_addr = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        pid,
        policy,
        param_addr,
    );

    let res = 0u32;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] sched_setscheduler => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn clone(
    unicorn: &mut Unicorn<'_, Context>,
    flags: u32,
    child_stack: u32,
    parent_tid_ptr: u32,
    child_tls: u32,
    child_tid_ptr: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] clone(flags = {:#x}, child_stack: {:#x}, parent_tid_ptr: {:#x}, child_tls: {:#x}, child_tid_ptr: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
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

    let clear_child_tid = if flags & 0x00200000 != 0 {
        // CLONE_CHILD_CLEARTID: zero the tid field at child_tidptr and wake one futex
        // waiter there when the child exits normally (kernel/fork.c release_task)
        Some(child_tid_ptr)
    } else {
        None
    };

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

    // snapshot the current (parent) CPU state - it is the base for the child
    let parent_context = unicorn.context_init().unwrap();

    // if there is no child_stack, clone the parent's stack
    let mut child_stack = child_stack;
    if child_stack == 0 {
        let mmu_arc = unicorn.get_data().inner.mmu.clone();
        let new_base = mmu_arc
            .lock()
            .unwrap()
            .heap_alloc(unicorn, STACK_SIZE, Prot::READ | Prot::WRITE, "");
        let mut buf = vec![0u8; STACK_SIZE as usize];
        unicorn.mem_read(STACK_BASE as u64, &mut buf).unwrap();
        unicorn.mem_write(new_base as u64, &buf).unwrap();

        let parent_sp = unicorn.reg_read(RegisterARM::SP).unwrap() as u32;
        child_stack = match parent_sp.checked_sub(STACK_BASE) {
            Some(delta) => new_base + delta,
            None => new_base,
        };
    }

    // temporarily set the child state on the shared vCPU, snapshot it,
    // then restore the parent state (we are inside the parent's syscall hook)
    unicorn
        .reg_write(RegisterARM::SP as i32, child_stack as u64)
        .unwrap();
    unicorn
        .reg_write(RegisterARM::C13_C0_3 as i32, child_tls as u64)
        .unwrap();
    // set 0 in R0 (result from syscall)
    unicorn.reg_write(RegisterARM::R0 as i32, 0u64).unwrap();

    let child_context = unicorn.context_init().unwrap();
    let pc = unicorn.reg_read(RegisterARM::PC).unwrap() as u32;
    unicorn.context_restore(&parent_context).unwrap();

    {
        let data = unicorn.get_data();
        data.threads.lock().unwrap().push(GuestThread {
            id: child_tid,
            status: ThreadStatus::Runnable,
            cpu_context: Some(child_context),
            pc,
            pending_result: None,
            clear_child_tid,
        });
    }

    let res = child_tid as u32;
    log::info!(
        "========== Clone thread [{}] at address: {:#x} ==========",
        child_tid,
        pc
    );
    log::trace!(
        "{:#x}: [{}] [SYSCALL] clone => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        res
    );

    res
}
