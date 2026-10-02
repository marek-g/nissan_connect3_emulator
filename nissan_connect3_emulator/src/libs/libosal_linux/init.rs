use crate::emulator::context::Context;
use crate::emulator::utils::read_string;
use unicorn_engine::{RegisterARM, Unicorn};

/// Core-code hooks are DISABLED for now: they stub out the real libosal init/IO
/// code (e.g. vInitOsalCoreIOSC, which sets up the message pool via /dev/iosc)
/// with fake return values, preventing the real path from exercising the
/// emulated drivers. Let the real libosal code run instead. Re-enable these
/// (and the `add_code_hook!` import) for performance work.
pub fn hook_core_code(_unicorn: &mut Unicorn<'_, Context>, _base_address: u32) {
    // original base address: 0x484d8000
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x34A5C, v_init_osal_core_iosc);
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x34838, v_generate_term_mq_handle);
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x178CC, v_init_osal_io);
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x3FD98, shared_memory_open);
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x2CB24, v_read_assert_mode);
}

#[allow(dead_code)] // kept for re-enabling during performance work
pub fn v_init_osal_core_iosc(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    0u32
}

#[allow(dead_code)] // kept for re-enabling during performance work
pub fn v_generate_term_mq_handle(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    //let name = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    //log::trace!("queue_name: {}", name);
    0u32
}

#[allow(dead_code)] // kept for re-enabling during performance work
pub fn v_init_osal_io(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    0u32
}

#[allow(dead_code)] // kept for re-enabling during performance work
pub fn shared_memory_open(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let arg1 = unicorn.reg_read(RegisterARM::R0).unwrap();
    let arg2 = unicorn.reg_read(RegisterARM::R1).unwrap();
    let arg3 = unicorn.reg_read(RegisterARM::R2).unwrap();
    let arg4 = unicorn.reg_read(RegisterARM::R3).unwrap();
    let arg1 = read_string(unicorn, arg1 as u32);
    //let arg3 = read_string(unicorn, arg3 as u32);
    log::warn!(
        "arg1: {}, arg2: {:#x}, arg3: {:#x}, arg4: {:#x}",
        arg1,
        arg2,
        arg3,
        arg4
    );
    0u32
}

#[allow(dead_code)] // kept for re-enabling during performance work
pub fn v_read_assert_mode(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    0u32
}
