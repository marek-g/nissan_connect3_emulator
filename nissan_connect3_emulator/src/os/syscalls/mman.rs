use crate::emulator::context::Context;
use crate::emulator::utils::{mem_align_down, mem_align_up};
use std::io::SeekFrom;
use unicorn_engine::unicorn_const::Prot;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn mmap(
    unicorn: &mut Unicorn<'_, Context>,
    addr: u32,
    length: u32,
    prot: u32,
    flags: u32,
    fd: u32,
    off_t: u32,
) -> u32 {
    log::trace!("{:#x} [{}] [SYSCALL] mmap(addr = {:#x}, length = {:#x}, prot = {:#x}, flags = {:#x}, fd = {:#x}, off_t: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        addr, length, prot, flags, fd, off_t);

    let res = mmapx(unicorn, addr, length, prot, flags, fd, off_t);

    log::trace!(
        "{:#x} [{}] [SYSCALL] mmap => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn mmap2(
    unicorn: &mut Unicorn<'_, Context>,
    addr: u32,
    length: u32,
    prot: u32,
    flags: u32,
    fd: u32,
    pgoffset: u32,
) -> u32 {
    log::trace!("{:#x} [{}] [SYSCALL] mmap2(addr = {:#x}, length = {:#x}, prot = {:#x}, flags = {:#x}, fd = {:#x}, pgoffset: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        addr, length, prot, flags, fd, pgoffset);

    let res = mmapx(unicorn, addr, length, prot, flags, fd, pgoffset * 0x1000);

    log::trace!(
        "{:#x} [{}] [SYSCALL] mmap2 => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn munmap(unicorn: &mut Unicorn<'_, Context>, addr: u32, length: u32) -> u32 {
    log::trace!(
        "{:#x} [{}] [SYSCALL] munmap(addr = {:#x}, len = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        addr,
        length,
    );

    let mmu_arc = unicorn.get_data().inner.mmu.clone();
    let mut mmu = mmu_arc.lock().unwrap();
    mmu.unmap(unicorn, addr, mem_align_up(length, None));

    let res = 0u32;
    log::trace!(
        "{:#x} [{}] [SYSCALL] munmap => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

pub fn mprotect(unicorn: &mut Unicorn<'_, Context>, addr: u32, len: u32, prot: u32) -> u32 {
    log::trace!(
        "{:#x} [{}] [SYSCALL] mprotect(addr = {:#x}, len = {:#x}, prot = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        addr,
        len,
        prot,
    );

    let mmu_arc = unicorn.get_data().inner.mmu.clone();
    let mut mmu = mmu_arc.lock().unwrap();
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
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

pub fn mincore(unicorn: &mut Unicorn<'_, Context>, addr: u32, length: u32, vec: u32) -> u32 {
    log::trace!(
        "{:#x} [{}] [SYSCALL] mincore(addr = {:#x}, length = {:#x}, vec = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        addr,
        length,
        vec,
    );

    let bytes = vec![1u8; ((length + 0x1000 - 1) / 0x1000) as usize];
    unicorn.mem_write(vec as u64, &bytes).unwrap();

    log::trace!(
        "{:#x} [{}] [SYSCALL] mincore => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        0
    );
    0
}

fn mmapx(
    unicorn: &mut Unicorn<'_, Context>,
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

    // load the file content to be mapped (like the kernel, an offset past the end
    // of the file maps zero-filled pages instead of failing)
    let mut buf = Vec::new();
    let mut filepath = String::new();
    if fd != 0xFFFFFFFFu32 {
        let file_system = &mut unicorn.get_data().inner.file_system.clone();
        let mut fs = file_system.lock().unwrap();

        if let Some(fileinfo) = fs.get_file_info(fd as i32) {
            filepath = fileinfo.file_path.clone();

            let file_pos = match fs.stream_position(fd as i32) {
                Ok(pos) => pos,
                Err(_) => return -22i32 as u32, // -EINVAL
            };

            let file_len = fs.get_length(fd as i32);
            if (off_t as u64) < file_len {
                if fs.seek(fd as i32, SeekFrom::Start(off_t as u64)).is_err() {
                    return -22i32 as u32; // -EINVAL
                }
                let bytes_to_read = (length as u64)
                    .min(file_len.saturating_sub(off_t as u64)) as u32;
                buf.resize(bytes_to_read as usize, 0u8);
                if fs.read_all(fd as i32, &mut buf).is_err() {
                    return -5i32 as u32; // -EIO
                }
            }

            let _ = fs.seek(fd as i32, SeekFrom::Start(file_pos));
        }
    }

    // allocate memory
    let mmu_arc = unicorn.get_data().inner.mmu.clone();
    let addr = if flags & 0x10 != 0 || addr != 0 {
        // MAP_FIXED - don't interpret addr as a hint
        mmu_arc.lock().unwrap().map(
            unicorn,
            addr,
            length,
            perms,
            "[heap (fixed addr)]",
            &filepath,
        );
        addr
    } else {
        mmu_arc.lock().unwrap().heap_alloc(unicorn, length, perms, &filepath)
    };

    // write file
    if buf.len() > 0 {
        unicorn.mem_write(addr as u64, &buf).unwrap();

        if (perms & Prot::EXEC) != Prot::NONE {
            mmu_arc.lock().unwrap().update_library_hooks(unicorn);
        }
    }

    addr
}

fn prot_to_permission(prot: u32) -> Prot {
    let mut perms = Prot::NONE;
    if prot & 1 != 0 {
        perms |= Prot::READ;
    }
    if prot & 2 != 0 {
        perms |= Prot::WRITE;
    }
    if prot & 4 != 0 {
        perms |= Prot::EXEC;
    }

    perms
}
