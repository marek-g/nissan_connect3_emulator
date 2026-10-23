use crate::emulator::context::Context;
use crate::emulator::thread::{BlockReason, ThreadAction, ThreadStatus};
use crate::emulator::utils::unpack_u32;
use std::time::{Duration, Instant};
use unicorn_engine::{RegisterARM, Unicorn};

pub fn set_robust_list(unicorn: &mut Unicorn<'_, Context>, head: u32, len: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_robust_list(head = {:#x}, len: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        head,
        len,
    );

    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_robust_list => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        res
    );

    res
}

pub fn futex(
    unicorn: &mut Unicorn<'_, Context>,
    uaddr: u32,
    futex_op: u32,
    val: u32,
    timeout: u32,
    _uaddr2: u32,
    _val3: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] futex(uaddr = {:#x}, futex_op: {:#x}, val: {:#x}, timeout: {:#x}, uaddr2: {:#x}, val3: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        uaddr,
        futex_op,
        val,
        timeout,
        _uaddr2,
        _val3,
    );

    if futex_op & 0x80 == 0 {
        // no FUTEX_PRIVATE_FLAG: cross-process synchronization may be needed. If
        // the word lives in shared memory it is dispatched to the global futex
        // registry (see shm_futex_key below); otherwise it is handled per-process.
        log::trace!("futex without FUTEX_PRIVATE_FLAG");
    }

    let res = match futex_op & 0x7F {
        0x00 | 0x09 => {
            // FUTEX_WAIT / FUTEX_WAIT_BITSET
            let mut buf = [0u8; 4];
            unicorn.mem_read(uaddr as u64, &mut buf).unwrap();
            let val_read = unpack_u32(&buf);

            if val_read != val {
                -11i32 as u32 // EAGAIN
            } else {
                // wait - block the current guest thread until woken by FUTEX_WAKE
                // or by the timeout (whichever comes first)
                log::trace!(
                    "{:#x}: [{}] [SYSCALL] futex - wait (caller={:#x})",
                    unicorn.reg_read(RegisterARM::PC).unwrap(),
                    unicorn.get_data().thread_id(),
                    unicorn.reg_read(RegisterARM::R14).unwrap(),
                );

                log_futex_backtrace(unicorn, uaddr);
                let deadline = read_timeout_deadline(unicorn, timeout);
                let data = unicorn.get_data();
                let thread_id = data.thread_id();

                if let Some(key) = shm_futex_key(unicorn, uaddr) {
                    log::info!(
                        "futex: SHM-WAIT id-word {:#x} key {:#?} t={} thread [{}] (deadline {:?})",
                        uaddr,
                        key,
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        unicorn.get_data().thread_id(),
                        deadline.map(|d| d.duration_since(Instant::now()).as_millis() as u64),
                    );
                    // r799n44/r799n49d: remember which id-words hosts
                    // waiters - but ONLY when the parker is a svcdone
                    // worker (TIDKEY).  The unscoped recording let the
                    // answer-kick storm the OSAL sync-pool parks (SpmEvEnd,
                    // license, AE dispatch - the r776/r777 zero-word set
                    // that keeps the state-6/8 lanes alive): n322/n323
                    // boots died EARLIER the more the kick fired.  The
                    // svcdone id-words are the only legitimate kick target.
                    if TIDKEY
                        .lock()
                        .map(|m| m.contains_key(&thread_id))
                        .unwrap_or(false)
                    {
                        if let Ok(mut w) = PARK_WORDS.lock() {
                            if !w.contains(&uaddr) && w.len() < 48 {
                                w.push(uaddr);
                            }
                        }
                    }
                    log_futex_backtrace(unicorn, uaddr);
                    // word is in shared memory: register globally so another
                    // process' FUTEX_WAKE can signal it; this process' host
                    // thread reaps it via advance_blocked.
                    //
                    // r654: an open-ended shared wait gets a 20 s virtual
                    // watchdog deadline.  The engine's AIL administration path
                    // (`ail_bExamineInfluenceToAdministration` -> list mutex)
                    // deadlocked forever in ~1/3 of runs (r652b/r653/f: thread
                    // [66] parked on `futex-shared` with the holder gone); the
                    // scheduler's deadline path now zeroes the word and wakes
                    // the waiter (dead-owner repair), so the engine survives a
                    // lost unlock instead of stalling every mbx_1024 consumer.
                    let watchdog = deadline.is_none();
                    let deadline = deadline
                        .or_else(|| Some(crate::emulator::vtime::deadline(Duration::from_secs(20))));
                    let namespace = data.namespace.clone();
                    let owner_pid = unicorn.get_data().inner.process_id;
                    let preferred = wait_preferred(unicorn, thread_id);
                    let id = namespace
                        .lock()
                        .unwrap()
                        .futex_wait_shared(key, uaddr, owner_pid, preferred);
                    unicorn.get_data().set_action(ThreadAction::Block(
                        BlockReason::FutexWaitShared {
                            id,
                            deadline,
                            watchdog,
                        },
                    ));
                    return 0;
                }

                {
                    let state = &mut data.inner.sys_calls_state.lock().unwrap();
                    state
                        .futex_waiters
                        .entry(uaddr)
                        .or_insert(Vec::new())
                        .push(thread_id);
                }

                unicorn
                    .get_data()
                    .set_action(ThreadAction::Block(BlockReason::FutexWait {
                        addr: uaddr,
                        deadline,
                    }));

                log::trace!(
                    "{:#x}: [{}] [SYSCALL] futex - woken up",
                    unicorn.reg_read(RegisterARM::PC).unwrap(),
                    unicorn.get_data().thread_id()
                );

                0u32
            }
        }
        0x01 | 0x0A | 0x05 => {
            // FUTEX_WAKE / FUTEX_WAKE_BITSET / FUTEX_WAKE_OP (op semantics not
            // evaluated - a plain wake of `val` waiters covers the common cases)
            if let Some(key) = shm_futex_key(unicorn, uaddr) {
                let namespace = unicorn.get_data().namespace.clone();
                let count = namespace
                    .lock()
                    .unwrap()
                    .futex_wake_shared(&key, val as usize);
                log::info!(
                    "futex: SHM-WAKE word {:#x} key {:#?} woke {} t={} thread [{}]",
                    uaddr,
                    key,
                    count,
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    unicorn.get_data().thread_id(),
                );
                // ring every doorbell so each parked owner reaps its signaled waiter
                namespace.lock().unwrap().notify_waiters();
                count as u32
            } else {
                if (0x4000_0000..0x4100_0000).contains(&uaddr) {
                    // a wake inside the shared-memory arena that found NO shm
                    // key: it only reaches this process' own waiters - if the
                    // lock owner lives elsewhere the cross-process wake is lost
                    // (this is the shape of the 20 s watchdog repairs)
                    log::warn!(
                        "futex: WAKE word {:#x} has NO shm key - in-process only t={} thread [{}] elf={}",
                        uaddr,
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        unicorn.get_data().thread_id(),
                        unicorn.get_data().elf_path,
                    );
                }
                wake_waiters(unicorn, uaddr, val)
            }
        }
        op => {
            log::error!("unsupported futex operation: {}", op);
            -38i32 as u32 // ENOSYS
        }
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] futex => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        res
    );

    res
}

