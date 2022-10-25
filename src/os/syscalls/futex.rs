use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn set_robust_list(unicorn: &mut Unicorn<Context>, head: u32, len: u32) -> u32 {
    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [SYSCALL] set_robust_list(head = {:#x}, len: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        head,
        len,
        res
    );

    res
}

pub fn futex(
    unicorn: &mut Unicorn<Context>,
    uaddr: u32,
    futex_op: u32,
    val: u32,
    timeout: u32,
    uaddr2: u32,
    val3: u32,
) -> u32 {
    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [SYSCALL] futex(uaddr = {:#x}, futex_op: {:#x}, val: {:#x}, timeout: {:#x}, uaddr2: {:#x}, val3: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        uaddr,
        futex_op,
        val,
        timeout,
        uaddr2,
        val3,
        res
    );

    res
}
