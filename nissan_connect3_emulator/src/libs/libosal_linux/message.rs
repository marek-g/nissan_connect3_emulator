use crate::common::osal_queues::{
    callback_message_command, OsalQueueService, OSAL_CB_HDR_LI_MAIN, OSAL_START_PROC_COMMAND,
};
use crate::emulator::context::Context;
use crate::rtos::pwr_proxy;

const ORIGINAL_BASE: u32 = 0x484d_8000;
const OSAL_CORE_GLOBAL: u32 = 0x4856_79e0;
const V_START_PROC: u32 = 0x4851_caec - ORIGINAL_BASE;
const START_PROC_OPTION: u32 = 3;
const START_PROC_BUFFER_SIZE: u32 = 0x100;
const GUEST_CALL_STUB_SIZE: u32 = 4;
use crate::emulator::thread::{BlockReason, ThreadStatus};
use crate::emulator::utils::pack_u32;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};
use unicorn_engine::unicorn_const::Prot;

use crate::common::gva::GuestAddrCell;
use unicorn_engine::{RegisterARM, Unicorn};

static SYNTH_PWR_START_CONF_SENT: AtomicBool = AtomicBool::new(false);
static SYNTH_PWR_START_CONF_CONTENT: GuestAddrCell = GuestAddrCell::new();
static SYNTH_PWR_STATE_REQ_SENT: AtomicBool = AtomicBool::new(false);
static SYNTH_PWR_STATE_REQ_CONTENT: GuestAddrCell = GuestAddrCell::new();
static SYNTH_PWR_CVM_SIGNAL_CHANGED_SENT: AtomicBool = AtomicBool::new(false);
/// Guest OSAL handle (queue-info pointer) under which any thread has opened
/// `MQB_1024` (the map engine's body-thread queue).  Captured here because
/// the procmapengine hooks must restore it into the engine's wait/forward
/// field after the early repair_map_ipc_wait_param clobbers that field with
/// the entry-queue handle (r414: the body thread then leaves its MQB queue
/// after its first message and starves the claiming pass).
pub static MQB_1024_INFO: AtomicU32 = AtomicU32::new(0);
// Sequence number of the last posfi NavPositionStatus seen by the engine's
// pump plus how often it was delivered: the entry thread forwards it back
// onto mbx_1024 (sub-id 7) until the body thread's claiming pass picks it
// up, so a few re-deliveries per sequence are *wanted*; only a message that
// bounces far beyond that (claim failing) is dropped so it cannot starve
// the queue.
static POSFI_LAST_DELIVERED_SEQ: AtomicU32 = AtomicU32::new(0xffff_ffff);
static POSFI_SEQ_DELIVERIES: AtomicU32 = AtomicU32::new(0);

