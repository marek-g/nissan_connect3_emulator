use crate::emulator::context::Context;
use crate::emulator::utils::{read_string, unpack_u32};
use unicorn_engine::{RegisterARM, Unicorn};

pub fn hook_event_code(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    const EVENT_POST: u32 = 0x485065ac - 0x484d8000;
    const EVENT_WAIT: u32 = 0x485067c4 - 0x484d8000;
    const EVENT_OPEN: u32 = 0x48506da8 - 0x484d8000;
    const EVENT_CREATE: u32 = 0x485071c8 - 0x484d8000;

    for (offset, name) in [
        (EVENT_POST, "OSAL_s32EventPost"),
        (EVENT_WAIT, "OSAL_s32EventWait"),
        (EVENT_OPEN, "OSAL_s32EventOpen"),
        (EVENT_CREATE, "OSAL_s32EventCreate"),
    ] {
        unicorn
            .add_code_hook(
                (base_address + offset) as u64,
                (base_address + offset) as u64,
                move |uc, addr, _| log_event_call(uc, addr as u32, base_address, name),
            )
            .unwrap();
    }
}

fn log_event_call(unicorn: &mut Unicorn<'_, Context>, addr: u32, base_address: u32, name: &str) {
    let r0 = r0(unicorn);
    let r1 = r(unicorn, RegisterARM::R1);
    let r2 = r(unicorn, RegisterARM::R2);
    let r3 = r(unicorn, RegisterARM::R3);
    let thread = unicorn.get_data().inner.thread_id();

    if matches!(name, "OSAL_s32EventPost" | "OSAL_s32EventWait") {
        let event_name = read_string(unicorn, r0.wrapping_add(0x18));
        if event_name == "HMI_FW_LOOP" {
            crate::libs::prochmi::note_hmi_event_object(r0);
            if name == "OSAL_s32EventWait" && crate::libs::prochmi::should_force_hmi_event(unicorn, r1) {
                let bits_addr = r0.wrapping_add(0x14);
                let bits = read_u32(unicorn, bits_addr);
                let forced = bits | r1;
                let mut buf = forced.to_le_bytes();
                if unicorn.mem_write(bits_addr as u64, &mut buf).is_ok() {
                    log::info!(
                        "PROCHMI: pre-armed HMI_FW_LOOP event bits 0x{:08x} -> 0x{:08x}",
                        bits,
                        forced
                    );
                }
            }
        }
    }

    let details = match name {
        "OSAL_s32EventCreate" | "OSAL_s32EventOpen" => {
            format!("name={} out={:#x}", read_name(unicorn, r0), r1)
        }
        "OSAL_s32EventPost" => format!(
            "event={} mask={:#x} op={}",
            event_object(unicorn, r0),
            r1,
            r2
        ),
        "OSAL_s32EventWait" => format!(
            "event={} mask={:#x} mode={} timeout={}",
            event_object(unicorn, r0),
            read_u32(unicorn, r0.wrapping_add(0x14)),
            r2,
            r3
        ),
        _ => format!("r0={:#x} r1={:#x}", r0, r1),
    };

    log::info!(
        "0x{:x} [{}] [LIBOSAL] {}({})",
        addr - base_address + 0x484d8000,
        thread,
        name,
        details
    );
}

fn event_object(unicorn: &mut Unicorn<'_, Context>, object: u32) -> String {
    if object == 0 {
        return "0x0".to_string();
    }
    let magic = read_u32(unicorn, object);
    let name = read_string(unicorn, object.wrapping_add(0x18));
    let sem = read_u32(unicorn, object.wrapping_add(0x10));
    format!(
        "{} magic={:#x} sem={} object={:#x}",
        name, magic, sem, object
    )
}

fn read_name(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> String {
    if addr == 0 {
        return "0x0".to_string();
    }
    read_string(unicorn, addr)
}

fn read_u32(unicorn: &Unicorn<'_, Context>, addr: u32) -> u32 {
    let mut buf = [0u8; 4];
    match unicorn.mem_read(addr as u64, &mut buf) {
        Ok(()) => unpack_u32(&buf),
        Err(_) => 0xffff_ffff,
    }
}

fn r0(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    r(unicorn, RegisterARM::R0)
}

fn r(unicorn: &mut Unicorn<'_, Context>, reg: RegisterARM) -> u32 {
    unicorn.reg_read(reg).unwrap_or(0) as u32
}
