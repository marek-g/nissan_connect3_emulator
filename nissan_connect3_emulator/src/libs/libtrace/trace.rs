use crate::emulator::context::Context;
use crate::os::code_stub::add_code_stub;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn hook_trace_code(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    add_code_stub(
        unicorn,
        "LIBTRACE",
        base_address + 0x00002f58,
        "trace_init",
        trace_init,
    );
    add_code_stub(
        unicorn,
        "LIBTRACE",
        base_address + 0x00004634,
        "trace_tr_chan_access",
        trace_tr_chan_access,
    );
    add_code_stub(
        unicorn,
        "LIBTRACE",
        base_address + 0x000043a0,
        "trace_tr_core_uw_trace_out",
        trace_tr_core_uw_trace_out,
    );
    add_code_stub(
        unicorn,
        "LIBTRACE",
        base_address + 0x00004400,
        "trace_tr_chan_unreg",
        trace_tr_chan_unreg,
    );
    add_code_stub(
        unicorn,
        "LIBTRACE",
        base_address + 0x00007864,
        "trace_sharedmem_create_dual_os",
        trace_sharedmem_create_dual_os,
    );
    add_code_stub(
        unicorn,
        "LIBTRACE",
        base_address + 0x0000513c,
        "trace_stop",
        trace_stop,
    );
    add_code_stub(
        unicorn,
        "LIBTRACE",
        base_address + 0x000076e4,
        "trace_tr_core_is_class_selected",
        trace_tr_core_is_class_selected,
    );
}

pub fn trace_init(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    0u32
}

pub fn trace_tr_chan_access(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    0u32
}

pub fn trace_tr_core_uw_trace_out(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let thread = unicorn.get_data().inner.thread_id();
    let r0 = unicorn.reg_read(RegisterARM::R0).unwrap_or(0);
    let r1 = unicorn.reg_read(RegisterARM::R1).unwrap_or(0);
    let r2 = unicorn.reg_read(RegisterARM::R2).unwrap_or(0);
    let r3 = unicorn.reg_read(RegisterARM::R3).unwrap_or(0);
    let lr = unicorn.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
    let caller = {
        let mmu = unicorn.get_data().mmu.lock().unwrap();
        match mmu.executable_location(lr) {
            Some((library, offset)) => format!("{}+0x{:x}", library, offset),
            None => format!("0x{:x}", lr),
        }
    };
    log::trace!(
        "[{}] [LIBTRACE] TR_core_uwTraceOut(r0=0x{:x}, r1=0x{:x}, r2=0x{:x}, r3=0x{:x}, caller={}) => 0",
        thread,
        r0,
        r1,
        r2,
        r3,
        caller
    );
    if matches!(r1 as u32, 0x1923 | 0xc000 | 0xc001 | 0xc007 | 0xc032) {
        log_trace_stack(unicorn, thread);
        log_trace_payload(unicorn, thread, r3 as u32, r2 as u32, r1 as u32);
    }
    0u32
}

fn log_trace_payload(unicorn: &mut Unicorn<'_, Context>, thread: u32, ptr: u32, len: u32, id: u32) {
    if ptr == 0 {
        return;
    }

    let dump_len = if id == 0xc007 {
        0x80
    } else if len == 0 {
        return;
    } else {
        (len.min(0x100) as usize).max(64)
    };
    let mut data = vec![0u8; dump_len];
    if unicorn.mem_read(ptr as u64, &mut data).is_err() {
        return;
    }

    let ascii: String = data
        .iter()
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '.'
            }
        })
        .collect();

    log::trace!(
        "[{}] [LIBTRACE] TR_core_uwTraceOut id=0x{:x} payload ptr=0x{:x} len=0x{:x} bytes={} ascii={}",
        thread,
        id,
        ptr,
        len,
        data.iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join(" "),
        ascii
    );
}

fn log_trace_stack(unicorn: &mut Unicorn<'_, Context>, thread: u32) {
    let sp = unicorn.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
    if sp == 0 {
        return;
    }

    let mut frames = Vec::new();
    for i in 0..32u32 {
        let mut raw = [0u8; 4];
        if unicorn
            .mem_read((sp + i * 4) as u64, &mut raw)
            .is_err()
        {
            break;
        }

        let value = u32::from_le_bytes(raw) & !1;
        let location = {
            let mmu = unicorn.get_data().mmu.lock().unwrap();
            mmu.executable_location(value)
                .map(|(library, offset)| format!("{}+0x{:x}", library, offset))
        };
        if let Some(location) = location {
            frames.push(format!("[{i}] 0x{value:x} -> {location}"));
        }
    }

    log::trace!(
        "[{}] [LIBTRACE] TR_core_uwTraceOut id=0x1923 stack sp=0x{:x} frames:\n{}",
        thread,
        sp,
        frames.join("\n")
    );
}

pub fn trace_tr_chan_unreg(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    0u32
}

pub fn trace_sharedmem_create_dual_os(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    1u32
}

pub fn trace_stop(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    1u32
}

pub fn trace_tr_core_is_class_selected(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    1u32
}