/// Bounded repeat detector for the map engine's mailbox (`mbx_1024`).  Messages whose FI
/// header the engine's CCA pump cannot route (register-id `0xfffe`, see the delivery site)
/// are re-posted by the pump instead of being consumed, which livelocks the queue.  Count
/// deliveries per header (FNV-1a over the message's FI header) and only drop a message once
/// the *same* header has been handed out `MAX_MBX1024_DELIVERIES` times, so distinct answers
/// - which always carry a different service/sequence - still reach the engine.
fn mbx1024_repeat_too_often(header: &[u8]) -> bool {
    const MAX_MBX1024_DELIVERIES: u32 = 4;
    static DELIVERIES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<u64, u32>>> =
        std::sync::OnceLock::new();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in header {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    let table = DELIVERIES.get_or_init(Default::default);
    let mut table = match table.lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    if table.len() > 512 {
        table.clear();
    }
    let count = table.entry(hash).or_insert(0);
    *count += 1;
    *count > MAX_MBX1024_DELIVERIES
}

/// Assign and stamp the CCA registration id of a `(client queue, service)` pair; see the
/// call site in the post path.  Handles the message classes that carry the register-id
/// field at wire offset `+0x16`: register `0x41`, register-conf `0x43`, unregister-conf
/// `0x44` and service data `0x45`.  A conf that already carries an id (a fabricated
/// registration) is *learned* instead of overwritten, so hand-assigned ids keep working.
fn stamp_registration_id(queue: &str, body: &mut [u8]) {
    const REGISTER_ID_OFFSET: usize = 0x16;
    const UNASSIGNED: u16 = 0xffff;
    // Stay clear of the ids handed out by hand elsewhere (the fabricated posfi
    // registration uses 0x17).
    const FIRST_ASSIGNED_ID: u16 = 0x00e0;
    static REGISTRATIONS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<(String, u16), u16>>,
    > = std::sync::OnceLock::new();
    static NEXT_ID: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(FIRST_ASSIGNED_ID);
    static STAMPS_LOGGED: AtomicU32 = AtomicU32::new(0);

    if !queue.starts_with("mbx_") || body.len() <= REGISTER_ID_OFFSET + 1 {
        return;
    }
    let class = body[0x0b];
    if !matches!(class, 0x41 | 0x43 | 0x44 | 0x45) {
        return;
    }
    let service = u16::from_le_bytes([body[0x14], body[0x15]]);
    if service == 0 || service == UNASSIGNED {
        return;
    }
    let current = u16::from_le_bytes([body[REGISTER_ID_OFFSET], body[REGISTER_ID_OFFSET + 1]]);
    let table = REGISTRATIONS.get_or_init(Default::default);
    let mut table = match table.lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    let key = (queue.to_string(), service);
    if class == 0x43 {
        if current != UNASSIGNED {
            table.insert(key, current);
            return;
        }
        let id = *table
            .entry(key)
            .or_insert_with(|| NEXT_ID.fetch_add(1, Ordering::Relaxed));
        body[REGISTER_ID_OFFSET..REGISTER_ID_OFFSET + 2].copy_from_slice(&id.to_le_bytes());
        log::info!(
            "[LIBOSAL-MBX] assigned registration id {id:#06x} to service {service:#06x} on {queue}"
        );
        return;
    }
    let Some(&id) = table.get(&key) else {
        return;
    };
    if current == id {
        return;
    }
    body[REGISTER_ID_OFFSET..REGISTER_ID_OFFSET + 2].copy_from_slice(&id.to_le_bytes());
    if STAMPS_LOGGED.fetch_add(1, Ordering::Relaxed) < 12 {
        log::info!(
            "[LIBOSAL-MBX] stamped registration id {id:#06x} into class {class:#04x} of service {service:#06x} on {queue} (was {current:#06x})"
        );
    }
}
/// Give DAPI's map-data answers the unique id the *receiver* can claim them with.
///
/// The claim walk on both sides is `ail_tclServiceReference::bIsDataSet` (0x66e3d0):
/// it compares the message's unique id (content +0x0e, the word the mbx_7
/// normalization clears to zero on purpose) against the slot the receiver's service
/// reference stores at +0x08, and its 0xffff wildcard only covers the message
/// argument, so the stored slot has to compare equal to what the peer really sends.
/// DAPI sends `AIL_NO_UNIQUE_ID` (0xfffe) there while MAPENGINE's client reference for
/// the DAPI map service (0x26) keeps unique id 0, because its async register path
/// (`bRegisterAsyncExecute` 0x660f74 / `vHandleAsyncRegisterConf` 0x665720) never
/// assigns that slot from the conf.  DAPI's answers therefore could not be claimed and
/// the block ids never reached the request; patching the slot in the *engine's*
/// reference instead made the engine's own `GetBlockIDs` requests fail their validity
/// check (`ail_enCheckClientMailValidity` 0x66dd00 refused them, `enPostMessage`
/// 0x661a3c answered 7, so only the first two requests were ever sent - r551).
/// Normalizing the answer field per receiver, the same way `mbx_7` does, satisfies the
/// claim walk of both peers at once and leaves guest memory alone.
fn stamp_unique_id_for_receiver(queue: &str, body: &mut [u8]) {
    /// Content offset of the unique id the claim walk compares.
    const UNIQUE_ID_OFFSET: usize = 0x0e;
    const NO_UNIQUE_ID: u16 = 0xfffe;
    /// DAPI's application id, the peer whose answers carry `AIL_NO_UNIQUE_ID`.
    const DAPI_APP_ID: u16 = 0x7;
    /// The map-data service whose answers the engine's `GetBlockIDs` waits for.
    const DAPI_MAP_SERVICE: u16 = 0x26;
    const MAPENGINE_APP_ID: u16 = 0x0400;

    if queue == "mbx_7"
        || body.len() <= UNIQUE_ID_OFFSET + 1
        || body[0x0b] != 0x45
        || u16::from_le_bytes([body[UNIQUE_ID_OFFSET], body[UNIQUE_ID_OFFSET + 1]]) != NO_UNIQUE_ID
        || u16::from_le_bytes([body[0x14], body[0x15]]) != DAPI_MAP_SERVICE
    {
        return;
    }
    let peer = u16::from_le_bytes([body[0], body[1]]);
    let other = u16::from_le_bytes([body[0x02], body[0x03]]);
    let src = u16::from_le_bytes([body[0x08], body[0x09]]);
    if ![peer, other, src].contains(&DAPI_APP_ID) || ![peer, other, src].contains(&MAPENGINE_APP_ID)
    {
        return;
    }
    body[UNIQUE_ID_OFFSET..UNIQUE_ID_OFFSET + 2].copy_from_slice(&0u16.to_le_bytes());
    static STAMPS: AtomicU32 = AtomicU32::new(0);
    if STAMPS.fetch_add(1, Ordering::Relaxed) < 12 {
        log::info!(
            "[LIBOSAL-MBX] cleared the unique id of DAPI's service {DAPI_MAP_SERVICE:#06x} answer on {queue} (peer={peer:#06x} other={other:#06x} src={src:#06x}) so MAPENGINE can claim it"
        );
    }
}

/// Temporary: show the id fields of every DAPI map-service ServiceData that passes,
/// so the offsets `stamp_unique_id_for_receiver` keys on can be verified.
// r640: pop-trace for the fabricated DAPI-fastmap ServiceStatus pair
// (service id 0x26, mbx_1024).  Stalled runs (r638/r639) re-posted the pair
// 18 times and `OnUpdateServiceStateDAPIFastmap` never ran - the r611 note
// says a consumer that skips the administration walk swallows it.  Log every
// pop of a 0x26 status message with the popping thread to identify that
// consumer.
fn trace_fastmap_status_pop(unicorn: &Unicorn<'_, Context>, queue: &str, data: &[u8]) {
    if queue != "mbx_1024" {
        return;
    }
    let hit = data.get(0x14..0x16) == Some(&[0x26, 0][..]) || data.get(0x1c..0x1e) == Some(&[0x26, 0][..]);
    if !hit {
        return;
    }
    static C: AtomicU32 = AtomicU32::new(0);
    if C.fetch_add(1, Ordering::Relaxed) >= 60 {
        return;
    }
    let word = |o: usize| -> u32 {
        data.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .unwrap_or(0)
    };
    log::info!(
        "MSGTRACE svc26-status popped t={}ms thread=[{}] lr={:#x} queue={} len={} w0={:#x} w4={:#x} w10={:#x} w14={:#x} w18={:#x} w1c={:#x} b18={:#x}",
        crate::libs::dapi::emu_t0_elapsed_ms(),
        unicorn.get_data().inner.thread_id(),
        unicorn.reg_read(RegisterARM::LR).unwrap_or(0) as u32,
        queue,
        data.len(),
        word(0),
        word(4),
        word(0x10),
        word(0x14),
        word(0x18),
        word(0x1c),
        data.get(0x18).copied().unwrap_or(0xff),
    );
}

fn trace_dapi_map_service_data(queue: &str, body: &[u8]) {    if body.len() <= 0x1a || body[0x0b] != 0x45 {
        return;
    }
    if u16::from_le_bytes([body[0x14], body[0x15]]) != 0x26 {
        return;
    }
    let count = UNIQUE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
    if count >= 60 {
        return;
    }
    let words = [0x00u32, 0x02, 0x08, 0x0a, 0x0c, 0x0e, 0x16, 0x18]
        .iter()
        .map(|&i| {
            format!(
                "{:#06x}",
                u16::from_le_bytes([body[i as usize], body[i as usize + 1]])
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    log::info!(
        "[LIBOSAL-MBX] DAPI map ServiceData on {queue}: opcode={:#04x} words[0,2,8,a,c,e,16,18]=[{words}]",
        body[0x1a]
    );
}

static UNIQUE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

static SYNTH_PWR_CVM_SIGNAL_CHANGED_CONTENT: GuestAddrCell = GuestAddrCell::new();
static SYNTH_MAP_PWR_START_CONF_SENT: AtomicBool = AtomicBool::new(false);
static SYNTH_MAP_PWR_START_CONF_CONTENT: GuestAddrCell = GuestAddrCell::new();
static SYNTH_MAP_PWR_STATE_REQ_SENT: AtomicBool = AtomicBool::new(false);
static SYNTH_MAP_PWR_STATE_REQ_CONTENT: GuestAddrCell = GuestAddrCell::new();
static SYNTH_MAP_PWR_CVM_SIGNAL_CHANGED_SENT: AtomicBool = AtomicBool::new(false);
static SYNTH_MAP_PWR_CVM_SIGNAL_CHANGED_CONTENT: GuestAddrCell = GuestAddrCell::new();
static SYNTH_DAPI_PWR_START_CONF_SENT: AtomicBool = AtomicBool::new(false);
static SYNTH_DAPI_PWR_START_CONF_CONTENT: GuestAddrCell = GuestAddrCell::new();
static SYNTH_DAPI_PWR_STATE_REQ_SENT: AtomicBool = AtomicBool::new(false);
static SYNTH_DAPI_PWR_STATE_REQ_CONTENT: GuestAddrCell = GuestAddrCell::new();
static SYNTH_DAPI_PWR_CVM_SIGNAL_CHANGED_SENT: AtomicBool = AtomicBool::new(false);
static SYNTH_DAPI_PWR_CVM_SIGNAL_CHANGED_CONTENT: GuestAddrCell = GuestAddrCell::new();


thread_local! {
    static SYNTH_PWR_PERIODIC_NEXT: Cell<Option<Instant>> = const { Cell::new(None) };
    static SYNTH_MAP_PWR_PERIODIC_NEXT: Cell<Option<Instant>> = const { Cell::new(None) };
    static SYNTH_DAPI_PWR_PERIODIC_NEXT: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Which thread is currently servicing a mailbox queue, and since when.
///
/// The head unit is single core, so a message handler runs to its next blocking
/// point before the next waiter of the same queue is scheduled - the queue is
/// effectively served by one consumer at a time. Our host threads interleave at
/// instruction granularity, so two DAPIAPP handler threads waiting on `mbx_7`
/// can take a ServiceUnregister and the ServiceRegister that follows it and
/// finish them in reverse order, which deletes a registration that was just
/// created. Marking the queue while a message is in service keeps that order.
static MBX_IN_SERVICE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, (u32, Instant)>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Longest time a queue stays blocked for its current servicer. A guest thread
/// that takes a message and then blocks elsewhere must not stall the queue
/// forever.
const MBX_SERVICE_LIMIT: std::time::Duration = std::time::Duration::from_millis(250);

/// Returns true when `queue` is being serviced by another thread. The same
/// thread coming back for the next message means it finished processing.
fn queue_busy_for_other(queue: &str, thread: u32) -> bool {
    let mut guard = MBX_IN_SERVICE.lock().unwrap();
    match guard.get(queue) {
        Some((owner, since)) if *owner != thread && since.elapsed() < MBX_SERVICE_LIMIT => true,
        Some((owner, _)) if *owner == thread => {
            guard.insert(queue.to_string(), (thread, Instant::now()));
            false
        }
        _ => false,
    }
}

fn queue_serviced(queue: &str, thread: u32) {
    MBX_IN_SERVICE
        .lock()
        .unwrap()
        .insert(queue.to_string(), (thread, Instant::now()));
}

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
    const MESSAGE_CREATE: u32 = 0x48513cc0 - 0x484d8000;
    const MESSAGE_DELETE: u32 = 0x48513850 - 0x484d8000;

    
crate::os::code_stub::trap_site(unicorn, (base_address + QUEUE_OPEN), crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/message.rs:334", move |uc: &mut Unicorn<'_, Context>| {
            let addr = (base_address + QUEUE_OPEN) as u64;
                fallback_open_to_create(uc, addr as u32, base_address, QUEUE_CREATE);
            
            0
        });

    
crate::os::code_stub::trap_site(unicorn, (base_address + CHECK_FOR_IOS_QUEUE), crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/message.rs:344", move |uc: &mut Unicorn<'_, Context>| {
            let addr = (base_address + CHECK_FOR_IOS_QUEUE) as u64;
                force_non_iosc_queue(uc, addr as u32, base_address);
            
            0
        });

    for (offset, name) in [
        (QUEUE_WAIT, "OSAL_s32MessageQueueWait"),
        (QUEUE_POST, "OSAL_s32MessageQueuePost"),
        (QUEUE_NOTIFY, "OSAL_s32MessageQueueNotify"),
        (QUEUE_PRIORITY_WAIT, "OSAL_s32MessageQueuePriorityWait"),
        (GET_FROM_MQ, "u32GetFromMessageQueue"),
    ] {
        
crate::os::code_stub::trap_site(unicorn, (base_address + offset), crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/message.rs:361", move |uc: &mut Unicorn<'_, Context>| {
                let addr = (base_address + offset) as u64;
handle_queue_api(uc, addr as u32, base_address, name);
                0
            });
    }

    
crate::os::code_stub::trap_site(unicorn, (base_address + MESSAGE_CREATE), crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/message.rs:370", move |uc: &mut Unicorn<'_, Context>| {
            let addr = (base_address + MESSAGE_CREATE) as u64;
emulate_message_create(uc, addr as u32, base_address);
            0
        });

    
crate::os::code_stub::trap_site(unicorn, (base_address + MESSAGE_DELETE), crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/message.rs:378", move |uc: &mut Unicorn<'_, Context>| {
            let addr = (base_address + MESSAGE_DELETE) as u64;
suppress_synthetic_message_delete(uc, addr as u32, base_address);
            0
        });

    // r791: the scd client library in every Linux process probes its SCD
    // queue handle with `OSAL_s32IOControl(handle, 10/0xb, buf)`
    // (`scd_bTestLookupIoControl` 0xfbed38) and, when the probe seems
    // answered, flips the whole library into SCD-queue mode
    // (flag 0x02ab44b9=1) where every service->app lookup must come back in
    // the ioctl buffer.  Nothing here answers the buffer, so every lookup
    // resolves to nothing and the navif `ProxyOutCCA::enConnect` registrations
    // die with 0x8000000b.  Deny the probe exactly the way the monitor denies
    // it on the registry path (`complete_io_control`): -1 with last-error
    // 0x72011, which `scd_bTestLookupIoControl` classifies as "no SCD server
    // here" and keeps the client in static registry-map mode - the mode the
    // emulated registry answers natively (and the mode PROCNAV stays in).
    const IOCONTROL: u32 = 0x484f_0da4 - 0x484d_8000;
    crate::os::code_stub::trap_site(unicorn, (base_address + IOCONTROL), crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/message.rs:iocontrol-scd", move |uc: &mut Unicorn<'_, Context>| {
        let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
        let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
        {
            static SEEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<u32>>> =
                std::sync::LazyLock::new(|| std::sync::Mutex::new(Default::default()));
            if let Ok(mut s) = SEEN.lock() {
                if s.len() < 40 && s.insert(r1) {
                    let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                    log::info!(
                        "LIBOSAL iocontrol-enter req={r1:#x} handle={r0:#x} lr={lr:#x} tid=[{}]",
                        uc.get_data().inner.thread_id(),
                    );
                }
            }
        }
        if !matches!(r1, 10 | 0xb) {
            return 0;
        }
        // Registry handles must keep flowing to the monitor (which answers
        // real queries); they open with a "/dev/registry/..." path stored at
        // handle+0x20 - the same field the monitor reads.
        if let Some(path) = crate::rtos::monitor::read_c_string(uc, r0.wrapping_add(0x20)) {
            if path.starts_with("/dev/registry") {
                return 0;
            }
        }
        // r791: this is the scd client channel (`scd_bCreateQueue` opened the
        // scd key's value path).  On the real unit the kernel SCD answers it;
        // here no real channel exists, and an unanswered probe leaves the
        // whole client library in queue mode with empty answers, so every
        // service lookup resolves to nothing and the navif registrations die
        // with 0x8000000b.  Serve the two lookup requests from the shared
        // registry: 0xa is the capability probe (answered "server present"),
        // 0xb carries the service id in the request block at buf+0x100
        // (`uStack_224` in `scd_bGetAppIdFromServiceId` 0xfc1378) and the
        // kernel overwrites it with the provider app id on success.
        // r791b: request 10 keeps its native body.  It has queue side
        // effects (the library messages the SCD about the caller), and
        // suppressing it stalls the caller's bring-up (r791 f95/f96: prochmi
        // main init slept at t~2.1s and never reached the enavi chain).  It
        // already "succeeds" natively, which is what sets the library into
        // queue mode; only request 0xb (the service lookup) is unanswered.
        let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
        if r1 == 10 {
            return 0;
        }
        let mut w = [0u8; 4];
        if uc.mem_read(r2.wrapping_add(0x100) as u64, &mut w).is_err() {
            return_to_caller(uc, u32::MAX);
            return 0;
        }
        let svc = u16::from_le_bytes([w[0], w[1]]);
        let namespace = uc.get_data().namespace.clone();
        let resolved: Option<u16> = {
            let state = namespace.lock().unwrap();
            state
                .registry
                .find_key_by_u32_value_prefer(
                    "SERVICEID",
                    u32::from(svc),
                    &crate::common::registry::HOSTED_SERVER_APPS,
                )
                .and_then(|key| {
                    // .../PROCESS/<group>/<app>/SERVICES/<svc> -> the app key
                    let services = key.rsplit_once('/')?.0;
                    let app = services.rsplit_once('/')?.0;
                    state.registry.query_u32(app, "APPID").map(|a| a as u16)
                })
        };
        match resolved {
            Some(app) => {
                let _ = uc.mem_write(r2.wrapping_add(0x100) as u64, &app.to_le_bytes());
                log::info!(
                    "LIBOSAL: scd lookup (0xb): service {svc:#x} -> app {app:#x} ({})",
                    if matches!(app, 0x0001 | 0x0007 | 0x0109) {
                        "emulated process must answer"
                    } else {
                        "external provider"
                    }
                );
                return_to_caller(uc, 0);
            }
            None => {
                log::info!(
                    "LIBOSAL: scd lookup (0xb): no registry entry publishes service {svc:#x}, lookup fails"
                );
                return_to_caller(uc, u32::MAX);
            }
        }
        0
    });

    // r755-diag: OSAL_vAssertFunction(expr, file, line).  The d02 errmem
    // storms report only the record; this names the assert text, its source
    // file/line and the first return addresses still on the stack, so a
    // failing OSAL operation is identifiable without symbol hunting.
    const ASSERT: u32 = 0x4850_4d48 - 0x484d_8000;
    crate::os::code_stub::trap_site(unicorn, (base_address + ASSERT), crate::os::code_stub::TrapMode::Arm, "libs/libosal_linux/message.rs:assert", move |uc: &mut Unicorn<'_, Context>| {
            static SEEN: std::sync::LazyLock<
                std::sync::Mutex<std::collections::HashSet<(u32, u32, u32)>>,
            > = std::sync::LazyLock::new(|| std::sync::Mutex::new(Default::default()));
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let fresh = SEEN
                .lock()
                .map(|mut s| s.insert((r0, r1, r2)) && s.len() <= 24)
                .unwrap_or(false);
            if fresh {
                let expr = crate::emulator::utils::read_string(uc, r0);
                let file = crate::emulator::utils::read_string(uc, r1);
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                let mut chain: Vec<u32> = Vec::new();
                if sp > 0x1000 && sp < 0xf000_0000 {
                    for i in 0..128u32 {
                        let mut w = [0u8; 4];
                        if uc.mem_read((sp + i * 4) as u64, &mut w).is_err() {
                            break;
                        }
                        let w = u32::from_le_bytes(w);
                        if (w >= 0x484d_8000 && w < 0x4855_e414)
                            || (w >= 0x8000 && w < 0x0300_0000 && w & 3 == 0)
                        {
                            if !chain.contains(&w) {
                                chain.push(w);
                                if chain.len() >= 10 {
                                    break;
                                }
                            }
                        }
                    }
                }
                log::warn!(
                    "[LIBOSAL-ASSERT] {} [{}] expr={expr:?} file={file:?} line={r2} lr={lr:#x} chain={chain:?}",
                    uc.get_data().elf_path,
                    uc.get_data().inner.thread_id(),
                );
            }
            0
        });
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
            if decoded.name == "MQB_1024" {
                MQB_1024_INFO.store(r0, Ordering::Relaxed);
            }
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
                log_osal_message_ref(unicorn, base_address, &format!("post {}", decoded.name), r1);
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
            let synthetic_stack_timeout = if is_synthetic_periodic_queue(&decoded.name)
                && matches!(
                    api_name,
                    "OSAL_s32MessageQueueWait" | "OSAL_s32MessageQueuePriorityWait"
                )
                && stack_timeout == u32::MAX
                && shorten_native_wait_timeout(unicorn)
            {
                u32::MAX
            } else {
                stack_timeout
            };



            if pwr_proxy_service_handle(
                unicorn,
                api_name,
                &decoded.name,
                r1,
                r2,
            ) {
                log::info!(
                    "0x{:x} [{}] [LIBOSAL-PWR-PROXY] {} handled name={}",
                    addr - base_address + 0x484d8000,
                    thread,
                    api_name,
                    decoded.name
                );
                return;
            }

            if bridge_mbx_queue(
                unicorn,
                api_name,
                base_address,
                &decoded.name,
                r1,
                r2,
                r3,
                synthetic_stack_timeout,
            ) {
                log::info!(
                    "0x{:x} [{}] [LIBOSAL-MBX] {} handled name={}",
                    addr - base_address + 0x484d8000,
                    thread,
                    api_name,
                    decoded.name
                );
                return;
            }

            if bridge_guest_osal_queue(
                unicorn,
                api_name,
                base_address,
                &decoded.name,
                r1,
                r2,
                r3,
                synthetic_stack_timeout,
            ) {
                log::info!(
                    "0x{:x} [{}] [LIBOSAL-OSAL-SERVICE] {} handled name={}",
                    addr - base_address + 0x484d8000,
                    thread,
                    api_name,
                    decoded.name
                );
            }

            if bridge_dyn_mbx_queue(
                unicorn,
                api_name,
                &decoded.name,
                r1,
                r2,
                r3,
                synthetic_stack_timeout,
            ) {
                log::info!(
                    "0x{:x} [{}] [LIBOSAL-DYN] {} handled name={}",
                    addr - base_address + 0x484d8000,
                    thread,
                    api_name,
                    decoded.name
                );
                return;
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
    pwr_proxy::register_app_queue_open(&name);
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
        return Some(match id {
            // The map engine's dispatch mailbox (app id 1024) carries full DAPI
            // envelopes - forwarded posfi NavPositionStatus messages are ~0xa0
            // bytes - so the host queue must size for those, not for refs.
            1024 => (120, 0x110),
            265 => (800, 8),
            _ => (120, 8),
        });
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

fn log_osal_message_ref(
    unicorn: &Unicorn<'_, Context>,
    base_address: u32,
    source: &str,
    ref_addr: u32,
) {
    const MSG_POOL_CONTENT_BASE: u32 = 0x4856_f800;
    const MSG_POOL_BLOCK_BASE: u32 = 0x4856_f7f4;
    const MSG_POOL_ENTRY_SIZE: u32 = 12;

    let raw_type = read_u32_or_invalid(unicorn, ref_addr);
    let raw_handle = read_u32_or_invalid(unicorn, ref_addr + 4);
    match raw_type & 0xff {
        1 if raw_handle != 0 && raw_handle < 0xf000_0000 => {
            log_osal_message_bytes(unicorn, source, raw_type, raw_handle, raw_handle, "direct");
        }
        2 => {
            let block_base = read_u32_or_invalid(unicorn, base_address + (MSG_POOL_BLOCK_BASE - ORIGINAL_BASE));
            let content_base = read_u32_or_invalid(unicorn, base_address + (MSG_POOL_CONTENT_BASE - ORIGINAL_BASE));
            let valid_base = |addr: u32| addr != 0 && addr != 0xffff_ffff && addr < 0xf000_0000;

            for (label, base) in [("content", content_base), ("block", block_base)] {
                if !valid_base(base) {
                    continue;
                }
                let content = base.wrapping_add((raw_handle.wrapping_add(1) * MSG_POOL_ENTRY_SIZE) + 0x18);
                if content < 0xf000_0000 {
                    log_osal_message_bytes(unicorn, source, raw_type, raw_handle, content, label);
                    return;
                }
            }
            log::info!(
                "[LIBOSAL] {} message ref type={} handle=0x{:x} pool bases content=0x{:x} block=0x{:x}",
                source,
                raw_type,
                raw_handle,
                content_base,
                block_base
            );
        }
        _ => {}
    }
}

fn log_osal_message_bytes(
    unicorn: &Unicorn<'_, Context>,
    source: &str,
    raw_type: u32,
    raw_handle: u32,
    content: u32,
    base_label: &str,
) {
    let Some(data) = read_guest_buffer(unicorn, content, 0x40) else {
        return;
    };
    let printable: String = data
        .iter()
        .map(|b| if (0x20..=0x7e).contains(b) { *b as char } else { '.' })
        .collect();
    log::info!(
        "[LIBOSAL] {} message ref type={} handle=0x{:x} content=0x{:x} base={} bytes={} ascii={}",
        source,
        raw_type,
        raw_handle,
        content,
        base_label,
        data.iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join(" "),
        printable
    );
}

fn is_synthetic_periodic_queue(name: &str) -> bool {
    // Synthetic power-message injection has been removed. Real IPC now
    // carries power state changes. Keep the helper for future debugging;
    // returning false disables the timeout-shortening crutch.
    let _ = name;
    false
}

fn shorten_native_wait_timeout(unicorn: &mut Unicorn<'_, Context>) -> bool {
    const TIMEOUT_MS: u32 = 100;

    let sp = unicorn.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
    if sp == 0 || sp > 0xf000_0000 {
        return false;
    }

    unicorn
        .mem_write(sp as u64, &TIMEOUT_MS.to_le_bytes())
        .is_ok()
}

fn synthesize_ail_power_startup_sequence(
    unicorn: &mut Unicorn<'_, Context>,
    api_name: &str,
    name: &str,
    buf: u32,
    stack_timeout: u32,
    elf_path: &str,
) -> bool {
    if name == "mbx_1024" {
        return elf_path.contains("procmapengine")
            && synthesize_map_power_startup_sequence(
                unicorn,
                api_name,
                name,
                buf,
                stack_timeout,
            );
    }

    if name == "mbx_7" {
        return elf_path.contains("DAPIAPP")
            && synthesize_dapi_power_startup_sequence(
                unicorn,
                api_name,
                name,
                buf,
                stack_timeout,
            );
    }

    if !elf_path.contains("prochmi") {
        return false;
    }

    for message in [
        (
            3u16,
            0u32,
            0u32,
            "PWR_PROXY_START_CONF" as &str,
            &SYNTH_PWR_START_CONF_SENT as &AtomicBool,
            &SYNTH_PWR_START_CONF_CONTENT,
        ),
        (
            0x10,
            3,
            0,
            "PWR_STATE_CHANGE_REQ",
            &SYNTH_PWR_STATE_REQ_SENT,
            &SYNTH_PWR_STATE_REQ_CONTENT,
        ),
        (
            0x50,
            0,
            0,
            "PWR_CVM_SIGNAL_CHANGED",
            &SYNTH_PWR_CVM_SIGNAL_CHANGED_SENT,
            &SYNTH_PWR_CVM_SIGNAL_CHANGED_CONTENT,
        ),
    ] {
        if synthesize_ail_power_message(
            unicorn,
            api_name,
            name,
            buf,
            stack_timeout,
            message.0,
            message.1,
            message.2,
            message.3,
            message.4,
            message.5,
        ) {
            return true;
        }
    }

    if SYNTH_PWR_START_CONF_SENT.load(Ordering::SeqCst)
        && SYNTH_PWR_STATE_REQ_SENT.load(Ordering::SeqCst)
        && SYNTH_PWR_CVM_SIGNAL_CHANGED_SENT.load(Ordering::SeqCst)
    {
        return synthesize_periodic_ail_power_state_req(unicorn, buf);
    }

    false
}



fn synthesize_ail_power_message(
    unicorn: &mut Unicorn<'_, Context>,
    api_name: &str,
    name: &str,
    buf: u32,
    stack_timeout: u32,
    power_type: u16,
    power_data1: u32,
    power_data2: u32,
    message_name: &str,
    sent: &AtomicBool,
    content_slot: &GuestAddrCell,
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

    if power_type != 3 && !SYNTH_PWR_START_CONF_SENT.load(Ordering::SeqCst) {
        return false;
    }
    if sent
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
        .is_err()
    {
        return false;
    }

    let mut content = content_slot.load(unicorn);
    if content == 0 || content > 0xf000_0000 {
        let mmu_arc = unicorn.get_data().mmu.clone();
        content = mmu_arc.lock().unwrap().heap_alloc(
            unicorn,
            CONTENT_LEN,
            Prot::READ | Prot::WRITE,
            "[synthetic-cca-power-conf]",
        );
        if content == 0 {
            return false;
        }

        let mut body = [0u8; CONTENT_LEN as usize];
        body[0x00..0x02].copy_from_slice(&0x0109u16.to_le_bytes());
        body[0x04..0x08].copy_from_slice(&CONTENT_LEN.to_le_bytes());
        body[0x08..0x0a].copy_from_slice(&0x0002u16.to_le_bytes());
        body[0x0b] = 0x40;
        body[0x0c..0x0e].copy_from_slice(&0x0001u16.to_le_bytes());
        body[0x14..0x16].copy_from_slice(&power_type.to_le_bytes());
        body[0x18..0x1c].copy_from_slice(&power_data1.to_le_bytes());
        body[0x1c..0x20].copy_from_slice(&power_data2.to_le_bytes());

        if unicorn.mem_write(content as u64, &body).is_err() {
            return false;
        }

        content_slot.store(unicorn, content);
    }

    if unicorn.mem_write(buf as u64, &1u32.to_le_bytes()).is_err()
        || unicorn
            .mem_write((buf + 4) as u64, &content.to_le_bytes())
            .is_err()
    {
        return false;
    }
    let thread = unicorn.get_data().inner.thread_id();
    log::info!(
        "[{}] [LIBOSAL] {}({}) answered with synthetic {} content=0x{:x}",
        thread,
        api_name,
        name,
        message_name,
        content
    );
    return_to_caller(unicorn, 8);
    true
}

fn synthesize_periodic_ail_power_state_req(
    unicorn: &mut Unicorn<'_, Context>,
    buf: u32,
) -> bool {
    const CONTENT_LEN: u32 = 0x20;
    const TARGET_QUEUE: &str = "mbx_265";

    if buf == 0 || buf > 0xf000_0000 {
        return false;
    }

    let now = Instant::now();
    let ready = SYNTH_PWR_PERIODIC_NEXT.with(|cell| match cell.get() {
        None => {
            cell.set(Some(now + crate::emulator::vtime::host_interval(Duration::from_millis(1000))));
            true
        }
        Some(next) => now >= next,
    });
    if !ready {
        return false;
    }

    let mut content = SYNTH_PWR_STATE_REQ_CONTENT.load(unicorn);
    if content == 0 || content > 0xf000_0000 {
        let mmu_arc = unicorn.get_data().mmu.clone();
        content = mmu_arc.lock().unwrap().heap_alloc(
            unicorn,
            CONTENT_LEN,
            Prot::READ | Prot::WRITE,
            "[synthetic-cca-power-state-req]",
        );
        if content == 0 {
            return false;
        }

        let mut body = [0_u8; CONTENT_LEN as usize];
        body[0x00..0x02].copy_from_slice(&0x0109u16.to_le_bytes());
        body[0x04..0x08].copy_from_slice(&CONTENT_LEN.to_le_bytes());
        body[0x08..0x0a].copy_from_slice(&0x0002u16.to_le_bytes());
        body[0x0b] = 0x40;
        body[0x0c..0x0e].copy_from_slice(&0x0001u16.to_le_bytes());
        body[0x14..0x16].copy_from_slice(&0x10u16.to_le_bytes());
        body[0x18..0x1c].copy_from_slice(&3u32.to_le_bytes());
        body[0x1c..0x20].copy_from_slice(&0u32.to_le_bytes());

        if unicorn.mem_write(content as u64, &body).is_err() {
            return false;
        }
        SYNTH_PWR_STATE_REQ_CONTENT.store(unicorn, content);
    }

    crate::libs::prochmi::force_hmi_gui_state(unicorn);

    if unicorn.mem_write(buf as u64, &1u32.to_le_bytes()).is_err()
        || unicorn
            .mem_write((buf + 4) as u64, &content.to_le_bytes())
            .is_err()
    {
        return false;
    }

    let thread = unicorn.get_data().inner.thread_id();
    log::info!(
        "[{}] [LIBOSAL] OSAL_s32MessageQueueWait({}) answered with periodic synthetic PWR_STATE_CHANGE_REQ content=0x{:x}",
        thread,
        TARGET_QUEUE,
        content
    );
    SYNTH_PWR_PERIODIC_NEXT.with(|cell| cell.set(Some(now + crate::emulator::vtime::host_interval(Duration::from_millis(1000)))));
    return_to_caller(unicorn, 8);
    true
}

fn synthesize_map_power_startup_sequence(
    unicorn: &mut Unicorn<'_, Context>,
    api_name: &str,
    name: &str,
    buf: u32,
    stack_timeout: u32,
) -> bool {
    const MAP_APP_ID: u16 = 0x0400;

    for message in [
        (
            3u16,
            0u32,
            0u32,
            "MAP PWR_PROXY_START_CONF",
            &SYNTH_MAP_PWR_START_CONF_SENT as &AtomicBool,
            &SYNTH_MAP_PWR_START_CONF_CONTENT,
        ),
        (
            0x10,
            3,
            0,
            "MAP PWR_STATE_CHANGE_REQ",
            &SYNTH_MAP_PWR_STATE_REQ_SENT,
            &SYNTH_MAP_PWR_STATE_REQ_CONTENT,
        ),
        (
            0x50,
            0,
            0,
            "MAP PWR_CVM_SIGNAL_CHANGED",
            &SYNTH_MAP_PWR_CVM_SIGNAL_CHANGED_SENT,
            &SYNTH_MAP_PWR_CVM_SIGNAL_CHANGED_CONTENT,
        ),
    ] {
        if synthesize_map_power_message(
            unicorn,
            api_name,
            name,
            buf,
            stack_timeout,
            MAP_APP_ID,
            message.0,
            message.1,
            message.2,
            message.3,
            message.4,
            message.5,
        ) {
            return true;
        }
    }

    if SYNTH_MAP_PWR_START_CONF_SENT.load(Ordering::SeqCst)
        && SYNTH_MAP_PWR_STATE_REQ_SENT.load(Ordering::SeqCst)
        && SYNTH_MAP_PWR_CVM_SIGNAL_CHANGED_SENT.load(Ordering::SeqCst)
    {
        return synthesize_periodic_map_power_state_req(unicorn, buf);
    }

    false
}

fn synthesize_map_power_message(
    unicorn: &mut Unicorn<'_, Context>,
    api_name: &str,
    name: &str,
    buf: u32,
    stack_timeout: u32,
    target_app: u16,
    power_type: u16,
    power_data1: u32,
    power_data2: u32,
    message_name: &str,
    sent: &AtomicBool,
    content_slot: &GuestAddrCell,
) -> bool {
    const TARGET_QUEUE: &str = "mbx_1024";
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

    if power_type != 3 && !SYNTH_MAP_PWR_START_CONF_SENT.load(Ordering::SeqCst) {
        return false;
    }
    if sent
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
        .is_err()
    {
        return false;
    }

    let mut content = content_slot.load(unicorn);
    if content == 0 || content > 0xf000_0000 {
        let mmu_arc = unicorn.get_data().mmu.clone();
        content = mmu_arc.lock().unwrap().heap_alloc(
            unicorn,
            CONTENT_LEN,
            Prot::READ | Prot::WRITE,
            "[synthetic-cca-map-power]",
        );
        if content == 0 {
            return false;
        }

        let mut body = [0u8; CONTENT_LEN as usize];
        body[0x00..0x02].copy_from_slice(&target_app.to_le_bytes());
        body[0x04..0x08].copy_from_slice(&CONTENT_LEN.to_le_bytes());
        body[0x08..0x0a].copy_from_slice(&0x0002u16.to_le_bytes());
        body[0x0b] = 0x40;
        body[0x0c..0x0e].copy_from_slice(&0x0001u16.to_le_bytes());
        body[0x14..0x16].copy_from_slice(&power_type.to_le_bytes());
        body[0x18..0x1c].copy_from_slice(&power_data1.to_le_bytes());
        body[0x1c..0x20].copy_from_slice(&power_data2.to_le_bytes());

        if unicorn.mem_write(content as u64, &body).is_err() {
            return false;
        }

        content_slot.store(unicorn, content);
    }

    if unicorn.mem_write(buf as u64, &1u32.to_le_bytes()).is_err()
        || unicorn
            .mem_write((buf + 4) as u64, &content.to_le_bytes())
            .is_err()
    {
        return false;
    }

    crate::libs::procmapengine::force_map_power_cca_mode(unicorn);

    let thread = unicorn.get_data().inner.thread_id();
    log::info!(
        "[{}] [LIBOSAL] {}({}) answered with synthetic {} content=0x{:x}",
        thread,
        api_name,
        name,
        message_name,
        content
    );
    return_to_caller(unicorn, 8);
    true
}

fn synthesize_periodic_map_power_state_req(unicorn: &mut Unicorn<'_, Context>, buf: u32) -> bool {
    const CONTENT_LEN: u32 = 0x20;
    const TARGET_QUEUE: &str = "mbx_1024";

    if buf == 0 || buf > 0xf000_0000 {
        return false;
    }

    let now = Instant::now();
    let ready = SYNTH_MAP_PWR_PERIODIC_NEXT.with(|cell| match cell.get() {
        None => {
            cell.set(Some(now + crate::emulator::vtime::host_interval(Duration::from_millis(1000))));
            true
        }
        Some(next) => now >= next,
    });
    if !ready {
        return false;
    }

    let mut content = SYNTH_MAP_PWR_STATE_REQ_CONTENT.load(unicorn);
    if content == 0 || content > 0xf000_0000 {
        let mmu_arc = unicorn.get_data().mmu.clone();
        content = mmu_arc.lock().unwrap().heap_alloc(
            unicorn,
            CONTENT_LEN,
            Prot::READ | Prot::WRITE,
            "[synthetic-cca-map-power-state]",
        );
        if content == 0 {
            return false;
        }

        let mut body = [0_u8; CONTENT_LEN as usize];
        body[0x00..0x02].copy_from_slice(&0x0400u16.to_le_bytes());
        body[0x04..0x08].copy_from_slice(&CONTENT_LEN.to_le_bytes());
        body[0x08..0x0a].copy_from_slice(&0x0002u16.to_le_bytes());
        body[0x0b] = 0x40;
        body[0x0c..0x0e].copy_from_slice(&0x0001u16.to_le_bytes());
        body[0x14..0x16].copy_from_slice(&0x10u16.to_le_bytes());
        body[0x18..0x1c].copy_from_slice(&3u32.to_le_bytes());
        body[0x1c..0x20].copy_from_slice(&0u32.to_le_bytes());

        if unicorn.mem_write(content as u64, &body).is_err() {
            return false;
        }
        SYNTH_MAP_PWR_STATE_REQ_CONTENT.store(unicorn, content);
    }

    if unicorn.mem_write(buf as u64, &1u32.to_le_bytes()).is_err()
        || unicorn
            .mem_write((buf + 4) as u64, &content.to_le_bytes())
            .is_err()
    {
        return false;
    }

    let thread = unicorn.get_data().inner.thread_id();
    log::info!(
        "[{}] [LIBOSAL] OSAL_s32MessageQueueWait({}) answered with periodic synthetic MAP PWR_STATE_CHANGE_REQ content=0x{:x}",
        thread,
        TARGET_QUEUE,
        content
    );
    SYNTH_MAP_PWR_PERIODIC_NEXT.with(|cell| cell.set(Some(now + crate::emulator::vtime::host_interval(Duration::from_millis(1000)))));
    return_to_caller(unicorn, 8);
    true
}

fn synthesize_dapi_power_startup_sequence(
    unicorn: &mut Unicorn<'_, Context>,
    api_name: &str,
    name: &str,
    buf: u32,
    stack_timeout: u32,
) -> bool {
    const DAPI_APP_ID: u16 = 7;

    for message in [
        (
            3u16,
            0u32,
            0u32,
            "DAPI PWR_PROXY_START_CONF",
            &SYNTH_DAPI_PWR_START_CONF_SENT as &AtomicBool,
            &SYNTH_DAPI_PWR_START_CONF_CONTENT,
        ),
        (
            0x10,
            3,
            0,
            "DAPI PWR_STATE_CHANGE_REQ",
            &SYNTH_DAPI_PWR_STATE_REQ_SENT,
            &SYNTH_DAPI_PWR_STATE_REQ_CONTENT,
        ),
        (
            0x50,
            0,
            0,
            "DAPI PWR_CVM_SIGNAL_CHANGED",
            &SYNTH_DAPI_PWR_CVM_SIGNAL_CHANGED_SENT,
            &SYNTH_DAPI_PWR_CVM_SIGNAL_CHANGED_CONTENT,
        ),
    ] {
        if synthesize_dapi_power_message(
            unicorn,
            api_name,
            name,
            buf,
            stack_timeout,
            DAPI_APP_ID,
            message.0,
            message.1,
            message.2,
            message.3,
            message.4,
            message.5,
        ) {
            return true;
        }
    }

    if SYNTH_DAPI_PWR_START_CONF_SENT.load(Ordering::SeqCst)
        && SYNTH_DAPI_PWR_STATE_REQ_SENT.load(Ordering::SeqCst)
        && SYNTH_DAPI_PWR_CVM_SIGNAL_CHANGED_SENT.load(Ordering::SeqCst)
    {
        return synthesize_periodic_dapi_power_state_req(unicorn, buf);
    }

    false
}

fn synthesize_dapi_power_message(
    unicorn: &mut Unicorn<'_, Context>,
    api_name: &str,
    name: &str,
    buf: u32,
    stack_timeout: u32,
    target_app: u16,
    power_type: u16,
    power_data1: u32,
    power_data2: u32,
    message_name: &str,
    sent: &AtomicBool,
    content_slot: &GuestAddrCell,
) -> bool {
    const TARGET_QUEUE: &str = "mbx_7";
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

    if power_type != 3 && !SYNTH_DAPI_PWR_START_CONF_SENT.load(Ordering::SeqCst) {
        return false;
    }
    if sent
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
        .is_err()
    {
        return false;
    }

    let mut content = content_slot.load(unicorn);
    if content == 0 || content > 0xf000_0000 {
        let mmu_arc = unicorn.get_data().mmu.clone();
        content = mmu_arc.lock().unwrap().heap_alloc(
            unicorn,
            CONTENT_LEN,
            Prot::READ | Prot::WRITE,
            "[synthetic-cca-dapi-power]",
        );
        if content == 0 {
            return false;
        }

        let mut body = [0u8; CONTENT_LEN as usize];
        body[0x00..0x02].copy_from_slice(&target_app.to_le_bytes());
        body[0x04..0x08].copy_from_slice(&CONTENT_LEN.to_le_bytes());
        body[0x08..0x0a].copy_from_slice(&0x0002u16.to_le_bytes());
        body[0x0b] = 0x40;
        body[0x0c..0x0e].copy_from_slice(&0x0001u16.to_le_bytes());
        body[0x14..0x16].copy_from_slice(&power_type.to_le_bytes());
        body[0x18..0x1c].copy_from_slice(&power_data1.to_le_bytes());
        body[0x1c..0x20].copy_from_slice(&power_data2.to_le_bytes());

        if unicorn.mem_write(content as u64, &body).is_err() {
            return false;
        }

        content_slot.store(unicorn, content);
    }

    if unicorn.mem_write(buf as u64, &1u32.to_le_bytes()).is_err()
        || unicorn
            .mem_write((buf + 4) as u64, &content.to_le_bytes())
            .is_err()
    {
        return false;
    }

    let thread = unicorn.get_data().inner.thread_id();
    log::info!(
        "[{}] [LIBOSAL] {}({}) answered with synthetic {} content=0x{:x}",
        thread,
        api_name,
        name,
        message_name,
        content
    );
    return_to_caller(unicorn, 8);
    true
}

fn synthesize_periodic_dapi_power_state_req(unicorn: &mut Unicorn<'_, Context>, buf: u32) -> bool {
    const CONTENT_LEN: u32 = 0x20;
    const TARGET_QUEUE: &str = "mbx_7";

    if buf == 0 || buf > 0xf000_0000 {
        return false;
    }

    let now = Instant::now();
    let ready = SYNTH_DAPI_PWR_PERIODIC_NEXT.with(|cell| match cell.get() {
        None => {
            cell.set(Some(now + crate::emulator::vtime::host_interval(Duration::from_millis(1000))));
            true
        }
        Some(next) => now >= next,
    });
    if !ready {
        return false;
    }

    let mut content = SYNTH_DAPI_PWR_STATE_REQ_CONTENT.load(unicorn);
    if content == 0 || content > 0xf000_0000 {
        let mmu_arc = unicorn.get_data().mmu.clone();
        content = mmu_arc.lock().unwrap().heap_alloc(
            unicorn,
            CONTENT_LEN,
            Prot::READ | Prot::WRITE,
            "[synthetic-cca-dapi-power-state]",
        );
        if content == 0 {
            return false;
        }

        let mut body = [0_u8; CONTENT_LEN as usize];
        body[0x00..0x02].copy_from_slice(&7u16.to_le_bytes());
        body[0x04..0x08].copy_from_slice(&CONTENT_LEN.to_le_bytes());
        body[0x08..0x0a].copy_from_slice(&0x0002u16.to_le_bytes());
        body[0x0b] = 0x40;
        body[0x0c..0x0e].copy_from_slice(&0x0001u16.to_le_bytes());
        body[0x14..0x16].copy_from_slice(&0x10u16.to_le_bytes());
        body[0x18..0x1c].copy_from_slice(&3u32.to_le_bytes());
        body[0x1c..0x20].copy_from_slice(&0u32.to_le_bytes());

        if unicorn.mem_write(content as u64, &body).is_err() {
            return false;
        }
        SYNTH_DAPI_PWR_STATE_REQ_CONTENT.store(unicorn, content);
    }

    if unicorn.mem_write(buf as u64, &1u32.to_le_bytes()).is_err()
        || unicorn
            .mem_write((buf + 4) as u64, &content.to_le_bytes())
            .is_err()
    {
        return false;
    }

    let thread = unicorn.get_data().inner.thread_id();
    log::info!(
        "[{}] [LIBOSAL] OSAL_s32MessageQueueWait({}) answered with periodic synthetic DAPI PWR_STATE_CHANGE_REQ content=0x{:x}",
        thread,
        TARGET_QUEUE,
        content
    );
    SYNTH_DAPI_PWR_PERIODIC_NEXT.with(|cell| cell.set(Some(now + crate::emulator::vtime::host_interval(Duration::from_millis(1000)))));
    return_to_caller(unicorn, 8);
    true
}

fn emulate_message_create(unicorn: &mut Unicorn<'_, Context>, addr: u32, base_address: u32) {
    const TYPE_HEAP: u32 = 1;
    const TYPE_POOL: u32 = 2;

    let handle_ptr = unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    let size = unicorn.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
    let message_type = unicorn.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
    if handle_ptr == 0 || handle_ptr > 0xf000_0000 {
        return;
    }
    if message_type != TYPE_POOL {
        return;
    }

    let thread = unicorn.get_data().inner.thread_id();
    let state_arc = unicorn.get_data().sys_calls_state.clone();
    let needs_base = state_arc.lock().unwrap().osal_messages.base() == 0;
    if needs_base {
        let pool_size = 0x1000 * 1024;
        let mmu_arc = unicorn.get_data().mmu.clone();
        let base = mmu_arc.lock().unwrap().heap_alloc(
            unicorn,
            pool_size,
            Prot::READ | Prot::WRITE,
            "[osal-msgpool]",
        );
        state_arc.lock().unwrap().osal_messages.set_base(base);
        log::info!(
            "0x{:x} [{}] [LIBOSAL] created emulated OSAL message pool base=0x{:x} slots=1024 chunk=0x1000",
            addr - base_address + ORIGINAL_BASE,
            thread,
            base
        );
    }

    let mut allocated = {
        let mut state = state_arc.lock().unwrap();
        let pool = &mut state.osal_messages;
        if size <= pool.chunk_size() {
            pool.take_slot()
                .map(|index| pool.base() + index * pool.chunk_size())
                .unwrap_or(0)
        } else {
            0
        }
    };

    if allocated == 0 {
        allocated = state_arc
            .lock()
            .unwrap()
            .osal_messages
            .take_freed(size)
            .unwrap_or(0);
    }

    if allocated == 0 && size > 0x1000 {
        let mmu_arc = unicorn.get_data().mmu.clone();
        allocated = mmu_arc.lock().unwrap().heap_alloc(
            unicorn,
            size,
            Prot::READ | Prot::WRITE,
            "[osal-msg-large]",
        );
        state_arc
            .lock()
            .unwrap()
            .osal_messages
            .mark_dynamic(allocated, size);
    }

    if allocated == 0 {
        let mmu_arc = unicorn.get_data().mmu.clone();
        allocated = mmu_arc.lock().unwrap().heap_alloc(
            unicorn,
            size,
            Prot::READ | Prot::WRITE,
            "[osal-msg-dynamic]",
        );
        state_arc
            .lock()
            .unwrap()
            .osal_messages
            .mark_dynamic(allocated, size);
    }

    if allocated == 0 {
        log::warn!(
            "0x{:x} [{}] [LIBOSAL] OSAL_s32MessageCreate emulated pool exhausted size=0x{:x}",
            addr - base_address + ORIGINAL_BASE,
            thread,
            size
        );
        return_to_caller(unicorn, u32::MAX);
        return;
    }

    if unicorn
        .mem_write(handle_ptr as u64, &TYPE_HEAP.to_le_bytes())
        .is_err()
        || unicorn
            .mem_write((handle_ptr + 4) as u64, &allocated.to_le_bytes())
            .is_err()
    {
        let mut state = state_arc.lock().unwrap();
        state.osal_messages.release(allocated);
        log::warn!(
            "0x{:x} [{}] [LIBOSAL] failed to write emulated OSAL message handle=0x{:x} content=0x{:x}",
            addr - base_address + ORIGINAL_BASE,
            thread,
            handle_ptr,
            allocated
        );
        return;
    }

    log::info!(
        "0x{:x} [{}] [LIBOSAL] OSAL_s32MessageCreate(pool=0x{:x}, size=0x{:x}) -> heap content=0x{:x}",
        addr - base_address + ORIGINAL_BASE,
        thread,
        handle_ptr,
        size,
        allocated
    );
    return_to_caller(unicorn, 0);
}

fn suppress_synthetic_message_delete(
    unicorn: &mut Unicorn<'_, Context>,
    addr: u32,
    base_address: u32,
) {
    let handle = unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    let content = unicorn.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
    let _ = handle;
    if content != 0 && content < 0xf000_0000 {
        let state_arc = unicorn.get_data().sys_calls_state.clone();
        let released = state_arc.lock().unwrap().osal_messages.release(content);
        if released {
            log::info!(
                "0x{:x} [{}] [LIBOSAL] OSAL_s32MessageDelete released emulated message content=0x{:x}",
                addr - base_address + ORIGINAL_BASE,
                unicorn.get_data().inner.thread_id(),
                content
            );
            return_to_caller(unicorn, 0);
            return;
        }
    }
    // Synthetic power-message injection has been disabled. Never treat a
    // live OSAL message as a synthetic one that should escape deletion.
    return;

    let thread = unicorn.get_data().inner.thread_id();
    log::info!(
        "0x{:x} [{}] [LIBOSAL] OSAL_s32MessageDelete ignored synthetic power message content=0x{:x}",
        addr - base_address + ORIGINAL_BASE,
        thread,
        content
    );
    return_to_caller(unicorn, 0);
}

/// Dispatch hook for the host-side Bosch PWR-proxy policy. Runs before
/// `bridge_guest_osal_queue` for the small set of OSAL queue calls the
/// proxy cares about. `mbx_0` posts are observed (never intercepted) so
/// the proxy always sees them; the `mbx_` bridge then snapshots them for the
/// Triton-side waiters (`<mbx_<app_id>` waits are also served from the
/// proxy's pending queue when it has anything queued).
fn pwr_proxy_service_handle(
    unicorn: &mut Unicorn<'_, Context>,
    api_name: &str,
    name: &str,
    r1: u32,
    r2: u32,
) -> bool {
    match api_name {
        "OSAL_s32MessageQueueWait" | "OSAL_s32MessageQueuePriorityWait" => {
            inject_pwr_proxy_pending_message(unicorn, name, r1, r2)
        }
        "OSAL_s32MessageQueuePost" => {
            observe_pwr_proxy_post(unicorn, name, r1, r2);
            observe_cca_directory(unicorn, name, r1, r2);
            false
        }
        _ => false,
    }
}

/// Serve a Wait on `mbx_<app_id>` from the PWR-proxy's pending queue. If
/// the proxy has nothing queued we return `false` so the Wait falls
/// through to the guest's own queue semantics.
fn inject_pwr_proxy_pending_message(
    unicorn: &mut Unicorn<'_, Context>,
    name: &str,
    buf: u32,
    buf_len: u32,
) -> bool {
    let Some(app_id) = pwr_proxy::parse_mbx_app_id(name) else {
        return false;
    };
    if buf == 0 || buf > 0xf000_0000 || buf_len < 8 {
        return false;
    }
    let Some(message) = pwr_proxy::take_pending_for(app_id) else {
        return false;
    };

    let size = pwr_proxy::POWER_MESSAGE_LEN as u32;
    let content = {
        let state_arc = unicorn.get_data().sys_calls_state.clone();
        let mut state = state_arc.lock().unwrap();
        match state.osal_messages.take_freed(size) {
            Some(content) => content,
            None => {
                drop(state);
                let mmu_arc = unicorn.get_data().mmu.clone();
                let content = mmu_arc.lock().unwrap().heap_alloc(
                    unicorn,
                    size,
                    Prot::READ | Prot::WRITE,
                    "[pwr-proxy-power-message]",
                );
                unicorn
                    .get_data()
                    .sys_calls_state
                    .lock()
                    .unwrap()
                    .osal_messages
                    .mark_dynamic(content, size);
                content
            }
        }
    };
    if content == 0 || content > 0xf000_0000 {
        return false;
    }

    if unicorn.mem_write(content as u64, &message.body).is_err() {
        return false;
    }
    // Register the allocation with the recipient's OSAL message pool so the
    // eventual `amt_tclMappableMessage::bDelete` -> `OSAL_s32MessageDelete`
    // path finds it and returns success. Without this the caller trips
    // `OSAL_vAssertFunction("ALWAYS", "amt_MMObj.cpp", ...)` which aborts the
    // process. See bDelete at procmap 0x00388ef0.
    // (Already registered as dynamic by the branch above.)
    // The guest's `ail_bIpcMessageWait` expects an 8-byte OSAL message
    // reference: `[type_flag, content_ptr]`. Type flag 1 means "direct
    // pointer to content" (see `OSAL_pu8MessageContentGet`); the historical
    // synthetic used the same encoding, so procmap's dispatch accepts our
    // injection unchanged.
    if unicorn.mem_write(buf as u64, &1u32.to_le_bytes()).is_err()
        || unicorn
            .mem_write((buf + 4) as u64, &content.to_le_bytes())
            .is_err()
    {
        return false;
    }

    // The map application's `bDispatchCCAMessages` silently drops a
    // START_CONF unless its CCA state has already been bumped to
    // INITIALIZED. Real hardware gets that bump for free from the body
    // thread's `vAppBody`; the emulated entry thread wins the race and
    // observes state==NOT_STARTED. Hand it the same state the body
    // thread would have set before we return from Wait.
    if message.power_type == pwr_proxy::PWR_PROXY_START_CONF && app_id == 0x0400 {
        crate::libs::procmapengine::prepare_app_state_for_start_conf(unicorn);
    }

    let thread = unicorn.get_data().inner.thread_id();
    log::info!(
        "[{}] [LIBOSAL] PWR proxy delivered power type {} (data1 {} data2 {}) to app 0x{:04x} via Wait({}) content=0x{:x}",
        thread,
        message.power_type,
        message.power_data1,
        message.power_data2,
        app_id,
        name,
        content
    );
    return_to_caller(unicorn, 8);
    true
}

/// Observe a Post to `mbx_0` (the shared LPM inbound queue). The OSAL
/// queue itself only carries an 8-byte reference; we resolve the pointer
/// against the sender's own message pool while the sender's VM is still
/// accessible and hand the parsed fields to the proxy. Returns nothing
/// so the guest's own Post proceeds normally (the queue is unused by any
/// recipient but the sender expects it to succeed).
fn observe_pwr_proxy_post(
    unicorn: &mut Unicorn<'_, Context>,
    name: &str,
    msg_ptr: u32,
    msg_len: u32,
) {
    if name != pwr_proxy::LPM_IN_QUEUE {
        return;
    }
    if msg_ptr == 0 || msg_ptr > 0xf000_0000 || msg_len < 8 {
        return;
    }
    let type_flag = match read_guest_buffer(unicorn, msg_ptr, 4) {
        Some(data) => u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
        None => return,
    };
    if type_flag != 1 {
        return;
    }
    let content_ptr = match read_guest_buffer(unicorn, msg_ptr + 4, 4) {
        Some(data) => u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
        None => return,
    };
    if content_ptr == 0 || content_ptr > 0xf000_0000 {
        return;
    }
    let Some(body) = read_guest_buffer(unicorn, content_ptr, pwr_proxy::POWER_MESSAGE_LEN) else {
        return;
    };
    if let Some((sender_app_id, power_type, data1, data2)) = pwr_proxy::parse_power_message(&body)
    {
        pwr_proxy::observe_power_post(sender_app_id, power_type, data1, data2);
    }
}

/// r755: CCA directory (LPM) emulation.  `ApplicationInfoRegister` (wire
/// class 0x49, `amt_tclApplicationInfoRegister`, ServerAppID @body+0x14) is
/// every client's request for the LPM to track interest in a server
/// application; the unit's LPM answers with `ApplicationInfoStatus` (class
/// 0x50, ServerAppID @+0x14, ServerAppState byte @+0x16) on the client's own
/// queue, where `ail_bHandleMsgAppInfoMessage` feeds the server-application
/// list that gates prochmi's whole hmicca registration chain.  Nothing here
/// consumed `mbx_0`, so the registrations posted at HMI init were the only
/// CCA traffic that ever left the HMI - and none ever came back.
///
/// Scope is milestone 1: answer the HMI (0x0109) only.  The answer must not
/// arrive before the client's AIL admin state reaches RUNNING (dispatch
/// drops class 0x50 below state 3), so replies are cached and flushed when
/// the PWR proxy hands out the NORMAL `PWR_STATE_CHANGE_REQ` - the message
/// whose processing promotes that state - or live once the promotion is
/// known to have happened.  Replies must also not create the recipient's
/// host queue before its owner did (guest `mq_create` is O_EXCL, r755): the
/// flush path refuses to create and re-caches instead.
static DIR_REPLY_CACHE: std::sync::LazyLock<std::sync::Mutex<Vec<Vec<u8>>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(Default::default()));

