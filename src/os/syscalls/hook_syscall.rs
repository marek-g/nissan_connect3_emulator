use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn hook_syscall(unicorn: &mut Unicorn<Context>, intno: u32) {
    let syscall_number = unicorn.reg_read_i32(RegisterARM::R7).unwrap();

    let res = match syscall_number {
        //45 => 0u32,
        _ => {
            panic!(
                "{:#x}: not implemented syscall #{} (int {}), args: {:#x}, {:#x}, {:#x}, ...",
                unicorn.reg_read(RegisterARM::PC).unwrap(),
                unicorn.reg_read_i32(RegisterARM::R7).unwrap(),
                intno,
                unicorn.reg_read_i32(RegisterARM::R0).unwrap(),
                unicorn.reg_read_i32(RegisterARM::R1).unwrap(),
                unicorn.reg_read_i32(RegisterARM::R2).unwrap(),
            );
        }
    };

    unicorn
        .reg_write(RegisterARM::R0 as i32, res as u64)
        .unwrap();
}
