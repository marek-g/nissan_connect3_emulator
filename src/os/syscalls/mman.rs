use crate::emulator::context::Context;
use crate::emulator::mmu::MmuExtension;
use crate::emulator::utils::{mem_align_down, mem_align_up};
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

pub fn munmap(unicorn: &mut Unicorn<Context>, addr: u32, length: u32) -> u32 {
    let res = 0u32;

    unicorn
        .mem_unmap(addr as u64, mem_align_up(length, None) as libc::size_t)
        .unwrap();

    log::trace!(
        "{:#x} [SYSCALL] munmap(addr = {:#x}, len = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        addr,
        length,
        res
    );
    res
}

pub fn mprotect(unicorn: &mut Unicorn<Context>, addr: u32, len: u32, prot: u32) -> u32 {
    let res = 0u32; //mmapx(unicorn, addr, length, prot, flags, fd, pgoffset * 0x1000);

    unicorn
        .mem_protect(
            addr as u64,
            mem_align_up(len, None) as libc::size_t,
            prot_to_permission(prot),
        )
        .unwrap();

    log::trace!(
        "{:#x} [SYSCALL] mprotect(addr = {:#x}, len = {:#x}, prot = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        addr,
        len,
        prot,
        res
    );
    res
}

pub fn mincore(unicorn: &mut Unicorn<Context>, addr: u32, length: u32, vec: u32) -> u32 {
    let bytes = vec![1u8; ((length + 0x1000 - 1) / 0x1000) as usize];
    unicorn.mem_write(vec as u64, &bytes).unwrap();
    log::trace!(
        "{:#x} [SYSCALL] mincore(addr = {:#x}, length = {:#x}, vec = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        addr,
        length,
        vec,
        0
    );
    0
}

fn mmapx(
    unicorn: &mut Unicorn<Context>,
    addr: u32,
    mut length: u32,
    prot: u32,
    flags: u32,
    mut fd: u32,
    off_t: u32,
) -> u32 {
    let perms = prot_to_permission(prot);

    // MAP_ANONYMOUS - do not use fd
    if flags & 0x20u32 != 0 {
        fd = 0xFFFFFFFFu32;
    }

    if addr != mem_align_down(addr, None) {
        panic!("wrong address alignment for mmap");
    }

    length = mem_align_up(length, None);

    // load file
    let mut buf = Vec::new();
    let mut filepath = String::new();
    if let Some(fileinfo) = unicorn..get_data_mut().file_system.fd_to_file(fd) {
        filepath = fileinfo.filepath.clone();

        let file_pos = fileinfo.file.stream_position().unwrap();
        fileinfo.file.seek(SeekFrom::Start(off_t as u64)).unwrap();

        let bytes_to_read = length.min(fileinfo.file.metadata().unwrap().len() as u32 - off_t);
        buf.resize(bytes_to_read as usize, 0u8);
        fileinfo.file.read_exact(&mut buf).unwrap();

        fileinfo.file.seek(SeekFrom::Start(file_pos)).unwrap();
    }

    // allocate memory
    let addr = if flags & 0x10 != 0 || addr != 0 {
        // MAP_FIXED - don't interpret addr as a hint
        unicorn.mmu_map(addr, length, perms, "[heap (fixed addr)]", &filepath);
        addr
    } else {
        unicorn.heap_alloc(length, perms, &filepath)
    };

    // write file
    if buf.len() > 0 {
        unicorn.mem_write(addr as u64, &buf).unwrap();
    }

    addr
}

fn prot_to_permission(prot: u32) -> Permission {
    let mut perms = Permission::NONE;
    if prot & 1 != 0 {
        perms |= Permission::READ;
    }
    if prot & 2 != 0 {
        perms |= Permission::WRITE;
    }
    if prot & 4 != 0 {
        perms |= Permission::EXEC;
    }

    perms
}
