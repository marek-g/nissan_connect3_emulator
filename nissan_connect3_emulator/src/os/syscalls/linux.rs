use crate::emulator::context::Context;
use crate::emulator::memory_map::GET_TLS_ADDR;
use crate::emulator::utils::pack_u32;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn set_tls(unicorn: &mut Unicorn<Context>, address: u32) -> u32 {
    let res = 0;

    unicorn
        .reg_write(RegisterARM::C13_C0_3, address as u64)
        .unwrap();

    unicorn
        .mem_write(GET_TLS_ADDR as u64 + 16, &pack_u32(address))
        .unwrap();

    log::trace!(
        "{:#x}: [SYSCALL] set_tls(addr: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        address,
        res
    );

    res
}