/// r799n44: words on which some guest thread has registered a shared futex
/// wait (SHM-WAIT).  The state-9 answer kick iterates these.
static PARK_WORDS: std::sync::LazyLock<std::sync::Mutex<Vec<u32>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(Vec::new()));

/// Snapshot of the id-words with known parked waiters.
pub fn shm_parked_wait_words() -> Vec<u32> {
    PARK_WORDS.lock().map(|w| w.clone()).unwrap_or_default()
}

/// r799n48: wake-priority hint.  GOOD spawned ~4 svcdone legs per key, BAD
/// 28-220 (re-fire rounds until removal), so the completer's wake(val=1) -
/// which pops ONE waiter FIFO - hits the answered key's owner with GOOD's
/// ~25% probability but BAD's ~1-4%: the wake lottery IS the era lottery.
/// The host knows both sides of the association: the spawn site knows
/// worker-tid -> transaction key (note_svcdone_tid_key), and the answer
/// lanes know which keys already have an answer frame posted
/// (note_key_answered).  A worker parking for an ANSWERED key registers at
/// the FRONT of the wake queue, so the very next completer wake pops it -
/// legal (futex makes no FIFO promise) and it restores GOOD's hit rate.
/// EMU_WAKE_PRIORITY=0 disables.
static TIDKEY: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<u32, u16>>> =
    std::sync::LazyLock::new(Default::default);
