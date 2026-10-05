use crate::emulator::context::Context;
use crate::emulator::utils::read_string;
use crate::os::code_stub::add_code_stub;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn hook_trace_code(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    // original base address: 0x484d8000
    add_code_stub(unicorn, "LIBOSAL", base_address + 0x304B0, "v_init_trace", v_init_trace);
    add_code_stub(unicorn, "LIBOSAL", base_address + 0x446F8, "trace_string", trace_string);
    add_code_stub(
        unicorn,
        "LIBOSAL",
        base_address + 0x36940,
        "v_trace_mq_info",
        v_trace_mq_info,
    );
    add_code_stub(
        unicorn,
        "LIBOSAL",
        base_address + 0x13E7C,
        "v_write_to_err_mem",
        v_write_to_err_mem,
    );
}

pub fn v_init_trace(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    0u32
}

pub fn trace_string(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let text = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    log::warn!(
        "[{}] trace: {} (caller={:#x})",
        unicorn.get_data().inner.thread_id(),
        text,
        unicorn.reg_read(RegisterARM::LR).unwrap_or(0) as u32
    );
    0u32
}

pub fn v_trace_mq_info(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let addr = unicorn.reg_read(RegisterARM::R0).unwrap() as u32;
    let arg2 = unicorn.reg_read(RegisterARM::R1).unwrap() as u32;
    let arg3 = unicorn.reg_read(RegisterARM::R2).unwrap() as u32;
    log::warn!(
        "[{}] mq info {:#x} {:#x} {:#x} (caller={:#x})",
        unicorn.get_data().inner.thread_id(),
        addr,
        arg2,
        arg3,
        unicorn.reg_read(RegisterARM::LR).unwrap_or(0) as u32
    );
    if addr != 0 {
        let text = read_string(unicorn, addr);
        log::warn!("{}", text);
    }
    0u32
}

pub fn v_write_to_err_mem(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let arg1 = unicorn.reg_read(RegisterARM::R0).unwrap() as u32;
    let arg2 = unicorn.reg_read(RegisterARM::R1).unwrap() as u32;
    log::warn!(
        "[{}] err {:#x} {:#x} (caller={:#x})",
        unicorn.get_data().inner.thread_id(),
        arg1,
        arg2,
        unicorn.reg_read(RegisterARM::LR).unwrap_or(0) as u32
    );
    if arg2 != 0 {
        let text = read_string(unicorn, arg2);
        log::warn!("{}", text);
    }
    0u32
}
