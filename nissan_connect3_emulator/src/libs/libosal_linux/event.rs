use crate::emulator::context::Context;
use crate::emulator::thread::{block_current_thread, BlockReason, ThreadStatus};
use crate::emulator::utils::unpack_u32;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use unicorn_engine::{RegisterARM, Unicorn};

const EVENT_MAGIC: u32 = 0x4556_4e54;
const EVENT_ACTIVE: u32 = 0x04;
const EVENT_BITS: u32 = 0x14;
const EVENT_REQUESTED: u32 = 0x08;
const EVENT_MODE: u32 = 0x0c;
const EVENT_NAME: u32 = 0x18;
const EVENT_IN_WAIT: u32 = 0x38;

struct EventWaiter {
    thread_id: u32,
    deadline: Option<Instant>,
}

fn event_waiters() -> &'static Mutex<HashMap<u32, Vec<EventWaiter>>> {
    static WAITERS: OnceLock<Mutex<HashMap<u32, Vec<EventWaiter>>>> = OnceLock::new();
    WAITERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Waiter/event pairs that already received the faked SPM start confirmation.
fn spm_confirmations() -> &'static Mutex<std::collections::HashSet<(u32, String)>> {
    static SEEN: OnceLock<Mutex<std::collections::HashSet<(u32, String)>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

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
        
crate::os::code_stub::trap_site(unicorn, (base_address + offset), crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/event.rs:40", move |uc: &mut Unicorn<'_, Context>| {
                let addr = (base_address + offset) as u64;
handle_event_call(uc, addr as u32, base_address, name);
                0
            });
    }
}

fn handle_event_call(unicorn: &mut Unicorn<'_, Context>, addr: u32, base_address: u32, name: &str) {
    let r0 = r0(unicorn);
    let r1 = r(unicorn, RegisterARM::R1);
    let r2 = r(unicorn, RegisterARM::R2);
    let r3 = r(unicorn, RegisterARM::R3);
    let thread = unicorn.get_data().thread_id();

    match name {
        "OSAL_s32EventWait" => {
            handle_event_wait(unicorn, r0, r1, r2, r3, addr);
        }
        "OSAL_s32EventPost" => {
            handle_event_post(unicorn, r0, r1, r2);
        }
        _ => {
            let details = match name {
                "OSAL_s32EventCreate" | "OSAL_s32EventOpen" => {
                    format!("name={} out={:#x}", read_name(unicorn, r0), r1)
                }
                _ => format!("r0={:#x} r1={:#x}", r0, r1),
            };
            log_call(unicorn, addr, base_address, name, thread, details);
        }
    }
}

