use crate::emulator::context::Context;
use crate::emulator::users::{GID, UID};
use crate::emulator::utils::{pack_i32, pack_u32, pack_u64};
use std::time::SystemTime;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn fstat64(unicorn: &mut Unicorn<Context>, fd: u32, statbuf: u32) -> u32 {
    let res = if let Some(fileinfo) = unicorn.get_data_mut().file_system.fd_to_file(fd) {
        let metadata = fileinfo.file.metadata().unwrap();

        let mut stat_data = Vec::new();

        // st_dev
        stat_data.extend_from_slice(&pack_u64(1));

        // padding
        stat_data.extend_from_slice(&pack_u32(0));

        // st_ino
        stat_data.extend_from_slice(&pack_u32(1));

        // st_mode
        let mut st_mode = 0u32;
        if metadata.is_file() {
            st_mode |= 0o0100000u32;
        } else if metadata.is_symlink() {
            st_mode |= 0o0120000u32;
        } else if metadata.is_dir() {
            st_mode |= 0o0040000u32;
        } else {
            panic!("st_mode not implemented");
        }

        if metadata.permissions().readonly() {
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
        stat_data.extend_from_slice(&pack_u64(metadata.len()));

        // st_blksize
        stat_data.extend_from_slice(&pack_i32(4096));

        // padding
        stat_data.extend_from_slice(&pack_u32(0));

        // st_blocks
        stat_data.extend_from_slice(&pack_u64((metadata.len() + 4095) / 4096));

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
        stat_data.extend_from_slice(&pack_u64(fileinfo.inode));

        unicorn.mem_write(statbuf as u64, &stat_data).unwrap();

        0u32
    } else {
        -1i32 as u32
    };

    log::trace!(
        "{:#x}: [SYSCALL] fstat64(fd = {:#x}, statbuf = {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        fd,
        statbuf,
        res
    );
    res
}
