use crate::emulator::context::Context;
use crate::os::add_code_hook;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn hook_trace_code(unicorn: &mut Unicorn<Context>, base_address: u32) {
    add_code_hook!(unicorn, "LIBTRACE", base_address + 0x00002f58, trace_init);
    add_code_hook!(
        unicorn,
        "LIBTRACE",
        base_address + 0x00004634,
        trace_tr_chan_access
    );
    add_code_hook!(
        unicorn,
        "LIBTRACE",
        base_address + 0x000043a0,
        trace_tr_core_uw_trace_out
    );
    add_code_hook!(
        unicorn,
        "LIBTRACE",
        base_address + 0x00007864,
        trace_sharedmem_create_dual_os
    );
    add_code_hook!(unicorn, "LIBTRACE", base_address + 0x0000513c, trace_stop);
    add_code_hook!(
        unicorn,
        "LIBTRACE",
        base_address + 0x000076e4,
        trace_tr_core_is_class_selected
    );
}

pub fn trace_init(unicorn: &mut Unicorn<Context>) -> u32 {
    0u32
}

pub fn trace_tr_chan_access(unicorn: &mut Unicorn<Context>) -> u32 {
    0u32
}

pub fn trace_tr_core_uw_trace_out(unicorn: &mut Unicorn<Context>) -> u32 {
    0u32
}

pub fn trace_sharedmem_create_dual_os(unicorn: &mut Unicorn<Context>) -> u32 {
    1u32
}

pub fn trace_stop(unicorn: &mut Unicorn<Context>) -> u32 {
    1u32
}

pub fn trace_tr_core_is_class_selected(unicorn: &mut Unicorn<Context>) -> u32 {
    1u32
}
