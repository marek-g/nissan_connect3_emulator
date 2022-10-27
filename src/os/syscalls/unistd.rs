use crate::emulator::context::Context;
use crate::emulator::mmu::MmuExtension;
use crate::emulator::utils::{mem_align_up, pack_u16, pack_u64};
use crate::file_system::FileType;
use std::path::Path;
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

pub fn access(unicorn: &mut Unicorn<Context>, path_name: u32, mode: u32) -> u32 {
    let path_name = unicorn.read_string(path_name);
    let exists = unicorn
        .get_data()
        .file_system
        .borrow_mut()
        .exists(&path_name);
    let res = if exists { 0 } else { -1i32 as u32 };

    log::trace!(
        "{:#x}: [SYSCALL] access(pathname = {}, mode = {:#x}) => {:#x} [{}]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        path_name,
        mode,
        res,
        if exists { "FOUND" } else { "NOT FOUND" }
    );
    res
}

pub fn close(unicorn: &mut Unicorn<Context>, fd: u32) -> u32 {
    let res = if let Ok(_) = unicorn.get_data().file_system.borrow_mut().close(fd as i32) {
        0u32
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
    let mut buf2 = vec![0u8; length as usize];
    let file_system = &mut unicorn.get_data_mut().file_system.clone();
    let res = if file_system.borrow().is_open(fd as i32) {
        match file_system.borrow_mut().read(fd as i32, &mut buf2) {
            Ok(len) => {
                unicorn
                    .mem_write(buf as u64, &buf2[0..len as usize])
                    .unwrap();
                len as u32
            }
            Err(_) => -1i32 as u32,
        }
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

pub fn write(unicorn: &mut Unicorn<Context>, fd: u32, buf: u32, length: u32) -> u32 {
    let mut buf2 = vec![0u8; length as usize];
    unicorn.mem_read(buf as u64, &mut buf2).unwrap();
    let file_system = &mut unicorn.get_data().file_system.clone();
    let is_open = file_system.borrow().is_open(fd as i32);
    let res = if is_open {
        match file_system.borrow_mut().write(fd as i32, &buf2) {
            Ok(len) => {
                unicorn
                    .mem_write(buf as u64, &buf2[0..len as usize])
                    .unwrap();
                len as u32
            }
            Err(_) => -1i32 as u32,
        }
    } else {
        -1i32 as u32
    };

    log::trace!(
        "{:#x}: [SYSCALL] write(fd: {:#x}, buf: {:#x}, length: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        fd,
        buf,
        length,
        res
    );

    res
}

pub fn getdents64(unicorn: &mut Unicorn<Context>, fd: u32, dirp: u32, count: u32) -> u32 {
    let file_system = unicorn.get_data().file_system.clone();

    let dir_info = file_system.borrow_mut().get_file_info(fd as i32);
    let res = if let Some(dir_info) = dir_info {
        let read_dir = file_system.borrow_mut().read_dir(&dir_info.file_path);
        if let Ok(mut dir_entries) = read_dir {
            let mut res = Vec::new();

            dir_entries.push(".".to_string());
            dir_entries.push("..".to_string());

            for dir_entry in dir_entries {
                let full_path = Path::new(&dir_info.file_path).join(&dir_entry);
                let full_path = full_path.to_str().unwrap();

                if let Some(file_info) = file_system
                    .borrow_mut()
                    .get_file_info_from_filepath(full_path)
                {
                    let rec_len = 20u16 + dir_entry.as_bytes().len() as u16;

                    // d_ino - inode number
                    res.extend_from_slice(&pack_u64(file_info.inode));

                    // d_off - offset to next structure, not implemented (not sure how exactly)
                    res.extend_from_slice(&pack_u64(0u64));

                    // d_reclen - size of this dirent
                    res.extend_from_slice(&pack_u16(rec_len));

                    // d_type - file type
                    res.push(match file_info.file_details.file_type {
                        FileType::File => 8u8,
                        FileType::Link => 10u8,
                        FileType::Directory => 4u8,
                        FileType::Socket => 12u8,
                        FileType::BlockDevice => 6u8,
                        FileType::CharacterDevice => 2u8,
                        FileType::NamedPipe => 1u8,
                    });

                    // d_name
                    res.extend_from_slice(dir_entry.as_bytes());
                    res.push(0u8);
                }
            }

            if res.len() > count as usize {
                22u32 // EINVAL
            } else {
                unicorn.mem_write(dirp as u64, &res).unwrap();
                res.len() as u32
            }
        } else {
            -1i32 as u32
        }
    } else {
        -1i32 as u32
    };

    log::trace!(
        "{:#x}: [SYSCALL] getdents64(fd: {:#x}, dirp: {:#x}, count: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        fd,
        dirp,
        count,
        res
    );

    res
}

pub fn set_tid_address(unicorn: &mut Unicorn<Context>, addr: u32) -> u32 {
    // TODO: implement
    let res = 1;

    log::trace!(
        "{:#x}: [SYSCALL] set_tid_address(addr: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        addr,
        res
    );

    res
}

pub fn get_tid(unicorn: &mut Unicorn<Context>) -> u32 {
    // TODO: implement
    let res = 1;

    log::trace!(
        "{:#x}: [SYSCALL] get_tid() => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        res
    );

    res
}

pub fn exit_group(unicorn: &mut Unicorn<Context>, status: u32) -> u32 {
    unicorn.emu_stop().unwrap();

    log::trace!(
        "{:#x}: [SYSCALL] exit_group(status: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        status,
        0u32
    );

    0u32
}
