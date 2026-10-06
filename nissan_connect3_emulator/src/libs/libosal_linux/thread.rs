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

    if std::env::var_os("EMU_OSAL_TRACE_THREADS").is_some_and(|v| !v.is_empty() && v != "0") {
        for (offset, reason, log_r10) in [
            (
                0x4851_58ac - ORIGINAL_BASE,
                "OSAL_ThreadCreate duplicate process/thread name",
                true,
            ),
            (
                0x4851_5b1c - ORIGINAL_BASE,
                "OSAL_ThreadCreate pthread_create failed",
                false,
            ),
            (
                0x4851_5b68 - ORIGINAL_BASE,
                "OSAL_ThreadCreate thread table full",
                false,
            ),
        ] {
            let addr = base_address + offset;
            unicorn
                .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                    let r10 = if log_r10 {
                        uc.reg_read(RegisterARM::R10).unwrap_or(0) as u32
                    } else {
                        0
                    };
                    log::warn!(
                        "0x{:x} [{}] [LIBOSAL] {} (r10={:#x})",
                        addr,
                        uc.get_data().inner.thread_id(),
                        reason,
                        r10
                    );
                })
                .unwrap();
        }
    }
}

fn log_thread_call(unicorn: &mut Unicorn<'_, Context>, addr: u32, base_address: u32, name: &str) {
    let r0 = unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    let thread = unicorn.get_data().inner.thread_id();
    let details = if name == "OSAL_ThreadCreate" {
        let name_ptr = read_u32(unicorn, r0);
        let stack_size = read_u32(unicorn, r0 + 8);
        let entry = read_u32(unicorn, r0 + 0x0c);
        let arg = read_u32(unicorn, r0 + 0x10);
        let name = read_c_string(unicorn, name_ptr).unwrap_or_else(|| "<bad name ptr>".to_string());
        if name.starts_with("AE_") {
            hook_ail_entry(unicorn, entry);
        }
        format!("name={name}, entry=0x{entry:x}, arg=0x{arg:x}, stack=0x{stack_size:x}")
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

fn hook_ail_entry(unicorn: &mut Unicorn<'_, Context>, entry: u32) {
    if entry == 0 {
        return;
    }

    unicorn
        .add_code_hook(entry as u64, entry as u64, |uc, addr, _| {
            let obj = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let vtable = read_u32(uc, obj);
            let app_entry = read_u32(uc, vtable + 0x10);
            let thread = uc.get_data().inner.thread_id();
            log::info!(
                "0x{:x} [{}] [PROCHMI] AIL entry(obj=0x{:x}, vtable=0x{:x}, vStart=0x{:x})",
                addr,
                thread,
                obj,
                vtable,
                app_entry
            );
        })
        .unwrap();
}

fn read_u32(unicorn: &Unicorn<'_, Context>, addr: u32) -> u32 {
    if addr == 0 {
        return 0;
    }
    let mut buf = [0; 4];
    match unicorn.mem_read(addr as u64, &mut buf) {
        Ok(()) => u32::from_le_bytes(buf),
        Err(_) => 0,
    }
}

fn read_c_string(unicorn: &Unicorn<'_, Context>, addr: u32) -> Option<String> {
    if addr == 0 {
        return None;
    }
    let mut bytes = Vec::new();
    for offset in 0..64 {
        let mut byte = [0; 1];
        unicorn.mem_read((addr + offset) as u64, &mut byte).ok()?;
        if byte[0] == 0 {
            break;
        }
        bytes.push(byte[0]);
    }
    String::from_utf8(bytes).ok()
}
