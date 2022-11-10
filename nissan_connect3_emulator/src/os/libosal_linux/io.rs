use crate::emulator::context::Context;
use crate::emulator::utils::read_string;
use crate::os::add_code_hook;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn hook_io_code(unicorn: &mut Unicorn<Context>, base_address: u32) {
    // original base address: 0x484d8000
    add_code_hook!(unicorn, "LIBOSAL", base_address + 0x1994C, io_open);
    add_code_hook!(unicorn, "LIBOSAL", base_address + 0x19D74, io_create);
    add_code_hook!(unicorn, "LIBOSAL", base_address + 0x18DA4, s32_io_control);
    add_code_hook!(
        unicorn,
        "LIBOSAL",
        base_address + 0x31744,
        s32_check_for_iosc_queue
    );
}

pub fn io_open(unicorn: &mut Unicorn<Context>) -> u32 {
    let name = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    let param = unicorn.reg_read(RegisterARM::R1).unwrap();
    log::trace!("name: {}, param: {:#x}", name, param);
    5u32
}

pub fn io_create(unicorn: &mut Unicorn<Context>) -> u32 {
    let name = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    let param = unicorn.reg_read(RegisterARM::R1).unwrap();
    log::trace!("name: {}, param: {:#x}", name, param);
    0u32
}

pub fn s32_io_control(unicorn: &mut Unicorn<Context>) -> u32 {
    let fd = unicorn.reg_read(RegisterARM::R0).unwrap();
    let param = unicorn.reg_read(RegisterARM::R1).unwrap();
    log::trace!("fd: {:#x}, param: {:#x}", fd, param);
    0u32
}

pub fn s32_check_for_iosc_queue(unicorn: &mut Unicorn<Context>) -> u32 {
    let name = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    log::trace!("queue_name: {}", name);
    1u32
}
