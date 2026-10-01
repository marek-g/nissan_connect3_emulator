use crate::emulator::context::Context;
use crate::emulator::utils::unpack_u32;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn rt_sigaction(
    unicorn: &mut Unicorn<Context>,
    signum: u32,
    action: u32,
    old_action: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigaction(signum = {:#x}, action: {:#x}, old_action: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        signum,
        action,
        old_action,
    );

    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigaction => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}

pub fn rt_sigprocmask(
    unicorn: &mut Unicorn<Context>,
    how: u32,
    set: u32,
    old_set: u32,
    sig_set_size: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigprocmask(how: {:#x}, set: {:#x}, old_set: {:#x}, sig_set_size: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        how,
        set,
        old_set,
        sig_set_size,
    );

    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigprocmask => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}

pub fn sigaltstack(unicorn: &mut Unicorn<Context>, ss: u32, old_ss: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sigaltstack(ss: {:#x}, old_ss: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        ss,
        old_ss,
    );

    if ss != 0 {
        let mut mem = vec![0u8; 12];
        unicorn.mem_read(ss as u64, &mut mem).unwrap();
        let ss_sp = unpack_u32(&mem[0..4]);
        let ss_flags = unpack_u32(&mem[4..8]);
        let ss_size = unpack_u32(&mem[8..12]);
        log::trace!(
            "ss_sp: {:#x}, ss_flags: {:#x}, ss_size: {:#x}",
            ss_sp,
            ss_flags,
            ss_size
        );
    }

    // TODO: implement
    let res = 0i32 as u32;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] sigaltstack => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}
pub fn rt_sigtimedwait(
    unicorn: &mut Unicorn<Context>,
    set: u32,
    info: u32,
    timeout: u32,
    sig_set_size: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigtimedwait(set: {:#x}, info: {:#x}, timeout: {:#x}, sig_set_size: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        set,
        info,
        timeout,
        sig_set_size,
    );

    // TODO: implement
    let res = -11i32 as u32; // EAGAIN

    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigtimedwait => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}