fn handle_event_wait(
    unicorn: &mut Unicorn<'_, Context>,
    event: u32,
    requested: u32,
    mode: u32,
    timeout: u32,
    site: u32,
) {
    if event == 0 {
        // r753: silent before; procbaselx's `s32RunEnv` shutdown wait churns
        // through this path (its SpmEvEnd handle never materialises) and every
        // -1 here records an err 0xd02, so the death chain was invisible.
        log::info!(
            "LIBOSAL: OSAL_s32EventWait null event handle requested={:#x} mode={} timeout={} lr={:#x} elf={}",
            requested,
            mode,
            timeout,
            r(unicorn, RegisterARM::R14),
            unicorn.get_data().elf_path,
        );
        return_result(unicorn, -1);
        return;
    }

    let magic = match try_read_u32(unicorn, event) {
        Some(value) => value,
        None => {
            log::warn!("LIBOSAL: OSAL_s32EventWait unreadable event object {:#x}", event);
            return_result(unicorn, -1);
            return;
        }
    };
    if magic != EVENT_MAGIC {
        log::warn!(
            "LIBOSAL: OSAL_s32EventWait invalid event object {:#x} magic={:#x}",
            event,
            magic
        );
        return_result(unicorn, -1);
        return;
    }

    let active = try_read_u32(unicorn, event.wrapping_add(EVENT_ACTIVE)).unwrap_or(0) & 0xff;
    if active == 0 {
        log::warn!(
            "LIBOSAL: OSAL_s32EventWait inactive event object {:#x} magic={:#x}",
            event,
            magic
        );
        return_result(unicorn, -1);
        return;
    }

    let event_name = read_event_name(unicorn, event);
    if event_name == "HMI_FW_LOOP" {
        crate::libs::prochmi::note_hmi_event_object(event);
        if crate::libs::prochmi::should_force_hmi_event(unicorn, requested) {
            let bits = try_read_u32(unicorn, event.wrapping_add(EVENT_BITS)).unwrap_or(0);
            let forced = bits | requested;
            write_u32(unicorn, event.wrapping_add(EVENT_BITS), forced);
            write_u32(unicorn, event.wrapping_add(EVENT_IN_WAIT), 0);
            remove_thread_waiter(event, unicorn.get_data().thread_id());
            log::info!(
                "PROCHMI: forced HMI_FW_LOOP event wait bits 0x{:08x} -> 0x{:08x} requested=0x{:08x}",
                bits,
                forced,
                requested
            );
            return_result(unicorn, 0);
            return;
        }
    }

    // r753: procbaselx is the SPM *slave*.  Its Bosch app framework
    // (`fc_spmslv_tclApp` -> `ahl_tclBaseOneThreadApp::vAppEntry`,
    // procbaselx `0xe5068`) creates `ev_<app-id>` for its own app id
    // (base-lx is app `0x1c`, so `ev_28`), installs the reply callback on its
    // message queue and sends the "start application" request to the SPM
    // master (`spsdm`, not part of the emulated process set).  The
    // master's answer is what makes the callback post the event bits;
    // without a master nobody posts, the framework polls the event with
    // `OSAL_s32EventWait(mask 0xffffffff, mode 1, 5000 ms)` ten times, and
    // the `s32RunEnv` start chain in main gives up with err 0xd02: procbaselx
    // ran `exit()` at t~56 s every single boot (golden era included) and the
    // user-visible HMI compositing broke shortly after that death.
    // Faking the confirmation at the OSAL event layer is the sanctioned
    // RTOS-fidelity knob here (same pattern as the HMI_FW_LOOP force above).
    // Exactly *one* satisfaction per waiter and event (the real confirmation
    // is a single post that the first waiter consumes): an always-satisfied
    // wait turns the slave's poll loop into a 3.5 kHz host spin.  The forced
    // bits are consumed immediately so the generic satisfaction path cannot
    // re-satisfy later polls either.  `SpmEvEnd` is deliberately NOT
    // answered, it marks shutdown.
    if unicorn.get_data().elf_path.contains("procbaselx") && is_spm_app_event(&event_name) {
        let key = (unicorn.get_data().thread_id(), event_name.clone());
        let first = spm_confirmations()
            .lock()
            .map(|mut seen| seen.insert(key))
            .unwrap_or(false);
        if first {
            let bits = try_read_u32(unicorn, event.wrapping_add(EVENT_BITS)).unwrap_or(0);
            write_u32(unicorn, event.wrapping_add(EVENT_BITS), bits & !requested);
            write_u32(unicorn, event.wrapping_add(EVENT_IN_WAIT), 0);
            remove_thread_waiter(event, unicorn.get_data().thread_id());
            log::info!(
                "LIBOSAL: faked SPM start confirmation for event={} bits 0x{:08x} consumed requested=0x{:08x}",
                event_name,
                bits,
                requested
            );
            return_result(unicorn, 0);
            return;
        }
    }

    let bits = match try_read_u32(unicorn, event.wrapping_add(EVENT_BITS)) {
        Some(value) => value,
        None => {
            log::warn!(
                "LIBOSAL: OSAL_s32EventWait unreadable event bits event={} object={:#x}",
                event_name,
                event
            );
            return_result(unicorn, -1);
            return;
        }
    };
    let requested_mask = if requested == 0 { bits } else { requested };
    // r788c: `mode` selects the Bosch OSAL match rule (OSAL_EVENT_AND = 0,
    // OSAL_EVENT_OR = 1).  The old check was AND-only for both modes, so a
    // multi-bit OR wait could only ever be satisfied when *every* requested
    // bit happened to be set at once.  enavi_tclEnaviThread::vEntry waits
    // (mask 0x6e = process/CCA/two timer bits, mode 1, infinite) while every
    // poster posts a single bit - the HMINAVSRV thread slept through its
    // STARTUP process (posted t=3.0 s, vEntry t=3.0 s) and the whole enavi
    // server-state machine never left STARTING.  For OR waits only the
    // matched bits are consumed (the guest loop re-arms and drains the rest
    // bit by bit) and the matched bits are stored through the out-param, the
    // fifth AAPCS argument on the stack, which the skipped library body
    // would have written.
    let matched = if mode == 1 {
        bits & requested_mask
    } else if (bits & requested_mask) == requested_mask {
        bits
    } else {
        0
    };
    if matched != 0 {
        let rest = if mode == 1 {
            bits & !matched
        } else {
            0
        };
        write_u32(unicorn, event.wrapping_add(EVENT_BITS), rest);
        write_u32(unicorn, event.wrapping_add(EVENT_IN_WAIT), 0);
        let sp = r(unicorn, RegisterARM::R13);
        let result_ptr = try_read_u32(unicorn, sp).unwrap_or(0);
        if result_ptr != 0 {
            write_u32(unicorn, result_ptr, matched);
        }
        remove_thread_waiter(event, unicorn.get_data().thread_id());
        log::info!(
            "LIBOSAL: OSAL_s32EventWait event={} satisfied bits=0x{:08x} requested=0x{:08x} mode={} matched=0x{:08x}",
            event_name,
            bits,
            requested_mask,
            mode,
            matched
        );
        return_result(unicorn, 0);
        return;
    }

    let now = Instant::now();
    let existing_deadline = existing_deadline(event, unicorn.get_data().thread_id());
    if timeout != u32::MAX {
        let deadline =
            existing_deadline.unwrap_or_else(|| crate::emulator::vtime::deadline(
                Duration::from_millis(timeout as u64),
            ));
        if now >= deadline || mode == 0 || timeout == 0 {
            remove_thread_waiter(event, unicorn.get_data().thread_id());
            write_u32(unicorn, event.wrapping_add(EVENT_IN_WAIT), 0);
            log::info!(
                "LIBOSAL: OSAL_s32EventWait timeout event={} requested=0x{:08x} bits=0x{:08x} mode={} timeout={}",
                event_name,
                requested_mask,
                bits,
                mode,
                timeout
            );
            return_result(unicorn, -1);
            return;
        }
    } else if mode == 0 {
        remove_thread_waiter(event, unicorn.get_data().thread_id());
        write_u32(unicorn, event.wrapping_add(EVENT_IN_WAIT), 0);
        return_result(unicorn, -1);
        return;
    }

    let block_until = if timeout == u32::MAX {
        now + Duration::from_secs(60)
    } else {
        existing_deadline.unwrap_or_else(|| {
            crate::emulator::vtime::deadline(Duration::from_millis(timeout as u64))
        })
    };

    write_u32(unicorn, event.wrapping_add(EVENT_REQUESTED), requested_mask);
    write_u32(unicorn, event.wrapping_add(EVENT_MODE), mode);
    write_u32(unicorn, event.wrapping_add(EVENT_IN_WAIT), 1);
    register_waiter(
        event,
        EventWaiter {
            thread_id: unicorn.get_data().thread_id(),
            deadline: if timeout == u32::MAX {
                None
            } else {
                Some(block_until)
            },
        },
    );

    log::debug!(
        "LIBOSAL: OSAL_s32EventWait blocking event={} requested=0x{:08x} bits=0x{:08x} mode={} timeout={}",
        event_name,
        requested_mask,
        bits,
        mode,
        timeout
    );
    // r753: an *infinite* OSAL event wait must stay asleep until a real
    // `OSAL_s32EventPost`.  The block below parks the thread and lets the
    // library body (the trap is an observer) execute on wake, and that body
    // records err 0xd02 and answers -1 the moment it regains the CPU:
    // procbaselx's `s32RunEnv` waits `SpmEvEnd` with timeout=0xffffffff
    // ("run the environment until the SPM says shutdown"), timed out of our
    // 60 s park into that body, and `main()` returned - the golden-era
    // t~56 s death that visibly broke the HMI compositing behind it.
    // Pointing PC back at the trap site makes the wake re-enter this
    // handler: it re-checks the bits (a real post then finishes the wait in
    // the satisfaction branch above) and re-arms the park, so the waiter
    // stays a sleeper forever, like on the real device.  Finite waits keep
    // the old native-timeout behaviour (the chronic, survivable d02s).
    if timeout == u32::MAX {
        let _ = unicorn.reg_write(RegisterARM::PC, site as u64);
    }
    block_current_thread(unicorn, BlockReason::SleepUntil(block_until));
}