/// Deliver every cached ApplicationInfoStatus to the HMI queue (which its
/// owner must already exist in the host table).  Re-caches on failure.
fn dir_reply_flush(unicorn: &mut Unicorn<'_, Context>) {
    let bodies = match DIR_REPLY_CACHE.lock() {
        Ok(mut c) => std::mem::take(&mut *c),
        Err(_) => return,
    };
    if bodies.is_empty() {
        return;
    }
    let mut kept: Vec<Vec<u8>> = Vec::new();
    let mut posted = 0usize;
    for body in bodies {
        let mut blob = Vec::with_capacity(8 + body.len());
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
        blob.extend_from_slice(&body);
        let mut state = unicorn.get_data().namespace.lock().unwrap();
        match state.mq.queue_id_for_name("mbx_265") {
            Some(q) => {
                state.mq.grow_msgsize(q, blob.len() as i64);
                if OsalQueueService::guest_post(&mut state.mq, "mbx_265", blob, 0) {
                    posted += 1;
                    state.notify_waiters();
                }
            }
            None => kept.push(body),
        }
    }
    if !kept.is_empty() {
        if let Ok(mut c) = DIR_REPLY_CACHE.lock() {
            c.append(&mut kept);
        }
    }
    log::info!("[LIBOSAL-MBX] cca directory: flushed ApplicationInfoStatus replies to mbx_265: {posted}");
}

