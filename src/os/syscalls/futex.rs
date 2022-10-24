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
