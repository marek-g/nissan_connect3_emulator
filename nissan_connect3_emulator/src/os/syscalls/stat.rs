use crate::emulator::context::Context;
use crate::emulator::mmu::MmuExtension;
use crate::emulator::users::{GID, UID};
use crate::emulator::utils::{pack_i32, pack_u32, pack_u64};
use crate::file_system::{FileSystemType, FileType, OpenFileFlags};
use crate::os::syscalls::fcntl::get_path_relative_to_dir;
use crate::os::syscalls::SysCallError;
use std::time::SystemTime;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn stat64(unicorn: &mut Unicorn<Context>, path: u32, stat_buf: u32) -> u32 {
    let pathstr = unicorn.read_string(path);
    let file_system = unicorn.get_data().inner.file_system.clone();
    let open_res = file_system
        .lock()
        .unwrap()
        .open(&pathstr, OpenFileFlags::READ);
    let res = if let Ok(fd) = open_res {
        let res = fstat64_internal(unicorn, fd as u32, stat_buf);
        file_system.lock().unwrap().close(fd).unwrap();
        res
    } else {
        -1i32 as u32
    };
    log::trace!(
        "{:#x}: [{}] [SYSCALL] stat64(path = {}, stat_buf = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        pathstr,
        stat_buf,
        res
    );
    res
}

