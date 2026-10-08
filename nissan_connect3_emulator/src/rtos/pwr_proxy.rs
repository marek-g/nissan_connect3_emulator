//! Host-side emulation of the Bosch PWR-proxy daemon.
//!
//! On real hardware the power-state handshake lives in a BSP/PMU daemon below
//! Linux. None of the shipped guest binaries contain the proxy: every
//! `/opt/bosch/processes/*` file carries only the libail *client* half
//! (`PWR_PROXY_START_CONF received, PWR_APP_INITIALIZED sent`,
//! `STATE_CHANGE_REQ from %s to %s`, `CVM_SIGNAL_CHANGED to %s`). Without a
//! peer, `ail_tclAppInterface::vAppEntry` blocks forever in
//! `ail_bIpcMessageWait(mbx_<app_id>, ...)` and applications such as
//! `procmapengine` never call `vStartApp`.
//!
//! This service owns the proxy side of that handshake on the host:
//!
//! * discovers `mbx_<app_id>` queues as applications open them,
//! * posts `PWR_PROXY_START_CONF` on every discovered `mbx_<app_id>`,
//! * drains `mbx_0` (the shared LPM inbound queue) looking for
//!   `PWR_APP_INITIALIZED` replies,
//! * once every expected application has acknowledged (or after a settle
//!   timeout) broadcasts `STATE_CHANGE_REQ` from `INITIALIZED` to `NORMAL`,
//! * emits one `CVM_SIGNAL_CHANGED(NORMAL voltage)`, and
//! * keeps draining `mbx_0` for the rest of the run so it never overflows.
//!
//! All message bodies follow the exact `amt_tclPowerMessage` layout used by
//! libail. The reference for the layout is procmap's
//! `amt_tclPowerMessage::amt_tclPowerMessage(...)` constructor at Ghidra
//! `0x003882f8` (see the byte-by-byte decode in [`power_message`]).

use crate::common::osal_queues::{message_words, OsalQueueService};
use crate::os::syscalls::namespace::SystemNamespace;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Application state values (`ail_coszAppStatus`).
pub const APP_STATE_UNINITIALIZED: u32 = 1;
pub const APP_STATE_INITIALIZED: u32 = 2;
pub const APP_STATE_NORMAL: u32 = 3;
pub const APP_STATE_DIAGNOSIS: u32 = 4;

/// CVM voltage signal values (`ail_coszCVMStatus`).
pub const CVM_VOLTAGE_NORMAL: u32 = 0;

/// `amt_tclPowerMessage` power-type discriminators (the switch inside
/// `ail_tclInternalDispatch::ail_bHandleMsgPowerMessage`).
pub const PWR_APP_INITIALIZED: u16 = 1;
pub const PWR_PROXY_START_CONF: u16 = 3;
pub const PWR_PROXY_START_REJ: u16 = 4;
pub const PWR_STATE_CHANGE_REQ: u16 = 0x10;
pub const PWR_SYNC_COMM: u16 = 0x13;
pub const PWR_SHUTDOWN: u16 = 0x20;
pub const PWR_WDG_KEEPALIVE: u16 = 0x41;
pub const PWR_CVM_SIGNAL_CHANGED: u16 = 0x50;

/// `amt_tclPowerMessage` message-type tag written at offset 8 of the body.
const MSG_TYPE_POWER: u16 = 2;

/// Every PowerMessage is 0x20 bytes on the wire.
const POWER_MESSAGE_LEN: usize = 0x20;

/// The shared LPM inbound queue every application opens in addition to its own
/// `mbx_<app_id>`. Applications post `PWR_APP_INITIALIZED`, watchdog
/// keep-alives and other proxy-destined traffic here; the proxy never needs to
/// write to it.
pub const LPM_IN_QUEUE: &str = "mbx_0";

#[derive(Clone, Debug)]
pub struct PwrProxyConfig {
    pub enabled: bool,
    /// Time the proxy waits before it starts driving the handshake. Gives
    /// libosal enough time to see `OSAL_s32MessageQueueOpen` calls from every
    /// boot-critical application.
    pub discovery_grace: Duration,
    /// Time the proxy waits for `PWR_APP_INITIALIZED` replies before it
    /// broadcasts the state change anyway.
    pub init_reply_timeout: Duration,
    /// Sleep between host-side polls.
    pub poll_interval: Duration,
    /// Sub-id written at offset 0x0c of the PowerMessage body. procmap's own
    /// `bSendCCAPowerMsg` uses 0, but the historical synthetic used 1 and both
    /// are accepted by libail (the field is unused by dispatch).
    pub sub_id: u16,
    /// Sender app id used for proxy-originated messages. On the real headunit
    /// this is the LPM/PWR-proxy app id; procmap does not check it.
    pub proxy_app_id: u16,
}

