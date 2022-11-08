use crate::emulator::context::Context;
use crate::emulator::utils::{mem_align_down, mem_align_up};
use std::io::SeekFrom;
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
    log::trace!("{:#x} [{}] [SYSCALL] mmap(addr = {:#x}, length = {:#x}, prot = {:#x}, flags = {:#x}, fd = {:#x}, off_t: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        addr, length, prot, flags, fd, off_t);

    let res = mmapx(unicorn, addr, length, prot, flags, fd, off_t);

    log::trace!(
        "{:#x} [{}] [SYSCALL] mmap => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

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
    log::trace!("{:#x} [{}] [SYSCALL] mmap2(addr = {:#x}, length = {:#x}, prot = {:#x}, flags = {:#x}, fd = {:#x}, pgoffset: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        addr, length, prot, flags, fd, pgoffset);

    let res = mmapx(unicorn, addr, length, prot, flags, fd, pgoffset * 0x1000);

    log::trace!(
        "{:#x} [{}] [SYSCALL] mmap2 => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}

pub fn munmap(unicorn: &mut Unicorn<Context>, addr: u32, length: u32) -> u32 {
    log::trace!(
        "{:#x} [{}] [SYSCALL] munmap(addr = {:#x}, len = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        addr,
        length,
    );

    let unicorn_context = unicorn.get_data();
    let mmu = &mut unicorn_context.inner.mmu.lock().unwrap();
    mmu.unmap(unicorn, addr, mem_align_up(length, None));

    let res = 0u32;
    log::trace!(
        "{:#x} [{}] [SYSCALL] munmap => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );
    res
}

pub fn mprotect(unicorn: &mut Unicorn<Context>, addr: u32, len: u32, prot: u32) -> u32 {
    log::trace!(
        "{:#x} [{}] [SYSCALL] mprotect(addr = {:#x}, len = {:#x}, prot = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        addr,
        len,
        prot,
    );

    let unicorn_context = unicorn.get_data();
    let mmu = &mut unicorn_context.inner.mmu.lock().unwrap();
    mmu.mem_protect(
        unicorn,
        addr,
        mem_align_up(len, None),
        prot_to_permission(prot),
    );

    let res = 0u32;
    log::trace!(
        "{:#x} [{}] [SYSCALL] mprotect => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );
    res
}

pub fn mincore(unicorn: &mut Unicorn<Context>, addr: u32, length: u32, vec: u32) -> u32 {
    log::trace!(
        "{:#x} [{}] [SYSCALL] mincore(addr = {:#x}, length = {:#x}, vec = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        addr,
        length,
        vec,
    );

    let bytes = vec![1u8; ((length + 0x1000 - 1) / 0x1000) as usize];
    unicorn.mem_write(vec as u64, &bytes).unwrap();

    log::trace!(
        "{:#x} [{}] [SYSCALL] mincore => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        0
    );
    0
}

fn mmapx(
    mut unicorn: &mut Unicorn<Context>,
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

    if flags & 0x800u32 != 0 {
        // MAP_DENYWRITE
    }

    if addr != mem_align_down(addr, None) {
        panic!("wrong address alignment for mmap");
    }

    length = mem_align_up(length, None);

    // load file
    let mut buf = Vec::new();
    let mut filepath = String::new();
    let file_system = &mut unicorn.get_data().inner.file_system.clone();
    let file_info_res = file_system.lock().unwrap().get_file_info(fd as i32);
    if let Some(fileinfo) = file_info_res {
        let mut file_system = file_system.lock().unwrap();

        filepath = fileinfo.file_path.clone();

        let file_pos = file_system.stream_position(fd as i32).unwrap();
        file_system
            .seek(fd as i32, SeekFrom::Start(off_t as u64))
            .unwrap();

        let bytes_to_read = length.min(file_system.get_length(fd as i32) as u32 - off_t);
        buf.resize(bytes_to_read as usize, 0u8);
        file_system.read_all(fd as i32, &mut buf).unwrap();

        file_system
            .seek(fd as i32, SeekFrom::Start(file_pos))
            .unwrap();
    }

    // allocate memory
    let unicorn_context = unicorn.get_data();
    let addr = if flags & 0x10 != 0 || addr != 0 {
        // MAP_FIXED - don't interpret addr as a hint
        unicorn_context.inner.mmu.lock().unwrap().map(
            unicorn,
            addr,
            length,
            perms,
            "[heap (fixed addr)]",
            &filepath,
        );
        addr
    } else {
        unicorn_context
            .inner
            .mmu
            .lock()
            .unwrap()
            .heap_alloc(unicorn, length, perms, &filepath)
    };

    // write file
    if buf.len() > 0 {
        unicorn.mem_write(addr as u64, &buf).unwrap();

        if perms.contains(Permission::EXEC) {
            unicorn_context
                .inner
                .mmu
                .lock()
                .unwrap()
                .update_library_hooks_for_all_threads(&unicorn);
        }
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