/// Post every due PWR-proxy PowerMessage into its target application's host
/// queue.  Delayed messages (the HMI's spaced NORMAL/CVM after the OFF step)
/// must not ride the Wait-serve channel because the HMI's AIL dispatcher sits
/// in an eternal `mbx_265` Wait the moment the OFF request has been processed
/// (run_fix77) - a plain host `guest_post` wakes it like any other poster.
/// The blob layout matches `dir_reply_flush` / the GPS provider
/// (`[u32 flag, u32 len, body]`), which the monitor's `0x2d` wait materialises
/// into the waiter's VM.  Called from `prochmi::tick`.
pub fn post_due_power_messages(unicorn: &mut Unicorn<'_, Context>) {
    for message in crate::rtos::pwr_proxy::take_due_all() {
        let name = format!("mbx_{}", message.app_id);
        let mut blob = Vec::with_capacity(8 + message.body.len());
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&(message.body.len() as u32).to_le_bytes());
        blob.extend_from_slice(&message.body);
        let mut state = unicorn.get_data().namespace.lock().unwrap();
        match state.mq.queue_id_for_name(&name) {
            Some(q) => {
                state.mq.grow_msgsize(q, blob.len() as i64);
                if OsalQueueService::guest_post(&mut state.mq, &name, blob, 0) {
                    state.notify_waiters();
                    log::info!(
                        "[LIBOSAL-MBX] PWR proxy: posted delayed PowerType {:#x} (data {}) to {name}",
                        message.power_type,
                        message.power_data1
                    );
                } else {
                    log::warn!("[LIBOSAL-MBX] PWR proxy: delayed post to {name} failed, message dropped");
                }
            }
            None => log::warn!(
                "[LIBOSAL-MBX] PWR proxy: {name} missing, delayed PowerType {:#x} dropped",
                message.power_type
            ),
        }
    }
}

/// Parse one `mbx_0` post; when it is an HMI `ApplicationInfoRegister`,
/// build and spool/queue the LPM's ApplicationInfoStatus answer.
fn observe_cca_directory(    unicorn: &mut Unicorn<'_, Context>,
    name: &str,
    msg_ptr: u32,
    msg_len: u32,
) {
    if name != pwr_proxy::LPM_IN_QUEUE || msg_ptr == 0 || msg_ptr > 0xf000_0000 || msg_len < 8 {
        return;
    }
    let Some(data) = read_guest_buffer(unicorn, msg_ptr, 8) else {
        return;
    };
    if u32::from_le_bytes([data[0], data[1], data[2], data[3]]) != 1 {
        return;
    }
    let content = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    if content == 0 || content > 0xf000_0000 {
        return;
    }
    let Some(body) = read_guest_buffer(unicorn, content, 0x18) else {
        return;
    };
    if body[0xb] != 0x49 {
        return;
    }
    let src = u16::from_le_bytes([body[0], body[1]]);
    if src != 0x0109 {
        return;
    }
    let server_app = u16::from_le_bytes([body[0x14], body[0x15]]);
    // r757: on the unit the HMI framework boots through fw state OFF(4) *before*
    // NORMAL(3): `clFi_Navi::vNewAppState` starts the enavi server (and with it
    // the whole DAPI<->PROCNAV FI link that eventually hides the
    // "Starting navigation." wait-widget) only on a `previous == 4 -> 3`
    // transition.  The framework state mirrors the LPM (app 0x0000) application
    // status: an ApplicationInfoStatus with ServerAppState 0 runs
    // `clAppStates::vEntryOff` (fw 4), state 1 runs `vEntryNormal` (fw 3).
    // Stand in for the LPM's "not running yet" announcement: on its own
    // registration spool a state-0 answer first, so the flush delivers
    // UNAVAILABLE before AVAILABLE (FIFO).
    if server_app == 0 {
        static SEEN_OFF: std::sync::LazyLock<std::sync::atomic::AtomicBool> =
            std::sync::LazyLock::new(|| std::sync::atomic::AtomicBool::new(false));
        if !SEEN_OFF.swap(true, Ordering::Relaxed) {
            log::info!(
                "[LIBOSAL-MBX] cca directory: self ApplicationInfoRegister -> spooling ApplicationInfoStatus(state 0 UNAVAILABLE) before state 1"
            );
            let mut off = vec![0u8; 0x18];
            off[0..2].copy_from_slice(&0u16.to_le_bytes());
            off[2..4].copy_from_slice(&src.to_le_bytes());
            off[4..8].copy_from_slice(&(0x18u32).to_le_bytes());
            off[8..10].copy_from_slice(&2u16.to_le_bytes());
            off[0xb] = 0x50;
            off[0x10..0x14].copy_from_slice(&0x1FFFu32.to_le_bytes());
            off[0x14..0x16].copy_from_slice(&server_app.to_le_bytes());
            off[0x16] = 0; // ServerAppState: UNAVAILABLE
            if let Ok(mut c) = DIR_REPLY_CACHE.lock() {
                c.insert(0, off);
            }
        }
    }
    // The LPM's answer.  Layout mirrors the register body (same base
    // message header); the fields `ail_bHandleMsgAppInfoMessage` reads are
    // ServerAppID @+0x14 and ServerAppState @+0x16.  State 1 is the value
    // `bRegisterAsync`'s scan matches on (`state == '\x01'`) and the state
    // the LPM reports for a live server application.
    static MSG_ID: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0x2000);
    let msg_id = MSG_ID.fetch_add(1, Ordering::Relaxed);
    let mut reply = vec![0u8; 0x18];
    reply[0..2].copy_from_slice(&0u16.to_le_bytes()); // src: the LPM (app 0)
    reply[2..4].copy_from_slice(&src.to_le_bytes()); // dst: the HMI
    reply[4..8].copy_from_slice(&(0x18u32).to_le_bytes());
    reply[8..10].copy_from_slice(&2u16.to_le_bytes()); // opcode
    reply[0xb] = 0x50; // wire class: ApplicationInfoStatus
    reply[0x10..0x14].copy_from_slice(&(msg_id as u32).to_le_bytes());
    reply[0x14..0x16].copy_from_slice(&server_app.to_le_bytes());
    reply[0x16] = 1; // ServerAppState: RUNNING
    static SEEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<u16>>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(Default::default()));
    let fresh = SEEN
        .lock()
        .map(|mut s| s.insert(server_app) && s.len() <= 48)
        .unwrap_or(false);
    if fresh {
        log::info!(
            "[LIBOSAL-MBX] cca directory: ApplicationInfoRegister(svc {server_app:#x}) from HMI -> answering ApplicationInfoStatus(state 1)"
        );
    }
    if pwr_proxy::app_state_change_delivered(src) {
        // Client is (or is about to be) at admin state 3: deliver live.
        let mut blob = Vec::with_capacity(8 + reply.len());
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&(reply.len() as u32).to_le_bytes());
        blob.extend_from_slice(&reply);
        let mut state = unicorn.get_data().namespace.lock().unwrap();
        let deliver = match state.mq.queue_id_for_name("mbx_265") {
            Some(q) => {
                state.mq.grow_msgsize(q, blob.len() as i64);
                let ok = OsalQueueService::guest_post(&mut state.mq, "mbx_265", blob, 0);
                if ok {
                    state.notify_waiters();
                }
                ok
            }
            None => false,
        };
        if deliver {
            return;
        }
    }
    if let Ok(mut c) = DIR_REPLY_CACHE.lock() {
        if c.len() < 64 {
            c.push(reply);
        }
    }
}