impl Default for PwrProxyConfig {
    fn default() -> Self {
        Self {
            enabled: env_flag_or_default("EMU_RTOS_PWR_PROXY", true),
            discovery_grace: env_duration_ms("EMU_RTOS_PWR_DISCOVERY_MS", Duration::from_millis(500)),
            init_reply_timeout: env_duration_ms("EMU_RTOS_PWR_INIT_TIMEOUT_MS", Duration::from_millis(2000)),
            poll_interval: env_duration_ms("EMU_RTOS_PWR_POLL_MS", Duration::from_millis(5)),
            sub_id: 1,
            proxy_app_id: env_u16("EMU_RTOS_PWR_PROXY_APP_ID", 0x0109),
        }
    }
}

pub struct PwrProxyService {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl PwrProxyService {
    pub fn start(namespace: Arc<Mutex<SystemNamespace>>, config: PwrProxyConfig) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let service_stop = stop.clone();
        let handle = thread::spawn(move || {
            run(namespace, config, service_stop);
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Waiting for `discovery_grace` to elapse so applications can open their
    /// `mbx_<app_id>` queues.
    Discovering,
    /// Sending `PWR_PROXY_START_CONF` to each discovered application.
    Handshaking,
    /// Every known application is initialized; broadcast state change and CVM.
    Promoting,
    /// Steady state: keep draining `mbx_0` so its slots never fill up.
    Idle,
}

fn run(namespace: Arc<Mutex<SystemNamespace>>, config: PwrProxyConfig, stop: Arc<AtomicBool>) {
    if !config.enabled {
        log::info!("PWR proxy disabled via EMU_RTOS_PWR_PROXY=0");
        return;
    }

    let started_at = Instant::now();
    let mut phase = Phase::Discovering;
    let mut handshake_started_at: Option<Instant> = None;
    let mut apps: HashMap<u16, AppRecord> = HashMap::new();
    let mut state_change_sent = false;
    let mut cvm_signal_sent = false;

    while !stop.load(Ordering::Relaxed) {
        let mut notify = false;
        {
            let mut guard = namespace.lock().unwrap();

            discover_new_applications(&mut guard.mq, &mut apps, started_at, &mut notify);
            notify |= drain_lpm_inbound(&mut guard.mq, &mut apps);

            match phase {
                Phase::Discovering => {
                    if started_at.elapsed() >= config.discovery_grace {
                        log::info!(
                            "PWR proxy: discovery window closed with {} application queue(s): {}",
                            apps.len(),
                            format_app_ids(&apps)
                        );
                        for (&app_id, record) in apps.iter_mut() {
                            if send_start_conf(&mut guard.mq, app_id, record, &config) {
                                record.start_conf_sent = true;
                                notify = true;
                            }
                        }
                        handshake_started_at = Some(Instant::now());
                        phase = Phase::Handshaking;
                    }
                }
                Phase::Handshaking => {
                    for (&app_id, record) in apps.iter_mut() {
                        if !record.start_conf_sent
                            && send_start_conf(&mut guard.mq, app_id, record, &config)
                        {
                            record.start_conf_sent = true;
                            notify = true;
                        }
                    }

                    let all_ready = !apps.is_empty() && apps.values().all(|r| r.initialized);
                    let timed_out = handshake_started_at
                        .map(|t| t.elapsed() >= config.init_reply_timeout)
                        .unwrap_or(false);

                    if all_ready || timed_out {
                        if timed_out && !all_ready {
                            log::warn!(
                                "PWR proxy: init-reply timeout reached with pending apps: {} (promoting anyway)",
                                pending_app_ids(&apps)
                            );
                        }
                        phase = Phase::Promoting;
                    }
                }
                Phase::Promoting => {
                    if !state_change_sent {
                        let mut ok = true;
                        for (&app_id, record) in apps.iter() {
                            ok &= send_state_change_req(&mut guard.mq, app_id, record, &config);
                        }
                        if ok {
                            state_change_sent = true;
                            notify = true;
                        }
                    }

                    if state_change_sent && !cvm_signal_sent {
                        let mut ok = true;
                        for (&app_id, record) in apps.iter() {
                            ok &= send_cvm_signal_changed(&mut guard.mq, app_id, record, &config);
                        }
                        if ok {
                            cvm_signal_sent = true;
                            notify = true;
                            log::info!("PWR proxy: state change + CVM signal delivered");
                            phase = Phase::Idle;
                        }
                    }
                }
                Phase::Idle => {
                    for (&app_id, record) in apps.iter_mut() {
                        if !record.start_conf_sent && !record.initialized {
                            // A late-registered application that we missed
                            // during the promoting phase. Kick it back in.
                            if send_start_conf(&mut guard.mq, app_id, record, &config) {
                                record.start_conf_sent = true;
                                notify = true;
                            }
                        }
                    }
                }
            }

            if notify {
                guard.notify_waiters();
            }
        }

        thread::sleep(config.poll_interval);
    }
}

#[derive(Clone, Debug)]
struct AppRecord {
    app_id: u16,
    queue_name: String,
    start_conf_sent: bool,
    initialized: bool,
}

fn discover_new_applications(
    mq: &mut crate::common::queues::MqState,
    apps: &mut HashMap<u16, AppRecord>,
    started_at: Instant,
    notify: &mut bool,
) {
    for (name, _id) in mq.name_to_id.iter() {
        if let Some(app_id) = parse_mbx_app_id(name) {
            if apps.contains_key(&app_id) {
                continue;
            }
            log::info!(
                "PWR proxy: discovered application queue {} (app_id 0x{:04x}) after {:?}",
                name,
                app_id,
                started_at.elapsed()
            );
            apps.insert(
                app_id,
                AppRecord {
                    app_id,
                    queue_name: name.clone(),
                    start_conf_sent: false,
                    initialized: false,
                },
            );
            *notify = true;
        }
    }
}

/// `mbx_<N>` where `<N>` is a decimal application id. Everything else (LPM in,
/// terminal queues, IOSC queues) is ignored by discovery.
fn parse_mbx_app_id(name: &str) -> Option<u16> {
    let tail = name.strip_prefix("mbx_")?;
    if tail.is_empty() {
        return None;
    }
    if !tail.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: u32 = tail.parse().ok()?;
    if value == 0 || value > u32::from(u16::MAX) {
        return None;
    }
    Some(value as u16)
}

/// Drain every pending message off `mbx_0` (the shared LPM inbound queue). Any
/// message with the PowerMessage tag and `PWR_APP_INITIALIZED` power-type
/// acknowledges an application; everything else is logged and dropped.
fn drain_lpm_inbound(
    mq: &mut crate::common::queues::MqState,
    apps: &mut HashMap<u16, AppRecord>,
) -> bool {
    let mut touched = false;
    while let Some(message) = OsalQueueService::guest_wait_nonblock(mq, LPM_IN_QUEUE, 0x100) {
        touched = true;
        match PowerMessage::parse(&message.data) {
            Some(power) => {
                log::info!(
                    "PWR proxy: {} delivered from app 0x{:04x} type {} data1 {} data2 {}",
                    power_type_name(power.power_type),
                    power.sender_app_id,
                    power.power_type,
                    power.power_data1,
                    power.power_data2
                );
                if power.power_type == PWR_APP_INITIALIZED {
                    if let Some(record) = apps.get_mut(&power.sender_app_id) {
                        if !record.initialized {
                            record.initialized = true;
                            log::info!(
                                "PWR proxy: app 0x{:04x} acknowledged initialization",
                                power.sender_app_id
                            );
                        }
                    } else {
                        log::warn!(
                            "PWR proxy: app 0x{:04x} sent INITIALIZED before discovery recorded its queue",
                            power.sender_app_id
                        );
                    }
                }
            }
            None => {
                let words = message_words(&message.data);
                log::debug!(
                    "PWR proxy: non-power message on {} (len {}, words [{:#x}, {:#x}, {:#x}, {:#x}])",
                    LPM_IN_QUEUE,
                    message.data.len(),
                    words[0],
                    words[1],
                    words[2],
                    words[3]
                );
            }
        }
    }
    touched
}

fn send_start_conf(
    mq: &mut crate::common::queues::MqState,
    app_id: u16,
    record: &AppRecord,
    config: &PwrProxyConfig,
) -> bool {
    let accepted = post_power_message(
        mq,
        &record.queue_name,
        config.proxy_app_id,
        app_id,
        PWR_PROXY_START_CONF,
        0,
        0,
        config.sub_id,
    );
    log::info!(
        "PWR proxy: PWR_PROXY_START_CONF -> {} accepted {}",
        record.queue_name,
        accepted
    );
    accepted
}

fn send_state_change_req(
    mq: &mut crate::common::queues::MqState,
    app_id: u16,
    record: &AppRecord,
    config: &PwrProxyConfig,
) -> bool {
    let accepted = post_power_message(
        mq,
        &record.queue_name,
        config.proxy_app_id,
        app_id,
        PWR_STATE_CHANGE_REQ,
        APP_STATE_INITIALIZED,
        APP_STATE_NORMAL,
        config.sub_id,
    );
    log::info!(
        "PWR proxy: STATE_CHANGE_REQ (INITIALIZED -> NORMAL) -> {} accepted {}",
        record.queue_name,
        accepted
    );
    accepted
}

fn send_cvm_signal_changed(
    mq: &mut crate::common::queues::MqState,
    app_id: u16,
    record: &AppRecord,
    config: &PwrProxyConfig,
) -> bool {
    let accepted = post_power_message(
        mq,
        &record.queue_name,
        config.proxy_app_id,
        app_id,
        PWR_CVM_SIGNAL_CHANGED,
        CVM_VOLTAGE_NORMAL,
        0,
        config.sub_id,
    );
    log::info!(
        "PWR proxy: CVM_SIGNAL_CHANGED (NORMAL voltage) -> {} accepted {}",
        record.queue_name,
        accepted
    );
    accepted
}

fn post_power_message(
    mq: &mut crate::common::queues::MqState,
    queue_name: &str,
    sender_app_id: u16,
    target_app_id: u16,
    power_type: u16,
    power_data1: u32,
    power_data2: u32,
    sub_id: u16,
) -> bool {
    let body = encode_power_message(
        sender_app_id,
        target_app_id,
        power_type,
        power_data1,
        power_data2,
        sub_id,
    );
    // libail posts every CCA message at OSAL priority 8 (see
    // `ail_bIpcMessagePost`).
    OsalQueueService::guest_post(mq, queue_name, body, 8)
}

/// Byte-for-byte reproduction of `amt_tclPowerMessage::amt_tclPowerMessage`
/// (`0x003882f8` in procmap Ghidra). The body is exactly 0x20 bytes:
///
/// ```text
/// 0x00: u16 sender_app_id
/// 0x02: u16 target_app_id
/// 0x04: u32 message_length (=0x20)
/// 0x08: u16 message_type  (=2 for PowerMessage)
/// 0x0a: u8  (0)
/// 0x0b: u8  flags         (=0x40)
/// 0x0c: u16 sub_id
/// 0x0e: u16 param7        (=0)
/// 0x10: u32 param8        (=0)
/// 0x14: u16 power_type
/// 0x16: u16 (0)
/// 0x18: u32 power_data1
/// 0x1c: u32 power_data2
/// ```
fn encode_power_message(
    sender_app_id: u16,
    target_app_id: u16,
    power_type: u16,
    power_data1: u32,
    power_data2: u32,
    sub_id: u16,
) -> Vec<u8> {
    let mut body = vec![0u8; POWER_MESSAGE_LEN];
    body[0x00..0x02].copy_from_slice(&sender_app_id.to_le_bytes());
    body[0x02..0x04].copy_from_slice(&target_app_id.to_le_bytes());
    body[0x04..0x08].copy_from_slice(&(POWER_MESSAGE_LEN as u32).to_le_bytes());
    body[0x08..0x0a].copy_from_slice(&MSG_TYPE_POWER.to_le_bytes());
    body[0x0b] = 0x40;
    body[0x0c..0x0e].copy_from_slice(&sub_id.to_le_bytes());
    body[0x14..0x16].copy_from_slice(&power_type.to_le_bytes());
    body[0x18..0x1c].copy_from_slice(&power_data1.to_le_bytes());
    body[0x1c..0x20].copy_from_slice(&power_data2.to_le_bytes());
    body
}

#[derive(Clone, Copy, Debug)]
struct PowerMessage {
    sender_app_id: u16,
    target_app_id: u16,
    power_type: u16,
    power_data1: u32,
    power_data2: u32,
}

impl PowerMessage {
    fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < POWER_MESSAGE_LEN {
            return None;
        }
        let message_type = u16::from_le_bytes([data[0x08], data[0x09]]);
        if message_type != MSG_TYPE_POWER {
            return None;
        }
        Some(Self {
            sender_app_id: u16::from_le_bytes([data[0x00], data[0x01]]),
            target_app_id: u16::from_le_bytes([data[0x02], data[0x03]]),
            power_type: u16::from_le_bytes([data[0x14], data[0x15]]),
            power_data1: u32::from_le_bytes([data[0x18], data[0x19], data[0x1a], data[0x1b]]),
            power_data2: u32::from_le_bytes([data[0x1c], data[0x1d], data[0x1e], data[0x1f]]),
        })
    }
}

fn power_type_name(power_type: u16) -> &'static str {
    match power_type {
        PWR_APP_INITIALIZED => "PWR_APP_INITIALIZED",
        PWR_PROXY_START_CONF => "PWR_PROXY_START_CONF",
        PWR_PROXY_START_REJ => "PWR_PROXY_START_REJ",
        PWR_STATE_CHANGE_REQ => "PWR_STATE_CHANGE_REQ",
        PWR_SYNC_COMM => "PWR_SYNC_COMM",
        PWR_SHUTDOWN => "PWR_SHUTDOWN",
        PWR_WDG_KEEPALIVE => "PWR_WDG_KEEPALIVE",
        PWR_CVM_SIGNAL_CHANGED => "PWR_CVM_SIGNAL_CHANGED",
        _ => "UNKNOWN_PWR_TYPE",
    }
}

fn format_app_ids(apps: &HashMap<u16, AppRecord>) -> String {
    let mut seen: HashSet<u16> = apps.keys().copied().collect();
    let mut sorted: Vec<u16> = seen.drain().collect();
    sorted.sort();
    sorted
        .iter()
        .map(|id| format!("0x{:04x}", id))
        .collect::<Vec<_>>()
        .join(",")
}

fn pending_app_ids(apps: &HashMap<u16, AppRecord>) -> String {
    let mut sorted: Vec<u16> = apps
        .iter()
        .filter(|(_, r)| !r.initialized)
        .map(|(id, _)| *id)
        .collect();
    sorted.sort();
    sorted
        .iter()
        .map(|id| format!("0x{:04x}", id))
        .collect::<Vec<_>>()
        .join(",")
}

fn env_flag_or_default(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => match value.as_str() {
            "0" | "off" | "false" | "FALSE" => false,
            "1" | "on" | "true" | "TRUE" => true,
            _ => default,
        },
        Err(_) => default,
    }
}