static ANSWERED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<u16>>> =
    std::sync::LazyLock::new(Default::default);

fn wake_priority_enabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !*OFF.get_or_init(|| {
        std::env::var_os("EMU_WAKE_PRIORITY").is_some_and(|v| v == "0")
    })
}

/// monitor.rs: a freshly spawned svcdone worker handles this transaction key.
pub fn note_svcdone_tid_key(tid: u32, key: u16) {
    if !wake_priority_enabled() {
        return;
    }
    if let Ok(mut m) = TIDKEY.lock() {
        if m.len() < 8192 {
            m.insert(tid, key);
        }
    }
}

/// message/prochmi lanes: an answer frame for this key is now posted
/// (matches the completer's key-only matching).
pub fn note_key_answered(key: u16) {
    if !wake_priority_enabled() {
        return;
    }
    if let Ok(mut s) = ANSWERED.lock() {
        s.insert(key);
    }
}

/// Drop keys whose request-table entry was REMOVEd (remove probe), so old
/// answers cannot bias the queue forever.
pub fn note_key_resolved(key: u16) {
    if let Ok(mut s) = ANSWERED.lock() {
        s.remove(&key);
    }
}

/// True when this thread's svcdone key already has a posted answer AND the
/// wait is the LEG's id-word park (r799n48b): a worker's OTHER parks
/// (sync-pool semaphores, AE dispatch slots, PS mutexes) must NOT steal
/// front-of-queue from their own domains - n319's boots stalled in state 8
/// once priority went unscoped.  The leg park is identified by a return
/// frame inside the prochmi LEG thunk page (0x1c75104-0x1c751cc, the
/// vtable +0xac8/+0xad4 thunks that wrap bHandleResponseSVCData).
fn wait_preferred(unicorn: &Unicorn<'_, Context>, tid: u32) -> bool {
    if !wake_priority_enabled() {
        return false;
    }
    let answered = TIDKEY
        .lock()
        .ok()
        .and_then(|m| m.get(&tid).copied())
        .is_some_and(|k| ANSWERED.lock().map(|s| s.contains(&k)).unwrap_or(false));
    if !answered {
        return false;
    }
    let sp = unicorn.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
    let mmu = unicorn.get_data().mmu.clone();
    for i in 0..48u32 {
        let mut buf = [0u8; 4];
        if unicorn
            .mem_read(sp.wrapping_add(i * 4) as u64, &mut buf)
            .is_err()
        {
            break;
        }
        let c = u32::from_le_bytes(buf);
        if !(0x1c75_000..0x1c76_000).contains(&c) {
            continue;
        }
        let prochmi = {
            let g = mmu.lock().unwrap();
            g.executable_location(c)
                .map(|(l, _)| l.contains("prochmi_out"))
                .unwrap_or(false)
        };
        if prochmi {
            return true;
        }
    }
    false
}

/// r799n44: host-side kick used by the state-9 answer lanes - wakes every
/// waiter parked on the given shared id-word (FUTEX_WAKE INT_MAX), mirroring
/// GOOD's dense wake storm that the guest's wake(val=1) cannot reproduce
/// through the emulator's spawn-herd wait queues.  Returns the waiter count
/// (0 when the word is not shared memory in this VM).
pub fn wake_all_shared_waiters(unicorn: &Unicorn<'_, Context>, uaddr: u32) -> usize {
    wake_all_shared_waiters_owned(unicorn, uaddr, None, usize::MAX)
}

/// r799n49: `Some(owner_pid)` restricts the wake to waiters registered by
/// that process (the svcdone herd kick targets prochmi's own VM so one
/// answer cannot unpark every foreign waiter in the box); `limit` bounds
/// the slice like GOOD's wake storm.
pub fn wake_all_shared_waiters_owned(
    unicorn: &Unicorn<'_, Context>,
    uaddr: u32,
    owner: Option<u32>,
    limit: usize,
) -> usize {
    let key = match shm_futex_key(unicorn, uaddr) {
        Some(k) => k,
        None => return 0,
    };
    let namespace = unicorn.get_data().namespace.clone();
    let count = namespace
        .lock()
        .unwrap()
        .futex_wake_slice(&key, owner, limit);
    if count > 0 {
        log::warn!(
            "futex: ANSWER-KICK word {:#x} key {:#?} woke-all {} t={} thread [{}]",
            uaddr,
            key,
            count,
            crate::libs::dapi::emu_t0_elapsed_ms(),
            unicorn.get_data().thread_id(),
        );
        namespace.lock().unwrap().notify_waiters();
    }
    count
}

