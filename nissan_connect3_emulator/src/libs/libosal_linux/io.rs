use crate::emulator::context::Context;
use crate::emulator::utils::read_string;
use unicorn_engine::{RegisterARM, Unicorn};

/// IO-code hooks are DISABLED for now: they stub out the real libosal IO code
/// (open/create/ioctl and the IOSC-queue classification) with fake return
/// values. Let the real libosal code run against the emulated drivers instead.
/// Re-enable these (and the `add_code_hook!` import) for performance work.
pub fn hook_io_code(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    // original base address: 0x484d8000
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x1994C, io_open);
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x19D74, io_create);
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x18DA4, s32_io_control);
    // add_code_hook!(unicorn, "LIBOSAL", base_address + 0x31744, s32_check_for_iosc_queue);

    // Diagnostic: dump the live OSAL dispatcher table (0x48567b58, 0x77 slots
    // of 0x38 bytes, entry+0 = device name pointer) on every table lookup, so
    // we can see which device names each process actually registered and why
    // opening /dev/cryptcard misses.
    let lookup = base_address + (0x484f_12b4u32 - 0x484d_8000);
    let table = base_address + (0x4856_7b58u32 - 0x484d_8000);
    unicorn
        .add_code_hook(lookup as u64, lookup as u64, move |uc, _addr, _| {
            use std::sync::atomic::{AtomicU32, Ordering};
            static DUMPS: AtomicU32 = AtomicU32::new(0);
            let path = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let len = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let query = crate::emulator::utils::read_string(uc, path);
            if !query.starts_with("/dev/crypt") {
                return;
            }
            if DUMPS.fetch_add(1, Ordering::Relaxed) >= 30 {
                return;
            }
            let mut names: Vec<String> = Vec::new();
            for slot in 0..0x77u32 {
                let mut buf = [0u8; 4];
                if uc
                    .mem_read((table + slot * 0x38) as u64, &mut buf)
                    .is_err()
                {
                    continue;
                }
                let name_ptr = u32::from_le_bytes(buf);
                if name_ptr != 0 {
                    names.push(crate::emulator::utils::read_string(uc, name_ptr));
                }
            }
            log::warn!(
                "LIBOSAL {} [{}] dispatcher lookup path={} len={} registered=[{}]",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                query,
                len,
                names.join(", ")
            );
        })
        .unwrap();

    hook_osal_ioopen_internals(unicorn, base_address);

    // prm_vInit builds the PRM device table (incl. cryptcard/cryptnav status
    // bytes) and spawns PRM_RECOGNITION. Log which processes run it.
    let prm_init = base_address + (0x484f_ff00u32 - 0x484d_8000);
    unicorn
        .add_code_hook(prm_init as u64, prm_init as u64, |uc, _addr, _| {
            log::warn!(
                "LIBOSAL {} [{}] prm_vInit enter",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id()
            );
        })
        .unwrap();
}

/// Read the OSAL shared-memory base pointer the way OSAL_IOOpen does:
/// `r7` and `r9` (callee-saved) hold GOT-relative pointers; the global at
/// `r7 + r9` points to a struct whose first word is the shared base.
fn osal_shared_base(uc: &mut Unicorn<'_, Context>) -> u32 {
    let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
    let r9 = uc.reg_read(RegisterARM::R9).unwrap_or(0) as u32;
    let mut buf = [0u8; 4];
    if uc.mem_read((r7 + r9) as u64, &mut buf).is_err() {
        return 0;
    }
    let slot = u32::from_le_bytes(buf);
    if uc.mem_read(slot as u64, &mut buf).is_err() {
        return 0;
    }
    u32::from_le_bytes(buf)
}

/// Instrument the internals of OSAL_IOOpen (0x484f194c) at the three decision
/// points of the dispatcher path: right after `FUN_484f13b4` returns the
/// table entry, right after the driver open handler runs, and on the final
/// error trace so we learn which stage rejects /dev/cryptcard.
fn dispatcher_name(uc: &mut Unicorn<'_, Context>, entry: u32) -> String {
    if entry == 0 {
        return "<none>".into();
    }
    let mut buf = [0u8; 4];
    match uc.mem_read(entry as u64, &mut buf) {
        Ok(()) => read_string(uc, u32::from_le_bytes(buf)),
        Err(_) => "<unreadable>".into(),
    }
}

