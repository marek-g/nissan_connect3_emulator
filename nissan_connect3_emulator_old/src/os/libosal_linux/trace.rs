use crate::emulator::context::Context;
use crate::emulator::utils::read_string;
use crate::os::add_code_hook;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn hook_trace_code(unicorn: &mut Unicorn<Context>, base_address: u32) {
    // original base address: 0x484d8000
    add_code_hook!(unicorn, "LIBOSAL", base_address + 0x304B0, v_init_trace);
    add_code_hook!(
        unicorn,
        "LIBOSAL",
        base_address + 0x34A5C,
        v_init_osal_core_iosc
    );
    add_code_hook!(unicorn, "LIBOSAL", base_address + 0x446F8, trace_string);
    add_code_hook!(unicorn, "LIBOSAL", base_address + 0x36940, v_trace_mq_info);
    add_code_hook!(
        unicorn,
        "LIBOSAL",
        base_address + 0x13E7C,
        v_write_to_err_mem
    );
}

pub fn v_init_trace(_unicorn: &mut Unicorn<Context>) -> u32 {
    0u32
}

pub fn v_init_osal_core_iosc(_unicorn: &mut Unicorn<Context>) -> u32 {
    0u32
}

pub fn trace_string(unicorn: &mut Unicorn<Context>) -> u32 {
    let text = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    log::warn!("{}", text);
    0u32
}

pub fn v_trace_mq_info(unicorn: &mut Unicorn<Context>) -> u32 {
    let addr = unicorn.reg_read(RegisterARM::R0).unwrap() as u32;
    let arg2 = unicorn.reg_read(RegisterARM::R1).unwrap() as u32;
    let arg3 = unicorn.reg_read(RegisterARM::R2).unwrap() as u32;
    log::warn!("{:#x} {:#x} {:#x}", addr, arg2, arg3);
    if addr != 0 {
        let text = read_string(unicorn, addr);
        log::warn!("{}", text);
    }
    0u32
}

pub fn v_write_to_err_mem(unicorn: &mut Unicorn<Context>) -> u32 {
    let arg1 = unicorn.reg_read(RegisterARM::R0).unwrap() as u32;
    let arg2 = unicorn.reg_read(RegisterARM::R1).unwrap() as u32;
    log::warn!("{:#x} {:#x}", arg1, arg2);
    if arg2 != 0 {
        let text = read_string(unicorn, arg2);
        log::warn!("{}", text);
    }
    0u32
}
