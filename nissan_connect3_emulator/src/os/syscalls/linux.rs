use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn set_tls(unicorn: &mut Unicorn<Context>, address: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_tls(addr: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        address,
    );

    // the TLS register is per-thread CPU state - it is saved and restored
    // with each thread's context snapshot by the scheduler
    unicorn
        .reg_write(RegisterARM::C13_C0_3, address as u64)
        .unwrap();

    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_tls => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        res
    );

    res
}
