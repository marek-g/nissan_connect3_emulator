use crate::emulator::context::Context;
use crate::emulator::mmu::MmuExtension;
use unicorn_engine::unicorn_const::Permission;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn mmap(
    unicorn: &mut Unicorn<Context>,
    addr: u32,
    length: u32,
    prot: u32,
    flags: u32,
    fd: u32,
    off_t: u32,
) -> u32 {
    let res = mmapx(unicorn, addr, length, prot, flags, fd, off_t);
    log::trace!("{:#x} [SYSCALL] mmap(addr = {:#x}, length = {:#x}, prot = {:#x}, flags = {:#x}, fd = {:#x}, off_t: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        addr, length, prot, flags, fd, off_t, res);
    res
}

pub fn mmap2(
    unicorn: &mut Unicorn<Context>,
    addr: u32,
    length: u32,
    prot: u32,
    flags: u32,
    fd: u32,
    pgoffset: u32,
) -> u32 {
    let res = mmapx(unicorn, addr, length, prot, flags, fd, pgoffset * 0x1000);
    log::trace!("{:#x} [SYSCALL] mmap2(addr = {:#x}, length = {:#x}, prot = {:#x}, flags = {:#x}, fd = {:#x}, pgoffset: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        addr, length, prot, flags, fd, pgoffset, res);
    res
}

fn mmapx(
    unicorn: &mut Unicorn<Context>,
    addr: u32,
    length: u32,
    prot: u32,
    _flags: u32,
    _fd: u32,
    _off_t: u32,
) -> u32 {
    if addr != 0 {
        panic!("not implemented");
    }

    let mut permissions = Permission::NONE;
    if prot & 1 != 0 {
        permissions |= Permission::READ;
    }
    if prot & 2 != 0 {
        permissions |= Permission::WRITE;
    }
    if prot & 4 != 0 {
        permissions |= Permission::EXEC;
    }

    unicorn.heap_alloc(length, permissions)
}
