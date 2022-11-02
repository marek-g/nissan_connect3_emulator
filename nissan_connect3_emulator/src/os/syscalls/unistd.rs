use crate::emulator::context::Context;
use crate::emulator::mmu::MmuExtension;
use crate::emulator::utils::{mem_align_up, pack_u16, pack_u64};
use crate::file_system::{FileType, MountFileSystem};
use crate::os::syscalls::SysCallError;
use std::path::Path;
use std::sync::{Arc, Mutex};
use unicorn_engine::unicorn_const::Permission;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn brk(unicorn: &mut Unicorn<Context>, addr: u32) -> u32 {
    let res = if addr == 0 {
        unicorn.get_data().inner.mmu.lock().unwrap().brk_mem_end
    } else {
        let brk_mem_end = unicorn.get_data().inner.mmu.lock().unwrap().brk_mem_end;
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
        unicorn.get_data().inner.mmu.lock().unwrap().brk_mem_end = new_brk_mem_end;
        new_brk_mem_end
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] brk(addr = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        addr,
        res
    );
    res
}

pub fn access(unicorn: &mut Unicorn<Context>, path_name: u32, mode: u32) -> u32 {
    let path_name = unicorn.read_string(path_name);
    let exists = unicorn
        .get_data()
        .inner
        .file_system
        .lock()
        .unwrap()
        .exists(&path_name);
    let res = if exists { 0 } else { -1i32 as u32 };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] access(pathname = {}, mode = {:#x}) => {:#x} [{}]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        path_name,
        mode,
        res,
        if exists { "FOUND" } else { "NOT FOUND" }
    );
    res
}

pub fn close(unicorn: &mut Unicorn<Context>, fd: u32) -> u32 {
    unicorn
        .get_data()
        .inner
        .sys_calls_state
        .lock()
        .unwrap()
        .get_dents_list
        .remove(&fd);

    let res = if let Ok(_) = unicorn
        .get_data()
        .inner
        .file_system
        .lock()
        .unwrap()
        .close(fd as i32)
    {
        0u32
    } else {
        -1i32 as u32
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] close(fd: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        fd,
        res
    );

    res
}

