//! Read-only observation hooks for PROCNAV.OUT's amt message dispatch.
//!
//! PROCNAV is statically linked, so none of the libosal library-level traces
//! cover its internal message routing. These hooks only *read* guest memory at
//! known decode points (see docs/reverse_engineering/procnav_gps_transport.md)
//! and log what each accepted message looks like, so the wire geometry the
//! dispatcher expects can be confirmed against real traffic instead of guessed
//! from static disassembly:
//!
//! * the `rtf_tclPropertyDispatcher::s32ProcessMessage` loop compares every
//!   handler entry against `[[msg+4]+0x14]` (serviceId u16) and
//!   `[[msg+4]+0x18]` (propertyId u16),
//! * `pc_tclSensorMsgHandler::s32OnStatusMsg`'s GPS sink (ARM code!) is the
//!   end of the GPS-data path: reaching it proves an injected GPS message was
//!   deserialized and dispatched.
//!
//! The hook at the pump's field helper (`0x258b6a`) shows the reference-
//! resolution result even for messages that never reach the dispatcher.

use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

const ORIGINAL_BASE: u32 = 0x0000_8000;
/// `rtf_tclPropertyDispatcher::s32ProcessMessage(amt_tclServiceData&)` (THUMB).
const PROPERTY_DISPATCHER: u32 = 0x002e_5526 - ORIGINAL_BASE;
/// `rtf2::tclMultiThreadAppThreadContext::bDispatchServiceDataMsg(amt_tclServiceData&)`.
const DISPATCH_SERVICE_DATA_MSG: u32 = 0x0005_ac30 - ORIGINAL_BASE;
/// `rtf2::tclThreadContext::vDispatchServiceDataMsg(amt_tclServiceData&)`.
const VDISPATCH_SERVICE_DATA_MSG: u32 = 0x0011_bd60 - ORIGINAL_BASE;
/// `pc_tclJobManager::s32ProcessServerAnswer(amt_tclServiceData&)`.
const PROCESS_SERVER_ANSWER: u32 = 0x002c_f240 - ORIGINAL_BASE;
/// `pc_tclThreadDispatchJobs::poConvertFIMessage(amt_tclServiceData&, u16)`.
const CONVERT_FI_MESSAGE: u32 = 0x001b_f600 - ORIGINAL_BASE;
/// The AE task's message handler entry (`vMsgHandler`).
const VMSG_HANDLER: u32 = 0x005e_8d98 - ORIGINAL_BASE;
/// `ail_tclInternalDispatch::ail_bHandleMsgServiceData` (class 0x45 handler).
const HANDLE_SERVICE_DATA: u32 = 0x00bd_50c - ORIGINAL_BASE;
/// `ail_tclAppInterfaceRestricted::bPostIpcMessage` (the class-0x45 re-post route).
const POST_IPC_MESSAGE: u32 = 0x0024_9614 - ORIGINAL_BASE;
/// `ail_tclInternalDispatch::bDispatchCCAMessages` - the AE loop's CCA router.
const CCA_DISPATCHER: u32 = 0x00be_396 - ORIGINAL_BASE;
/// `ail_tclAppInterfaceRestricted::bSendCCAPowerMsg` - builds the class-0x40
/// CCA power/state messages PROCNAV both sends and must receive to advance its
/// component state machine.
const SEND_CCA_POWER_MSG: u32 = 0x0024_9364 - ORIGINAL_BASE;
/// Field helper invoked by the AE pump after a queue accept (THUMB):
/// double-dereferences the OSAL reference-resolution answer.
const PUMP_FIELD_HELPER: u32 = 0x0025_8b6a - ORIGINAL_BASE;
/// Branch inside `ail_tclInternalDispatch::bDispatchCCAMessages` that hands a
/// class-0x40 power message to `ail_bHandleMsgPowerMessage` (taken only when the
/// dispatcher's third argument is -1 and the `[inner+0x6c]` flag is set).
const POWER_ON_INIT_BRANCH: u32 = 0x00be_6b6 - ORIGINAL_BASE;
/// Branch that re-posts an unprocessable power message (the retry spin).
const POWER_REPOST_BRANCH: u32 = 0x00be_6ca - ORIGINAL_BASE;
/// `STAT_tclApplication::bOnInit` - runs on the mode-3 (START_CONF) power message.
const STAT_ON_INIT: u32 = 0x0057_210 - ORIGINAL_BASE;
// Path probes through `bDispatchCCAMessages` (see `0xbe396`): the entry hook only
// proves the call, these mark which exit the message takes (trace branch, defer
// call, class switch, class-0x40 case, drop stub).
const PROBE_TRACE_EXIT: u32 = 0x00be_44c - ORIGINAL_BASE;
const PROBE_MID_FLOW: u32 = 0x00be_596 - ORIGINAL_BASE;
const PROBE_DEFER_CALL: u32 = 0x00be_5a0 - ORIGINAL_BASE;
const PROBE_SWITCH: u32 = 0x00be_5aa - ORIGINAL_BASE;
const PROBE_CLASS40: u32 = 0x00be_660 - ORIGINAL_BASE;
const PROBE_DROP_STUB: u32 = 0x00be_6b4 - ORIGINAL_BASE;
/// Inside `ail_bCheckExistenceRequirement` (`0xbe358`): right after the field getter
/// (`0xbe368`) - `r0` is the compared id; and right after the gate call at `0xbe68a`.
const PROBE_CHECK_GETTER: u32 = 0x00be_36e - ORIGINAL_BASE;
const PROBE_AFTER_GATE: u32 = 0x00be_68e - ORIGINAL_BASE;
/// The adapter's `vAppStateChanged` thunk (`vtable + 0x68`) invoked by the gate.
const PROBE_VT68: u32 = 0x0024_b786 - ORIGINAL_BASE;
/// After the `bOnInit` vtable call in the power handler (`0xbe088 blx`): `0xbe08a`
/// sees its return value, `0xbe094` stores the state-3 advance, `0xbe144` the failure.
const PROBE_INIT_RESULT: u32 = 0x00be_08a - ORIGINAL_BASE;
const PROBE_STATE_STORE: u32 = 0x00be_094 - ORIGINAL_BASE;
const PROBE_INIT_FAIL: u32 = 0x00be_144 - ORIGINAL_BASE;
/// GPS-data sink inside `pc_tclSensorMsgHandler::s32OnStatusMsg` (ARM).
const GPS_STATUS_SINK: u32 = 0x0068_ff8c - ORIGINAL_BASE;

