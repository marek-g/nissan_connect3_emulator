use crate::emulator::context::Context;
use crate::emulator::utils::{pack_u32, read_string};
use unicorn_engine::{RegisterARM, Unicorn};

/// Message-queue observation hooks.
///
/// These are intentionally read-only: they do not stub the real libosal code, but
/// decode enough of the OSAL queue handle structure to log which queue is being
/// waited on or posted to. OSAL queue handles point to a queue info block whose
/// `+0x38` field is the queue name and whose `+0x0a` field is the queue type.
pub fn hook_message_code(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    // original base address: 0x484d8000
    const QUEUE_WAIT: u32 = 0x4850fe18 - 0x484d8000;
    const QUEUE_POST: u32 = 0x48510650 - 0x484d8000;
    const QUEUE_NOTIFY: u32 = 0x4850e1a8 - 0x484d8000;
    const QUEUE_PRIORITY_WAIT: u32 = 0x4850d8a8 - 0x484d8000;
    const GET_FROM_MQ: u32 = 0x48509c8c - 0x484d8000;

    for (offset, name) in [
        (QUEUE_WAIT, "OSAL_s32MessageQueueWait"),
        (QUEUE_POST, "OSAL_s32MessageQueuePost"),
        (QUEUE_NOTIFY, "OSAL_s32MessageQueueNotify"),
        (QUEUE_PRIORITY_WAIT, "OSAL_s32MessageQueuePriorityWait"),
        (GET_FROM_MQ, "u32GetFromMessageQueue"),
    ] {
        unicorn
            .add_code_hook(
                (base_address + offset) as u64,
                (base_address + offset) as u64,
                move |uc, addr, _| log_queue_api(uc, addr as u32, base_address, name),
            )
            .unwrap();
    }
}

fn log_queue_api(
    unicorn: &mut Unicorn<'_, Context>,
    addr: u32,
    base_address: u32,
    api_name: &str,
) {
    let thread = unicorn.get_data().inner.thread_id();
    let r0 = unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    let r1 = unicorn.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
    let r2 = unicorn.reg_read(RegisterARM::R2).unwrap_or(0) as u32;

    match api_name {
        "u32GetFromMessageQueue" => {
            let message_type = read_u32_or_invalid(unicorn, r1);
            log::info!(
                "0x{:x} [{}] [LIBOSAL] {}(iosc_handle=0x{:x}, msg=0x{:x}, msg_type=0x{:x}, timeout=0x{:x})",
                addr - base_address + 0x484d8000,
                thread,
                api_name,
                r0,
                r1,
                message_type,
                r2
            );
        }
        _ => {
            let (queue_name, queue_type, info_addr) = decode_queue_handle(unicorn, r0);
            if api_name == "OSAL_s32MessageQueuePost" {
                let words = [
                    read_u32_or_invalid(unicorn, r1),
                    read_u32_or_invalid(unicorn, r1 + 4),
                    read_u32_or_invalid(unicorn, r1 + 8),
                    read_u32_or_invalid(unicorn, r1 + 12),
                ];
                log::info!(
                    "0x{:x} [{}] [LIBOSAL] {}(handle=0x{:x}, info=0x{:x}, type={}, name={}, msg=0x{:x}, len=0x{:x}, data=[{:x}, {:x}, {:x}, {:x}])",
                    addr - base_address + 0x484d8000,
                    thread,
                    api_name,
                    r0,
                    info_addr,
                    queue_type,
                    queue_name,
                    r1,
                    r2,
                    words[0],
                    words[1],
                    words[2],
                    words[3]
                );
            } else {
                log::info!(
                    "0x{:x} [{}] [LIBOSAL] {}(handle=0x{:x}, info=0x{:x}, type={}, name={}, r1=0x{:x}, timeout=0x{:x})",
                    addr - base_address + 0x484d8000,
                    thread,
                    api_name,
                    r0,
                    info_addr,
                    queue_type,
                    queue_name,
                    r1,
                    r2
                );
            }
        }
    }

}

fn decode_queue_handle(unicorn: &Unicorn<'_, Context>, handle: u32) -> (String, u32, u32) {
    let info_addr = read_u32_or_invalid(unicorn, handle + 0xc);
    if info_addr == 0 || info_addr > 0xf000_0000 {
        return ("<invalid>".to_string(), 0xffff, info_addr);
    }

    let queue_type = read_u16_or_invalid(unicorn, info_addr + 0x0a);
    let name = read_string(unicorn, info_addr + 0x38);
    (name, queue_type, info_addr)
}

fn read_u32_or_invalid(unicorn: &Unicorn<'_, Context>, addr: u32) -> u32 {
    let mut bytes = [0u8; 4];
    match unicorn.mem_read(addr as u64, &mut bytes) {
        Ok(()) => u32::from_le_bytes(bytes),
        Err(_) => 0xffff_ffff,
    }
}

fn read_u16_or_invalid(unicorn: &Unicorn<'_, Context>, addr: u32) -> u32 {
    let mut bytes = [0u8; 2];
    match unicorn.mem_read(addr as u64, &mut bytes) {
        Ok(()) => u16::from_le_bytes(bytes) as u32,
        Err(_) => 0xffff,
    }
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