/// r755: CCA service-server emulation for the HMI's service registrations.
/// Once the `ApplicationInfoStatus` answer above makes
/// `vOnServerApplicationStatus` AVAILABLE, the HMI's `bRegisterAsyncExecute`
/// posts a ServiceRegister (class 0x41) to the *server application's* queue
/// (service 0x0024 opened `mbx_36` at HMI init).  The providers of the GUI's
/// service ids are Triton-executive-side apps with no Linux-side presence
/// here, so those registrations would go unanswered forever.  Answer as a
/// live unit's directory/server does: a RegisterConf (class 0x43, result
/// accepted @+0x18) immediately followed by a ServiceStatus (class 0x44,
/// state ACTIVE @+0x18) on the client queue.
/// `ail_bHandleMsgServiceRegisterConf` turns the conf into an
/// `ail_tclServiceReference` and `ail_bHandleMsgServiceStatus` then has a
/// matching reference to dispatch `vOnServiceState` through - the event the
/// HMI popup/map page react to.  The regid (u16 @+0x16) stays 0xffff:
/// `stamp_registration_id` assigns one per (queue, service) and keeps the
/// conf and status consistent.
///
/// Milestone-1 scope: only HMI (0x0109) registrations on server-app queues;
/// other clients keep their existing bespoke answers.  Timing gate is the
/// same as the directory's: both classes need admin state RUNNING, so the
/// spool rides `DIR_REPLY_CACHE` until the pump reaches a bridged Wait
/// after promotion.
/// r791: RegisterConf/ServiceStatus bodies that missed their live delivery
/// window, tagged with the client queue they belong to.  Flushed when that
/// queue next appears in a bridged Wait.
static SCD_REG_SPOOL: std::sync::LazyLock<std::sync::Mutex<Vec<(String, Vec<u8>)>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(Default::default()));

fn scd_reg_spool_flush(unicorn: &mut Unicorn<'_, Context>, queue: &str) {
    let owned: Vec<Vec<u8>> = match SCD_REG_SPOOL.lock() {
        Ok(mut c) => {
            let take: Vec<usize> = c
                .iter()
                .enumerate()
                .filter(|(_, (q, _))| q == queue)
                .map(|(i, _)| i)
                .collect();
            if take.is_empty() {
                return;
            }
            take.into_iter().rev().map(|i| c.remove(i).1).collect()
        }
        Err(_) => return,
    };
    let mut posted = 0usize;
    for body in owned {
        let mut blob = Vec::with_capacity(8 + body.len());
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
        blob.extend_from_slice(&body);
        let mut state = unicorn.get_data().namespace.lock().unwrap();
        if let Some(q) = state.mq.queue_id_for_name(queue) {
            state.mq.grow_msgsize(q, blob.len() as i64);
            if OsalQueueService::guest_post(&mut state.mq, queue, blob, 0) {
                posted += 1;
                state.notify_waiters();
            }
        }
    }
    if posted > 0 {
        log::info!("[LIBOSAL-MBX] cca server: flushed {posted} spooled registrations to {queue}");
    }
}

fn cca_server_answer_register(
    unicorn: &mut Unicorn<'_, Context>,
    server_app: u16,
    client: u16,
    service: u16,
) {
    static MSG_ID: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0x3000);
    // r791: answers go to the client's own app queue (mbx_<client>): that is
    // the queue `scd_bCreateQueue` hands the client and where its AIL pump
    // waits (HMI: mbx_265 = mbx_<0x109>, navif server: mbx_289 = mbx_<0x121>).
    let target = format!("mbx_{client}");
    let mut answers: Vec<Vec<u8>> = Vec::with_capacity(2);
    for class in [0x43u8, 0x44] {
        let mut body = vec![0u8; 0x20];
        body[0..2].copy_from_slice(&server_app.to_le_bytes());
        body[2..4].copy_from_slice(&client.to_le_bytes());
        body[4..8].copy_from_slice(&(0x20u32).to_le_bytes());
        body[8..10].copy_from_slice(&2u16.to_le_bytes());
        body[0xb] = class;
        // r796c: the claim walk (`ail_tclServiceReference::bIsDataSet`) compares
        // the message's unique id at +0x0e against the receiver reference's
        // stored slot, and the 0xffff wildcard covers only the message side.
        // DAPI's genuine conf/status on mbx_289 carry AIL_NO_UNIQUE_ID (0xfffe)
        // there, which the navsrv client's references match; our zeros made
        // every answer unclaimable, so `bIsDataSet` never flipped and the CL
        // entry event (the gate-word's state-2 writer) never fired.
        body[0x0e..0x10].copy_from_slice(&0xfffeu16.to_le_bytes());
        let msg_id = MSG_ID.fetch_add(1, Ordering::Relaxed) as u32;
        body[0x10..0x14].copy_from_slice(&msg_id.to_le_bytes());
        body[0x14..0x16].copy_from_slice(&service.to_le_bytes());
        body[0x16..0x18].copy_from_slice(&0xffffu16.to_le_bytes());
        if class == 0x43 {
            body[0x18] = 1; // conf result: accepted
        } else if service == 0x0027 {
            // r758: the HMI-side FI-8 branch that starts the enavi server fires
            // on event id 0x80108031 = the CMNAVI event base 0x80108030 + the
            // service state, and the popup's whole release chain keys off it.
            // A state-0 (fastmap ACTIVE) status only ever produces the base
            // id; the nav service needs state 1 to reach the startup case.
            // r796 note: do NOT copy this for the CL service 0x000a - state 1
            // reads as UNAVAILABLE there and drops the state-5 gate again
            // (f187: c6-svc0a flipped 1 -> 0).
            body[0x18] = 1; // ServiceStatus: state 1 for the NAVI service
        } // else ServiceStatus: state byte 0 = ACTIVE (fastmap precedent)
        stamp_registration_id(&target, &mut body);
        answers.push(body);
    }
    if service == 0x000a {
        // r796c: re-announce the ServiceStatus once, the way the genuine
        // providers do (DAPI posts the svc 0x06 status twice per register).
        // The enavi CL-entry transaction consumes the conf + first status;
        // only the *second* status is seen by the connector's event walk
        // (HMINavServerLib_1 pump -> entry vfn+0x1c 0x1719b74 -> 0x17192ac
        // -> vfn+0x4c), whose sink-vector check stores the READY(2) state
        // the startup gate 0x17c2760 waits for.  With a single status the
        // entry stayed at REGISTERED(1) forever (f181-f196).
        if let Some(status) = answers.iter().find(|b| b[0x0b] == 0x44).cloned() {
            let mut second = status;
            let msg_id = MSG_ID.fetch_add(1, Ordering::Relaxed) as u32;
            second[0x10..0x14].copy_from_slice(&msg_id.to_le_bytes());
            stamp_registration_id(&target, &mut second);
            log::info!(
                "[LIBOSAL-MBX] cca directory: re-announcing ServiceStatus(svc {service:#06x}) for the CL entry event walk"
            );
            answers.push(second);
        }
    }
    let answer_total = answers.len();
    let mut kept: Vec<Vec<u8>> = Vec::new();
    let mut posted = 0usize;
    for body in answers {
        let mut blob = Vec::with_capacity(8 + body.len());
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
        blob.extend_from_slice(&body);
        let mut state = unicorn.get_data().namespace.lock().unwrap();
        let delivered = match state.mq.queue_id_for_name(&target) {
            // r791l: the state-change promotion gate guards the HMI's AIL
            // admin state (class 0x50 is dropped below RUNNING).  The navif
            // server-app clients (0x0121) are consumed by plain ProxyInCCA
            // pumps with no such gate, and no NORMAL state change is ever
            // handed to those app ids - waiting for it spooled every navif
            // answer forever (f107: live 0, spooled 2).  Deliver live.
            Some(q) if client != 0x0109 || pwr_proxy::app_state_change_delivered(client) => {
                state.mq.grow_msgsize(q, blob.len() as i64);
                let cls = blob.get(8 + 0xb).copied().unwrap_or(0xff);
                let ok = OsalQueueService::guest_post(&mut state.mq, &target, blob, 0);
                if ok {
                    log::info!(
                        "[LIBOSAL-MBX] cca answer posted to {target} class={cls:#04x} svc={service:#06x}"
                    );
                    state.notify_waiters();
                }
                ok
            }
            _ => false,
        };
        if delivered {
            posted += 1;
        } else {
            kept.push(body);
        }
    }
    if !kept.is_empty() {
        if client == 0x0109 {
            if let Ok(mut c) = DIR_REPLY_CACHE.lock() {
                for body in kept {
                    if c.len() < 96 {
                        c.push(body);
                    }
                }
            }
        } else {
            // r791: other clients (the navif server, app 0x0121) ride their
            // own spool: the answers are replayed at that client's next
            // bridged queue Wait, not the HMI directory pump's.
            if let Ok(mut c) = SCD_REG_SPOOL.lock() {
                for body in kept {
                    if c.len() < 96 {
                        c.push((target.clone(), body));
                    }
                }
            }
        }
    }
    log::info!(
        "[LIBOSAL-MBX] cca server {server_app:#06x}: answered ServiceRegister(svc {service:#06x}) from {client:#06x} with RegisterConf+ServiceStatus (live {posted}, spooled {})",
        answer_total.saturating_sub(posted),
    );
    // r758 experiment: see post_navi_event_probe.  Runs once, right after the
    // NAVI service registration is answered (the HMI's nav client is live by
    // then; the data-request phase never starts in the emulator, so this is
    // the last organic moment on that client's timeline).
    if service == 0x0027 {
        static FIRED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !FIRED.swap(true, Ordering::Relaxed) {
            post_navi_event_probe(unicorn, client);
            // The enavi start-request flag (the counterpart of the
            // phone-side FI event 0x80108031) is fabricated right before the
            // first post-OFF NORMAL broadcast - see OFF_SEEN and the
            // hsimngr-dispatch trap in libs/prochmi.rs.
        }
    }
}

/// r755: answer the `ServiceData` *requests* (class 0x45, OPTYPE byte @+0x1a
/// in the client set) the HMI sends once a service registration is
/// confirmed - the navigation service (0x0027) queries are what the HMI
/// waits for before it drops the "Starting navigation." wait-widget.  A live
/// server answers with a server OPTYPE (bIsServerMessage accepts 6..0x0c,
/// 0x0e): here OPTYPE 9, result word @+0x18 = 0 (no error), dynamic-data
/// length byte @+0x19 = 0, echoing svc/regid/function so
/// `ail_tclServiceReference::bIsDataSet(svc, src, regid, func)` matches the
/// client's own reference and `ail_bHandleMsgServiceData` forwards the
/// message into the app receiver (dispatch vtable+0x20).
/// Milestone-1 scope: navigation service 0x0027 on server queue mbx_36 only;
/// other services need payloads whose schema is not decoded yet.
fn cca_server_answer_data_request(
    unicorn: &mut Unicorn<'_, Context>,
    server_app: u16,
    client: u16,
    service: u16,
    reg_id: u16,
    function: u16,
) {
    static MSG_ID: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0x4000);
    let mut body = vec![0u8; 0x20];
    body[0..2].copy_from_slice(&server_app.to_le_bytes());
    body[2..4].copy_from_slice(&client.to_le_bytes());
    body[4..8].copy_from_slice(&(0x20u32).to_le_bytes());
    body[8..10].copy_from_slice(&2u16.to_le_bytes());
    body[0xb] = 0x45;
    let msg_id = MSG_ID.fetch_add(1, Ordering::Relaxed) as u32;
    body[0x10..0x14].copy_from_slice(&msg_id.to_le_bytes());
    body[0x14..0x16].copy_from_slice(&service.to_le_bytes());
    body[0x16..0x18].copy_from_slice(&reg_id.to_le_bytes());
    // +0x18 stays 0: in a response this word is result/no-error (+0x19 is
    // the dynamic-data length, also 0).  The FunctionId the reference match
    // (`bIsDataSet`) compares is carried at wire +0x0c, 0 on the request and
    // 0 here.
    body[0x1a] = 9; // OPTYPE: server data response
    let mut blob = Vec::with_capacity(8 + body.len());
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
    blob.extend_from_slice(&body);
    let mut spooled = false;
    {
        let mut state = unicorn.get_data().namespace.lock().unwrap();
        let delivered = match state.mq.queue_id_for_name("mbx_265") {
            Some(q) if pwr_proxy::app_state_change_delivered(client) => {
                state.mq.grow_msgsize(q, blob.len() as i64);
                let ok = OsalQueueService::guest_post(&mut state.mq, "mbx_265", blob.clone(), 0);
                if ok {
                    state.notify_waiters();
                }
                ok
            }
            _ => false,
        };
        if !delivered {
            spooled = true;
        }
    }
    if spooled {
        if let Ok(mut c) = DIR_REPLY_CACHE.lock() {
            if c.len() < 96 {
                c.push(body);
            }
        }
    }
    log::info!(
        "[LIBOSAL-MBX] cca server {server_app:#06x}: answered ServiceData(svc {service:#06x} regid {reg_id:#04x} func {function:#06x}) from {client:#06x} (spooled {spooled})"
    );
}

/// r758: see the call site above.  The AppInfo-consumption evidence (fix117:
/// records whose word@+4 was 0x80108031 surfaced as `ail_bHandleMsgAppInfoMessage`,
/// i.e. word+4 is the OSAL message type AIL dispatches on) plus the consumer's
/// own id math (`id - 0x80108030 < 0x4f4`) say FI-8 events arrive as *service
/// data notifications* whose FunctionId is 0x8030+n and whose receiver composes
/// `event = 0x80100000 | function`.  So post NAVI (svc 0x27) data records with
/// func 0x8031 in the four plausible OPTYPE/regid combinations and watch
/// hsi-fievent-router / fi-nav-events / enavi-startup.
fn post_navi_event_probe(unicorn: &mut Unicorn<'_, Context>, client: u16) {
    static MSG_ID: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0x8031);
    // (func @ +0x18, OPTYPE @ +0x1a, regid @ +0x16)
    const VARIANTS: [(u16, u8, u16); 4] = [
        (0x8031, 0x0e, 0x00ec), // notification OPTYPE, the client's own regid
        (0x8031, 0x09, 0x00ec), // plain server-response OPTYPE
        (0x8031, 0x0e, 0xffff), // notification, wildcard regid
        (0x0031, 0x0e, 0x00ec), // short function id variant
    ];
    for (idx, (func, optype, regid)) in VARIANTS.iter().enumerate() {
        let mut body = vec![0u8; 0x20];
        body[0..2].copy_from_slice(&0x0024u16.to_le_bytes());
        body[2..4].copy_from_slice(&client.to_le_bytes());
        body[4..8].copy_from_slice(&(0x20u32).to_le_bytes());
        body[8..10].copy_from_slice(&2u16.to_le_bytes());
        body[0xb] = 0x45;
        let msg_id = MSG_ID.fetch_add(1, Ordering::Relaxed) as u32;
        body[0x10..0x14].copy_from_slice(&msg_id.to_le_bytes());
        body[0x14..0x16].copy_from_slice(&0x0027u16.to_le_bytes());
        body[0x16..0x18].copy_from_slice(&regid.to_le_bytes());
        body[0x18..0x1a].copy_from_slice(&func.to_le_bytes());
        body[0x1a] = *optype;
        let mut blob = Vec::with_capacity(8 + body.len());
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
        blob.extend_from_slice(&body);
        let mut state = unicorn.get_data().namespace.lock().unwrap();
        let delivered = match state.mq.queue_id_for_name("mbx_265") {
            Some(q) => {
                state.mq.grow_msgsize(q, blob.len() as i64);
                let ok = OsalQueueService::guest_post(&mut state.mq, "mbx_265", blob, 0);
                if ok {
                    state.notify_waiters();
                }
                ok
            }
            _ => false,
        };
        log::info!(
            "[LIBOSAL-MBX] navi-event-probe[{idx}]: func={func:#06x} optype={optype:#04x} regid={regid:#04x} delivered {delivered}"
        );
    }
}

/// r755: answer `ServiceData` *requests* (class 0x45, OPTYPE byte @+0x1a in
/// the client set) the HMI sends once a service registration is confirmed.
/// Only decodes what the navigation service needs (milestone 1).
fn cca_server_answer_data_request_if_unhosted(
    unicorn: &mut Unicorn<'_, Context>,
    name: &str,
    body: &[u8],
) {
    if body.len() < 0x1c || body[0xb] != 0x45 {
        return;
    }
    let client = u16::from_le_bytes([body[0], body[1]]);
    if client != 0x0109 {
        return;
    }
    let service = u16::from_le_bytes([body[0x14], body[0x15]]);
    if service != 0x0027 {
        return;
    }
    let server_app = match name.strip_prefix("mbx_").and_then(|d| d.parse::<u32>().ok()) {
        Some(v) if (1..=0xffff).contains(&v) => v as u16,
        _ => return,
    };
    let reg_id = u16::from_le_bytes([body[0x16], body[0x17]]);
    let function = u16::from_le_bytes([body[0x18], body[0x19]]);
    cca_server_answer_data_request(unicorn, server_app, client, service, reg_id, function);
}

