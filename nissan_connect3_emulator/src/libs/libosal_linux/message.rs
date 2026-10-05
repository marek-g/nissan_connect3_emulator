use crate::common::osal_queues::OsalQueueService;
use crate::emulator::context::Context;

const ORIGINAL_BASE: u32 = 0x484d_8000;
const OSAL_CORE_GLOBAL: u32 = 0x4856_79e0;
use crate::emulator::thread::{BlockReason, ThreadStatus};
use crate::emulator::utils::pack_u32;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};
use unicorn_engine::unicorn_const::Prot;
use unicorn_engine::{RegisterARM, Unicorn};

static SYNTH_PWR_START_CONF_SENT: AtomicBool = AtomicBool::new(false);
static SYNTH_PWR_START_CONF_CONTENT: AtomicU32 = AtomicU32::new(0);

/// Message-queue observation and OSAL service bridge hooks.
///
/// These hooks decode enough of the OSAL queue handle structure to log which
/// queue is being waited on or posted to. For the boot queues that participate in
/// Linux <-> RTOS communication, they also bridge the guest libosal queue traffic
/// into the shared OSAL queue service. OSAL queue handles point to a queue info
/// block whose `+0x38` field is the queue name and whose `+0x0a` field is the
/// queue type.
pub fn hook_message_code(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    // original base address: 0x484d8000
    const QUEUE_OPEN: u32 = 0x4850f028 - 0x484d8000;
    const QUEUE_CREATE: u32 = 0x4850f2e0 - 0x484d8000;
    const QUEUE_WAIT: u32 = 0x4850fe18 - 0x484d8000;
    const QUEUE_POST: u32 = 0x48510650 - 0x484d8000;
    const QUEUE_NOTIFY: u32 = 0x4850e1a8 - 0x484d8000;
    const QUEUE_PRIORITY_WAIT: u32 = 0x4850d8a8 - 0x484d8000;
    const GET_FROM_MQ: u32 = 0x48509c8c - 0x484d8000;
    const CHECK_FOR_IOS_QUEUE: u32 = 0x48509744 - 0x484d8000;
    const MESSAGE_DELETE: u32 = 0x48513850 - 0x484d8000;

    unicorn
        .add_code_hook(
            (base_address + QUEUE_OPEN) as u64,
            (base_address + QUEUE_OPEN) as u64,
            move |uc, addr, _| {
                fallback_open_to_create(uc, addr as u32, base_address, QUEUE_CREATE);
            },
        )
        .unwrap();

    unicorn
        .add_code_hook(
            (base_address + CHECK_FOR_IOS_QUEUE) as u64,
            (base_address + CHECK_FOR_IOS_QUEUE) as u64,
            move |uc, addr, _| {
                force_non_iosc_queue(uc, addr as u32, base_address);
            },
        )
        .unwrap();

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
                move |uc, addr, _| handle_queue_api(uc, addr as u32, base_address, name),
            )
            .unwrap();
    }

    unicorn
        .add_code_hook(
            (base_address + MESSAGE_DELETE) as u64,
            (base_address + MESSAGE_DELETE) as u64,
            move |uc, addr, _| suppress_synthetic_message_delete(uc, addr as u32, base_address),
        )
        .unwrap();
}