pub fn fstatat64(
    unicorn: &mut Unicorn<Context>,
    dir_fd: u32,
    path: u32,
    stat_buf: u32,
    flags: u32,
) -> u32 {
    let path_name = unicorn.read_string(path);
    let path_name_new = get_path_relative_to_dir(unicorn, dir_fd, &path_name);
    let file_system = unicorn.get_data().inner.file_system.clone();

    let open_res = file_system
        .lock()
        .unwrap()
        .open(&path_name_new, OpenFileFlags::READ);
    let res = if let Ok(fd) = open_res {
        let res = fstat64_internal(unicorn, fd as u32, stat_buf);
        file_system.lock().unwrap().close(fd).unwrap();
        res
    } else {
        -1i32 as u32
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] fstatat64(dir_fd: {:#x}, path = {}, stat_buf = {:#x}, flags = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        dir_fd,
        path_name,
        stat_buf,
        flags,
        res
    );
    res
}

pub fn lstat64(unicorn: &mut Unicorn<Context>, path: u32, stat_buf: u32) -> u32 {
    // TODO: handle symbolic links
    let pathstr = unicorn.read_string(path);
    let file_system = unicorn.get_data().inner.file_system.clone();

    let open_res = file_system
        .lock()
        .unwrap()
        .open(&pathstr, OpenFileFlags::READ | OpenFileFlags::NO_FOLLOW);

    let res = match open_res {
        Ok(fd) => {
            let res = fstat64_internal(unicorn, fd as u32, stat_buf);
            file_system.lock().unwrap().close(fd).unwrap();
            res
        }
        Err(err) => err.to_syscall_error(),
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] lstat64(path = {}, stat_buf = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        pathstr,
        stat_buf,
        res
    );
    res
}

pub fn fstat64(unicorn: &mut Unicorn<Context>, fd: u32, stat_buf: u32) -> u32 {
    let res = fstat64_internal(unicorn, fd, stat_buf);

    log::trace!(
        "{:#x}: [{}] [SYSCALL] fstat64(fd = {:#x}, stat_buf = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        fd,
        stat_buf,
        res
    );
    res
}

pub fn statfs(unicorn: &mut Unicorn<Context>, path: u32, buf: u32) -> u32 {
    let file_path = unicorn.read_string(path);

    let mut vec = Vec::new();

    let res = if let Some((mount_point, _path)) = unicorn
        .get_data()
        .inner
        .file_system
        .lock()
        .unwrap()
        .get_mount_point_from_filepath_mut(&file_path)
    {
        // f_type - type of filesystem
        vec.extend_from_slice(&pack_u32(
            match mount_point.file_system.file_system_type() {
                FileSystemType::Normal => 0xef53, // EXT4
                FileSystemType::Dev => 0x1373,
                FileSystemType::Proc => 0x9fa0,
                FileSystemType::Temp => 0x01021994,
                FileSystemType::Stream => 0,
            },
        ));

        // f_bsize - optimal transfer block size
        vec.extend_from_slice(&pack_u32(4096));

        // f_blocks - total data blocks in file system
        vec.extend_from_slice(&pack_u64(100000u64));

        // f_bfree - free blocks in fs
        vec.extend_from_slice(&pack_u64(100000000u64));

        // f_bavail - free blocks available to unprivileged user
        vec.extend_from_slice(&pack_u64(100000000u64));

        // f_files - total file nodes in file system
        vec.extend_from_slice(&pack_u64(100000u64));

        // f_ffree - free file nodes in fs
        vec.extend_from_slice(&pack_u64(100000000u64));

        // f_fsid - file system id
        vec.extend_from_slice(&pack_u64(1u64));

        // f_namelen - maximum length of filenames
        vec.extend_from_slice(&pack_u32(4096u32));

        // f_frsize - fragment size
        vec.extend_from_slice(&pack_u32(0u32));

        // f_flags
        vec.extend_from_slice(&pack_u32(0u32));

        // spare
        vec.extend_from_slice(&pack_u32(0u32));
        vec.extend_from_slice(&pack_u32(0u32));
        vec.extend_from_slice(&pack_u32(0u32));
        vec.extend_from_slice(&pack_u32(0u32));

        0u32
    } else {
        -1i32 as u32
    };

    if res == 0u32 {
        unicorn.mem_write(buf as u64, &vec).unwrap();
    }

    log::trace!(
        "{:#x}: [{}] [SYSCALL] statfs(path = {}, buf = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        file_path,
        buf,
        res
    );
    res
}

fn fstat64_internal(unicorn: &mut Unicorn<Context>, fd: u32, stat_buf: u32) -> u32 {
    let file_system = unicorn.get_data().inner.file_system.clone();

    let res = if let Some(file_info) = file_system.lock().unwrap().get_file_info(fd as i32) {
        let mut stat_data = Vec::new();

        // st_dev
        stat_data.extend_from_slice(&pack_u64(1));

        // padding
        stat_data.extend_from_slice(&pack_u32(0));

        // st_ino
        stat_data.extend_from_slice(&pack_u32(1));

        // st_mode
        let mut st_mode = 0u32;
        match file_info.file_details.file_type {
            FileType::File => st_mode |= 0o0100000u32,
            FileType::Link => st_mode |= 0o0120000u32,
            FileType::Directory => st_mode |= 0o0040000u32,
            FileType::Socket => st_mode |= 0o0140000u32,
            FileType::BlockDevice => st_mode |= 0o0060000u32,
            FileType::CharacterDevice => st_mode |= 0o0020000u32,
            FileType::NamedPipe => st_mode |= 0o0010000u32,
        }

        if file_info.file_details.is_readonly {
            st_mode |= 0o000555;
        } else {
            st_mode |= 0o000777;
        }

        stat_data.extend_from_slice(&pack_u32(st_mode));

        // st_nlink
        stat_data.extend_from_slice(&pack_u32(0));

        // st_uid
        stat_data.extend_from_slice(&pack_u32(UID));

        // st_gid
        stat_data.extend_from_slice(&pack_u32(GID));

        // st_rdev
        stat_data.extend_from_slice(&pack_u64(0));

        // padding
        stat_data.extend_from_slice(&pack_u64(0));

        // st_size
        stat_data.extend_from_slice(&pack_u64(file_info.file_details.length));

        // st_blksize
        stat_data.extend_from_slice(&pack_i32(4096));

        // padding
        stat_data.extend_from_slice(&pack_u32(0));

        // st_blocks
        stat_data.extend_from_slice(&pack_u64((file_info.file_details.length + 511) / 512));

        // st_atime
        let time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        stat_data.extend_from_slice(&pack_u32(time as u32));

        // st_atime_ns
        stat_data.extend_from_slice(&pack_u32(0));

        // st_mtime
        stat_data.extend_from_slice(&pack_u32(time as u32));

        // st_mtime_ns
        stat_data.extend_from_slice(&pack_u32(0));

        // st_ctime
        stat_data.extend_from_slice(&pack_u32(time as u32));

        // st_ctime_ns
        stat_data.extend_from_slice(&pack_u32(0));

        // st_ino
        stat_data.extend_from_slice(&pack_u64(file_info.inode));

        unicorn.mem_write(stat_buf as u64, &stat_data).unwrap();

        0u32
    } else {
        -1i32 as u32
    };

    res
}