/// The emulator stands in for the SENSORFI/POSFI server app (registry appid
/// `0x0015`, part of PROCMW - not among the emulated processes), answering
/// the class-0x45 `ServiceData` requests its registration fake handed out
/// register-ids for.  The HMI's navif client (0x0121) posts exactly one
/// `svc 0x000a fn 0x0f02 op 01` position query to mbx_21 and blocks on its
/// mbx_289 pump waiting for the answer - on the real unit the position page
/// fills from that reply.  The answer reuses the sanctioned GPS fabrication
/// (the position values are the very payload the GPS provider feeds the FC_STAT
/// sink, see `procnav::gps_status_message` / `procnav::nav_position_status_message`),
/// so the emulator never invents a second position truth.  Visible in the log
/// per answered request; `EMU_FAKE_SENSORFI=0` switches the fabrication off
/// (the request then stays unanswered, as on a unit without the FI server).
fn cca_server_answer_sensorfi_data_request(
    unicorn: &mut Unicorn<'_, Context>,
    name: &str,
    body: &[u8],
) {
    if body.len() < 0x1c || body[0xb] != 0x45 {
        return;
    }
    if std::env::var_os("EMU_FAKE_SENSORFI").is_some_and(|v| v == "0") {
        return;
    }
    let server_app = match name.strip_prefix("mbx_").and_then(|d| d.parse::<u32>().ok()) {
        Some(0x15) => 0x0015u16,
        _ => return,
    };
    let service = u16::from_le_bytes([body[0x14], body[0x15]]);
    if !matches!(service, 0x000a | 0x001e) {
        return;
    }
    let client = u16::from_le_bytes([body[0], body[1]]);
    let reg_id = u16::from_le_bytes([body[0x16], body[0x17]]);
    let function = u16::from_le_bytes([body[0x18], body[0x19]]);
    static SEQ: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(1);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    // Echo the client's reference tuple (svc/regid and the +0x0c function
    // slot, both 0-matched by `bIsDataSet`, wire layout pinned by
    // `trace_dapi_map_service_data`: svc@+0x14, regid@+0x16, fn@+0x0c) and
    // append the sensorfi/posfi status payload at +0x20 - the frame shape
    // `cca_server_answer_data_request` proved reaches the AIL app receiver.
    let (payload, what) = if service == 0x000a {
        (crate::libs::procnav::gps_wire_body(), "GpsDataStatus")
    } else {
        (
            crate::libs::procnav::posfi_nav_position_payload(seq),
            "NavPositionStatus",
        )
    };
    let mut body = vec![0u8; 0x20 + payload.len()];
    let total_len = body.len() as u32;
    body[0..2].copy_from_slice(&server_app.to_le_bytes());
    body[2..4].copy_from_slice(&client.to_le_bytes());
    body[4..8].copy_from_slice(&total_len.to_le_bytes());
    body[8..10].copy_from_slice(&2u16.to_le_bytes());
    body[0xb] = 0x45;
    // r796c: AIL_NO_UNIQUE_ID at +0x0e so `bIsDataSet` can claim the answer
    // (same reasoning as in `cca_server_answer_register`).  Only for this
    // responder's client (navsrv 0x0121), whose references keep 0xfffe; the
    // working 0x0027 answer to the HMI keeps zero.
    body[0x0e..0x10].copy_from_slice(&0xfffeu16.to_le_bytes());
    static MSG_ID: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0x5000);
    let msg_id = MSG_ID.fetch_add(1, Ordering::Relaxed) as u32;
    body[0x10..0x14].copy_from_slice(&msg_id.to_le_bytes());
    body[0x14..0x16].copy_from_slice(&service.to_le_bytes());
    body[0x16..0x18].copy_from_slice(&reg_id.to_le_bytes());
    // +0x18 word: result 0 (no error), +0x19 dynamic-length 0; the function
    // slot +0x0c stays 0 to match the reference (see comment above).
    body[0x1a] = 9; // OPTYPE: server data response
    body[0x20..].copy_from_slice(&payload);
    let mut blob = Vec::with_capacity(8 + body.len());
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
    blob.extend_from_slice(&body);
    let target = format!("mbx_{client}");
    let mut delivered = false;
    {
        let mut state = unicorn.get_data().namespace.lock().unwrap();
        if let Some(q) = state.mq.queue_id_for_name(&target) {
            state.mq.grow_msgsize(q, blob.len() as i64);
            delivered = OsalQueueService::guest_post(&mut state.mq, &target, blob, 0);
            if delivered {
                state.notify_waiters();
            }
        }
    }
    log::info!(
        "[LIBOSAL-MBX] sensorfi fake server {server_app:#06x}: answered ServiceData(svc {service:#06x} regid {reg_id:#04x} func {function:#06x}) from {client:#06x} on {name} with {what} (delivered {delivered} -> {target})"
    );
}

fn bridge_guest_osal_queue(
    unicorn: &mut Unicorn<'_, Context>,
    api_name: &str,
    base_address: u32,
    name: &str,
    msg_ptr: u32,
    msg_len: u32,
    prio_ptr: u32,
    timeout: u32,
) -> bool {
    if matches!(
        api_name,
        "OSAL_s32MessageQueueWait" | "OSAL_s32MessageQueuePriorityWait"
    ) {
        scd_reg_spool_flush(unicorn, name);
    }
    match api_name {
        "OSAL_s32MessageQueuePost" => {
            if !OsalQueueService::is_rtos_intercepted_queue(name) {
                return false;
            }

            let data = match read_guest_buffer(unicorn, msg_ptr, msg_len as usize) {
                Some(data) => data,
                None => return false,
            };

            // r757 diagnostic: name the CCA wire posts (msgid is what the
            // HSI layer later forwards as the FI event id).
            if data.len() >= 0x1c {
                log::info!(
                    "[LIBOSAL-MBX] {name} <- post tid=[{}] src={:#x} dst={:#x} class={:#x} msgid={:#x} svc={:#x} regid={:#x} func={:#x}",
                    unicorn.get_data().inner.thread_id(),
                    u16::from_le_bytes([data[0], data[1]]),
                    u16::from_le_bytes([data[2], data[3]]),
                    data[0xb],
                    u32::from_le_bytes([data[0x10], data[0x11], data[0x12], data[0x13]]),
                    u16::from_le_bytes([data[0x14], data[0x15]]),
                    u16::from_le_bytes([data[0x16], data[0x17]]),
                    u32::from_le_bytes([data[0x18], data[0x19], data[0x1a], data[0x1b]]),
                );
            }

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
                trace_fastmap_status_pop(unicorn, name, &message.data);
                if name == OSAL_CB_HDR_LI_MAIN
                    && callback_message_command(&message.data) == Some(OSAL_START_PROC_COMMAND)
                {
                    if let Some(path) = start_proc_path_from_callback(&message.data) {
                        if deliver_start_proc_callback(
                            unicorn,
                            base_address,
                            name,
                            path,
                            message.data.len() as u32,
                        ) {
                            return true;
                        }
                    }
                }

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

            log::info!(
                "[{}] [LIBOSAL] OSAL queue wait blocking name={} queue_id={}",
                unicorn.get_data().inner.thread_id(),
                name,
                queue_id
            );
            block_guest_osal_wait(
                unicorn,
                queue_id,
                msg_ptr,
                msg_len,
                prio_ptr,
                deadline_from_osal_timeout(timeout),
                false,
            );
            true
        }
        _ => false,
    }
}

/// Post/Wait bridge for the DAPI dynamic request/reply mailboxes `DYN<n>MBX`.
///
/// These are IOSC-class OSAL queues (`queue_type == 3`) that DAPI uses for its
/// internal request/response handoff (the CCA `DYN4MBX` a DAPI worker parks on
/// for a service answer). On the unit the IOSC layer is served by the Triton
/// monitor's OSAL-overlay object 0x64 via `svc #6`; the emulator serves 0x64 as
/// a catch-all, so a native `DYN*MBX` wait never receives anything - the waiter
/// times out and DAPI answers the outer client with `ServiceDataError 0x305`
/// (observed: 13 `DYN4MBX` waits, 0 deliveries, while 12 posts pile up).
///
/// Only the `DYN<n>MBX` mailboxes are bridged here - deliberately NOT the whole
/// `type == 3` class: the `Dp*` datapool and `NOIOSC*` queues are also IOSC but
/// are handled correctly by native `libosal` in-process, and intercepting those
/// stalls process bring-up. `DYN*MBX` traffic stays inside a single process
/// (DAPI's own threads), so the OSAL message-reference pointer remains valid in
/// that VM and the raw message bytes are queued and returned unchanged
/// (`materialize = false`), letting the guest resolve its own reference.
fn bridge_dyn_mbx_queue(
    unicorn: &mut Unicorn<'_, Context>,
    api_name: &str,
    name: &str,
    msg_ptr: u32,
    msg_len: u32,
    prio_ptr: u32,
    timeout: u32,
) -> bool {
    if !(name.starts_with("DYN") && name.ends_with("MBX")) {
        return false;
    }
    if !matches!(
        api_name,
        "OSAL_s32MessageQueuePost"
            | "OSAL_s32MessageQueueWait"
            | "OSAL_s32MessageQueuePriorityWait"
    ) {
        return false;
    }

    match api_name {
        "OSAL_s32MessageQueuePost" => {
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
                trace_fastmap_status_pop(unicorn, name, &message.data);
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
                false,
            );
            true
        }
        _ => false,
    }
}

