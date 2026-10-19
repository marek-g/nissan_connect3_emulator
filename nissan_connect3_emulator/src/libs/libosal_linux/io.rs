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
    // r792c: fix-size mempool block hand-out audit.  DAPI heap cells got
    // written by two threads at t=1.88s (double hand-out suspected).  On
    // GetBlock return log the returned block; keep a host-side set of
    // live blocks per pool and flag any block handed out twice without an
    // intervening RelBlock (double-alloc smoking gun).  Free-list ops of
    // the DAPI process only, filtered to the arena around the corrupted
    // cells (0x0100_0000..0x0105_0000) to keep the log usable.
    if unicorn.get_data().elf_path.contains("DAPIAPP") {
        use std::collections::HashMap;
        use std::sync::atomic::{AtomicU32, Ordering};
        static LIVE: std::sync::Mutex<Option<HashMap<u32, Vec<u32>>>> = std::sync::Mutex::new(None);
        {
            // entry probes: remember (pool, tid) of the in-flight request
            for (offset, name) in [
                (0x4850_d4e8u32, "GetBlock"),
                (0x4850_d32cu32, "RelBlock"),
            ] {
                let address = (base_address + (offset - 0x484d_8000)) as u64;
                let e_name = name;
                let res = unicorn.add_code_hook(address, address + 4, move |uc: &mut Unicorn<'_, Context>, _a, _| {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let tid = uc.get_data().inner.thread_id();
                    let mut guard = LIVE.lock().unwrap();
                    let map = guard.get_or_insert_with(HashMap::new);
                    map.entry(0xffff_ffff).or_default().clear();
                    map.insert(0xffff_fffe, vec![r0, r1, tid, e_name.as_bytes()[0] as u32]);
                });
                if res.is_err() {
                    log::warn!("LIBOSAL alloc-trace entry hook {name} failed");
                }
            }
        }
        for (offset, name) in [
            (0x4850_d63cu32, "GetBlock:ret"),
            (0x4850_d408u32, "RelBlock:ret"),
        ] {
            let address = (base_address + (offset - 0x484d_8000)) as u64;
            let e_name = name;
            let res = unicorn.add_code_hook(address, address + 4, move |uc: &mut Unicorn<'_, Context>, _a, _| {
                let block = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                if !(0x0100_0000..0x0105_0000).contains(&block) {
                    return;
                }
                static LOGGED: AtomicU32 = AtomicU32::new(0);
                let tid = uc.get_data().inner.thread_id();
                let mut guard = LIVE.lock().unwrap();
                let map = guard.get_or_insert_with(HashMap::new);
                let req = map.remove(&0xffff_fffe).unwrap_or_default();
                let pool = req.first().copied().unwrap_or(0);
                let live = map.entry(pool).or_default();
                let dup = if e_name.starts_with("Get") {
                    let seen = live.contains(&block);
                    live.push(block);
                    seen
                } else {
                    if let Some(pos) = live.iter().position(|b| *b == block) {
                        live.swap_remove(pos);
                    }
                    false
                };
                if dup || LOGGED.fetch_add(1, Ordering::Relaxed) < 80 {
                    log::warn!(
                        "LIBOSAL alloc-audit {} [{}] {} pool={pool:#x} block={block:#x}{}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        tid,
                        e_name,
                        if dup { "  <<<< DOUBLE HAND-OUT" } else { "" },
                    );
                }
            });
            if res.is_err() {
                log::warn!("LIBOSAL alloc-audit return hook {name} failed");
            }
        }
    }

    // r792: SDX signature-verification chain entries inside libosal (per
    // hack_cid.md analysis).  The emulator must let this chain run for real:
    // the partition signature hashes the card CID (ioctl 0x410), so serving
    // the real CID + real card files makes the check pass organically.  Log
    // every call with caller lr to find which link bails or never starts.
    for (offset, name) in [
        (0x4853_13a0u32, "fd_crypt_verify_signaturefile"),
        (0x4853_07d8u32, "fd_crypt_sdx_xml_verification"),
        (0x4853_204cu32, "fd_crypt_verify_xml_signature"),
        (0x4853_9c50u32, "BPCL_EC_DSA_Verify"),
        (0x4853_9ec0u32, "BPCL_EC_DSA_Verify:ret-setup"),
    ] {
        let address = base_address + (offset - 0x484d_8000);
        let name = name;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/io.rs:sdx-chain", move |uc: &mut Unicorn<'_, Context>| {
            use std::sync::atomic::{AtomicU32, Ordering};
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 60 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::warn!(
                "LIBOSAL {} [{}] sdx-chain {} r0={r0:#x} r1={r1:#x} lr={lr:#x} elf={}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                name,
                uc.get_data().elf_path,
            );
            0
        });
    }

    let lookup = base_address + (0x484f_12b4u32 - 0x484d_8000);
    let table = base_address + (0x4856_7b58u32 - 0x484d_8000);
    
crate::os::code_stub::trap_site(unicorn, lookup, crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/io.rs:23", move |uc: &mut Unicorn<'_, Context>| {
            use std::sync::atomic::{AtomicU32, Ordering};
            static DUMPS: AtomicU32 = AtomicU32::new(0);
            let path = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let len = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let query = crate::emulator::utils::read_string(uc, path);
            if !query.starts_with("/dev/crypt") {
                return 0;
            }
            if DUMPS.fetch_add(1, Ordering::Relaxed) >= 30 {
                return 0;
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
        
            0
        });

    hook_osal_ioopen_internals(unicorn, base_address);

    // prm_vInit builds the PRM device table (incl. cryptcard/cryptnav status
    // bytes) and spawns PRM_RECOGNITION. Log which processes run it.
    let prm_init = base_address + (0x484f_ff00u32 - 0x484d_8000);
    
crate::os::code_stub::trap_site(unicorn, prm_init, crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/io.rs:66", move |uc: &mut Unicorn<'_, Context>| {
            log::warn!(
                "LIBOSAL {} [{}] prm_vInit enter",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id()
            );
        
            0
        });
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
    
crate::os::code_stub::trap_site(unicorn, post_lookup, crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/io.rs:115", move |uc: &mut Unicorn<'_, Context>| {
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
                return 0;
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
        
            0
        });

    // r0 = open handler return; r6 = dispatcher entry still live (callee-saved).
    
crate::os::code_stub::trap_site(unicorn, post_open, crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/io.rs:168", move |uc: &mut Unicorn<'_, Context>| {
            let entry = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
            let name = dispatcher_name(uc, entry);
            if !name.starts_with("/dev/crypt") {
                return 0;
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
        
            0
        });

    // r4 = final OSAL error code on the trace/error path.
    
crate::os::code_stub::trap_site(unicorn, error_path, crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/io.rs:238", move |uc: &mut Unicorn<'_, Context>| {
            let entry = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
            let name = dispatcher_name(uc, entry);
            if !name.starts_with("/dev/crypt") {
                return 0;
            }
            let err = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            log::warn!(
                "LIBOSAL {} [{}] IOOpen error-path name={} err={:#x}",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                name,
                err
            );
        
            0
        });
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
