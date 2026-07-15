use crate::emulator::context::Context;
use crate::emulator::utils::{pack_u32, read_string};
use crate::os::add_code_hook;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn hook_message_code(unicorn: &mut Unicorn<Context>, base_address: u32) {
    // original base address: 0x484d8000
    add_code_hook!(
        unicorn,
        "LIBOSAL",
        base_address + 0x2F54C,
        v_init_message_pool
    );
    add_code_hook!(
        unicorn,
        "LIBOSAL",
        base_address + 0x3A020,
        s32_message_pool_create
    );
    add_code_hook!(
        unicorn,
        "LIBOSAL",
        base_address + 0x33A98,
        u32_open_msg_queue
    );
    /*add_code_hook!(
        unicorn,
        "LIBOSAL",
        base_address + 0x37028,
        message_queue_open
    );*/
}

// vInitMessagePool
pub fn v_init_message_pool(_unicorn: &mut Unicorn<Context>) -> u32 {
    0u32
}

// OSAL_s32MessagePoolCreate
pub fn s32_message_pool_create(unicorn: &mut Unicorn<Context>) -> u32 {
    let size = unicorn.reg_read(RegisterARM::R0).unwrap() as u32;
    log::warn!("size: {}", size);
    0u32
}

/// u32OpenMsgQueue
pub fn u32_open_msg_queue(unicorn: &mut Unicorn<Context>) -> u32 {
    let queue_name = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    let arg2 = unicorn.reg_read(RegisterARM::R1).unwrap();
    unicorn.mem_write(arg2, &pack_u32(1)).unwrap();
    log::warn!("queue_name: {}, arg2: {:#x}", queue_name, arg2);
    1u32
}

/// OSAL_s32MessageQueueOpen
pub fn message_queue_open(unicorn: &mut Unicorn<Context>) -> u32 {
    let queue_name = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    log::warn!("queue_name: {}", queue_name);
    0u32
}
