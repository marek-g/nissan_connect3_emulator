use crate::emulator::context::Context;
use crate::emulator::thread::{BlockReason, ThreadAction};
use crate::emulator::utils::{mem_align_up, pack_u16, pack_u32, pack_u64, read_string};
use crate::os::file_system::{FileType, MountFileSystem, OpenFileFlags};
use crate::os::syscalls::SysCallError;
use std::io::SeekFrom;
use std::path::Path;
use std::sync::{Arc, Mutex};
use unicorn_engine::unicorn_const::Prot;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn brk(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] brk(addr = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        addr,
    );

    let mmu_arc = unicorn.get_data().inner.mmu.clone();
    let mut mmu = mmu_arc.lock().unwrap();
    let res = if addr == 0 {
        mmu.brk_mem_end
    } else {
        let brk_mem_end = mmu.brk_mem_end;
        let new_brk_mem_end = mem_align_up(addr, None);
        if new_brk_mem_end > brk_mem_end {
            mmu.map(
                unicorn,
                brk_mem_end,
                new_brk_mem_end - brk_mem_end,
                Prot::ALL,
                "[brk]",
                "",
            );
        } else if new_brk_mem_end < brk_mem_end {
            mmu.unmap(unicorn, new_brk_mem_end, brk_mem_end - new_brk_mem_end);
        }
        mmu.brk_mem_end = new_brk_mem_end;
        new_brk_mem_end
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] brk => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

pub fn access(unicorn: &mut Unicorn<'_, Context>, path_name: u32, mode: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] access(pathname = {:#x}, mode = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        path_name,
        mode,
    );

    let path_name = read_string(unicorn, path_name);

    log::trace!("path_name: {}", path_name);

    let exists = unicorn
        .get_data()
        .inner
        .file_system
        .lock()
        .unwrap()
        .exists(&path_name);
    let res = if exists { 0 } else { -1i32 as u32 };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] access => {:#x} [{}]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res,
        if exists { "FOUND" } else { "NOT FOUND" }
    );
    res
}

