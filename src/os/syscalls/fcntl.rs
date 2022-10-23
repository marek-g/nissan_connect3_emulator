use crate::emulator::context::Context;
use crate::emulator::mmu::MmuExtension;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn open(unicorn: &mut Unicorn<Context>, pathname: u32, flags: u32, mode: u32) -> u32 {
    let pathname = unicorn.read_string(pathname);

    let fd = unicorn.get_data_mut().file_system.open(&pathname);

    if mode != 0x0 && mode != 0x1 {
        panic!("mode not implemented");
    }

    log::trace!(
        "{:#x}: [SYSCALL] open(pathname = {}, flags: {:#x}, mode: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        pathname,
        flags,
        mode,
        fd
    );

    fd
}