fn handle_queue_api(
    unicorn: &mut Unicorn<'_, Context>,
    addr: u32,
    base_address: u32,
    api_name: &str,
) {
    let thread = unicorn.get_data().inner.thread_id();
    let r0 = unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    let r1 = unicorn.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
    let r2 = unicorn.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
    let r3 = unicorn.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
    let lr = unicorn.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
    let caller = {
        let mmu = unicorn.get_data().mmu.lock().unwrap();
        match mmu.executable_location(lr) {
            Some((library, offset)) => format!("{}+0x{:x}", library, offset),
            None => format!("0x{:x}", lr),
        }
    };

    match api_name {
        "u32GetFromMessageQueue" => {
            let message_type = read_u32_or_invalid(unicorn, r1);
            log::info!(
                "0x{:x} [{}] [LIBOSAL] {}(iosc_handle=0x{:x}, msg=0x{:x}, msg_type=0x{:x}, timeout=0x{:x}, caller={})",
                addr - base_address + 0x484d8000,
                thread,
                api_name,
                r0,
                r1,
                message_type,
                r2,
                caller
            );
        }
        _ => {
            let decoded = decode_queue_handle(unicorn, r0);
            if api_name == "OSAL_s32MessageQueuePost" {
                let words = [
                    read_u32_or_invalid(unicorn, r1),
                    read_u32_or_invalid(unicorn, r1 + 4),
                    read_u32_or_invalid(unicorn, r1 + 8),
                    read_u32_or_invalid(unicorn, r1 + 12),
                ];
                log::info!(
                    "0x{:x} [{}] [LIBOSAL] {}(handle=0x{:x}, info=0x{:x}, type={}, name={}, msg=0x{:x}, len=0x{:x}, data=[{:x}, {:x}, {:x}, {:x}], caller={})",
                    addr - base_address + 0x484d8000,
                    thread,
                    api_name,
                    r0,
                    decoded.info,
                    decoded.queue_type,
                    decoded.name,
                    r1,
                    r2,
                    words[0],
                    words[1],
                    words[2],
                    words[3],
                    caller
                );
            } else {
                let stack_timeout = read_stack_timeout(unicorn, api_name);
                log::info!(
                    "0x{:x} [{}] [LIBOSAL] {}(handle=0x{:x}, info=0x{:x}, type={}, name={}, buf=0x{:x}, size=0x{:x}, timeout=0x{:x}, caller={})",
                    addr - base_address + 0x484d8000,
                    thread,
                    api_name,
                    r0,
                    decoded.info,
                    decoded.queue_type,
                    decoded.name,
                    r1,
                    r2,
                    stack_timeout,
                    caller
                );
            }

            let stack_timeout = read_stack_timeout(unicorn, api_name);
            if bridge_guest_osal_queue(unicorn, api_name, &decoded.name, r1, r2, r3, stack_timeout)
            {
                log::info!(
                    "0x{:x} [{}] [LIBOSAL-OSAL-SERVICE] {} handled name={}",
                    addr - base_address + 0x484d8000,
                    thread,
                    api_name,
                    decoded.name
                );
            } else {
                let _ = synthesize_ail_power_start_conf(
                    unicorn,
                    api_name,
                    &decoded.name,
                    r1,
                    stack_timeout,
                );
            }
        }
    }
}

fn fallback_open_to_create(
    unicorn: &mut Unicorn<'_, Context>,
    addr: u32,
    base_address: u32,
    create_offset: u32,
) {
    const OSAL_QUEUE_TABLE_OFFSET: u32 = 0x25220;
    const OSAL_QUEUE_ENTRY_SIZE: u32 = 0x5c;
    const OSAL_QUEUE_ENTRY_IN_USE_OFFSET: u32 = 0x08;
    const OSAL_QUEUE_ENTRY_NAME_OFFSET: u32 = 0x38;

    let core_global = base_address + (OSAL_CORE_GLOBAL - ORIGINAL_BASE);
    let name_ptr = unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    let flags = unicorn.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
    let handle_ptr = unicorn.reg_read(RegisterARM::R2).unwrap_or(0) as u32;

    let name = read_cstr(unicorn, name_ptr, 0x20);
    let thread = unicorn.get_data().inner.thread_id();
    let lr = unicorn.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
    let caller = {
        let mmu = unicorn.get_data().mmu.lock().unwrap();
        match mmu.executable_location(lr) {
            Some((library, offset)) => format!("{}+0x{:x}", library, offset),
            None => format!("0x{:x}", lr),
        }
    };
    log::info!(
        "0x{:x} [{}] [LIBOSAL] OSAL_s32MessageQueueOpen(name={}, flags={:#x}, out=0x{:x}, caller={})",
        addr - base_address + 0x484d8000,
        thread,
        name,
        flags,
        handle_ptr,
        caller
    );
    let Some((max_msg, msg_size)) = fallback_create_params(&name) else {
        return;
    };
    if handle_ptr == 0 || handle_ptr > 0xf000_0000 || flags > 4 {
        return;
    }

    if osal_queue_exists(
        unicorn,
        &name,
        core_global,
        OSAL_QUEUE_TABLE_OFFSET,
        OSAL_QUEUE_ENTRY_SIZE,
        OSAL_QUEUE_ENTRY_IN_USE_OFFSET,
        OSAL_QUEUE_ENTRY_NAME_OFFSET,
    ) {
        return;
    }

    let sp = unicorn.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
    if sp == 0
        || sp > 0xf000_0000
        || unicorn
            .mem_write(sp as u64, &handle_ptr.to_le_bytes())
            .is_err()
    {
        return;
    }

    log::info!(
        "0x{:x} [{}] [LIBOSAL] OSAL_s32MessageQueueOpen({}) creates missing queue max={} size={}",
        addr - base_address + 0x484d8000,
        thread,
        name,
        max_msg,
        msg_size
    );

    // OSAL_s32MessageQueueCreate(char *name, uint maxmsg, uint msgsize, int type, handle *out)
    unicorn.reg_write(RegisterARM::R1, max_msg as u64).unwrap();
    unicorn.reg_write(RegisterARM::R2, msg_size as u64).unwrap();
    unicorn.reg_write(RegisterARM::R3, flags as u64).unwrap();
    unicorn
        .reg_write(RegisterARM::PC, (base_address + create_offset) as u64)
        .unwrap();
}

