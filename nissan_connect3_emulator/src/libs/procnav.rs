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
/// Field helper invoked by the AE pump after a queue accept (THUMB):
/// double-dereferences the OSAL reference-resolution answer.
const PUMP_FIELD_HELPER: u32 = 0x0025_8b6a - ORIGINAL_BASE;
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
    for (name, offset, size) in [
        ("dispatcher", PROPERTY_DISPATCHER, 2u64),
        ("field-helper", PUMP_FIELD_HELPER, 2),
        ("gps-sink", GPS_STATUS_SINK, 4),
        ("dispatch-msg", DISPATCH_SERVICE_DATA_MSG, 2),
        ("vdispatch-msg", VDISPATCH_SERVICE_DATA_MSG, 2),
        ("server-answer", PROCESS_SERVER_ANSWER, 2),
        ("convert-fi", CONVERT_FI_MESSAGE, 2),
        ("vmsghandler", VMSG_HANDLER, 2),
    ] {
        let address = (base_address + offset) as u64;
        let result = unicorn.add_code_hook(address, address + size, move |uc, _, _| {
            let tid = uc.get_data().inner.thread_id();
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
    content[0x0b] = 0x41; // message class (service, like the DAPI FI traffic)
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