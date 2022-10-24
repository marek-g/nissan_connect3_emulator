use crate::emulator::context::Context;
use crate::emulator::mmu::MmuExtension;
use std::path::PathBuf;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn open(unicorn: &mut Unicorn<Context>, pathname: u32, flags: u32, mode: u32) -> u32 {
    let pathname = unicorn.read_string(pathname);

    let fd = open_internal(unicorn, &pathname, flags, mode);

    log::trace!(
        "{:#x}: [SYSCALL] open(pathname = {}, flags: {:#x}, mode: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        pathname,
        flags,
        mode,
        fd
    );

    fd
}

pub fn openat(
    unicorn: &mut Unicorn<Context>,
    dirfd: u32,
    pathname: u32,
    flags: u32,
    mode: u32,
) -> u32 {
    let mut pathname = unicorn.read_string(pathname);

    if !pathname.starts_with("/") {
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
            if let Some(dirinfo) = unicorn.get_data_mut().file_system.fd_to_file(dirfd) {
                dirinfo.filepath.clone()
            } else {
                unicorn
                    .get_data_mut()
                    .file_system
                    .current_working_dir
                    .clone()
            }
        };

        pathname = PathBuf::from(base_dir)
            .join(pathname)
            .to_str()
            .unwrap()
            .to_owned();
    }

    // TODO: handle symbolic links
    let fd = open_internal(unicorn, &pathname, flags, mode);

    log::trace!(
        "{:#x}: [SYSCALL] openat(dirfd = {:#x}, pathname = {}, flags: {:#x}, mode: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        dirfd,
        pathname,
        flags,
        mode,
        fd
    );

    fd
}

fn open_internal(unicorn: &mut Unicorn<Context>, pathname: &str, flags: u32, mode: u32) -> u32 {
    let fd = unicorn.get_data_mut().file_system.open(&pathname);

    if mode != 0x0 && mode != 0x1 {
        panic!("mode not implemented");
    }

    fd
}