/// Post/Wait bridge for the `mbx_<app_id>` application mailbox queues.
///
/// The 8-byte payload queued on an mbx is an OSAL message *reference*
/// `[type, content_ptr]` whose content pointer addresses the **sender's**
/// guest VM. Every process has its own Unicorn address space, so handing
/// that pointer to a cross-process recipient makes it dereference garbage
/// (observed: DAPIAPP's CCA dispatch tripped an assertion on procmap's
/// GetBlockIDs request and aborted its AE thread). We therefore snapshot
/// the content while the sender's VM is live at post time, and re-create
/// it in the recipient's own VM at delivery time, registering the new
/// pointer as a dynamic OSAL message so the eventual `OSAL_s32MessageDelete`
/// frees it.
///
/// Snapshot blob layout stored in the host queue:
/// `[u32 type_flag(1), u32 content_len, content bytes...]`.
/// Cache of hub (`mbx_0`) broadcasts destined for a recipient queue its
/// owner has not created yet (r755 fan-out).  Only the latest body per
/// (queue, src, class) is kept - the directory delivers current state, not
/// every historical re-broadcast.
static HUB_FANOUT_CACHE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<(&'static str, u16, u8), Vec<u8>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(Default::default()));

/// Recipient queue whose cache has already been drained.  The cache holds
/// last-state only, so one drain is enough; later broadcasts are posted
/// live by the POST path.
static HUB_FANOUT_FLUSHED: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<&'static str>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(Default::default()));

/// Deliver the cached hub broadcasts once the recipient is alive on its own
/// queue (first bridged Wait).  Must not run before the guest created the
/// queue: guest `mq_create` is O_EXCL and an EEXIST there aborts the HMI
/// boot (assert `osalmqueue.cpp:1181`, err 0xd02).
fn hub_fanout_flush(unicorn: &mut Unicorn<'_, Context>, queue: &str) {
    if queue != "mbx_265" {
        return;
    }
    let queue: &'static str = "mbx_265";
    let fresh = HUB_FANOUT_FLUSHED
        .lock()
        .map(|mut s| s.insert(queue))
        .unwrap_or(false);
    if !fresh {
        return;
    }
    let bodies: Vec<Vec<u8>> = HUB_FANOUT_CACHE
        .lock()
        .map(|c| {
            c.iter()
                .filter(|((q, _, _), _)| *q == queue)
                .map(|(_, v)| v.clone())
                .collect()
        })
        .unwrap_or_default();
    let mut posted = 0usize;
    for body in bodies {
        let mut blob = Vec::with_capacity(8 + body.len());
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
        blob.extend_from_slice(&body);
        let mut state = unicorn.get_data().namespace.lock().unwrap();
        let q = OsalQueueService::ensure_queue(&mut state.mq, queue);
        state.mq.grow_msgsize(q, blob.len() as i64);
        if OsalQueueService::guest_post(&mut state.mq, queue, blob, 0) {
            posted += 1;
            state.notify_waiters();
        }
    }
    log::info!("[LIBOSAL-MBX] hub fanout flush -> {queue}: {posted} bodies");
}

fn bridge_mbx_queue(
    unicorn: &mut Unicorn<'_, Context>,
    api_name: &str,
    base_address: u32,
    name: &str,
    msg_ptr: u32,
    msg_len: u32,
    prio_or_prio_ptr: u32,
    timeout: u32,
) -> bool {
    if !name.starts_with("mbx_") {
        return false;
    }
    if !matches!(
        api_name,
        "OSAL_s32MessageQueuePost"
            | "OSAL_s32MessageQueueWait"
            | "OSAL_s32MessageQueuePriorityWait"
    ) {
        return false;
    }

    if matches!(
        api_name,
        "OSAL_s32MessageQueueWait" | "OSAL_s32MessageQueuePriorityWait"
    ) {
        // r791l: this - not bridge_guest_osal_queue - is the path that
        // services the navif/SM pumps (mbx_<app> host queues), so the
        // cca-server registration spool must be flushed here too.
        scd_reg_spool_flush(unicorn, name);
    }
    match api_name {
        "OSAL_s32MessageQueuePost" => {
            if msg_ptr == 0 || msg_ptr > 0xf000_0000 || msg_len < 8 {
                return false;
            }
            let Some(content) = resolve_osal_ref_content(unicorn, base_address, msg_ptr) else {
                return false;
            };
            let Some(mut body) = snapshot_content_bytes(unicorn, content) else {
                return false;
            };
            // DAPIAPP's ServiceRegister handler initializes a new registry
            // entry's state to ACTIVE (0) only when the request's sourceSubID
            // (u32 @ +0xc) equals the "no sub-id" sentinel 0xFFFE; any other
            // value leaves the entry REGISTERED (1), which then rejects
            // data requests (opcode 2) with ServiceDataError 0xb
            // "temporarily unavailable". procmap's libosal leaves the field
            // uninitialized, so normalize it for traffic into DAPI's mailbox.
            if name == "mbx_7" && body.len() >= 0x10 && (body[0xb] == 0x41 || body[0xb] == 0x45) {
                body[0xc..0x10].copy_from_slice(&0x0000_fffeu32.to_le_bytes());
            }
            // Complete the service-registration handshake the way the unit's TE-side CCA
            // directory would: it owns the registration ids and stamps them into the
            // messages of a registered pair as they pass. That directory lives in the
            // encrypted region registries, so here every natural `RegisterConf` arrives
            // with register-id 0xffff ("nothing assigned"), the client's service handler
            // stays unregistered, and every later answer from the provider - which stamps
            // its own id - fails the receiver's registration match and is re-posted. Give
            // each (client queue, service) pair an id when its conf arrives and stamp that
            // id into every provider message delivered to the same pair afterwards.
            // r755: hub emulation, step 1.  On the unit the TE-side CCA hub
            // consumes mbx_0 (every app's `dst 0x0000` broadcast) and
            // re-delivers each body to every subscriber's own mbx_<appid>.
            // r755 analysis showed the class-0x40/0x49 traffic prochmi posts
            // is its own CCA power chatter (dispatch class 0x40 == power
            // message), and that path is already emulated by the PWR proxy,
            // so the fan-out stays off by default.  Kept behind
            // EMU_HUB_FANOUT=1 for experiments.
            if std::env::var_os("EMU_HUB_FANOUT").is_some()
                && name == "mbx_0"
                && body.len() >= 0x10
                && u16::from_le_bytes([body[2], body[3]]) == 0
                && matches!(body[0xb], 0x40 | 0x49)
                && u16::from_le_bytes([body[0], body[1]]) != 0x0109
            {
                let mut clone = body.clone();
                clone[2..4].copy_from_slice(&0x0109u16.to_le_bytes());
                let delivered = {
                    let mut state = unicorn.get_data().namespace.lock().unwrap();
                    match state.mq.queue_id_for_name("mbx_265") {
                        Some(q) => {
                            let mut blob = Vec::with_capacity(8 + clone.len());
                            blob.extend_from_slice(&1u32.to_le_bytes());
                            blob.extend_from_slice(&(clone.len() as u32).to_le_bytes());
                            blob.extend_from_slice(&clone);
                            state.mq.grow_msgsize(q, blob.len() as i64);
                            let ok =
                                OsalQueueService::guest_post(&mut state.mq, "mbx_265", blob, 0);
                            if ok {
                                state.notify_waiters();
                            }
                            ok
                        }
                        None => {
                            let src = u16::from_le_bytes([body[0], body[1]]);
                            if let Ok(mut c) = HUB_FANOUT_CACHE.lock() {
                                c.insert(("mbx_265", src, body[0xb]), clone);
                            }
                            false
                        }
                    }
                };
                static FANOUT_LOG: AtomicU32 = AtomicU32::new(0);
                if FANOUT_LOG.fetch_add(1, Ordering::Relaxed) < 24 {
                    log::info!(
                        "[LIBOSAL-MBX] hub fanout mbx_0->mbx_265 src={:#06x} class={:#04x} live={delivered}",
                        u16::from_le_bytes([body[0], body[1]]),
                        body[0xb],
                    );
                }
            }
            stamp_registration_id(name, &mut body);
            // r755: CCA service-server emulation for HMI (0x0109)
            // ServiceRegisters landing on server-app queues no Linux-side
            // process hosts (e.g. service 0x0024's mbx_36).  See
            // `cca_server_answer_register`.
            if body.len() >= 0x18 && body[0xb] == 0x41 {
                let client = u16::from_le_bytes([body[0], body[1]]);
                if name != "mbx_265" && name != "mbx_0" && name != &format!("mbx_{client}") {
                    if let Some(dec) = name.strip_prefix("mbx_") {
                        if let Ok(server) = dec.parse::<u32>() {
                            // r791: only answer for providers that are NOT
                            // among the emulated processes - a registration
                            // landing on PROCNAV (0x0001), DAPI (0x0007) or
                            // the HMI itself must be answered by that guest,
                            // never fabricated here (fidelity rule).
                            if (1..=0xffff).contains(&server)
                                && !crate::common::registry::HOSTED_SERVER_APPS
                                    .contains(&(server as u32))
                            {
                                cca_server_answer_register(
                                    unicorn,
                                    server as u16,
                                    client,
                                    u16::from_le_bytes([body[0x14], body[0x15]]),
                                );
                            }
                        }
                    }
                }
            }
            // r788: kernel-side SPM (Service Pool Manager, registry appid
            // 0x0000) answers its own service registration.  The HMI posts
            // ServiceRegister(svc 0x0100) to dst 0x0000 (mbx_0) and nothing
            // on the unit's Linux side ever answers - the SPM lives in the
            // Triton executive, which this emulator IS the Linux side of.
            // The answer content is the same RegisterConf+ServiceStatus
            // pair any server app sends (see cca_server_answer_register),
            // with src 0.  Behind EMU_FAKE_SPM_SERVER=1: a diagnostic run
            // without the flag must stay on the genuine no-answer path.
            if name == "mbx_0"
                && body.len() >= 0x18
                && body[0xb] == 0x41
                && u16::from_le_bytes([body[0], body[1]]) == 0x0109
                && u16::from_le_bytes([body[0x14], body[0x15]]) == 0x0100
                && std::env::var_os("EMU_FAKE_SPM_SERVER").is_some_and(|v| v == "1")
            {
                log::warn!(
                    "[LIBOSAL-MBX] EMU_FAKE_SPM_SERVER: answering ServiceRegister(svc 0x0100) as kernel SPM"
                );
                cca_server_answer_register(unicorn, 0x0000, 0x0109, 0x0100);
            }
            // r755: and the follow-up ServiceData requests for the
            // navigation service (wait-widget data), see
            // `cca_server_answer_data_request_if_unhosted`.
            if name != "mbx_265" && name != "mbx_0" {
                cca_server_answer_data_request_if_unhosted(unicorn, name, &body);
                cca_server_answer_sensorfi_data_request(unicorn, name, &body);
            }
            trace_dapi_map_service_data(name, &body);
            // r755-diag: every mbx_0 post body once per (src, class), so the
            // HMI's AIL traffic shape (svc 0x27 subscribe?) becomes visible.
            if name == "mbx_0" {
                static SEEN0: std::sync::LazyLock<
                    std::sync::Mutex<std::collections::HashSet<(u16, u8)>>,
                > = std::sync::LazyLock::new(|| std::sync::Mutex::new(Default::default()));
                if body.len() >= 0x14 {
                    let src = u16::from_le_bytes([body[0], body[1]]);
                    let fresh = SEEN0
                        .lock()
                        .map(|mut s| s.insert((src, body[0xb])))
                        .unwrap_or(false);
                    if fresh {
                        log::info!(
                            "[LIBOSAL-MBX] mbx_0 body src={src:#06x} class={:#04x}: {}",
                            body[0xb],
                            body.iter()
                                .take(0x28)
                                .map(|b| format!("{b:02x}"))
                                .collect::<Vec<_>>()
                                .join(" "),
                        );
                    }
                }
            }
            stamp_unique_id_for_receiver(name, &mut body);
            // The stamped RegisterConf (class 0x43) on the engine's mailbox
            // proves the (engine, svc 0x26) registration pair carries its
            // final id; that is the moment the missing CCA directory would
            // announce the service to the engine. Emit the publishes there,
            // not on the first answered GetBlockIDs: r608 showed the
            // answer-triggered pair consumed ~30 s after posting, its
            // `OnUpdateServiceStateDAPIFastmap` reset jobs landing exactly on
            // top of the accepted 34-block round. Triggering on the conf lets
            // those two resets land while the request is still young.
            // The class-0x45 leg stays as a fallback (once-guarded).
            if name == "mbx_1024"
                && body.len() >= 0x1c
                && matches!(body[0xb], 0x43 | 0x45)
                && u16::from_le_bytes([body[2], body[3]]) == 0x0400
                && u16::from_le_bytes([body[0x14], body[0x15]]) == 0x0026
                && (body[0xb] == 0x43 || body[0x1a] == 0x07)
            {
                post_engine_fastmap_service_status(
                    unicorn,
                    u16::from_le_bytes([body[0x16], body[0x17]]),
                );
            }
            if body.len() >= 0x18
                && matches!(u16::from_le_bytes([body[0], body[1]]), 0x0400 | 0x0109)
                && matches!(body[0xb], 0x41 | 0x42 | 0x44 | 0x45)
            {
                if matches!(body[0xb], 0x42 | 0x45) {
                    log::info!(
                        "[LIBOSAL-MBX] class{:#04x} body: {}",
                        body[0xb],
                        body.iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                }
                if body[0xb] == 0x42 {
                    // A registration tells us which handle procmapengine uses
                    // for this service; the conf may arrive noticeably later,
                    // while requests are already being built.
                    crate::libs::procmapengine::note_dapi_register_conf(
                        u16::from_le_bytes([body[0x14], body[0x15]]),
                        u16::from_le_bytes([body[0x16], body[0x17]]),
                    );
                }
                log::info!(
                    "[LIBOSAL-MBX] from-{:#06x} q={} dst={:#06x} class={:#04x} svc={:#06x} regid={:#06x} sub12={:#06x} sub14={:#06x}",
                    u16::from_le_bytes([body[0], body[1]]),
                    name,
                    u16::from_le_bytes([body[2], body[3]]),
                    body[0xb],
                    u16::from_le_bytes([body[0x14], body[0x15]]),
                    u16::from_le_bytes([body[0x16], body[0x17]]),
                    u16::from_le_bytes([body[0xc], body[0xd]]),
                    u16::from_le_bytes([body[0xe], body[0xf]]),
                );
            }
            let mut blob = Vec::with_capacity(8 + body.len());
            blob.extend_from_slice(&1u32.to_le_bytes());
            blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
            blob.extend_from_slice(&body);

            let accepted = {
                let mut state = unicorn.get_data().namespace.lock().unwrap();
                let queue_id = OsalQueueService::ensure_queue(&mut state.mq, name);
                state.mq.grow_msgsize(queue_id, blob.len() as i64);
                let accepted =
                    OsalQueueService::guest_post(&mut state.mq, name, blob, prio_or_prio_ptr);
                if accepted {
                    state.notify_waiters();
                }
                accepted
            };
            if accepted {
                // Ownership of an OSAL message transfers to the queue on a
                // successful Post; the recipient gets a materialized copy,
                // so the poster's buffer can be recycled right away. Without
                // this the sender's pool slots (and eventually fresh heap
                // sections) leak with every CCA message, tripping QEMU's
                // 4096-section assertion on chatty services like DAPIAPP's
                // periodic ServiceStatus publications.
                unicorn
                    .get_data()
                    .sys_calls_state
                    .lock()
                    .unwrap()
                    .osal_messages
                    .release(content);
                post_app_info_companion(unicorn, name, &body);
                return_to_caller(unicorn, 0);
            }
            accepted
        }
        "OSAL_s32MessageQueueWait" | "OSAL_s32MessageQueuePriorityWait" => {
            let buf = msg_ptr;
            let buf_len = msg_len;
            let prio_ptr = prio_or_prio_ptr;
            if buf == 0 || buf > 0xf000_0000 || buf_len < 8 {
                return false;
            }
            // r755: the recipient is alive on this queue now - deliver the
            // hub broadcasts that arrived before it created the queue.
            hub_fanout_flush(unicorn, name);
            // r755: and the CCA directory answers that spooled while the HMI
            // was still below AIL admin state RUNNING.  Reaching a bridged
            // Wait on the pump thread implies every previously injected power
            // message (incl. the NORMAL PWR_STATE_CHANGE_REQ) has already
            // been dispatched, so a class-0x50 delivered now is not dropped.
            if name == "mbx_265" && pwr_proxy::app_state_change_delivered(0x0109) {
                dir_reply_flush(unicorn);
            }
            // Keep one consumer per queue active at a time so a handler cannot
            // overtake the message that was posted before it (see MBX_IN_SERVICE).
            let servicer = unicorn.get_data().inner.thread_id();
            let served_by_other = queue_busy_for_other(name, servicer);
            let (queue_id, message) = {
                let mut state = unicorn.get_data().namespace.lock().unwrap();
                let queue_id = OsalQueueService::ensure_queue(&mut state.mq, name);
                let message = if served_by_other {
                    None
                } else {
                    // The map engine livelock: an error response whose
                    // register-id is 0xfffe matches no client handler, so
                    // `bDispatchCCAMessages` re-posts it to mbx_1024 forever
                    // (subtype-7 re-posts carry sub-id 0x0007, class 0x45 and
                    // reg-id 0xfffe at content +0x00/+0x0a/+0x0e).  On the unit
                    // such a message is routed and consumed; here the
                    // registration handshake that would hand out a routable id
                    // never completes, so the re-post is pure livelock and it
                    // also starves the queue for every other message.  Drop it
                    // at delivery instead of handing it back to the pump.
                    let mut message =
                        OsalQueueService::pop_guest_message(&mut state.mq, queue_id, usize::MAX);
                    if name == "mbx_1024" {
                        let mut dropped = 0usize;
                        while let Some(blob) = message.as_ref().map(|m| &m.data) {
                            trace_fastmap_status_pop(unicorn, name, blob);
                            // content starts at blob[8]: sub-id @ +0x00,
                            // class @ +0x0b, register-id @ +0x0e, service
                            // @ +0x14, seq @ +0x1c (all relative to content).
                            // `reg-id == 0xfffe` alone must not drop the message: it is what
                            // every DAPI answer carries here (the engine's `vRegisterService`
                            // handshake never gets a routable id from the emulated SCD, so the
                            // result header is stamped `0xfffe` and `bDispatchCCAMessages`
                            // cannot match it to a client handler), and on the unit such a
                            // result is delivered and consumed anyway.  Stamping on it threw
                            // away real answers, including the GetBlockIDs results.  Bound the
                            // livelock by the *content* instead: an identical header reaching
                            // this queue repeatedly is the re-post loop the pump cannot
                            // resolve, distinct answers are all delivered.
                            let bounces = blob.len() >= 0x28
                                && u16::from_le_bytes([blob[8], blob[9]]) == 0x0007
                                && blob[0x13] == 0x45
                                && u16::from_le_bytes([blob[0x16], blob[0x17]]) == 0xfffe
                                && mbx1024_repeat_too_often(&blob[8..blob.len().min(0x30)]);
                            let posfi = blob.len() >= 0x28
                                && blob[0x13] == 0x45
                                && u16::from_le_bytes([blob[0x1c], blob[0x1d]]) == 0x001e;
                            let posfi_repeat = posfi && {
                                let seq =
                                    u16::from_le_bytes([blob[0x24], blob[0x25]]) as u32;
                                if POSFI_LAST_DELIVERED_SEQ.load(Ordering::Relaxed) != seq {
                                    POSFI_LAST_DELIVERED_SEQ.store(seq, Ordering::Relaxed);
                                    POSFI_SEQ_DELIVERIES.store(1, Ordering::Relaxed);
                                    false
                                } else {
                                    POSFI_SEQ_DELIVERIES.fetch_add(1, Ordering::Relaxed) >= 4
                                }
                            };
                            if !bounces && !posfi_repeat {
                                break;
                            }
                            drop(message);
                            dropped += 1;
                            message =
                                OsalQueueService::pop_guest_message(&mut state.mq, queue_id, usize::MAX);
                        }
                        if dropped > 0 {
                            static MBX1024_LIVELOCK_DROPS: std::sync::atomic::AtomicU32 =
                                std::sync::atomic::AtomicU32::new(0);                            let total = MBX1024_LIVELOCK_DROPS.fetch_add(
                                dropped as u32,
                                Ordering::Relaxed,
                            );
                            if total < 8 || total % 500 == 0 {
                                log::info!(
                                    "[LIBOSAL-MBX] mbx_1024 dropped {} unroutable reg-id 0xfffe error re-posts (total {})",
                                    dropped,
                                    total + dropped as u32
                                );
                            }
                        }
                    }
                    message
                };
                if message.is_some() {
                    state.notify_waiters();
                }
                (queue_id, message)
            };
            if message.is_some() {
                queue_serviced(name, servicer);
            }

            if let Some(message) = message {
                let Some(result) =
                    deliver_snapshot_message(unicorn, &message.data, buf, prio_ptr, message.priority)
                else {
                    log::warn!(
                        "[LIBOSAL-MBX] failed to materialize snapshot for Wait({})",
                        name
                    );
                    return false;
                };
                let thread = unicorn.get_data().inner.thread_id();
                let (cls, svc) = message
                    .data
                    .get(8..)
                    .filter(|b| b.len() >= 0x16)
                    .map(|b| (b[0xb], u16::from_le_bytes([b[0x14], b[0x15]])))
                    .unwrap_or((0xff, 0));
                log::info!(
                    "[{}] [LIBOSAL-MBX] Wait({}) delivered materialized snapshot blob_len={} class={cls:#04x} svc={svc:#06x}",
                    thread,
                    name,
                    message.data.len()
                );
                if name == "mbx_1024" {
                    let dump: Vec<String> = message
                        .data
                        .iter()
                        .take(0x2c)
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    log::info!("[{}] [LIBOSAL-MBX] mbx_1024 blob: {}", thread, dump.join(" "));
                    // The blob carries an 8-byte OSAL prefix before the DAPI
                    // message body. Record the register-id of a
                    // ServiceRegister conf so procmapengine's emulated client
                    // reference can stamp it into later requests.
                    if message.data.len() >= 0x20 && message.data[0x13] == 0x43 {
                        let service = u16::from_le_bytes([message.data[0x1c], message.data[0x1d]]);
                        let register_id =
                            u16::from_le_bytes([message.data[0x1e], message.data[0x1f]]);
                        crate::libs::procmapengine::note_dapi_register_conf(service, register_id);
                    }
                }
                let _ = result;
                return true;
            }

            if timeout == 0 {
                // prochmi's AIL run-env mailbox loop (caller prochmi_out.out
                // +0xfe0990, the wait right after `Activate(AE_109)`) passes
                // timeout 0 and expects an indefinite wait on its bring-up
                // mailbox.  The empty-success shortcut makes the run-env treat
                // the untouched 8-byte buffer as a torn-down command: main
                // closes mbx_265 and exit(1)s at t~56 s, which orphans the GUI
                // worker threads - the render/swap thread [51] dies mid-frame
                // and the window freezes.  In the golden world main parks
                // around this point and never returns; park here until a real
                // message lands on the queue (the scheduler poll releases it).
                let this_thread = unicorn.get_data().inner.thread_id();
                let lr = unicorn.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                let elf = &unicorn.get_data().elf_path;
                let run_env_wait = elf.contains("prochmi_out.out")
                    && base_address != 0
                    && (lr & !1).wrapping_sub(base_address) == 0xfe0990;
                if run_env_wait {
                    log::info!(
                        "[{this_thread}] [LIBOSAL-MBX] run-env wait on empty {name}: parking instead of empty-success"
                    );
                    block_guest_osal_wait(unicorn, queue_id, buf, buf_len, prio_ptr, None, true);
                    return true;
                }
                return_to_caller(unicorn, 0);
                return true;
            }

            let thread = unicorn.get_data().inner.thread_id();
            log::info!(
                "[{}] [LIBOSAL-MBX] mbx wait blocking name={}",
                thread,
                name
            );
            block_guest_osal_wait(
                unicorn,
                queue_id,
                buf,
                buf_len,
                prio_ptr,
                deadline_from_osal_timeout(timeout),
                true,
            );
            true
        }
        _ => false,
    }
}

/// The CCA directory that normally answers `bRegisterAsync`'s
/// ApplicationInfoRegister probe lives on the TE side and never replies
/// here, so a client's registration request is deferred forever. Synthe-
/// size the ApplicationInfoStatus the directory would have broadcast for
/// the server app: the client's handler then marks the server known, runs
/// the deferred registration and fires the client-state observer which
/// triggers the natural re-registration. Emitted once per (server, target)
/// pair via the caller-suppressed guard.
pub(crate) fn post_app_info_status_to(
    unicorn: &mut Unicorn<'_, Context>,
    server_app: u32,
    client_app: u32,
    once: &std::sync::atomic::AtomicBool,
) -> bool {
    if server_app == 0 || server_app == 0xffff {
        return false;
    }
    if once.swap(true, Ordering::Relaxed) {
        return false;
    }

    let queue_name = format!("mbx_{}", client_app);
    let mut info = vec![0u8; 0x20];
    let info_len = info.len() as u32;
    info[0..2].copy_from_slice(&(server_app as u16).to_le_bytes());
    info[2..4].copy_from_slice(&(client_app as u16).to_le_bytes());
    info[4..8].copy_from_slice(&info_len.to_le_bytes());
    info[8..10].copy_from_slice(&2u16.to_le_bytes());
    info[0xb] = 0x50;
    info[0x14..0x16].copy_from_slice(&(server_app as u16).to_le_bytes());
    // ail ApplicationInfoStatus states: 0=UNAVAILABLE 1=AVAILABLE
    // 2=UNKNOWN 3=ERROR 4=DOES_NOT_EXIST. Only 1 flips the deferred
    // registration entry to AVAILABLE and runs bRegisterAsyncExecute.
    info[0x16] = 1;

    let mut blob = Vec::with_capacity(8 + info.len());
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&(info.len() as u32).to_le_bytes());
    blob.extend_from_slice(&info);
    let accepted = {
        let mut state = unicorn.get_data().namespace.lock().unwrap();
        let queue_id = OsalQueueService::ensure_queue(&mut state.mq, &queue_name);
        state.mq.grow_msgsize(queue_id, blob.len() as i64);
        let accepted = OsalQueueService::guest_post(&mut state.mq, &queue_name, blob, 0);
        if accepted {
            state.notify_waiters();
        }
        accepted
    };
    log::info!(
        "[LIBOSAL-MBX] synthesized ApplicationInfoStatus(app 0x{:04x}, state 1) into {}: {}",
        server_app,
        queue_name,
        accepted
    );
    accepted
}

pub(crate) fn post_app_info_status(unicorn: &mut Unicorn<'_, Context>, server_app: u32) -> bool {
    use std::sync::atomic::AtomicBool;
    static APP_INFO_SENT: AtomicBool = AtomicBool::new(false);
    // A client that has to be told "the server application is running" before it
    // dares to register is us feeding its own state machine. EMU_NO_APP_INFO_SYNTH
    // suppresses it to see how much of the CCA trouble is self-inflicted.
    if std::env::var_os("EMU_NO_APP_INFO_SYNTH").is_some() {
        return false;
    }
    // procmap (app 0x400) is the intended client for this emission.
    post_app_info_status_to(unicorn, server_app, 0x400, &APP_INFO_SENT)
}

/// Companion trigger: a client that actually posts a ServiceRegister
/// (wire class 0x42) needs the directory's ApplicationInfoStatus for the
/// server app to complete the deferred registration.
fn post_app_info_companion(unicorn: &mut Unicorn<'_, Context>, queue_name: &str, body: &[u8]) {
    if queue_name == "mbx_0" || body.len() < 0x14 {
        return;
    }
    if body[0xb] != 0x42 {
        return;
    }
    let server_app = u16::from_le_bytes([body[2], body[3]]) as u32;
    post_app_info_status(unicorn, server_app);
}

/// DAPIAPP only publishes ServiceStatus(svc 0x26, regid 1, state 1)
/// "registered but not available" because its map medium (CRYPTNAV via
/// DAPDEVM) never comes up in the emulator. Its ServiceStatus consumer
/// copies the message's state byte into every registry entry with a
/// matching register-id, and the ServiceData gate rejects GetBlockIDs
/// while that entry is not ACTIVE (0). Synthesize the AVAILABLE status the
/// unit would emit once the medium is present, delivered to DAPI's own
/// mailbox so the client entry flips to 0. Emitted once.
pub(crate) fn post_dapi_status_available(unicorn: &mut Unicorn<'_, Context>) -> bool {
    use std::sync::atomic::AtomicBool;
    static STATUS_SENT: AtomicBool = AtomicBool::new(false);
    if STATUS_SENT.swap(true, Ordering::Relaxed) {
        return false;
    }

    let mut status = vec![0u8; 0x20];
    let status_len = status.len() as u32;
    status[0..2].copy_from_slice(&7u16.to_le_bytes());
    status[2..4].copy_from_slice(&7u16.to_le_bytes());
    status[4..8].copy_from_slice(&status_len.to_le_bytes());
    status[8..10].copy_from_slice(&2u16.to_le_bytes());
    status[0xb] = 0x44; // wire class: ServiceStatus
    status[0x14..0x16].copy_from_slice(&0x0026u16.to_le_bytes());
    status[0x16..0x18].copy_from_slice(&1u16.to_le_bytes()); // register-id from the conf
    status[0x18] = 0; // state: ACTIVE

    let mut blob = Vec::with_capacity(8 + status.len());
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&(status.len() as u32).to_le_bytes());
    blob.extend_from_slice(&status);
    let accepted = {
        let mut state = unicorn.get_data().namespace.lock().unwrap();
        let queue_id = OsalQueueService::ensure_queue(&mut state.mq, "mbx_7");
        state.mq.grow_msgsize(queue_id, blob.len() as i64);
        let accepted = OsalQueueService::guest_post(&mut state.mq, "mbx_7", blob, 0);
        if accepted {
            state.notify_waiters();
        }
        accepted
    };
    log::info!(
        "[LIBOSAL-MBX] synthesized ServiceStatus(svc 0x26, regid 1, state 0) into mbx_7: {}",
        accepted
    );
    accepted
}