fn handle_event_post(unicorn: &mut Unicorn<'_, Context>, event: u32, mask: u32, op: u32) {
    if event == 0 {
        return_result(unicorn, -1);
        return;
    }

    let magic = match try_read_u32(unicorn, event) {
        Some(value) => value,
        None => {
            log::warn!("LIBOSAL: OSAL_s32EventPost unreadable event object {:#x}", event);
            return_result(unicorn, -1);
            return;
        }
    };
    if magic != EVENT_MAGIC {
        log::warn!(
            "LIBOSAL: OSAL_s32EventPost invalid event object {:#x} magic={:#x}",
            event,
            magic
        );
        return_result(unicorn, -1);
        return;
    }

    let event_name = read_event_name(unicorn, event);
    if event_name == "HMI_FW_LOOP" {
        crate::libs::prochmi::note_hmi_event_object(event);
    }

    let old = match try_read_u32(unicorn, event.wrapping_add(EVENT_BITS)) {
        Some(value) => value,
        None => {
            log::warn!(
                "LIBOSAL: OSAL_s32EventPost unreadable event bits event={} object={:#x}",
                event_name,
                event
            );
            return_result(unicorn, -1);
            return;
        }
    };
    let new = match op {
        0 => old & mask,
        1 => old | mask,
        2 => old ^ mask,
        3 => mask,
        _ => {
            log::warn!(
                "LIBOSAL: OSAL_s32EventPost invalid op event={} op={} mask=0x{:08x}",
                event_name,
                op,
                mask
            );
            return_result(unicorn, -1);
            return;
        }
    };

    write_u32(unicorn, event.wrapping_add(EVENT_BITS), new);
    log::debug!(
        "LIBOSAL: OSAL_s32EventPost event={} op={} bits 0x{:08x} -> 0x{:08x} mask=0x{:08x}",
        event_name,
        op,
        old,
        new,
        mask
    );

    let in_wait = try_read_u32(unicorn, event.wrapping_add(EVENT_IN_WAIT)).unwrap_or(0) & 0xff;
    let wait_mode = try_read_u32(unicorn, event.wrapping_add(EVENT_MODE)).unwrap_or(0);
    if in_wait != 0 && wait_mode <= 1 {
        wake_event_waiters(unicorn, event);
    }
    return_result(unicorn, 0);
}