pub fn close(unicorn: &mut Unicorn<'_, Context>, fd: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] close(fd: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        fd,
    );

    // POSIX message queue handles are not file-system fds - glibc's mq_close()
    // closes them through the plain close syscall (kernel: mqueue file)
    let is_mq_handle = {
        let state = unicorn.get_data().inner.namespace.lock().unwrap();
        state.mq.queues.contains_key(&fd)
    };
    if is_mq_handle {
        return crate::os::syscalls::mqueue::mq_close(unicorn, fd);
    }

    // /dev/iosc descriptors are backed by the emulated IOSC driver, not the fs
    if crate::os::dev::iosc::is_iosc_fd(unicorn, fd) {
        return crate::os::dev::iosc::close_iosc(unicorn, fd);
    }

    let is_socket_fd = {
        let state = &mut unicorn.get_data().inner.sys_calls_state.lock().unwrap();
        state.get_dents_list.remove(&fd);
        state.inotify_fds.remove(&fd);
        state.socket_fds.remove(&fd)
    };
    if is_socket_fd {
        log::trace!(
            "{:#x}: [{}] [SYSCALL] close => {:#x}",
            unicorn.reg_read(RegisterARM::PC).unwrap(),
            unicorn.get_data().inner.thread_id(),
            0u32
        );
        return 0u32;
    }

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
        "{:#x}: [{}] [SYSCALL] close => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn read(unicorn: &mut Unicorn<'_, Context>, fd: u32, buf: u32, length: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] read(fd: {:#x}, buf: {:#x}, length: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        fd,
        buf,
        length,
    );

    let is_inotify_fd = unicorn
        .get_data()
        .inner
        .sys_calls_state
        .lock()
        .unwrap()
        .inotify_fds
        .contains(&fd);
    if is_inotify_fd {
        unicorn
            .get_data()
            .set_action(ThreadAction::Block(BlockReason::InotifyRead {
                fd,
                deadline: None,
            }));
        return 0u32;
    }

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
        "{:#x}: [{}] [SYSCALL] read => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn write(unicorn: &mut Unicorn<'_, Context>, fd: u32, buf: u32, length: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] write(fd: {:#x}, buf: {:#x}, length: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        fd,
        buf,
        length,
    );

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
        "{:#x}: [{}] [SYSCALL] write => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn lseek(unicorn: &mut Unicorn<'_, Context>, fd: u32, offset: u32, whence: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] lseek(fd: {:#x}, offset: {:#x}, whence: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        fd,
        offset,
        whence,
    );

    let file_system = &mut unicorn.get_data().inner.file_system.clone();
    let offset = offset as u64;
    let res = if let Ok(pos) = match whence {
        0 => Ok(SeekFrom::Start(offset)),
        1 => Ok(SeekFrom::Current(offset as i64)),
        2 => Ok(SeekFrom::End(offset as i64)),
        _ => Err(()),
    } {
        if let Ok(new_pos) = file_system.lock().unwrap().seek(fd as i32, pos) {
            new_pos as u32
        } else {
            -1i32 as u32
        }
    } else {
        -1i32 as u32
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] lseek => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn _llseek(
    unicorn: &mut Unicorn<'_, Context>,
    fd: u32,
    offset_high: u32,
    offset_low: u32,
    result: u32,
    whence: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] _llseek(fd: {:#x}, offset_high: {:#x}, offset_low: {:#x}, result: {:#x}, whence: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        fd,
        offset_high,
        offset_low,
        result,
        whence,
    );

    let file_system = &mut unicorn.get_data().inner.file_system.clone();
    let offset = ((offset_high as u64) << 32) | (offset_low as u64);
    let res = if let Ok(pos) = match whence {
        0 => Ok(SeekFrom::Start(offset)),
        1 => Ok(SeekFrom::Current(offset as i64)),
        2 => Ok(SeekFrom::End(offset as i64)),
        _ => Err(()),
    } {
        if let Ok(new_pos) = file_system.lock().unwrap().seek(fd as i32, pos) {
            unicorn
                .mem_write(result as u64, &pack_u64(new_pos))
                .unwrap();
            0 as u32
        } else {
            -1i32 as u32
        }
    } else {
        -1i32 as u32
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] _llseek => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn getdents64(unicorn: &mut Unicorn<'_, Context>, fd: u32, dirp: u32, count: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] getdents64(fd: {:#x}, dirp: {:#x}, count: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        fd,
        dirp,
        count,
    );

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
        "{:#x}: [{}] [SYSCALL] getdents64 => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn getdents(unicorn: &mut Unicorn<'_, Context>, fd: u32, dirp: u32, count: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] getdents(fd: {:#x}, dirp: {:#x}, count: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        fd,
        dirp,
        count,
    );

    let file_system = unicorn.get_data().inner.file_system.clone();

    let dir_entries = if let Some(prev_list) = unicorn
        .get_data()
        .inner
        .sys_calls_state
        .lock()
        .unwrap()
        .get_dents_list
        .remove(&fd)
    {
        Some(prev_list)
    } else {
        let dir_info = file_system.lock().unwrap().get_file_info(fd as i32);
        if let Some(dir_info) = dir_info {
            let read_dir = file_system.lock().unwrap().read_dir(&dir_info.file_path);
            if let Ok(mut dir_entries) = read_dir {
                dir_entries.insert(0, ".".to_string());
                dir_entries.insert(1, "..".to_string());
                Some(dir_entries)
            } else {
                None
            }
        } else {
            None
        }
    };

    let res = get_dents_old_internal(unicorn, fd, dirp, count, file_system, dir_entries);

    log::trace!(
        "{:#x}: [{}] [SYSCALL] getdents => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res,
    );

    res
}