pub fn read(unicorn: &mut Unicorn<Context>, fd: u32, buf: u32, length: u32) -> u32 {
    let mut buf2 = vec![0u8; length as usize];
    let file_system = &mut unicorn.get_data().inner.file_system.clone();
    let res = if file_system.lock().unwrap().is_open(fd as i32) {
        match file_system.lock().unwrap().read(fd as i32, &mut buf2) {
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
        "{:#x}: [{}] [SYSCALL] read(fd: {:#x}, buf: {:#x}, length: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
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
    let file_system = &mut unicorn.get_data().inner.file_system.clone();
    let is_open = file_system.lock().unwrap().is_open(fd as i32);
    let res = if is_open {
        match file_system.lock().unwrap().write(fd as i32, &buf2) {
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
        "{:#x}: [{}] [SYSCALL] write(fd: {:#x}, buf: {:#x}, length: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        fd,
        buf,
        length,
        res
    );

    res
}

pub fn getdents64(unicorn: &mut Unicorn<Context>, fd: u32, dirp: u32, count: u32) -> u32 {
    let file_system = unicorn.get_data().inner.file_system.clone();

    // get dir entries to iterate through
    let dir_entries = if let Some(prev_list) = unicorn
        .get_data()
        .inner
        .sys_calls_state
        .lock()
        .unwrap()
        .get_dents_list
        .remove(&fd)
    {
        // we have previously stored list - let's continue iteration over it
        Some(prev_list)
    } else {
        // we are called anew, let's ask filesystem for file list
        let dir_info = file_system.lock().unwrap().get_file_info(fd as i32);
        if let Some(dir_info) = dir_info {
            let read_dir = file_system.lock().unwrap().read_dir(&dir_info.file_path);
            if let Ok(mut dir_entries) = read_dir {
                dir_entries.push(".".to_string());
                dir_entries.push("..".to_string());
                Some(dir_entries)
            } else {
                None
            }
        } else {
            None
        }
    };

    let res = get_dents_internal(unicorn, fd, dirp, count, file_system, dir_entries);

    log::trace!(
        "{:#x}: [{}] [SYSCALL] getdents64(fd: {:#x}, dirp: {:#x}, count: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        fd,
        dirp,
        count,
        res
    );

    res
}

fn get_dents_internal(
    unicorn: &mut Unicorn<Context>,
    fd: u32,
    dirp: u32,
    count: u32,
    file_system: Arc<Mutex<MountFileSystem>>,
    dir_entries: Option<Vec<String>>,
) -> u32 {
    // check if the previous call returned all the results
    // (this situation is marked as an empty vector
    // and in that case return 0
    if let Some(dir_entries) = &dir_entries {
        if dir_entries.len() == 0 {
            return 0u32;
        }
    }

    // iterate through entries
    if let Some(dir_entries) = dir_entries {
        let mut res = Vec::new();

        let dir_info = file_system.lock().unwrap().get_file_info(fd as i32);
        if let Some(dir_info) = dir_info {
            let mut not_enough_space = false;
            let mut no_copied_entries = 0;
            for dir_entry in &dir_entries {
                let full_path = Path::new(&dir_info.file_path).join(&dir_entry);
                let full_path = full_path.to_str().unwrap();

                let rec_len = 20u16 + dir_entry.as_bytes().len() as u16;
                if res.len() + rec_len as usize > count as usize {
                    not_enough_space = true;
                    break;
                }

                if let Some(file_info) = file_system
                    .lock()
                    .unwrap()
                    .get_file_info_from_filepath(full_path)
                {
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

                no_copied_entries += 1;
            }

            return if not_enough_space && res.len() == 0 {
                22u32 // EINVAL
            } else {
                unicorn.mem_write(dirp as u64, &res).unwrap();

                let mut rest_entries = Vec::new();
                rest_entries.extend_from_slice(&dir_entries[no_copied_entries..]);
                unicorn
                    .get_data()
                    .inner
                    .sys_calls_state
                    .lock()
                    .unwrap()
                    .get_dents_list
                    .insert(fd, rest_entries);

                res.len() as u32
            };
        }
    };

    -1i32 as u32
}

pub fn set_tid_address(unicorn: &mut Unicorn<Context>, addr: u32) -> u32 {
    // TODO: implement
    let res = 1;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_tid_address(addr: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        addr,
        res
    );

    res
}

pub fn get_tid(unicorn: &mut Unicorn<Context>) -> u32 {
    // TODO: implement
    let res = 1;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] get_tid() => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}

pub fn get_pid(unicorn: &mut Unicorn<Context>) -> u32 {
    // TODO: implement
    let res = 2;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] get_pid() => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}

pub fn exit_group(unicorn: &mut Unicorn<Context>, status: u32) -> u32 {
    if let Some(threads) = unicorn.get_data().inner.threads.upgrade() {
        for thread in threads.lock().unwrap().iter_mut() {
            thread.exit().unwrap();
        }
    }

    log::trace!(
        "{:#x}: [{}] [SYSCALL] exit_group(status: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        status,
        0u32
    );

    0u32
}

pub fn link(unicorn: &mut Unicorn<Context>, old_path: u32, new_path: u32) -> u32 {
    let old_path = unicorn.read_string(old_path);
    let new_path = unicorn.read_string(new_path);

    let res = match unicorn
        .get_data()
        .inner
        .file_system
        .lock()
        .unwrap()
        .link(&old_path, &new_path)
    {
        Ok(_) => 0u32,
        Err(err) => err.to_syscall_error(),
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] link(old_path: {}, new_path: {}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        old_path,
        new_path,
        res
    );

    res
}

pub fn unlink(unicorn: &mut Unicorn<Context>, path: u32) -> u32 {
    let path = unicorn.read_string(path);

    let res = match unicorn
        .get_data()
        .inner
        .file_system
        .lock()
        .unwrap()
        .unlink(&path)
    {
        Ok(_) => 0u32,
        Err(err) => err.to_syscall_error(),
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] unlink(path: {}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        path,
        res
    );

    res
}

pub fn ftruncate(unicorn: &mut Unicorn<Context>, fd: u32, length: u32) -> u32 {
    let res = match unicorn
        .get_data()
        .inner
        .file_system
        .lock()
        .unwrap()
        .ftruncate(fd as i32, length)
    {
        Ok(_) => 0u32,
        Err(_) => -1i32 as u32,
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] ftruncate(fd: {:#x}, length: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        fd,
        length,
        res
    );

    res
}
