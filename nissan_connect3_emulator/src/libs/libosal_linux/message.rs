use crate::emulator::context::Context;
use crate::emulator::utils::{pack_u32, read_string};
use unicorn_engine::{RegisterARM, Unicorn};

/// Custom message-queue hooks are DISABLED for now: they stub out the real libosal
/// message-pool/queue code with fake return values, which prevents the real path from
/// exercising the emulated /dev/iosc + POSIX mqueue syscalls. Let the real libosal code
/// run instead. Re-enable these (and the `add_code_hook!` import) for performance work.
pub fn hook_message_code(_unicorn: &mut Unicorn<'_, Context>, _base_address: u32) {
    // original base address: 0x484d8000
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x2F54C, v_init_message_pool);
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x3A020, s32_message_pool_create);
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x33A98, u32_open_msg_queue);
}

// vInitMessagePool
#[allow(dead_code)] // kept for re-enabling during performance work
pub fn v_init_message_pool(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    0u32
}

// OSAL_s32MessagePoolCreate
#[allow(dead_code)] // kept for re-enabling during performance work
pub fn s32_message_pool_create(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let size = unicorn.reg_read(RegisterARM::R0).unwrap() as u32;
    log::warn!("size: {}", size);
    0u32
}

/// u32OpenMsgQueue
#[allow(dead_code)] // kept for re-enabling during performance work
pub fn u32_open_msg_queue(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let queue_name = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    let arg2 = unicorn.reg_read(RegisterARM::R1).unwrap();
    unicorn.mem_write(arg2, &pack_u32(1)).unwrap();
    log::warn!("queue_name: {}, arg2: {:#x}", queue_name, arg2);
    1u32
}


