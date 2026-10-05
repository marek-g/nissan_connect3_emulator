use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

const ORIGINAL_BASE: u32 = 0x484d_8000;

pub fn hook_thread_code(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    const THREAD_CREATE: u32 = 0x4851_578c - ORIGINAL_BASE;
    const THREAD_ACTIVATE: u32 = 0x4851_5650 - ORIGINAL_BASE;

    for (offset, name) in [
        (THREAD_CREATE, "OSAL_ThreadCreate"),
        (THREAD_ACTIVATE, "OSAL_s32ThreadActivate"),
    ] {
        unicorn
            .add_code_hook(
                (base_address + offset) as u64,
                (base_address + offset) as u64,
                move |uc, addr, _| log_thread_call(uc, addr as u32, base_address, name),
            )
            .unwrap();
    }
}

fn log_thread_call(unicorn: &mut Unicorn<'_, Context>, addr: u32, base_address: u32, name: &str) {
    let r0 = unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    let thread = unicorn.get_data().inner.thread_id();
    let details = if name == "OSAL_ThreadCreate" {
        format!("params={:#x}", r0)
    } else {
        format!("tid={}", r0)
    };
    log::info!(
        "0x{:x} [{}] [LIBOSAL] {}({})",
        addr - base_address + ORIGINAL_BASE,
        thread,
        name,
        details
    );
}