fn fallback_create_params(name: &str) -> Option<(u32, u32)> {
    if name == "PRM_MSGQUEUE" {
        return Some((100, 0x70));
    }

    if let Some(id) = name.strip_prefix("mbx_") {
        let id = id.parse::<u32>().ok()?;
        return Some(if id == 265 { (800, 8) } else { (120, 8) });
    }

    None
}

fn force_non_iosc_queue(unicorn: &mut Unicorn<'_, Context>, addr: u32, base_address: u32) {
    let name_ptr = unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    if name_ptr == 0 || name_ptr > 0xf000_0000 {
        return;
    }

    let name = read_cstr(unicorn, name_ptr, 0x20);
    if !name.starts_with("mbx_") {
        return;
    }

    let thread = unicorn.get_data().inner.thread_id();
    let lr = unicorn.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
    let caller = {
        let mmu = unicorn.get_data().mmu.lock().unwrap();
        match mmu.executable_location(lr) {
            Some((library, offset)) => format!("{}+0x{:x}", library, offset),
            None => format!("0x{:x}", lr),
        }
    };
    log::info!(
        "0x{:x} [{}] [LIBOSAL] s32CheckForIOSCQueue({}) forced non-IOSC caller={}",
        addr - base_address + 0x484d8000,
        thread,
        name,
        caller
    );
    return_to_caller(unicorn, 0xffff_ffff);
}

fn osal_queue_exists(
    unicorn: &Unicorn<'_, Context>,
    name: &str,
    core_global: u32,
    table_offset: u32,
    entry_size: u32,
    in_use_offset: u32,
    name_offset: u32,
) -> bool {
    let Some(core) = osal_core_base(unicorn, core_global) else {
        return false;
    };

    let table = core.wrapping_add(table_offset);
    for index in 0..0x100u32 {
        let entry = table.wrapping_add(index * entry_size);
        if read_u8_or_invalid(unicorn, entry + in_use_offset) != 1 {
            continue;
        }
        if read_cstr(unicorn, entry + name_offset, 0x20) == name {
            return true;
        }
    }

    false
}

fn osal_core_base(unicorn: &Unicorn<'_, Context>, core_global: u32) -> Option<u32> {
    let core_struct = read_u32_or_invalid(unicorn, core_global);
    if core_struct == 0 || core_struct > 0xf000_0000 {
        return None;
    }

    let core = read_u32_or_invalid(unicorn, core_struct);
    if core == 0 || core > 0xf000_0000 {
        return None;
    }

    Some(core)
}

fn read_u8_or_invalid(unicorn: &Unicorn<'_, Context>, addr: u32) -> u32 {
    let mut byte = [0u8; 1];
    match unicorn.mem_read(addr as u64, &mut byte) {
        Ok(()) => byte[0] as u32,
        Err(_) => 0xff,
    }
}

fn read_stack_timeout(unicorn: &Unicorn<'_, Context>, api_name: &str) -> u32 {
    if matches!(
        api_name,
        "OSAL_s32MessageQueueWait" | "OSAL_s32MessageQueuePriorityWait"
    ) {
        let sp = unicorn.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
        read_u32_or_invalid(unicorn, sp)
    } else {
        0
    }
}

