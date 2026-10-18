use crate::emulator::context::Context;
use crate::os::code_stub::{trap_site, TrapMode};
use std::collections::HashMap;
use unicorn_engine::{RegisterARM, Unicorn};

/// Observer for libosal's POSIX-semaphore PLT stubs.  Every OSAL inter-process
/// lock is a `sem_t` living in the shared sync-object pool (guest VAs in the
/// 0x4000_0000 arena); the 20 s shared-futex watchdog keeps repairing waits on
/// pool words nobody ever posts.  Logging each sem op with its word address
/// names the thread that grabbed the token (the stuck holder) and shows
/// whether a matching post was ever attempted.  Only shm-arena sems are
/// logged, rate-limited per (op, word) pair so a hot healthy lock cannot
/// flood the log.
///
/// r776 adds the OSAL-level API hooks (`hook_osal_trace`) because the bare
/// `sem_*` words do not name themselves: the sync-object slot table lives at
/// pool+0x10a22c (1003 x 0x40 B, `osallock.c`), handles are `AMES` records in
/// a table at pool+0x19608 with the semaphore *name* at +0x11 and the slot id
/// at +0x8, events are `NTVE` records at pool+0x20c (name ~+0x11, id +0x10),
/// and `CreateOsalLock` fixed locks keep their name at word+0x18.  Logging the
/// name/id/caller-LR at each OSAL call site is what finally says *which*
/// bring-up handshake a stuck waiter belongs to and which peer must post it.
pub fn hook_sem_trace(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    const LIBOSAL_BASE: u32 = 0x484d_8000;
    let ops: [(u32, &str); 4] = [
        (0x484e_b300, "sem_post"),
        (0x484e_b468, "sem_trywait"),
        (0x484e_bab0, "sem_wait"),
        (0x484e_8ea0, "sem_init"),
    ];
    for (plt_va, name) in ops {
        let site = base_address + (plt_va - LIBOSAL_BASE);
        let opname: &'static str = name;
        trap_site(
            unicorn,
            site,
            TrapMode::Arm,
            "libs/libosal_linux/sem_track.rs",
            move |uc: &mut Unicorn<'_, Context>| {
                let sem = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                // only the inter-process arena is interesting
                if !(0x4000_0000..0x4200_0000).contains(&sem) {
                    return 0;
                }
                static COUNTS: std::sync::Mutex<Option<HashMap<(u32, u32), u32>>> =
                    std::sync::Mutex::new(None);
                let key = (site, sem);
                let n = {
                    let mut guard = COUNTS.lock().unwrap();
                    let map = guard.get_or_insert_with(HashMap::new);
                    let c = map.entry(key).or_insert(0);
                    *c += 1;
                    *c
                };
                if n > 8 && n % 4096 != 0 {
                    return 0;
                }
                let mut b = [0u8; 4];
                let value = uc
                    .mem_read(sem as u64, &mut b)
                    .map(|_| u32::from_le_bytes(b))
                    .unwrap_or(0xffff_ffff);
                log::info!(
                    "semtrack: {} sem={:#x} value={} n={} t={}ms elf={} thread [{}] lr={:#x}",
                    opname,
                    sem,
                    value,
                    n,
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().elf_path,
                    uc.get_data().thread_id(),
                    uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32,
                );
                0
            },
        );
    }
}

