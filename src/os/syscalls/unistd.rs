use crate::emulator::context::Context;
use crate::emulator::mmu::MmuExtension;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn brk(unicorn: &mut Unicorn<Context>, addr: u32) -> u32 {
    let res = if addr == 0 {
        unicorn.get_data().mmu.heap_mem_end
    } else {
        panic!("not implemented");
    };

    log::trace!(
        "{:#x}: [SYSCALL] brk(addr = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        addr,
        res
    );
    res
}

pub fn access(unicorn: &mut Unicorn<Context>, pathname: u32, mode: u32) -> u32 {
    let pathname = unicorn.read_string(pathname);
    let fullpathname = unicorn
        .get_data()
        .file_system
        .path_transform_to_real(&pathname);
    let exists = fullpathname.exists();
    let res = if exists { 0 } else { -1i32 as u32 };

    log::trace!(
        "{:#x}: [SYSCALL] access(pathname = {}, mode = {:#x}) => {:#x} [{}]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        pathname,
        mode,
        res,
        if exists { "FOUND" } else { "NOT FOUND" }
    );
    res
}