fn hook_osal_ioopen_internals(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let post_lookup = base_address + (0x484f_1998u32 - 0x484d_8000);
    let post_open = base_address + (0x484f_1ad4u32 - 0x484d_8000);
    let error_path = base_address + (0x484f_1a4cu32 - 0x484d_8000);

    // r6 = dispatcher entry (or 0). Only report crypt* devices to stay quiet.
    unicorn
        .add_code_hook(post_lookup as u64, post_lookup as u64, |uc, _addr, _| {
            let entry = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
            let name = {
                let mut buf = [0u8; 4];
                if entry != 0 && uc.mem_read(entry as u64, &mut buf).is_ok() {
                    read_string(uc, u32::from_le_bytes(buf))
                } else {
                    String::new()
                }
            };
            if !name.starts_with("/dev/crypt") {
                return;
            }
            let open_fn = {
                let mut buf = [0u8; 4];
                if entry != 0 && uc.mem_read((entry + 4) as u64, &mut buf).is_ok() {
                    u32::from_le_bytes(buf)
                } else {
                    0
                }
            };
            let (flag, in_use) = {
                let mut b24 = [0u8; 1];
                let mut b30 = [0u8; 4];
                let _ = uc.mem_read((entry + 0x24) as u64, &mut b24);
                let _ = uc.mem_read((entry + 0x30) as u64, &mut b30);
                (b24[0], i32::from_le_bytes(b30))
            };
            let gbase = osal_shared_base(uc);
            let mut st = [0u8; 3];
            if gbase != 0 {
                let _ = uc.mem_read((gbase + 0x30dc4) as u64, &mut st[..2]);
                let _ = uc.mem_read((gbase + 0x30dd4) as u64, &mut st[2..]);
            }
            log::warn!(
                "LIBOSAL {} [{}] IOOpen entry found name={} entry={:#x} open={:#x} flag24={:#x} inuse30={:#x} gbase={:#x} card={} cryptcard={} cryptnav={}",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                name,
                entry,
                open_fn,
                flag,
                in_use,
                gbase,
                st[0],
                st[1],
                st[2]
            );
        })
        .unwrap();

    // r0 = open handler return; r6 = dispatcher entry still live (callee-saved).
    unicorn
        .add_code_hook(post_open as u64, post_open as u64, move |uc, _addr, _| {
            let entry = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
            let name = dispatcher_name(uc, entry);
            if !name.starts_with("/dev/crypt") {
                return;
            }
            let ret = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let mut buf = [0u8; 4];
            let open_fn = if uc.mem_read((entry + 4) as u64, &mut buf).is_ok() {
                u32::from_le_bytes(buf)
            } else {
                0
            };
            let dev_id = if uc.mem_read((entry + 0x20) as u64, &mut buf).is_ok() {
                u32::from_le_bytes(buf)
            } else {
                0
            };
            // LFS root-path table (libosal global at 0x90ad3100) + gate vars.
            let root_ptr = if uc
                .mem_read((0x90ad_3100u32 + dev_id * 4) as u64, &mut buf)
                .is_ok()
            {
                let root = u32::from_le_bytes(buf);
                if root != 0 {
                    read_string(uc, root)
                } else {
                    String::from("<null>")
                }
            } else {
                String::from("<unreadable>")
            };
            let flag31f0 = if uc.mem_read(0x90ad_31f0u64, &mut buf).is_ok() {
                u32::from_le_bytes(buf)
            } else {
                0xdeadbeef
            };
            let gbase = osal_shared_base(uc);
            let (mut s2f9fa, mut w2f9c0) = ([0u8; 2], [0u8; 4]);
            let (mut b30dc5, mut w30dc8, mut b30dd4) = ([0u8; 1], [0u8; 4], [0u8; 1]);
            if gbase != 0 {
                let _ = uc.mem_read((gbase + 0x2f9fa) as u64, &mut s2f9fa);
                let _ = uc.mem_read((gbase + 0x2f9c0) as u64, &mut w2f9c0);
                let _ = uc.mem_read((gbase + 0x30dc5) as u64, &mut b30dc5);
                let _ = uc.mem_read((gbase + 0x30dc8) as u64, &mut w30dc8);
                let _ = uc.mem_read((gbase + 0x30dd4) as u64, &mut b30dd4);
            }
            log::warn!(
                "LIBOSAL {} [{}] IOOpen open-handler name={} open={:#x} ret={:#x}{} id={:#x} root={} flag31f0={:#x} gbase={:#x} s2f9fa={} w2f9c0={:#x} st_card5={} media={} st_nav={}",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                name,
                open_fn,
                ret,
                if ret == 0x72000 { " (OK)" } else { " (FAIL)" },
                dev_id,
                root_ptr,
                flag31f0,
                gbase,
                u16::from_le_bytes(s2f9fa),
                u32::from_le_bytes(w2f9c0),
                b30dc5[0],
                u32::from_le_bytes(w30dc8),
                b30dd4[0],
            );
        })
        .unwrap();

    // r4 = final OSAL error code on the trace/error path.
    unicorn
        .add_code_hook(error_path as u64, error_path as u64, move |uc, _addr, _| {
            let entry = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
            let name = dispatcher_name(uc, entry);
            if !name.starts_with("/dev/crypt") {
                return;
            }
            let err = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            log::warn!(
                "LIBOSAL {} [{}] IOOpen error-path name={} err={:#x}",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                name,
                err
            );
        })
        .unwrap();
}

#[allow(dead_code)] // kept for re-enabling during performance work
pub fn io_open(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let name = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    let param = unicorn.reg_read(RegisterARM::R1).unwrap();
    log::trace!("name: {}, param: {:#x}", name, param);
    5u32
}

#[allow(dead_code)] // kept for re-enabling during performance work
pub fn io_create(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let name = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    let param = unicorn.reg_read(RegisterARM::R1).unwrap();
    log::trace!("name: {}, param: {:#x}", name, param);
    0u32
}

#[allow(dead_code)] // kept for re-enabling during performance work
pub fn s32_io_control(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let fd = unicorn.reg_read(RegisterARM::R0).unwrap();
    let param = unicorn.reg_read(RegisterARM::R1).unwrap();
    log::trace!("fd: {:#x}, param: {:#x}", fd, param);
    0u32
}

#[allow(dead_code)] // kept for re-enabling during performance work
pub fn s32_check_for_iosc_queue(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let name = read_string(unicorn, unicorn.reg_read(RegisterARM::R0).unwrap() as u32);
    log::trace!("queue_name: {}", name);
    1u32
}
