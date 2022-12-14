use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn ioctl(mut unicorn: &mut Unicorn<Context>, fd: u32, request: u32, addr: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] ioctl(fd = {:#x}, request: {:#x}, addr: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        fd,
        request,
        addr,
    );

    let file_system = unicorn.get_data().inner.file_system.clone();
    let res = file_system
        .lock()
        .unwrap()
        .ioctl(&mut unicorn, fd as i32, request, addr) as u32;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] ioctl => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}
