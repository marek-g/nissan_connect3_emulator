use crate::emulator::context::Context;
use crate::emulator::utils::read_string;
use unicorn_engine::{RegisterARM, Unicorn};

/// IO-code hooks are DISABLED for now: they stub out the real libosal IO code
/// (open/create/ioctl and the IOSC-queue classification) with fake return
/// values. Let the real libosal code run against the emulated drivers instead.
/// Re-enable these (and the `add_code_hook!` import) for performance work.
pub fn hook_io_code(_unicorn: &mut Unicorn<'_, Context>, _base_address: u32) {
    // original base address: 0x484d8000
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x1994C, io_open);
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x19D74, io_create);
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x18DA4, s32_io_control);
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x31744, s32_check_for_iosc_queue);
}

#[allow(dead_code)] // kept for re-enabling during performance work
pub fn io_open(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let name = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    let param = unicorn.reg_read(RegisterARM::R1).unwrap();
    log::trace!("name: {}, param: {:#x}", name, param);
    5u32
}

#[allow(dead_code)] // kept for re-enabling during performance work
pub fn io_create(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let name = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    let param = unicorn.reg_read(RegisterARM::R1).unwrap();
    log::trace!("name: {}, param: {:#x}", name, param);
    0u32
}

#[allow(dead_code)] // kept for re-enabling during performance work
pub fn s32_io_control(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let fd = unicorn.reg_read(RegisterARM::R0).unwrap();
    let param = unicorn.reg_read(RegisterARM::R1).unwrap();
    log::trace!("fd: {:#x}, param: {:#x}", fd, param);
    0u32
}

#[allow(dead_code)] // kept for re-enabling during performance work
pub fn s32_check_for_iosc_queue(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let name = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    log::trace!("queue_name: {}", name);
    1u32
}