/// The CCA directory on a live unit tells every *client* that a service is
/// available: when a registration is accepted it publishes ServiceStatus
/// (class 0x44) with state 1 to the client's mailbox. procmapengine's
/// `ail_bExamineInfluenceToAdministration` case 2 copies that state into the
/// service reference and - only for state 1/2 - marks the service as
/// influencing availability, which is what later runs
/// `vServiceAvailabilityChanged` -> `PortControl_CCAClientHandlerBase::
/// vOnServiceState` -> the FastMap property subscription that opens the
/// FastMap channel (CCA fn 0x101) and unblocks `LockFreeFastMapChannel` for
/// `DAPI_LoadBlocks`. DAPI itself only ever emits state 0 ("no state"), which
/// the client dispatch ignores outright, so with no directory in the
/// emulator the whole chain sleeps forever.
///
/// Emit that publish once the engine has received its first answered
/// GetBlockIDs (i.e. the (engine, svc 0x26) registration is fully stamped),
/// with state 1 - the value that flips the client reference, propagates
/// availability through `PortControl_CCAClientHandlerBase::vOnServiceState`,
/// and keeps `OnUpdateServiceStateDAPIFastmap` opening the channels.
pub(crate) fn post_engine_fastmap_service_status(
    unicorn: &mut Unicorn<'_, Context>,
    register_id: u16,
) -> bool {
    use std::sync::atomic::AtomicBool;
    static ENGINE_STATUS_SENT: AtomicBool = AtomicBool::new(false);
    if ENGINE_STATUS_SENT.swap(true, Ordering::Relaxed) {
        return false;
    }
    // r575/r576 semantics, from the engine's own getters:
    // `map_tclIClientServiceHandler::bIsServiceAvailable` (0x5902f0) is true
    // ONLY while the state byte at handler+0xe is 0, and the base
    // `map_tclClientPropertyRegHandler::vServiceStateChanged` (0x5931e0)
    // (re-)subscribes the property (vtable+0x10 -> vUpRegOrGet -> UpReg on the
    // wire) only on state 0 - every other value returns without sending
    // anything. r576 confirmed the notification carries the raw wire byte:
    // publishing 1 flipped the handler 0 -> 1, which made
    // bIsServiceAvailable() false and the MDM job close fastmap again. The
    // synthetic client reference is created with state 0 already, so a lone
    // 0-publish would be a no-change no-notification. Publish 1 first
    // (harmless: the MDM job it triggers just closes an already-closed
    // fastmap), then the real state 0 - that second transition notifies
    // `vOnServiceStateChange(0)`, sets handler+0xe = 0 (available) and fires
    // the property 0x401 UpReg toward DAPI.
    let first = post_engine_status_to(unicorn, 0x0026, register_id, 1);
    let accepted = post_engine_status_to(unicorn, 0x0026, register_id, 0);
    log::info!(
        "[LIBOSAL-MBX] synthesized directory ServiceStatus(svc 0x26 regid {:#x}, state 1 then 0) into mbx_1024: {} {}",
        register_id,
        first,
        accepted
    );
    accepted
}

/// One ServiceStatus (class 0x44) publish from DAPI (app 7) into the engine's
/// mailbox (`mbx_1024`, app id 0x400). Field placement follows
/// `amt_tclServiceStatus::amt_tclServiceStatus`'s serializer: length 0x1a at
/// +0x04, opcode 2 at +0x08, class at +0x0b, service id +0x14, register id
/// +0x16 and the state byte at +0x18 (mirrors what DAPI's real publishes
/// carry, except theirs always hold state 0).
/// Builds one directory ServiceStatus publish body (no header) for the
/// engine's svc-0x26 reference; see the field map on `post_engine_status_to`.
fn engine_status_body(state: u8) -> Vec<u8> {
    let mut status = vec![0u8; 0x20];
    let status_len = status.len() as u32;
    status[0..2].copy_from_slice(&7u16.to_le_bytes());
    status[2..4].copy_from_slice(&0x0400u16.to_le_bytes());
    status[4..8].copy_from_slice(&status_len.to_le_bytes());
    status[8..10].copy_from_slice(&2u16.to_le_bytes());
    status[0xb] = 0x44;
    // +0x0e is the sub-id the client walk (`bIsDataSet` at 0x66e3d0) matches
    // against the reference's unique id with 0xffff as the wildcard. The
    // engine's svc-0x26 reference stores unique 0, so use the wildcard: 0xfffe
    // ("none") matches nothing and the whole apply-then-propagate flow is
    // skipped with a "no reference" echo instead.
    status[0x0e..0x10].copy_from_slice(&0xffffu16.to_le_bytes());
    status[0x14..0x16].copy_from_slice(&0x0026u16.to_le_bytes());
    // Registration id: wildcard 0xffff.  `bIsDataSet` (0x66e3d0) only lets a
    // status message through when every wire id is 0xffff or equals the
    // reference field, and the engine's real svc-0x26 reference stores the
    // async-register transaction id 0xe0 (see the r1=0xe0 of
    // vHandleAsyncRegisterConf), not 1 - with reg-id 1 the pair never matched
    // and `ail_bExamineInfluenceToAdministration` consumed it without
    // setting `ref+6` (r643: zero `vSetServiceState` hits, stall).  The
    // wildcard matches, administration applies the state, the 0-return
    // forwards the pair to MQB_1024 and the body thread runs the client
    // walk (`vServiceStateChanged`).
    status[0x16..0x18].copy_from_slice(&0xffffu16.to_le_bytes());
    status[0x18] = state;
    status
}

fn wrap_status_blob(status: Vec<u8>) -> Vec<u8> {
    let mut blob = Vec::with_capacity(8 + status.len());
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&(status.len() as u32).to_le_bytes());
    blob.extend_from_slice(&status);
    blob
}

/// Host-thread repost variant: r610 showed the conf-triggered pair popped from
/// `mbx_1024` by an engine consumer that does not run the administration walk
/// (the state-change fan-in never fired), while r608 popped it in the right
/// context. Whether a pop applies is consumer-thread luck, so re-post the pair
/// from the host until the engine probe reports it was applied.
pub(crate) fn post_engine_fastmap_status_pair(
    namespace: &std::sync::Arc<std::sync::Mutex<crate::os::syscalls::namespace::SystemNamespace>>,
) -> (bool, bool) {
    let mut posted = [false; 2];
    for (slot, state) in posted.iter_mut().zip([1u8, 0]) {
        let blob = wrap_status_blob(engine_status_body(state));
        let mut guard = namespace.lock().unwrap();
        let queue_id = OsalQueueService::ensure_queue(&mut guard.mq, "mbx_1024");
        guard.mq.grow_msgsize(queue_id, blob.len() as i64);
        let accepted = OsalQueueService::guest_post(&mut guard.mq, "mbx_1024", blob, 0);
        if accepted {
            guard.notify_waiters();
        }
        *slot = accepted;
    }
    (posted[0], posted[1])
}

fn post_engine_status_to(
    unicorn: &mut Unicorn<'_, Context>,
    service_id: u16,
    register_id: u16,
    state: u8,
) -> bool {
    debug_assert_eq!(service_id, 0x0026);
    let mut status = engine_status_body(state);
    status[0x16..0x18].copy_from_slice(&register_id.to_le_bytes());
    let blob = wrap_status_blob(status);
    let mut state_guard = unicorn.get_data().namespace.lock().unwrap();
    let queue_id = OsalQueueService::ensure_queue(&mut state_guard.mq, "mbx_1024");
    state_guard.mq.grow_msgsize(queue_id, blob.len() as i64);
    let accepted = OsalQueueService::guest_post(&mut state_guard.mq, "mbx_1024", blob, 0);
    if accepted {
        state_guard.notify_waiters();
    }
    accepted
}

/// Resolve the content pointer of an 8-byte OSAL message reference while
/// the *poster's* VM is accessible. Type 1 references are direct pointers;
/// type 2 references index the poster's message pool (same resolution the
/// logging helper performs).
fn resolve_osal_ref_content(
    unicorn: &Unicorn<'_, Context>,
    base_address: u32,
    ref_addr: u32,
) -> Option<u32> {
    let raw_type = read_u32_or_invalid(unicorn, ref_addr);
    let raw_handle = read_u32_or_invalid(unicorn, ref_addr + 4);
    match raw_type & 0xff {
        1 if raw_handle != 0 && raw_handle < 0xf000_0000 => Some(raw_handle),
        2 => {
            const MSG_POOL_CONTENT_BASE: u32 = 0x4856_f800;
            const MSG_POOL_BLOCK_BASE: u32 = 0x4856_f7f4;
            const MSG_POOL_ENTRY_SIZE: u32 = 12;
            let block_base =
                read_u32_or_invalid(unicorn, base_address + (MSG_POOL_BLOCK_BASE - ORIGINAL_BASE));
            let content_base = read_u32_or_invalid(
                unicorn,
                base_address + (MSG_POOL_CONTENT_BASE - ORIGINAL_BASE),
            );
            let valid_base =
                |addr: u32| addr != 0 && addr != 0xffff_ffff && addr < 0xf000_0000;
            for base in [content_base, block_base] {
                if !valid_base(base) {
                    continue;
                }
                let content =
                    base.wrapping_add((raw_handle.wrapping_add(1) * MSG_POOL_ENTRY_SIZE) + 0x18);
                if content < 0xf000_0000 {
                    return Some(content);
                }
            }
            None
        }
        _ => None,
    }
}

/// Snapshot the referenced content out of the sender's VM. The shared
/// amt/ail message header stores its total length as a u32 at offset 4;
/// anything implausible falls back to a fixed conservative window.
fn snapshot_content_bytes(
    unicorn: &Unicorn<'_, Context>,
    content: u32,
) -> Option<Vec<u8>> {
    const FALLBACK_LEN: usize = 0x40;
    const MAX_LEN: usize = 0x2_0000;
    let header_len = read_u32_or_invalid(unicorn, content + 4);
    let len = if (8..=MAX_LEN as u32).contains(&header_len) {
        header_len as usize
    } else {
        FALLBACK_LEN
    };
    read_guest_buffer(unicorn, content, len).or_else(|| {
        read_guest_buffer(unicorn, content, FALLBACK_LEN)
    })
}

/// Write a snapshot blob's content into the recipient's own VM and hand it
/// an 8-byte direct-reference. Returns the Wait return value (8 bytes).
pub(crate) fn deliver_snapshot_message(
    unicorn: &mut Unicorn<'_, Context>,
    blob: &[u8],
    buf: u32,
    prio_ptr: u32,
    priority: u32,
) -> Option<u32> {
    let count = materialize_snapshot(unicorn, blob, buf, prio_ptr, priority)?;
    return_to_caller(unicorn, count);
    Some(count)
}

/// The register-free half of `deliver_snapshot_message`: materialize the snapshotted
/// content in this guest's VM and leave the 8-byte reference at `buf`. The Triton monitor
/// path delivers through its own completion machinery, so it uses this directly.
pub(crate) fn materialize_snapshot(
    unicorn: &mut Unicorn<'_, Context>,
    blob: &[u8],
    buf: u32,
    prio_ptr: u32,
    priority: u32,
) -> Option<u32> {
    if blob.len() < 8 {
        return None;
    }
    let content_len = u32::from_le_bytes([blob[4], blob[5], blob[6], blob[7]]) as usize;
    let body = blob.get(8..8 + content_len)?;

    let size = (content_len as u32).max(8);
    let content = {
        let state_arc = unicorn.get_data().sys_calls_state.clone();
        let mut state = state_arc.lock().unwrap();
        match state.osal_messages.take_freed(size) {
            Some(content) => content,
            None => {
                drop(state);
                let mmu_arc = unicorn.get_data().mmu.clone();
                let content = mmu_arc.lock().unwrap().heap_alloc(
                    unicorn,
                    size,
                    Prot::READ | Prot::WRITE,
                    "[mbx-snapshot]",
                );
                unicorn
                    .get_data()
                    .sys_calls_state
                    .lock()
                    .unwrap()
                    .osal_messages
                    .mark_dynamic(content, size);
                content
            }
        }
    };
    if content == 0 || content > 0xf000_0000 {
        return None;
    }
    unicorn.mem_write(content as u64, body).ok()?;
    unicorn.mem_write(buf as u64, &1u32.to_le_bytes()).ok()?;
    unicorn
        .mem_write((buf + 4) as u64, &content.to_le_bytes())
        .ok()?;
    if prio_ptr != 0 {
        unicorn
            .mem_write(prio_ptr as u64, &pack_u32(priority))
            .ok()?;
    }
    Some(8)
}

fn start_proc_path_from_callback(data: &[u8]) -> Option<&str> {
    let path_bytes = data.get(3..)?;
    let path_bytes = match path_bytes.iter().position(|b| *b == 0) {
        Some(end) => &path_bytes[..end],
        None => path_bytes,
    };
    if path_bytes.is_empty() {
        return None;
    }
    std::str::from_utf8(path_bytes).ok()
}

fn deliver_start_proc_callback(
    unicorn: &mut Unicorn<'_, Context>,
    base_address: u32,
    queue_name: &str,
    path: &str,
    return_value: u32,
) -> bool {
    if path.len() >= START_PROC_BUFFER_SIZE as usize {
        log::warn!(
            "[LIBOSAL] start-proc path is too long for synthesized OSAL callback delivery: {}",
            path
        );
        return false;
    }

    let Some(path_ptr) = allocate_guest_cstr(unicorn, path) else {
        log::warn!(
            "[LIBOSAL] failed to allocate guest path buffer for start-proc callback path={}",
            path
        );
        return false;
    };
    let Some(stub) = allocate_guest_call_stub(unicorn) else {
        log::warn!(
            "[LIBOSAL] failed to allocate ARM pop{{r0-r3,lr,pc}} stub for start-proc callback path={}",
            path
        );
        return false;
    };

    let sp = unicorn.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
    let lr = unicorn.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
    if sp < 0x1000 || lr == 0 || lr > 0xf000_0000 {
        log::warn!(
            "[LIBOSAL] refusing synthesized start-proc callback from SP={:#x} LR={:#x}",
            sp,
            lr
        );
        return false;
    }

    let new_sp = sp.wrapping_sub(24);
    let stack = [return_value, 0, 0, 0, lr, lr];
    if !stack
        .iter()
        .enumerate()
        .all(|(index, value)| write_u32(unicorn, new_sp + (index as u32 * 4), *value))
    {
        log::warn!(
            "[LIBOSAL] failed to prepare guest stack for start-proc callback at SP={:#x}",
            new_sp
        );
        return false;
    }

    unicorn
        .reg_write(RegisterARM::SP, new_sp as u64)
        .unwrap_or_default();
    for (register, value) in [
        (RegisterARM::R0, path_ptr),
        (RegisterARM::R1, START_PROC_OPTION),
        (RegisterARM::R2, 0),
        (RegisterARM::R3, 0),
        (RegisterARM::LR, stub),
        (RegisterARM::PC, base_address + V_START_PROC),
    ] {
        if unicorn.reg_write(register, value as u64).is_err() {
            log::warn!("[LIBOSAL] failed to set registers for synthesized start-proc callback");
            return false;
        }
    }

    let thread = unicorn.get_data().inner.thread_id();
    log::info!(
        "[{}] [LIBOSAL] {} delivered start-proc callback path={} via vStartProc(path, {})",
        thread,
        queue_name,
        path,
        START_PROC_OPTION
    );
    true
}

fn allocate_guest_cstr(unicorn: &mut Unicorn<'_, Context>, value: &str) -> Option<u32> {
    let bytes = value.as_bytes();
    if bytes.len() + 1 > START_PROC_BUFFER_SIZE as usize {
        return None;
    }

    let mmu_arc = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    let addr = mmu_arc.lock().unwrap().heap_alloc(
        unicorn,
        START_PROC_BUFFER_SIZE,
        Prot::READ | Prot::WRITE,
        "[libosal-start-proc-path]",
    );
    if addr == 0 {
        return None;
    }

    let mut buf = vec![0u8; START_PROC_BUFFER_SIZE as usize];
    buf[..bytes.len()].copy_from_slice(bytes);
    unicorn.mem_write(addr as u64, &buf).ok()?;
    Some(addr)
}

fn allocate_guest_call_stub(unicorn: &mut Unicorn<'_, Context>) -> Option<u32> {
    let mmu_arc = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    let addr = mmu_arc.lock().unwrap().heap_alloc(
        unicorn,
        GUEST_CALL_STUB_SIZE,
        Prot::READ | Prot::WRITE | Prot::EXEC,
        "[libosal-start-proc-stub]",
    );
    if addr == 0 {
        return None;
    }

    unicorn
        .mem_write(addr as u64, &[0x0f, 0xc0, 0xbd, 0xe8])
        .ok()?;
    Some(addr)
}

fn write_u32(unicorn: &mut Unicorn<'_, Context>, address: u32, value: u32) -> bool {
    unicorn
        .mem_write(address as u64, &value.to_le_bytes())
        .is_ok()
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
    materialize: bool,
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
                materialize,
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
