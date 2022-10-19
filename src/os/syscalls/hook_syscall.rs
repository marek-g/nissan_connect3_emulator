use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn hook_syscall(unicorn: &mut Unicorn<Context>, intno: u32) {
    /*let pc = uc.reg_read(RegisterARM64::PC as i32).unwrap();
    let syscall = get_syscall(uc);
    unicorn.syscall(syscall);*/
    log::debug!(
        "{:#x}: int {} syscall #{}, arg1: {:#x}, arg2: {:#x}, arg3: {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        intno,
        unicorn.reg_read_i32(RegisterARM::R7).unwrap(),
        unicorn.reg_read_i32(RegisterARM::R0).unwrap(),
        unicorn.reg_read_i32(RegisterARM::R1).unwrap(),
        unicorn.reg_read_i32(RegisterARM::R2).unwrap(),
    );

    /*let res = if unicorn.reg_read_i32(RegisterARM::R2).unwrap() == 0x17 {
        0x4738d000
    } else {
        0
    };*/
    let res = 0x17000;

    unicorn.reg_write(RegisterARM::R0 as i32, res).unwrap();
}
