use crate::emulator::context::Context;
use crate::emulator::mmu::MmuExtension;
use crate::emulator::users::{GID, UID};
use crate::emulator::utils::{pack_i32, pack_u32, pack_u64};
use crate::file_system::OpenFileFlags;
use std::time::SystemTime;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn stat64(unicorn: &mut Unicorn<Context>, path: u32, statbuf: u32) -> u32 {
    let pathstr = unicorn.read_string(path);
    let res = if let Ok(fd) = unicorn
        .get_data_mut()
        .file_system
        .open(&pathstr, OpenFileFlags::READ)
    {
        let res = fstat64_internal(unicorn, fd as u32, statbuf);
        unicorn.get_data_mut().file_system.close(fd).unwrap();
        res
    } else {
        -1i32 as u32
    };
    log::trace!(
        "{:#x}: [SYSCALL] stat64(path = {}, statbuf = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        pathstr,
        statbuf,
        res
    );
    res
}

pub fn lstat64(unicorn: &mut Unicorn<Context>, path: u32, statbuf: u32) -> u32 {
    // TODO: handle symbolic links
    let pathstr = unicorn.read_string(path);
    let res = if let Ok(fd) = unicorn
        .get_data_mut()
        .file_system
        .open(&pathstr, OpenFileFlags::READ | OpenFileFlags::NO_FOLLOW)
    {
        let res = fstat64_internal(unicorn, fd as u32, statbuf);
        unicorn.get_data_mut().file_system.close(fd).unwrap();
        res
    } else {
        -1i32 as u32
    };
    log::trace!(
        "{:#x}: [SYSCALL] lstat64(path = {}, statbuf = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        pathstr,
        statbuf,
        res
    );
    res
}

pub fn fstat64(unicorn: &mut Unicorn<Context>, fd: u32, statbuf: u32) -> u32 {
    let res = fstat64_internal(unicorn, fd, statbuf);

    log::trace!(
        "{:#x}: [SYSCALL] fstat64(fd = {:#x}, statbuf = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        fd,
        statbuf,
        res
    );
    res
}

pub fn statfs(unicorn: &mut Unicorn<Context>, path: u32, buf: u32) -> u32 {
    let pathstr = unicorn.read_string(path);

    let mut vec = Vec::new();

    // f_type
    vec.extend_from_slice(&pack_u32(0x01021994)); // TMPFS

    unicorn.mem_write(buf as u64, &vec).unwrap();

    //let mut bytes = vec![0u8; 21 * 4 as usize];

    /*let mut bytes = vec![0u8; 12 * 8 as usize];
    for i in 0..12 * 8 {
        bytes[i] = i as u8;
    }*/

    //unicorn.mem_write(buf as u64, &bytes).unwrap();

    let res = 0;

    log::trace!(
        "{:#x}: [SYSCALL] statfs(path = {}, buf = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        pathstr,
        buf,
        res
    );
    res
}

fn fstat64_internal(unicorn: &mut Unicorn<Context>, fd: u32, statbuf: u32) -> u32 {
    if let Some(file_info) = unicorn.get_data_mut().file_system.get_file_info(fd as i32) {
        let mut stat_data = Vec::new();

        // st_dev
        stat_data.extend_from_slice(&pack_u64(1));

        // padding
        stat_data.extend_from_slice(&pack_u32(0));

        // st_ino
        stat_data.extend_from_slice(&pack_u32(1));

        // st_mode
        let mut st_mode = 0u32;
        if file_info.file_details.is_file {
            st_mode |= 0o0100000u32;
        } else if file_info.file_details.is_symlink {
            st_mode |= 0o0120000u32;
        } else if file_info.file_details.is_dir {
            st_mode |= 0o0040000u32;
        } else {
            panic!("st_mode not implemented");
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

        unicorn.mem_write(statbuf as u64, &stat_data).unwrap();

        0u32
    } else {
        -1i32 as u32
    }
}