/// wake at most `val` blocked waiters of the futex at `uaddr`
pub(crate) fn wake_waiters(unicorn: &mut Unicorn<'_, Context>, uaddr: u32, val: u32) -> u32 {
    let data = unicorn.get_data();

    // pop up to `val` waiters (LIFO)
    let woken_candidates: Vec<u32> = {
        let mut state = data.sys_calls_state.lock().unwrap();
        let list = state.futex_waiters.entry(uaddr).or_insert(Vec::new());
        (0..val.min(list.len() as u32))
            .rev()
            .filter_map(|_| list.pop())
            .collect()
    };

    let mut count = 0;
    {
        let mut threads = data.threads.lock().unwrap();
        for tid in woken_candidates {
            if let Some(thread) = threads.iter_mut().find(|t| t.id == tid) {
                // only wake if the thread is still waiting on this futex
                // (it may have timed out and moved on in the meantime)
                if matches!(
                    &thread.status,
                    ThreadStatus::Blocked(BlockReason::FutexWait { addr, .. }) if *addr == uaddr
                ) {
                    thread.status = ThreadStatus::Runnable;
                    count += 1;
                }
            }
        }
    }

    count as u32
}

/// Canonical identity of a futex word if it lives in named shared memory. A
/// `FUTEX_WAIT`/`FUTEX_WAKE` on such a word must be seen by every process (they
/// alias the same bytes), so it is routed to the global futex registry keyed by
/// `(shm_path, offset)`. Returns `None` for a process-private word.
fn shm_futex_key(unicorn: &Unicorn<'_, Context>, uaddr: u32) -> Option<(String, u32)> {
    let mmu = unicorn.get_data().mmu.clone();
    let guard = mmu.lock().unwrap();
    let key = guard.shared_futex_key(uaddr);
    drop(guard);
    key
}

/// read an optional `struct timespec` timeout from guest memory
fn log_futex_backtrace(unicorn: &Unicorn<'_, Context>, uaddr: u32) {
    let sp = unicorn.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
    let lr = unicorn.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
    let mut seen = std::collections::HashSet::new();
    let mut frames = Vec::new();

    if let Some((library, offset)) = unicorn
        .get_data()
        .mmu
        .lock()
        .unwrap()
        .executable_location(lr)
    {
        frames.push(format!("{}+{:#x}", library, offset));
    }

    let mmu = unicorn.get_data().mmu.clone();
    for index in 0..128u32 {
        let addr = sp.wrapping_add(index * 4);
        let mut buf = [0u8; 4];
        if unicorn.mem_read(addr as u64, &mut buf).is_err() {
            break;
        }

        let candidate = unpack_u32(&buf);
        let location = {
            let guard = mmu.lock().unwrap();
            guard
                .executable_location(candidate)
                .map(|(library, offset)| (library.to_string(), offset))
        };
        if let Some((library, offset)) = location {
            if seen.insert((library.clone(), offset)) {
                frames.push(format!("{}+{:#x}", library, offset));
            }
        }
    }

    log::trace!(
        "futex backtrace uaddr={:#x} sp={:#x}: {}",
        uaddr,
        sp,
        frames.join(" <- ")
    );
    if frames.len() > 3 && (0x4000_0000..0x4100_0000).contains(&uaddr) {
        // duplicate at info for shm-word waits (see futex syscall): names
        // the guest code that parks on a shared object nobody signals
        log::info!(
            "futex backtrace uaddr={:#x}: {}",
            uaddr,
            frames.join(" <- ")
        );
    }
}

fn read_timeout_deadline(unicorn: &Unicorn<'_, Context>, timeout: u32) -> Option<Instant> {
    if timeout == 0 {
        return None;
    }

    let mut buf = [0u8; 4];
    unicorn.mem_read(timeout as u64, &mut buf).unwrap();
    let seconds = unpack_u32(&buf) as u64;
    unicorn.mem_read(timeout as u64 + 4, &mut buf).unwrap();
    let nanoseconds = unpack_u32(&buf) as u64;

    // relative timeout in guest time -> dilated host deadline (vtime)
    Some(crate::emulator::vtime::deadline(Duration::new(
        seconds,
        nanoseconds as u32,
    )))
}
