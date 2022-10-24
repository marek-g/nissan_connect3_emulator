use crate::emulator::context::Context;
use crate::emulator::mmu::MmuExtension;
use crate::emulator::utils::mem_align_up;
use std::io::{Read, Seek};
use unicorn_engine::unicorn_const::Permission;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn brk(unicorn: &mut Unicorn<Context>, addr: u32) -> u32 {
    let res = if addr == 0 {
        unicorn.get_data().mmu.brk_mem_end
    } else {
        let brk_mem_end = unicorn.get_data().mmu.brk_mem_end;
        let new_brk_mem_end = mem_align_up(addr, None);
        if new_brk_mem_end > brk_mem_end {
            unicorn.mmu_map(
                brk_mem_end,
                new_brk_mem_end - brk_mem_end,
                Permission::all(),
                "[brk]",
                "",
            );
        } else if new_brk_mem_end < brk_mem_end {
            unicorn.mmu_unmap(new_brk_mem_end, brk_mem_end - new_brk_mem_end);
        }
        unicorn.get_data_mut().mmu.brk_mem_end = new_brk_mem_end;
        new_brk_mem_end
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

pub fn close(unicorn: &mut Unicorn<Context>, fd: u32) -> u32 {
    let res = if unicorn.get_data_mut().file_system.close(fd) {
        0
    } else {
        -1i32 as u32
    };

    log::trace!(
        "{:#x}: [SYSCALL] close(fd: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        fd,
        res
    );

    res
}

pub fn read(unicorn: &mut Unicorn<Context>, fd: u32, buf: u32, length: u32) -> u32 {
    let mut buf2 = Vec::new();
    if let Some(fileinfo) = unicorn.get_data_mut().file_system.fd_to_file(fd) {
        let file_pos = fileinfo.file.stream_position().unwrap() as u32;
        let bytes_to_read = length.min(fileinfo.file.metadata().unwrap().len() as u32 - file_pos);
        buf2.resize(bytes_to_read as usize, 0u8);
        fileinfo.file.read_exact(&mut buf2).unwrap();
    }
    let res = if buf2.len() > 0 {
        unicorn.mem_write(buf as u64, &buf2).unwrap();
        buf2.len() as u32
    } else {
        -1i32 as u32
    };

    log::trace!(
        "{:#x}: [SYSCALL] read(fd: {:#x}, buf: {:#x}, length: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        fd,
        buf,
        length,
        res
    );

    res
}
