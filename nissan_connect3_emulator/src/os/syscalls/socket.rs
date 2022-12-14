use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn socket(unicorn: &mut Unicorn<Context>, domain: u32, socket_type: u32, protocol: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] socket(domain = {:#x}, socket_type: {:#x}, protocol: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        domain,
        socket_type,
        protocol,
    );

    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] socket => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}

pub fn connect(unicorn: &mut Unicorn<Context>, socket_fd: u32, addr: u32, addr_len: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] connect(socket_fd = {:#x}, addr: {:#x}, addr_len: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        socket_fd,
        addr,
        addr_len,
    );

    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] connect => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}

pub fn send(unicorn: &mut Unicorn<Context>, socket_fd: u32, buf: u32, len: u32, flags: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] send(socket_fd = {:#x}, buf: {:#x}, len: {:#x}, flags: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        socket_fd,
        buf,
        len,
        flags,
    );

    // TODO: implement
    let res = 0;

    let mut buf2 = vec![0u8; len as usize];
    unicorn.mem_read(buf as u64, &mut buf2).unwrap();
    let str = String::from_utf8(buf2).unwrap();

    log::trace!("Message: {}", str);

    log::trace!(
        "{:#x}: [{}] [SYSCALL] send => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}
