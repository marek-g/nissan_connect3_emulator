use crate::emulator::context::Context;
use crate::emulator::mmu::MmuExtension;
use crate::file_system::OpenFileFlags;
use std::path::PathBuf;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn open(unicorn: &mut Unicorn<Context>, path_name: u32, flags: u32, mode: u32) -> u32 {
    let path_name = unicorn.read_string(path_name);

    let fd = open_internal(unicorn, &path_name, flags, mode);

    log::trace!(
        "{:#x}: [SYSCALL] open(pathname = {}, flags: {:#x}, mode: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        path_name,
        flags,
        mode,
        fd
    );

    fd
}

pub fn openat(
    unicorn: &mut Unicorn<Context>,
    dirfd: u32,
    path_name: u32,
    flags: u32,
    mode: u32,
) -> u32 {
    let mut path_name = unicorn.read_string(path_name);

    if !path_name.starts_with("/") {
        // relative path
        let base_dir = if dirfd == 0xFFFFFF9C {
            // AT_FDCWD - pathname is interpreted relative to the current working directory
            // of the calling process (like open())
            unicorn
                .get_data_mut()
                .file_system
                .current_working_dir
                .clone()
        } else {
            if let Some(dirinfo) = unicorn
                .get_data_mut()
                .file_system
                .get_file_info(dirfd as i32)
            {
                dirinfo.filepath.clone()
            } else {
                unicorn
                    .get_data_mut()
                    .file_system
                    .current_working_dir
                    .clone()
            }
        };

        path_name = PathBuf::from(base_dir)
            .join(path_name)
            .to_str()
            .unwrap()
            .to_owned();
    }

    // TODO: handle symbolic links
    let fd = open_internal(unicorn, &path_name, flags, mode);

    log::trace!(
        "{:#x}: [SYSCALL] openat(dirfd = {:#x}, pathname = {}, flags: {:#x}, mode: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        dirfd,
        path_name,
        flags,
        mode,
        fd
    );

    fd
}

pub fn fcntl64(unicorn: &mut Unicorn<Context>, fd: u32, cmd: u32, arg1: u32) -> u32 {
    let res = match cmd {
        2 => {
            // F_SETFD
            if let Ok(_) = unicorn
                .get_data_mut()
                .file_system
                .set_file_status_flags(fd as i32, arg1)
            {
                0u32
            } else {
                -1i32 as u32
            }
        }
        3 => {
            // F_GETFD
            if let Some(fileinfo) = unicorn.get_data_mut().file_system.get_file_info(fd as i32) {
                fileinfo.file_status_flags
            } else {
                -1i32 as u32
            }
        }
        _ => panic!("unsupported command"),
    };

    log::trace!(
        "{:#x}: [SYSCALL] fcntl64(fd = {:#x}, cmd = {:#x}, arg1: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        fd,
        cmd,
        arg1,
        res
    );

    res
}

fn open_internal(unicorn: &mut Unicorn<Context>, path_name: &str, flags: u32, _mode: u32) -> u32 {
    let open_file_flags = convert_open_file_flags(flags);

    if let Ok(fd) = unicorn
        .get_data_mut()
        .file_system
        .open(&path_name, open_file_flags)
    {
        fd as u32
    } else {
        -1i32 as u32
    }
}

fn convert_open_file_flags(flags: u32) -> OpenFileFlags {
    let mut open_file_flags = OpenFileFlags::NONE;

    if flags & 0x2 == 0 {
        open_file_flags |= OpenFileFlags::READ;
    } else if flags & 0x2 == 1 {
        open_file_flags |= OpenFileFlags::WRITE;
    } else if flags & 0x2 == 2 {
        open_file_flags |= OpenFileFlags::READ | OpenFileFlags::WRITE;
    }

    if flags & 0x100 != 0 {
        open_file_flags |= OpenFileFlags::CREATE;
    }

    if flags & 0x200 != 0 {
        open_file_flags |= OpenFileFlags::EXCLUSIVE;
    }

    if flags & 0x1000 != 0 {
        open_file_flags |= OpenFileFlags::TRUNC;
    }

    if flags & 0x2000 != 0 {
        open_file_flags |= OpenFileFlags::APPEND;
    }

    if flags & 0x200000 != 0 {
        open_file_flags |= OpenFileFlags::DIRECTORY;
    }

    if flags & 0x400000 != 0 {
        open_file_flags |= OpenFileFlags::NO_FOLLOW;
    }

    open_file_flags
}
