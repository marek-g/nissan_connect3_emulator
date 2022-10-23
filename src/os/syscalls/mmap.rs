use crate::emulator::context::Context;
use crate::emulator::mmu::MmuExtension;
use std::io::{Read, Seek, SeekFrom};
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
    flags: u32,
    mut fd: u32,
    off_t: u32,
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

    // MAP_ANONYMOUS - do not use fd
    if flags & 0x20u32 != 0 {
        fd = 0xFFFFFFFFu32;
    }

    let addr = unicorn.heap_alloc(length, permissions);

    let mut buf = Vec::new();
    if let Some(file) = unicorn.get_data_mut().file_system.fd_to_file(fd) {
        let file_pos = file.stream_position().unwrap();
        file.seek(SeekFrom::Start(off_t as u64)).unwrap();

        let bytes_to_read = length.min(file.metadata().unwrap().len() as u32 - off_t);
        buf.resize(bytes_to_read as usize, 0u8);
        file.read_exact(&mut buf).unwrap();

        file.seek(SeekFrom::Start(file_pos)).unwrap();
    }
    if buf.len() > 0 {
        unicorn.mem_write(addr as u64, &buf).unwrap();
    }

    addr
}
