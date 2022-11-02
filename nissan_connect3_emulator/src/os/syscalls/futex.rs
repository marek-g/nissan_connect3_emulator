use crate::emulator::context::Context;
use crate::emulator::print::mem_dump;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn set_robust_list(unicorn: &mut Unicorn<Context>, head: u32, len: u32) -> u32 {
    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_robust_list(head = {:#x}, len: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
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
    if futex_op & 0x1 == 0 {
        // FUTEX_WAIT
        let val = vec![0u8; 4];
        unicorn.mem_write(uaddr as u64, &val).unwrap();
    } else {
        // FUTEX_WAKE
    }
    mem_dump(unicorn, uaddr, 4);

    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] futex(uaddr = {:#x}, futex_op: {:#x}, val: {:#x}, timeout: {:#x}, uaddr2: {:#x}, val3: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
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