/// r776: OSAL-level tracing of the inter-process lock APIs.  See the module
/// comment for the on-pool structures.  Every entry is rate-limited the same
/// way as the `sem_*` PLT tracers above (first 8 hits, then every 4096th).
pub fn hook_osal_trace(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    const LIBOSAL_BASE: u32 = 0x484d_8000;
    // (libosal VA, op label, arg kind)
    //   'n' = r0 is a name string pointer (create/open)
    //   'h' = r0 is a semaphore handle  (magic 'AMES', id at +8, name at +0x11)
    //   'e' = r0 is an event handle     (magic 'NTVE', id at +0x10, name at +0x11)
    //   'i' = r0 is a raw sync-object slot id (WaiSyncObj / RelSyncObj)
    //   'l' = r0 is a fixed lock word address (LockOsal/UnLockOsal, name at +0x18)
    let sites: [(u32, &'static str, char); 11] = [
        (0x4851_9160, "sem_open", 'n'),
        (0x4851_948c, "sem_create", 'n'),
        (0x4851_8bcc, "sem_wait", 'h'),
        (0x4851_8d68, "sem_post", 'h'),
        (0x4850_71c8, "evt_create", 'n'),
        (0x4850_67c4, "evt_wait", 'e'),
        (0x4850_65ac, "evt_post", 'e'),
        (0x484e_c50c, "sync_wait", 'i'),
        (0x484e_bfdc, "sync_post", 'i'),
        (0x484e_a1a8, "lock_osal", 'l'),
        (0x484e_9d70, "unlock_osal", 'l'),
    ];
    for (va, name, kind) in sites {
        let site = base_address + (va - LIBOSAL_BASE);
        let opname: &'static str = name;
        trap_site(
            unicorn,
            site,
            TrapMode::Arm,
            "libs/libosal_linux/sem_track.rs(osal)",
            move |uc: &mut Unicorn<'_, Context>| {
                let arg = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                // fixed locks: only the shared arena is interesting
                if kind == 'l' && !(0x4000_0000..0x4200_0000).contains(&arg) {
                    return 0;
                }
                let n = bump(&site, &arg);
                if n > 8 && n % 4096 != 0 {
                    return 0;
                }
                let extra = match kind {
                    'n' => format!("name={:?}", read_cstr(uc, arg)),
                    'h' => {
                        let magic = read_u32(uc, arg);
                        let id = read_u32(uc, arg + 8);
                        format!(
                            "handle={arg:#x} magic={magic:#x} id={id} name={:?}",
                            read_cstr(uc, arg + 0x11)
                        )
                    }
                    'e' => {
                        let magic = read_u32(uc, arg);
                        let id = read_u32(uc, arg + 0x10);
                        format!(
                            "event={arg:#x} magic={magic:#x} id={id} name={:?}",
                            read_cstr(uc, arg + 0x18)
                        )
                    }
                    'i' => format!("id={arg}"),
                    _ => format!(
                        "word={arg:#x} value={} name={:?}",
                        read_u32(uc, arg),
                        read_cstr(uc, arg + 0x18)
                    ),
                };
                log::info!(
                    "semtrack: {} {} n={} t={}ms elf={} thread [{}] lr={:#x}",
                    opname,
                    extra,
                    n,
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().elf_path,
                    uc.get_data().thread_id(),
                    lr,
                );
                0
            },
        );
    }
}

fn read_u32(uc: &mut Unicorn<'_, Context>, va: u32) -> u32 {
    let mut b = [0u8; 4];
    uc.mem_read(va as u64, &mut b)
        .map(|_| u32::from_le_bytes(b))
        .unwrap_or(0xffff_ffff)
}

/// NUL-terminated printable string (OSAL names are <= 0x20 bytes); stops at
/// the first non-printable byte so a junk pointer yields a short/empty debug
/// string instead of poisoning the log.
fn read_cstr(uc: &mut Unicorn<'_, Context>, va: u32) -> String {
    let mut s = String::new();
    for i in 0..31u32 {
        let mut b = [0u8; 1];
        if uc.mem_read((va + i) as u64, &mut b).is_err() {
            break;
        }
        match b[0] {
            0 => break,
            c if c.is_ascii_graphic() => s.push(c as char),
            _ => break,
        }
    }
    s
}

fn bump(site: &u32, key: &u32) -> u32 {
    static COUNTS: std::sync::Mutex<Option<HashMap<(u32, u32), u32>>> =
        std::sync::Mutex::new(None);
    let mut guard = COUNTS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    let c = map.entry((*site, *key)).or_insert(0);
    *c += 1;
    *c
}