fn get_dents_old_internal(
    unicorn: &mut Unicorn<'_, Context>,
    fd: u32,
    dirp: u32,
    count: u32,
    file_system: Arc<Mutex<MountFileSystem>>,
    dir_entries: Option<Vec<String>>,
) -> u32 {
    if let Some(dir_entries) = &dir_entries {
        if dir_entries.len() == 0 {
            return 0u32;
        }
    }

    if let Some(dir_entries) = dir_entries {
        let mut res = Vec::new();
        let dir_info = file_system.lock().unwrap().get_file_info(fd as i32);
        if let Some(dir_info) = dir_info {
            let mut not_enough_space = false;
            let mut no_copied_entries = 0;
            for dir_entry in &dir_entries {
                let full_path = Path::new(&dir_info.file_path).join(dir_entry);
                let full_path = full_path.to_str().unwrap();

                let name_len = dir_entry.as_bytes().len();
                let rec_len = align_up_u16(12 + name_len as u16, 8);
                if res.len() + rec_len as usize > count as usize {
                    not_enough_space = true;
                    break;
                }

                if let Some(file_info) = file_system
                    .lock()
                    .unwrap()
                    .get_file_info_from_filepath(full_path)
                {
                    let d_type = match file_info.file_details.file_type {
                        FileType::File => 8u8,
                        FileType::Link => 10u8,
                        FileType::Directory => 4u8,
                        FileType::Socket => 12u8,
                        FileType::BlockDevice => 6u8,
                        FileType::CharacterDevice => 2u8,
                        FileType::NamedPipe => 1u8,
                    };

                    res.extend_from_slice(&pack_u32(file_info.inode as u32));
                    res.extend_from_slice(&pack_u32(no_copied_entries as u32 + 1));
                    res.extend_from_slice(&pack_u16(rec_len));
                    res.extend_from_slice(dir_entry.as_bytes());
                    res.push(0u8);
                    while res.len() % 8 != 7 {
                        res.push(0u8);
                    }
                    res.push(d_type);

                    if res.len() % 8 != 0 {
                        while res.len() % 8 != 0 {
                            res.push(0u8);
                        }
                    }
                }

                no_copied_entries += 1;
            }

            return if not_enough_space && res.len() == 0 {
                22u32
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
    }

    -1i32 as u32
}

fn align_up_u16(value: u16, alignment: u16) -> u16 {
    let alignment = alignment.max(1) as u32;
    let value = value as u32;
    (((value + alignment - 1) / alignment) * alignment) as u16
}

fn get_dents_internal(
    unicorn: &mut Unicorn<'_, Context>,
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

pub fn set_tid_address(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_tid_address(addr: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        addr,
    );

    // TODO: implement
    let res = 1;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_tid_address => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn get_tid(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] get_tid() [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
    );

    let res = unicorn.get_data().thread_id();

    log::trace!(
        "{:#x}: [{}] [SYSCALL] get_tid => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        res
    );

    res
}

pub fn get_pid(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] get_pid() [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
    );

    // TODO: implement
    let res = 1;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] get_pid => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn exit(unicorn: &mut Unicorn<'_, Context>, status: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] exit(status: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        status,
    );

    unicorn
        .get_data()
        .set_action(ThreadAction::ExitThread(status as i32));

    log::trace!(
        "{:#x}: [{}] [SYSCALL] exit => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        0u32
    );

    0u32
}

pub fn exit_group(unicorn: &mut Unicorn<'_, Context>, status: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] exit_group(status: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        status,
    );

    unicorn
        .get_data()
        .set_action(ThreadAction::ExitProcess(status as i32));

    log::trace!(
        "{:#x}: [{}] [SYSCALL] exit_group => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        0u32
    );

    0u32
}

pub fn reboot(unicorn: &mut Unicorn<'_, Context>, status: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] reboot(status: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        status,
    );

    unicorn
        .get_data()
        .set_action(ThreadAction::ExitProcess(0));

    log::trace!(
        "{:#x}: [{}] [SYSCALL] reboot => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        0u32
    );

    0u32
}