fn wake_event_waiters(unicorn: &mut Unicorn<'_, Context>, event: u32) -> bool {
    let thread_ids: Vec<u32> = event_waiters()
        .lock()
        .ok()
        .and_then(|waiters| {
            waiters
                .get(&event)
                .map(|list| list.iter().map(|waiter| waiter.thread_id).collect())
        })
        .unwrap_or_default();

    if thread_ids.is_empty() {
        return false;
    }

    let data = unicorn.get_data();
    let mut threads = data.threads.lock().unwrap();
    for thread_id in thread_ids {
        if let Some(thread) = threads.iter_mut().find(|thread| thread.id == thread_id) {
            if matches!(thread.status, ThreadStatus::Blocked(_)) {
                thread.status = ThreadStatus::Runnable;
                thread.pending_result = None;
            }
        }
    }
    true
}

/// r799n37: fabricated `OSAL_s32EventPost` driven by the waiter registry:
/// find the event `tid` is currently parked on, OR the mask it requested
/// into its bits, and run the library's own wake sequence for it.  Used by
/// the state-9 tail nudge (prochmi.rs): the CSMA completion that posts the
/// enavi startup event on the real unit does not exist here, and the
/// vSignalSelf clone signals its own action context, so no genuine poster
/// ever reaches that sleeper.  The guest still runs its own satisfaction
/// branch and consumes the bits; only the POST itself is fabricated, and it
/// is logged (WARN) in every run that enables it.
pub fn force_post_event_for(unicorn: &mut Unicorn<'_, Context>, tid: u32) -> Option<String> {
    let event = event_waiters()
        .lock()
        .ok()
        .and_then(|waiters| {
            waiters
                .iter()
                .find(|(_, list)| list.iter().any(|w| w.thread_id == tid))
                .map(|(event, _)| *event)
        })?;
    if try_read_u32(unicorn, event)? != EVENT_MAGIC {
        return None;
    }
    let mask = try_read_u32(unicorn, event.wrapping_add(EVENT_REQUESTED))?;
    let mask = if mask == 0 { 1 } else { mask };
    let old = try_read_u32(unicorn, event.wrapping_add(EVENT_BITS))?;
    write_u32(unicorn, event.wrapping_add(EVENT_BITS), old | mask);
    let name = read_event_name(unicorn, event);
    // mirror handle_event_post's wake gate exactly
    let in_wait = try_read_u32(unicorn, event.wrapping_add(EVENT_IN_WAIT)).unwrap_or(0) & 0xff;
    let wait_mode = try_read_u32(unicorn, event.wrapping_add(EVENT_MODE)).unwrap_or(0);
    let woke = if in_wait != 0 && wait_mode <= 1 {
        wake_event_waiters(unicorn, event)
    } else {
        false
    };
    log::warn!(
        "LIBOSAL: s9-nudge force-post event={} bits 0x{:08x}->0x{:08x} mask=0x{:08x} woke={woke} (fabricated post for the absent CSMA completion; EMU_S9_NUDGE_SECS gates it, EMU_FAKE_SENSORFI=0 runs forbid it)",
        name,
        old,
        old | mask,
        mask
    );
    Some(name)
}