fn env_duration_ms(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(default)
}

fn env_u16(name: &str, default: u16) -> u16 {
    std::env::var(name)
        .ok()
        .and_then(|value| u32::from_str_radix(value.trim_start_matches("0x"), 16).ok())
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mbx_only_decimal_ids() {
        assert_eq!(parse_mbx_app_id("mbx_1024"), Some(1024));
        assert_eq!(parse_mbx_app_id("mbx_265"), Some(265));
        assert_eq!(parse_mbx_app_id("mbx_0"), None);
        assert_eq!(parse_mbx_app_id("mbx_"), None);
        assert_eq!(parse_mbx_app_id("mbx_0400"), Some(400));
        assert_eq!(parse_mbx_app_id("mbx_hex"), None);
    }

    #[test]
    fn encodes_power_message() {
        let body = encode_power_message(0x109, 0x400, PWR_PROXY_START_CONF, 0, 0, 1);
        assert_eq!(body.len(), POWER_MESSAGE_LEN);
        assert_eq!(u16::from_le_bytes([body[0x00], body[0x01]]), 0x109);
        assert_eq!(u16::from_le_bytes([body[0x02], body[0x03]]), 0x400);
        assert_eq!(u32::from_le_bytes([body[0x04], body[0x05], body[0x06], body[0x07]]), 0x20);
        assert_eq!(u16::from_le_bytes([body[0x08], body[0x09]]), MSG_TYPE_POWER);
        assert_eq!(body[0x0b], 0x40);
        assert_eq!(u16::from_le_bytes([body[0x14], body[0x15]]), PWR_PROXY_START_CONF);
    }

    #[test]
    fn round_trips_power_message() {
        let body = encode_power_message(0x400, 0x109, PWR_STATE_CHANGE_REQ, 2, 3, 7);
        let parsed = PowerMessage::parse(&body).expect("body parses");
        assert_eq!(parsed.sender_app_id, 0x400);
        assert_eq!(parsed.target_app_id, 0x109);
        assert_eq!(parsed.power_type, PWR_STATE_CHANGE_REQ);
        assert_eq!(parsed.power_data1, 2);
        assert_eq!(parsed.power_data2, 3);
    }

    #[test]
    fn rejects_non_power_message() {
        let body = vec![0u8; POWER_MESSAGE_LEN];
        assert!(PowerMessage::parse(&body).is_none());
    }
}