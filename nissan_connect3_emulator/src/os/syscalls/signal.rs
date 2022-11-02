use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn rt_sigaction(
    unicorn: &mut Unicorn<Context>,
    signum: u32,
    action: u32,
    old_action: u32,
) -> u32 {
    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigaction(signum = {:#x}, action: {:#x}, old_action: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        signum,
        action,
        old_action,
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
    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] rt_sigprocmask(how: {:#x}, set: {:#x}, old_set: {:#x}, sig_set_size: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        how,
        set,
        old_set,
        sig_set_size,
        res
    );

    res
}

pub fn sigaltstack(unicorn: &mut Unicorn<Context>, ss: u32, old_ss: u32) -> u32 {
    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] sigaltstack(ss: {:#x}, old_ss: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        ss,
        old_ss,
        res
    );

    res
}
