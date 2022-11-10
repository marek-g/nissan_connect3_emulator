use crate::emulator::context::Context;
use crate::os::add_code_hook;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn hook_trace_code(unicorn: &mut Unicorn<Context>, base_address: u32) {
    add_code_hook!(unicorn, "LIBOSAL", base_address + 0x304B0, v_init_trace);
}

pub fn v_init_trace(unicorn: &mut Unicorn<Context>) -> u32 {
    0u32
}
