use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn ioctl(mut unicorn: &mut Unicorn<Context>, fd: u32, request: u32, addr: u32) -> u32 {
    let file_system = unicorn.get_data().file_system.clone();
    let res = file_system
        .lock()
        .unwrap()
        .ioctl(&mut unicorn, fd as i32, request, addr) as u32;

    log::trace!(
        "{:#x}: [SYSCALL] ioctl(fd = {:#x}, request: {:#x}, addr: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        fd,
        request,
        addr,
        res
    );

    res
}