fn synthesize_ail_power_start_conf(
    unicorn: &mut Unicorn<'_, Context>,
    api_name: &str,
    name: &str,
    buf: u32,
    stack_timeout: u32,
) -> bool {
    const TARGET_QUEUE: &str = "mbx_265";
    const CONTENT_LEN: u32 = 0x20;

    if !matches!(
        api_name,
        "OSAL_s32MessageQueueWait" | "OSAL_s32MessageQueuePriorityWait"
    ) {
        return false;
    }
    if name != TARGET_QUEUE || stack_timeout != u32::MAX {
        return false;
    }
    if buf == 0 || buf > 0xf000_0000 {
        return false;
    }
    if SYNTH_PWR_START_CONF_SENT.swap(true, Ordering::Relaxed) {
        return false;
    }

    let mmu_arc = unicorn.get_data().mmu.clone();
    let content = mmu_arc.lock().unwrap().heap_alloc(
        unicorn,
        CONTENT_LEN,
        Prot::READ | Prot::WRITE,
        "[synthetic-cca-power-conf]",
    );
    if content == 0 {
        SYNTH_PWR_START_CONF_SENT.store(false, Ordering::Relaxed);
        return false;
    }

    let mut body = [0u8; CONTENT_LEN as usize];
    body[0x00..0x02].copy_from_slice(&0x0109u16.to_le_bytes());
    body[0x04..0x08].copy_from_slice(&CONTENT_LEN.to_le_bytes());
    body[0x08..0x0a].copy_from_slice(&0x0002u16.to_le_bytes());
    body[0x0b] = 0x40;
    body[0x0c..0x0e].copy_from_slice(&0x0001u16.to_le_bytes());
    body[0x14..0x16].copy_from_slice(&0x0003u16.to_le_bytes());

    if unicorn.mem_write(content as u64, &body).is_err()
        || unicorn.mem_write(buf as u64, &1u32.to_le_bytes()).is_err()
        || unicorn
            .mem_write((buf + 4) as u64, &content.to_le_bytes())
            .is_err()
    {
        SYNTH_PWR_START_CONF_SENT.store(false, Ordering::Relaxed);
        return false;
    }

    SYNTH_PWR_START_CONF_CONTENT.store(content, Ordering::Relaxed);
    let thread = unicorn.get_data().inner.thread_id();
    log::info!(
        "[{}] [LIBOSAL] {}({}) answered with synthetic PWR_PROXY_START_CONF content=0x{:x}",
        thread,
        api_name,
        name,
        content
    );
    return_to_caller(unicorn, 8);
    true
}

fn suppress_synthetic_message_delete(
    unicorn: &mut Unicorn<'_, Context>,
    addr: u32,
    base_address: u32,
) {
    let handle = unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    let content = unicorn.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
    let synthetic = SYNTH_PWR_START_CONF_CONTENT.load(Ordering::Relaxed);
    if synthetic == 0 || (handle & 0xff) != 1 || content != synthetic {
        return;
    }

    SYNTH_PWR_START_CONF_CONTENT.store(0, Ordering::Relaxed);
    let thread = unicorn.get_data().inner.thread_id();
    log::info!(
        "0x{:x} [{}] [LIBOSAL] OSAL_s32MessageDelete ignored synthetic PWR_PROXY_START_CONF content=0x{:x}",
        addr - base_address + ORIGINAL_BASE,
        thread,
        content
    );
    return_to_caller(unicorn, 0);
}

fn bridge_guest_osal_queue(
    unicorn: &mut Unicorn<'_, Context>,
    api_name: &str,
    name: &str,
    msg_ptr: u32,
    msg_len: u32,
    prio_ptr: u32,
    timeout: u32,
) -> bool {
    match api_name {
        "OSAL_s32MessageQueuePost" => {
            if !OsalQueueService::is_rtos_intercepted_queue(name) {
                return false;
            }

            let data = match read_guest_buffer(unicorn, msg_ptr, msg_len as usize) {
                Some(data) => data,
                None => return false,
            };

            let accepted = {
                let mut state = unicorn.get_data().namespace.lock().unwrap();
                let accepted = OsalQueueService::guest_post(&mut state.mq, name, data, prio_ptr);
                if accepted {
                    state.notify_waiters();
                }
                accepted
            };

            if accepted {
                return_to_caller(unicorn, 0);
            }
            accepted
        }
        "OSAL_s32MessageQueueWait" | "OSAL_s32MessageQueuePriorityWait" => {
            if !OsalQueueService::is_rtos_intercepted_queue(name) {
                return false;
            }

            let (queue_id, message) = {
                let mut state = unicorn.get_data().namespace.lock().unwrap();
                let queue_id = OsalQueueService::ensure_queue(&mut state.mq, name);
                let message =
                    OsalQueueService::pop_guest_message(&mut state.mq, queue_id, msg_len as usize);
                if message.is_some() {
                    state.notify_waiters();
                }
                (queue_id, message)
            };

            if let Some(message) = message {
                if !message.data.is_empty()
                    && unicorn.mem_write(msg_ptr as u64, &message.data).is_err()
                {
                    return false;
                }
                if prio_ptr != 0
                    && unicorn
                        .mem_write(prio_ptr as u64, &pack_u32(message.priority))
                        .is_err()
                {
                    return false;
                }

                return_to_caller(unicorn, message.data.len() as u32);
                return true;
            }

            if timeout == 0 {
                return_to_caller(unicorn, 0);
                return true;
            }

            block_guest_osal_wait(
                unicorn,
                queue_id,
                msg_ptr,
                msg_len,
                prio_ptr,
                deadline_from_osal_timeout(timeout),
            );
            true
        }
        _ => false,
    }
}

