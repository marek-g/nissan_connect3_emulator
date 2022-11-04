use crate::emulator::context::Context;
use crate::emulator::memory_map::GET_TLS_ADDR;
use crate::emulator::utils::pack_u32;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn set_tls(unicorn: &mut Unicorn<Context>, address: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_tls(addr: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        address,
    );

    unicorn
        .reg_write(RegisterARM::C13_C0_3, address as u64)
        .unwrap();

    unicorn
        .mem_write(GET_TLS_ADDR as u64 + 16, &pack_u32(address))
        .unwrap();

    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] => {:#x} (set_tls)",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}