fn word(unicorn: &mut Unicorn<'_, Context>, address: u32) -> Option<u32> {
    let mut bytes = [0u8; 4];
    unicorn
        .mem_read(address as u64, &mut bytes)
        .ok()
        .map(|_| u32::from_le_bytes(bytes))
}

fn halfword(unicorn: &mut Unicorn<'_, Context>, address: u32) -> Option<u16> {
    let mut bytes = [0u8; 2];
    unicorn
        .mem_read(address as u64, &mut bytes)
        .ok()
        .map(|_| u16::from_le_bytes(bytes))
}

fn read_byte(unicorn: &mut Unicorn<'_, Context>, address: u32) -> Option<u8> {
    let mut byte = [0u8; 1];
    unicorn.mem_read(address as u64, &mut byte).ok().map(|_| byte[0])
}

fn hexdump(unicorn: &mut Unicorn<'_, Context>, address: u32, len: usize) -> String {
    let mut bytes = vec![0u8; len];
    match unicorn.mem_read(address as u64, &mut bytes) {
        Ok(()) => bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" "),
        Err(_) => "<unmapped>".to_string(),
    }
}

pub fn procnav_add_code_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let repost_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    for (name, offset, size) in [
        ("dispatcher", PROPERTY_DISPATCHER, 2u64),
        ("field-helper", PUMP_FIELD_HELPER, 2),
        ("gps-sink", GPS_STATUS_SINK, 4),
        ("dispatch-msg", DISPATCH_SERVICE_DATA_MSG, 2),
        ("vdispatch-msg", VDISPATCH_SERVICE_DATA_MSG, 2),
        ("server-answer", PROCESS_SERVER_ANSWER, 2),
        ("convert-fi", CONVERT_FI_MESSAGE, 2),
        ("vmsghandler", VMSG_HANDLER, 2),
        ("svc-data", HANDLE_SERVICE_DATA, 2),
        ("post-ipc", POST_IPC_MESSAGE, 2),
        ("cca-router", CCA_DISPATCHER, 2),
        ("power-branch", POWER_ON_INIT_BRANCH, 2),
        ("power-repost", POWER_REPOST_BRANCH, 2),
        ("stat-oninit", STAT_ON_INIT, 2),
        ("probe-trace-exit", PROBE_TRACE_EXIT, 2),
        ("probe-mid-flow", PROBE_MID_FLOW, 2),
        ("probe-defer-call", PROBE_DEFER_CALL, 2),
        ("probe-switch", PROBE_SWITCH, 2),
        ("probe-class40", PROBE_CLASS40, 2),
        ("probe-drop-stub", PROBE_DROP_STUB, 2),
        ("probe-check-getter", PROBE_CHECK_GETTER, 2),
        ("probe-after-gate", PROBE_AFTER_GATE, 2),
        ("probe-vt68", PROBE_VT68, 2),
        ("probe-init-result", PROBE_INIT_RESULT, 2),
        ("probe-state-store", PROBE_STATE_STORE, 2),
        ("probe-init-fail", PROBE_INIT_FAIL, 2),
    ] {
        let address = (base_address + offset) as u64;
        let repost_count = repost_count.clone();
        let result = unicorn.add_code_hook(address, address + size, move |uc, _, _| {
            let tid = uc.get_data().inner.thread_id();
            if (name == "power-repost" || name.starts_with("probe-"))
                && repost_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 200
            {
                return; // dispatch retries would otherwise flood the log
            }
            match name {
                "dispatcher" => {
                    let table = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let msg = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let fields = word(uc, msg.wrapping_add(4));
                    let (svc, prop) = match fields {
                        Some(base) => (
                            halfword(uc, base.wrapping_add(0x14)),
                            halfword(uc, base.wrapping_add(0x18)),
                        ),
                        None => (None, None),
                    };
                    let svc_hex = svc.map(|v| format!("{v:#x}"));
                    let prop_hex = prop.map(|v| format!("{v:#x}"));
                    log::info!(
                        "PROCNAV[{tid}] dispatcher msg={msg:#010x} fields={fields:?} svc={svc_hex:?} prop={prop_hex:?} table={table:#010x} head=[{}]",
                        hexdump(uc, msg, 0x10),
                    );
                }
                "field-helper" => {
                    let data = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let inner = word(uc, data.wrapping_add(4));
                    log::info!(
                        "PROCNAV[{tid}] field helper data={data:#010x} +4={inner:?} head=[{}]",
                        hexdump(uc, data, 0x18),
                    );
                }
                "dispatch-msg" | "vdispatch-msg" | "server-answer" | "convert-fi"
                | "vmsghandler" => {
                    let obj = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let fields = word(uc, obj.wrapping_add(4));
                    let (svc, prop) = match fields {
                        Some(base) if base != 0 => (
                            halfword(uc, base.wrapping_add(0x14)),
                            halfword(uc, base.wrapping_add(0x18)),
                        ),
                        _ => (None, None),
                    };
                    let svc_hex = svc.map(|v| format!("{v:#x}"));
                    let prop_hex = prop.map(|v| format!("{v:#x}"));
                    log::info!(
                        "PROCNAV[{tid}] {name}: obj={obj:#010x} fields={fields:?} svc={svc_hex:?} prop={prop_hex:?} head=[{}]",
                        hexdump(uc, obj, 0x18),
                    );
                }
                "cca-router" => {
                    let adapter = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let msg = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let arg2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                    let inner = word(uc, adapter).unwrap_or(0);
                    let state = read_byte(uc, inner.wrapping_add(0x30));
                    let app = halfword(uc, inner.wrapping_add(0x10));
                    let vt = word(uc, inner).unwrap_or(0);
                    let gate = word(uc, vt.wrapping_add(0x30)).unwrap_or(0);
                    let defer_flag = read_byte(uc, inner.wrapping_add(0x6c));
                    let content = word(uc, msg.wrapping_add(4)).unwrap_or(0);
                    let class = read_byte(uc, content.wrapping_add(0xb));
                    let svc = halfword(uc, content.wrapping_add(0x14));
                    log::info!(
                        "PROCNAV[{tid}] cca-router: adapter={adapter:#010x} inner={inner:#010x} vt={vt:#010x} gate={gate:#010x} state={state:?} app={app:?} msg={msg:#010x} class={class:?} svc={svc:?} arg2={arg2:#010x} lr={lr:#010x} defer={defer_flag:?}"
                    );
                }
                "power-branch" => {
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                    log::info!("PROCNAV[{tid}] power message taken for processing (lr={lr:#010x})");
                }
                "power-repost" => {
                    let msg = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                    let content = word(uc, msg.wrapping_add(4)).unwrap_or(0);
                    let class = read_byte(uc, content.wrapping_add(0xb));
                    log::info!(
                        "PROCNAV[{tid}] power message reposted: msg={msg:#010x} class={class:?}"
                    );
                }
                "stat-oninit" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!("PROCNAV[{tid}] STAT bOnInit entered: object={r0:#010x}");
                }
                "probe-trace-exit"
                | "probe-mid-flow"
                | "probe-defer-call"
                | "probe-switch"
                | "probe-class40"
                | "probe-drop-stub" => {
                    let msg = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                    let content = word(uc, msg.wrapping_add(4)).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] {name}: msg={msg:#010x} content={content:#010x} src={:?} dst={:?} mode={:?}",
                        halfword(uc, content),
                        halfword(uc, content.wrapping_add(2)),
                        halfword(uc, content.wrapping_add(0x14)),
                    );
                }
                "probe-check-getter" => {
                    let getter_result = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let own = word(uc, 0x9022_6f90).and_then(|inner| halfword(uc, inner + 0x10));
                    log::info!(
                        "PROCNAV[{tid}] probe-check-getter: r0={getter_result:#x} own={own:?}"
                    );
                }
                "probe-after-gate" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-after-gate: gate result r0={r0:#x}");
                }
                "probe-vt68" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!("PROCNAV[{tid}] probe-vt68: vAppStateChanged object={r0:#010x}");
                }
                "probe-init-result" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-init-result: bOnInit returned {r0:#x}");
                }
                "probe-state-store" => {
                    log::info!("PROCNAV[{tid}] probe-state-store: writing state 3 (ACTIVE)");
                }
                "probe-init-fail" => {
                    log::info!("PROCNAV[{tid}] probe-init-fail: bOnInit failed");
                }
                "svc-data" => {
                    let msg = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let fields = word(uc, msg.wrapping_add(4));
                    let (svc, prop, opcode) = match fields {
                        Some(base) if base != 0 => (
                            halfword(uc, base.wrapping_add(0x14)),
                            halfword(uc, base.wrapping_add(0x18)),
                            word(uc, base.wrapping_add(0x18)).map(|v| v >> 16 & 0xff),
                        ),
                        _ => (None, None, None),
                    };
                    log::info!(
                        "PROCNAV[{tid}] svc-data: msg={msg:#010x} fields={fields:?} svc={svc:?} prop={prop:?} opcode={opcode:?}",
                    );
                }
                "post-ipc" => {
                    let app = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32 & 0xffff;
                    let msg = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let fields = word(uc, msg.wrapping_add(4));
                    let svc = fields.filter(|v| *v != 0).and_then(|base| halfword(uc, base.wrapping_add(0x14)));
                    log::info!(
                        "PROCNAV[{tid}] post-ipc: app={app:#x} msg={msg:#010x} svc={svc:?}"
                    );
                }
                "gps-sink" => {
                    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    let msg = word(uc, sp.wrapping_add(0xf0)).unwrap_or(0);
                    let svc = halfword(uc, msg.wrapping_add(0x14));
                    let prop = halfword(uc, msg.wrapping_add(0x16));
                    log::info!(
                        "PROCNAV[{tid}] GPS status sink hit: r0={:#010x} msg={msg:#010x} svc={:?} prop={:?} head=[{}]",
                        uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                        svc.map(|v| format!("{v:#x}")),
                        prop.map(|v| format!("{v:#x}")),
                        hexdump(uc, msg, 0x20),
                    );
                }
                _ => {}
            }
        });
        match result {
            Ok(_) => log::info!("PROCNAV: observation hook '{name}' at {address:#x}"),
            Err(error) => log::warn!("PROCNAV: cannot hook {name} at {address:#x}: {error}"),
        }
    }

    if std::env::var("EMU_SENSORFI")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
    {
        spawn_gps_provider(unicorn);
    }
}