fn read_guest_buffer(unicorn: &Unicorn<'_, Context>, addr: u32, len: usize) -> Option<Vec<u8>> {
    if len == 0 {
        return Some(Vec::new());
    }

    let mut data = vec![0u8; len];
    unicorn.mem_read(addr as u64, &mut data).ok()?;
    Some(data)
}

fn deadline_from_osal_timeout(timeout: u32) -> Option<Instant> {
    if timeout == u32::MAX {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(timeout as u64))
    }
}

fn block_guest_osal_wait(
    unicorn: &mut Unicorn<'_, Context>,
    queue_id: u32,
    msg_ptr: u32,
    msg_len: u32,
    prio_ptr: u32,
    deadline: Option<Instant>,
) {
    return_to_caller(unicorn, 0);

    let tid = unicorn.get_data().thread_id();
    {
        let threads = unicorn.get_data().threads.clone();
        let mut threads = threads.lock().unwrap();
        if let Some(thread) = threads.iter_mut().find(|thread| thread.id == tid) {
            thread.status = ThreadStatus::Blocked(BlockReason::OsalQueueReceive {
                queue_id,
                msg_ptr,
                msg_len,
                prio_ptr,
                deadline,
            });
        }
    }

    unicorn.emu_stop().unwrap();
}

fn return_to_caller(unicorn: &mut Unicorn<'_, Context>, ret: u32) {
    let lr = unicorn.reg_read(RegisterARM::LR).unwrap_or(0);
    unicorn.reg_write(RegisterARM::R0, ret as u64).unwrap();
    unicorn.reg_write(RegisterARM::PC, lr).unwrap();
}

struct DecodedQueueHandle {
    info: u32,
    queue_type: u32,
    name: String,
}

fn decode_queue_handle(unicorn: &Unicorn<'_, Context>, handle: u32) -> DecodedQueueHandle {
    let info_addr = handle
        .checked_add(0xc)
        .map(|addr| read_u32_or_invalid(unicorn, addr))
        .unwrap_or(0xffff_ffff);
    if info_addr == 0 || info_addr > 0xf000_0000 {
        return DecodedQueueHandle {
            info: info_addr,
            queue_type: 0xffff,
            name: "<invalid>".to_string(),
        };
    }

    let queue_type = read_u16_or_invalid(unicorn, info_addr + 0x0a);
    let name = read_cstr(unicorn, info_addr + 0x38, 0x20);
    DecodedQueueHandle {
        info: info_addr,
        queue_type,
        name,
    }
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

fn read_cstr(unicorn: &Unicorn<'_, Context>, addr: u32, limit: usize) -> String {
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];

    for offset in 0..limit {
        if unicorn
            .mem_read((addr + offset as u32) as u64, &mut byte)
            .is_err()
        {
            break;
        }
        if byte[0] == 0 {
            break;
        }
        bytes.push(byte[0]);
    }

    String::from_utf8_lossy(&bytes).to_string()
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
    let queue_name = crate::emulator::utils::read_string(
        unicorn,
        unicorn.reg_read(RegisterARM::R0).unwrap() as u32,
    );
    let arg2 = unicorn.reg_read(RegisterARM::R1).unwrap();
    unicorn.mem_write(arg2, &pack_u32(1)).unwrap();
    log::warn!("queue_name: {}, arg2: {:#x}", queue_name, arg2);
    1u32
}