pub fn link(unicorn: &mut Unicorn<'_, Context>, old_path: u32, new_path: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] link(old_path: {:#x}, new_path: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        old_path,
        new_path,
    );

    let old_path = read_string(unicorn, old_path);
    let new_path = read_string(unicorn, new_path);

    log::trace!("old_path: {}, new_path: {}", old_path, new_path);

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
        "{:#x}: [{}] [SYSCALL] link => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn unlink(unicorn: &mut Unicorn<'_, Context>, path: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] unlink(path: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        path,
    );

    let path = read_string(unicorn, path);

    log::trace!("path: {}", path);

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
        "{:#x}: [{}] [SYSCALL] unlink => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn symlink(unicorn: &mut Unicorn<'_, Context>, old_path: u32, new_path: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] symlink(old_path: {:#x}, new_path: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        old_path,
        new_path,
    );

    let old_path = read_string(unicorn, old_path);
    let new_path = read_string(unicorn, new_path);

    log::trace!("old_path: {}, new_path: {}", old_path, new_path);

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
        "{:#x}: [{}] [SYSCALL] symlink => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn ftruncate(unicorn: &mut Unicorn<'_, Context>, fd: u32, length: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] ftruncate(fd: {:#x}, length: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        fd,
        length,
    );

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
        "{:#x}: [{}] [SYSCALL] ftruncate => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn dup(unicorn: &mut Unicorn<'_, Context>, old_fd: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] dup(fd: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        old_fd
    );

    let new_fd = unicorn
        .get_data()
        .inner
        .file_system
        .lock()
        .unwrap()
        .dup(old_fd as i32);

    let res = match new_fd {
        Some(fd) => fd as u32,
        None => -1i32 as u32,
    };
    log::trace!(
        "{:#x}: [{}] [SYSCALL] dup => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

pub fn dup2(unicorn: &mut Unicorn<'_, Context>, old_fd: u32, new_fd: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] dup2(old: {:#x}, new: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        old_fd,
        new_fd
    );

    if old_fd == new_fd {
        return new_fd;
    }
    let ok = unicorn
        .get_data()
        .inner
        .file_system
        .lock()
        .unwrap()
        .dup2(old_fd as i32, new_fd as i32);
    if ok {
        new_fd
    } else {
        -1i32 as u32
    }
}

/// inotify_init / inotify_init1(flags): hand out a descriptor. The automount
/// watcher in libosal only opens the handle and registers watches, so a
/// plain readable handle (backed by /dev/null, which reports EOF on read) is
/// enough to keep it out of its failure path.
pub fn inotify_init(unicorn: &mut Unicorn<'_, Context>, _flags: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] inotify_init [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id()
    );
    let res = match unicorn
        .get_data()
        .inner
        .file_system
        .lock()
        .unwrap()
        .open("/dev/null", OpenFileFlags::READ)
    {
        Ok(fd) => {
            unicorn
                .get_data()
                .inner
                .sys_calls_state
                .lock()
                .unwrap()
                .inotify_fds
                .insert(fd as u32);
            fd as u32
        }
        Err(_) => -1i32 as u32,
    };
    log::trace!(
        "{:#x}: [{}] [SYSCALL] inotify_init => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

/// inotify_add_watch(fd, path, mask): accept and return a watch descriptor.
pub fn inotify_add_watch(
    _unicorn: &mut Unicorn<'_, Context>,
    _fd: u32,
    _path: u32,
    _mask: u32,
) -> u32 {
    static NEXT_WD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    NEXT_WD.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// inotify_rm_watch(fd, wd): always succeeds (no watches are tracked).
pub fn inotify_rm_watch(_unicorn: &mut Unicorn<'_, Context>, _fd: u32, _wd: u32) -> u32 {
    0u32
}