fn register_waiter(event: u32, waiter: EventWaiter) {
    if let Ok(mut waiters) = event_waiters().lock() {
        let list = waiters.entry(event).or_default();
        list.retain(|existing| existing.thread_id != waiter.thread_id);
        list.push(waiter);
    }
}

fn existing_deadline(event: u32, thread_id: u32) -> Option<Instant> {
    event_waiters()
        .lock()
        .ok()
        .and_then(|waiters| {
            waiters
                .get(&event)
                .and_then(|list| list.iter().find(|w| w.thread_id == thread_id))
                .and_then(|w| w.deadline)
        })
}

fn remove_thread_waiter(event: u32, thread_id: u32) {
    if let Ok(mut waiters) = event_waiters().lock() {
        if let Some(list) = waiters.get_mut(&event) {
            list.retain(|w| w.thread_id != thread_id);
            if list.is_empty() {
                waiters.remove(&event);
            }
        }
    }
}

fn return_result(unicorn: &mut Unicorn<'_, Context>, result: i32) {
    let lr = r(unicorn, RegisterARM::R14);
    let _ = unicorn.reg_write(RegisterARM::R0, result as u32 as u64);
    let _ = unicorn.reg_write(RegisterARM::PC, lr as u64);
}

fn log_call(
    _unicorn: &mut Unicorn<'_, Context>,
    addr: u32,
    base_address: u32,
    name: &str,
    thread: u32,
    details: String,
) {
    log::info!(
        "0x{:x} [{}] [LIBOSAL] {}({})",
        addr - base_address + 0x484d8000,
        thread,
        name,
        details
    );
}



fn read_event_name(unicorn: &Unicorn<'_, Context>, event: u32) -> String {
    read_name(unicorn, event.wrapping_add(EVENT_NAME))
}

/// `ev_<decimal>`: the per-app-id event names the Bosch app framework builds with
/// `sprintf("ev_%i", app_id)` (only procbaselx carries that format string).
fn is_spm_app_event(name: &str) -> bool {
    match name.strip_prefix("ev_") {
        Some(digits) => !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

fn read_name(unicorn: &Unicorn<'_, Context>, addr: u32) -> String {
    if addr == 0 {
        return "0x0".to_string();
    }
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    let mut addr = addr;
    loop {
        if unicorn.mem_read(addr as u64, &mut byte).is_err() {
            return "<unreadable>".to_string();
        }
        if byte[0] == 0 {
            break;
        }
        buf.push(byte[0]);
        addr = addr.wrapping_add(1);
        if buf.len() > 64 {
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn try_read_u32(unicorn: &Unicorn<'_, Context>, addr: u32) -> Option<u32> {
    let mut buf = [0u8; 4];
    unicorn
        .mem_read(addr as u64, &mut buf)
        .ok()
        .map(|_| unpack_u32(&buf))
}



fn write_u32(unicorn: &mut Unicorn<'_, Context>, addr: u32, value: u32) {
    let _ = unicorn.mem_write(addr as u64, &value.to_le_bytes());
}

fn r0(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    r(unicorn, RegisterARM::R0)
}

fn r(unicorn: &mut Unicorn<'_, Context>, reg: RegisterARM) -> u32 {
    unicorn.reg_read(reg).unwrap_or(0) as u32
}