/// Push periodic `cmb_sensorfi_tclMsgGpsDataStatus` messages into PROCNAV's
/// application mailbox (`mbx_19`), acting as the sensorfi server GPS provider.
///
/// The blob is pre-snapshotted (`[u32 flag, u32 len, content]`) so the monitor's
/// `0x2d` queue-wait materializes it into PROCNAV's own VM, and the `0x57`
/// reference resolves to the content base exactly like a Linux-side poster's.
/// The content's amt header uses the offsets the `amt_tclBaseMessage` accessors
/// read (`u16GetValue`: serviceId @ +0x14, propertyId @ +0x18, opCode @ +0x1a,
/// class @ +0xb), pinned by `cmb_sensorfi_tclMsgGpsDataStatus`'s getters:
/// service `0xb`, property `5`, opcode `6` (Status). The `cmb_fi_tcl_GpsBigdata`
/// payload stays zero until its wire layout is decoded - reaching the GPS sink
/// is the structural milestone this provider tests for.
fn spawn_gps_provider(unicorn: &mut Unicorn<'_, Context>) {
    let namespace = unicorn.get_data().namespace.clone();
    let result = std::thread::Builder::new()
        .name("gps-provider".into())
        .spawn(move || {
            // Let FC bring-up finish (component threads, queue opens, handler
            // registration) before the first push.
            std::thread::sleep(std::time::Duration::from_secs(10));
            let mut seq: u16 = 0;
            loop {
                seq = seq.wrapping_add(1);
                let blob = gps_status_message(seq);
                let posted = {
                    let mut guard = namespace.lock().unwrap();
                    crate::common::osal_queues::OsalQueueService::ensure_queue(
                        &mut guard.mq,
                        "mbx_19",
                    );
                    let posted = crate::common::osal_queues::OsalQueueService::guest_post(
                        &mut guard.mq,
                        "mbx_19",
                        blob,
                        0,
                    );
                    if posted {
                        guard.notify_waiters();
                    }
                    posted
                };
                if posted {
                    log::info!("GPS provider: posted GpsDataStatus #{seq} to mbx_19");
                } else {
                    log::debug!("GPS provider: mbx_19 full, skipping #{seq}");
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
    match result {
        Ok(_) => log::info!("PROCNAV: GPS sensorfi provider started (EMU_SENSORFI)"),
        Err(error) => log::warn!("PROCNAV: cannot start GPS provider: {error}"),
    }
}

/// One `GpsDataStatus` snapshot blob: envelope + amt header, zero payload.
fn gps_status_message(seq: u16) -> Vec<u8> {
    const LEN: usize = 0x80;
    let mut content = vec![0u8; LEN];
    content[0x00..0x02].copy_from_slice(&0xfffeu16.to_le_bytes()); // source app (unknown provider)
    content[0x02..0x04].copy_from_slice(&0x0013u16.to_le_bytes()); // target app: PROCNAV (19)
    content[0x04..0x08].copy_from_slice(&(LEN as u32).to_le_bytes()); // snapshot/object length
    // amt message class (read as u8GetValue(0xb) by ail_tclInternalDispatch::
    // bDispatchCCAMessages): 0x45 = ServiceData. 0x40 = state/LPM, 0x41 =
    // ServiceRegister, 0x42 = Unregister, 0x43 = RegisterConf, 0xff = DebugApp -
    // class 0x41 mis-injected r214-r216 as a (rejected) ServiceRegister.
    content[0x0b] = 0x45;
    content[0x0c..0x0e].copy_from_slice(&0xfffeu16.to_le_bytes()); // no source sub-id
    content[0x0e..0x10].copy_from_slice(&0xfffeu16.to_le_bytes()); // no target sub-id
    content[0x14..0x16].copy_from_slice(&0x000bu16.to_le_bytes()); // serviceId: sensorfi
    content[0x18..0x1a].copy_from_slice(&0x0005u16.to_le_bytes()); // propertyId: GPS data
    content[0x1a] = 0x06; // opCode: Status (server message)
    content[0x1c..0x1e].copy_from_slice(&seq.to_le_bytes()); // cmd counter
    let mut blob = Vec::with_capacity(8 + LEN);
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&(LEN as u32).to_le_bytes());
    blob.extend_from_slice(&content);
    blob
}