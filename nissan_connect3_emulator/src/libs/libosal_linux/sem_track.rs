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
                    "semtrack: {} sem={:#x} value={} n={} t={}ms elf={} thread [{}]",
                    opname,
                    sem,
                    value,
                    n,
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().elf_path,
                    uc.get_data().thread_id(),
                );
                0
            },
        );
    }
}
