//! Host-side policy for the Bosch PWR-proxy handshake.
//!
//! On real hardware the power-state handshake is driven by a BSP/PMU daemon
//! below Linux. None of the shipped guest binaries contain the proxy: every
//! `/opt/bosch/processes/*` file carries only the libail *client* half
//! (`PWR_PROXY_START_CONF received, PWR_APP_INITIALIZED sent`,
//! `STATE_CHANGE_REQ from %s to %s`, `CVM_SIGNAL_CHANGED to %s`). Without a
//! peer, `ail_tclAppInterface::vAppEntry` blocks forever in
//! `ail_bIpcMessageWait(mbx_<app_id>, ...)` and applications such as
//! `procmapengine` never call `vStartApp`.
//!
//! # Why this is only the policy layer
//!
//! `OSAL_s32MessageQueueWait(queue, out, 8, ...)` returns an eight-byte
//! *message reference*, not the message body. Two of those words are an
//! OSAL pool handle and a pointer to the payload, and the payload lives in
//! the recipient's own address space (each process has its own Unicorn VM
//! with its own OSAL message pool; see `docs/threading.md`). The host
//! therefore cannot pre-stage a PowerMessage in another VM and push it
//! through the shared `MqState`: it must be materialised from inside the
//! recipient's Wait hook, at the moment the recipient actually wakes up.
//!
//! That split is what this module encodes. It is pure state:
//!
//! * `register_app_queue_open` is called by the libosal hook when a process
//!   opens its own `mbx_<app_id>` queue. That is the only event the host
//!   reliably gets, and it marks the moment an application becomes reachable.
//! * `observe_power_post` is called by the libosal hook when a process posts
//!   a PowerMessage to `mbx_0` (the shared LPM inbound queue). We peek at
//!   the content before the guest's own queue stores it.
//! * `take_pending_for` is called from a guest's Wait hook on its
//!   `mbx_<app_id>`. It returns the next PowerMessage the proxy wants that
//!   application to receive; the caller then allocates a slot in the guest's
//!   OSAL message pool, writes the 0x20-byte body there and hands the
//!   8-byte ref back through the Wait out-parameter.
//!
//! The state machine matches libail's expectations byte-for-byte. Constants
//! come from procmap's `ail_bHandleMsgPowerMessage` switch (Ghidra
//! `0x00668004`) and the layout from `amt_tclPowerMessage::amt_tclPowerMessage`
//! (Ghidra `0x003882f8`), decoded in [`encode_power_message`].

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

/// Application state values (`ail_coszAppStatus`).
pub const APP_STATE_UNINITIALIZED: u32 = 1;
pub const APP_STATE_INITIALIZED: u32 = 2;
pub const APP_STATE_NORMAL: u32 = 3;
pub const APP_STATE_DIAGNOSIS: u32 = 4;
/// `ail_coszAppStatus` OFF: what the unit's LPM puts the HMI through before
/// NORMAL (fw state 4 via `clAppStates::vEntryOff`).
pub const APP_STATE_OFF: u32 = 8;

/// prochmi's CCA application id (`mbx_265`).
const APP_ID_HMI: u16 = 0x0109;

/// Virtual-time gap between the HMI's OFF and NORMAL `STATE_CHANGE_REQ` (see
/// `PendingPowerMessage::not_before_ms`).
const HMI_OFF_TO_NORMAL_MS: u64 = 3000;

/// CVM voltage signal values (`ail_coszCVMStatus`).
pub const CVM_VOLTAGE_NORMAL: u32 = 0;

/// `amt_tclPowerMessage` power-type discriminators.
pub const PWR_APP_INITIALIZED: u16 = 1;
pub const PWR_PROXY_START_CONF: u16 = 3;
pub const PWR_PROXY_START_REJ: u16 = 4;
pub const PWR_STATE_CHANGE_REQ: u16 = 0x10;
pub const PWR_SYNC_COMM: u16 = 0x13;
pub const PWR_SHUTDOWN: u16 = 0x20;
pub const PWR_WDG_KEEPALIVE: u16 = 0x41;
pub const PWR_CVM_SIGNAL_CHANGED: u16 = 0x50;

/// `amt_tclPowerMessage` message-type tag at offset 8 of the body.
const MSG_TYPE_POWER: u16 = 2;

/// Every PowerMessage is 0x20 bytes on the wire.
pub const POWER_MESSAGE_LEN: usize = 0x20;

/// The shared LPM inbound queue every application opens in addition to its
/// own `mbx_<app_id>`. Applications post `PWR_APP_INITIALIZED`, watchdog
/// keep-alives and other proxy-destined traffic here.
pub const LPM_IN_QUEUE: &str = "mbx_0";

/// Sender app id used for proxy-originated messages. procmap does not check
/// this field; the value matches the historical synthetic (which the
/// shipping unit's LPM used as its own app id).
const PROXY_APP_ID: u16 = 0x0109;

/// Sub-id written at offset 0x0c of the PowerMessage body. procmap's own
/// `bSendCCAPowerMsg` uses 0; the historical synthetic used 1. libail
/// ignores the field in dispatch.
const PROXY_SUB_ID: u16 = 1;

/// A PowerMessage the proxy has queued for delivery to a specific
/// application. The Wait hook calls [`take_pending_for`], receives one of
/// these, materialises `body` in the guest's own OSAL message pool and
/// returns the ref to the waiting thread.
#[derive(Clone, Debug)]
pub struct PendingPowerMessage {
    pub app_id: u16,
    pub power_type: u16,
    pub power_data1: u32,
    pub power_data2: u32,
    pub body: Vec<u8>,
    /// Earliest emulator virtual time (ms since t0) at which the message may
    /// leave the queue.  Used to pace the HMI's OFF -> NORMAL promotion:
    /// `clHMIMngr` stores the pending state in one byte and performs it from
    /// its main-loop `bExecute`, so two `STATE_CHANGE_REQ`s delivered within
    /// one main-loop period collapse into the last one and the OFF state is
    /// never observed by the FI broadcast.  The unit's LPM leaves far more
    /// than a main-loop period between the two; we space them in virtual
    /// time (`take_next` refuses to pop before this deadline).
    pub not_before_ms: u64,
}

#[derive(Clone, Debug)]
struct AppRecord {
    app_id: u16,
    queue_open_observed: bool,
    start_conf_delivered: bool,
    initialized: bool,
    state_change_delivered: bool,
    cvm_signal_delivered: bool,
    /// The OFF -> NORMAL promotion sequence has been queued once.
    promotion_queued: bool,
}

/// Everything the proxy tracks. Kept small and lock-fast so both the Wait
/// hook and the Post hook can call into it without contention.
#[derive(Debug, Default)]
pub struct PwrProxyState {
    apps: HashMap<u16, AppRecord>,
    pending: HashMap<u16, VecDeque<PendingPowerMessage>>,
    /// Messages waiting for their `not_before_ms` deadline.  They MUST NOT sit
    /// in `pending`: the Wait hooks (libosal entry and monitor 0x2d) only
    /// consult the proxy when a Wait *starts*, and the HMI dispatcher blocks
    /// forever on its `mbx_265` right after processing the OFF request - a
    /// front-held message would then never be reached.  Due messages are
    /// posted straight into the host message queue by
    /// `message::post_due_power_messages` (called from `prochmi::tick`),
    /// which wakes the blocked Wait like any other poster does.
    delayed: HashMap<u16, VecDeque<PendingPowerMessage>>,
}

impl PwrProxyState {
    fn note_queue_open(&mut self, app_id: u16) {
        if self.queue_open_observed(app_id) {
            return;
        }
        let record = self
            .apps
            .entry(app_id)
            .or_insert_with(|| AppRecord {
                app_id,
                queue_open_observed: false,
                start_conf_delivered: false,
                initialized: false,
                state_change_delivered: false,
                cvm_signal_delivered: false,
                promotion_queued: false,
            });
        record.queue_open_observed = true;
        log::info!(
            "PWR proxy: app 0x{:04x} opened its queue; queueing PWR_PROXY_START_CONF",
            app_id
        );
        self.push_pending(app_id, PWR_PROXY_START_CONF, 0, 0);
    }

    fn queue_open_observed(&self, app_id: u16) -> bool {
        self.apps
            .get(&app_id)
            .map(|r| r.queue_open_observed)
            .unwrap_or(false)
    }

    fn push_pending(&mut self, app_id: u16, power_type: u16, data1: u32, data2: u32) {
        self.push_pending_delayed(app_id, power_type, data1, data2, 0);
    }

    fn push_pending_delayed(
        &mut self,
        app_id: u16,
        power_type: u16,
        data1: u32,
        data2: u32,
        delay_ms: u64,
    ) {
        let body = encode_power_message(
            PROXY_APP_ID,
            app_id,
            power_type,
            data1,
            data2,
            PROXY_SUB_ID,
        );
        let message = PendingPowerMessage {
            app_id,
            power_type,
            power_data1: data1,
            power_data2: data2,
            not_before_ms: crate::libs::dapi::emu_t0_elapsed_ms() as u64 + delay_ms,
            body,
        };
        if delay_ms > 0 {
            // Delayed messages live in their own table (see the
            // `delayed` field comment): `pending` may only hold messages that
            // a Wait can take immediately.
            self.delayed.entry(app_id).or_default().push_back(message);
        } else {
            self.pending.entry(app_id).or_default().push_back(message);
        }
    }

    /// Mark `sender` as having acknowledged initialization. Immediately
    /// promote that specific application to NORMAL (STATE_CHANGE_REQ) and
    /// notify it of nominal CVM voltage. Doing this per-application,
    /// rather than waiting for every process to acknowledge, matches what
    /// a real PWR-proxy does: applications that come up late do not hold
    /// the rest of the system in INITIALIZED.
    fn note_initialized(&mut self, sender: u16) {
        if !self.apps.contains_key(&sender) {
            self.apps.insert(
                sender,
                AppRecord {
                    app_id: sender,
                    queue_open_observed: false,
                    start_conf_delivered: true,
                    initialized: false,
                    state_change_delivered: false,
                    cvm_signal_delivered: false,
                    promotion_queued: false,
                },
            );
        }
        if let Some(record) = self.apps.get_mut(&sender) {
            if record.initialized {
                return;
            }
            record.initialized = true;
        }
        log::info!(
            "PWR proxy: app 0x{:04x} acknowledged initialization; promoting to NORMAL",
            sender
        );
        let already_queued = self
            .apps
            .get(&sender)
            .map(|r| r.promotion_queued)
            .unwrap_or(false);
        if !already_queued {
            if let Some(record) = self.apps.get_mut(&sender) {
                record.promotion_queued = true;
            }
            if sender == APP_ID_HMI {
                // r757: the unit's LPM does not hand NORMAL over directly from
                // INITIALIZED - the HMI framework first passes through OFF
                // (`clAppStates::bSwitchFromInitialized(8)` legally runs
                // `vEntryOff`, fw state 4), and only the following `4 -> 3`
                // transition lets `clFi_Navi::vNewAppState` start the enavi
                // server (the FI link behind the "Starting navigation."
                // wait-widget).  `clHMIMngr` collapses state changes faster
                // than its main loop performs them, so NORMAL is released only
                // `HMI_OFF_TO_NORMAL_MS` of virtual time after OFF.
                self.push_pending(sender, PWR_STATE_CHANGE_REQ, APP_STATE_OFF, 0);
                self.push_pending_delayed(
                    sender,
                    PWR_STATE_CHANGE_REQ,
                    APP_STATE_NORMAL,
                    0,
                    HMI_OFF_TO_NORMAL_MS,
                );
                self.push_pending_delayed(
                    sender,
                    PWR_CVM_SIGNAL_CHANGED,
                    CVM_VOLTAGE_NORMAL,
                    0,
                    HMI_OFF_TO_NORMAL_MS + 1000,
                );
                log::info!(
                    "PWR proxy: HMI promotion queued as OFF -> NORMAL in {HMI_OFF_TO_NORMAL_MS}ms"
                );
            } else {
                self.push_pending(sender, PWR_STATE_CHANGE_REQ, APP_STATE_NORMAL, 0);
                self.push_pending(sender, PWR_CVM_SIGNAL_CHANGED, CVM_VOLTAGE_NORMAL, 0);
            }
        }
    }

    fn take_next(&mut self, app_id: u16) -> Option<PendingPowerMessage> {
        let message = self.pending.get_mut(&app_id)?.pop_front()?;
        if message.power_type == PWR_PROXY_START_CONF {
            if let Some(record) = self.apps.get_mut(&app_id) {
                record.start_conf_delivered = true;
            }
        }
        // `app_state_change_delivered` gates the CCA directory's class-0x50
        // flush (dispatch drops those below admin state RUNNING), so it flips
        // when the NORMAL message actually leaves the proxy - both delivery
        // channels report through this helper.
        self.note_state_change_taken(&message);
        Some(message)
    }

    /// Pop every delayed message for `app_id` whose deadline has passed.
    /// The caller is responsible for posting each message body into the
    /// application's `mbx_<app_id>` host queue.
    fn take_due(&mut self, app_id: u16) -> Vec<PendingPowerMessage> {
        let now = crate::libs::dapi::emu_t0_elapsed_ms() as u64;
        let mut due_list = Vec::new();
        if let Some(queue) = self.delayed.get_mut(&app_id) {
            while let Some(front) = queue.front() {
                let due = front.not_before_ms;
                if due > now {
                    break;
                }
                let message = queue.pop_front().expect("front checked");
                let _ = &message;
                // Inline `note_state_change_taken` semantics without
                // re-borrowing `self.delayed` (borrow-checker).
                if message.power_type == PWR_STATE_CHANGE_REQ
                    && message.power_data1 == APP_STATE_NORMAL
                {
                    if let Some(record) = self.apps.get_mut(&app_id) {
                        record.state_change_delivered = true;
                    }
                }
                due_list.push(message);
            }
        }
        due_list
    }

    fn note_state_change_taken(&mut self, message: &PendingPowerMessage) {
        if message.power_type == PWR_STATE_CHANGE_REQ
            && message.power_data1 == APP_STATE_NORMAL
        {
            if let Some(record) = self.apps.get_mut(&message.app_id) {
                record.state_change_delivered = true;
            }
        }
    }
}

/// Global proxy. Initialized lazily on first access; no configuration
/// needed because the handshake is deterministic.
fn global() -> &'static Mutex<PwrProxyState> {
    static STATE: OnceLock<Mutex<PwrProxyState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(PwrProxyState::default()))
}

/// Called by the libosal hook when a guest opens an `mbx_<decimal_app_id>`
/// queue. Nothing happens for `mbx_0` or names that don't match.
pub fn register_app_queue_open(name: &str) {
    let Some(app_id) = parse_mbx_app_id(name) else {
        return;
    };
    let mut state = global().lock().unwrap();
    state.note_queue_open(app_id);
}

/// Called by the libosal hook when a guest posts a message to `mbx_0`. The
/// caller is responsible for reading the content out of the guest's own
/// memory first (the recipient can never read another VM's message pool).
/// We only care about `PWR_APP_INITIALIZED`; anything else is logged and
/// dropped.
pub fn observe_power_post(sender_app_id: u16, power_type: u16, data1: u32, data2: u32) {
    if power_type != PWR_APP_INITIALIZED {
        // `vAppBody` posts a `PowerType=0` registration message on startup
        // carrying its own app id in PowerData1; the queue-open event we
        // already get from libosal is enough, so we ignore the duplicate.
        // Watchdog keepalives, shutdown notices and service chatter are
        // logged at debug to keep the bring-up log readable.
        log::debug!(
            "PWR proxy: ignoring {} from app 0x{:04x} (data1 {} data2 {})",
            power_type_name(power_type),
            sender_app_id,
            data1,
            data2
        );
        return;
    }
    let mut state = global().lock().unwrap();
    state.note_initialized(sender_app_id);
}

/// Called by the guest's Wait hook on `mbx_<app_id>`. Returns the next
/// PowerMessage to hand to that application, or `None` if the proxy has
/// nothing queued right now (in which case the Wait must fall through to
/// the guest's own queue semantics).
pub fn take_pending_for(app_id: u16) -> Option<PendingPowerMessage> {
    let mut state = global().lock().unwrap();
    state.take_next(app_id)
}

/// Pop every delayed PowerMessage whose deadline has passed, for any app
/// (see `PwrProxyState::delayed`).  The caller posts each body into the
/// application's host message queue, which wakes any blocked Wait.
pub fn take_due_all() -> Vec<PendingPowerMessage> {
    let mut state = global().lock().unwrap();
    let app_ids: Vec<u16> = state.delayed.keys().copied().collect();
    let mut due = Vec::new();
    for app_id in app_ids {
        due.append(&mut state.take_due(app_id));
    }
    due
}

/// Whether the proxy has already handed this application its NORMAL
/// `PWR_STATE_CHANGE_REQ` (the message whose processing moves the AIL admin
/// state to RUNNING).  The CCA directory uses this to time its replies:
/// class-0x50 answers are dropped by the client's dispatch below RUNNING.
pub fn app_state_change_delivered(app_id: u16) -> bool {
    let state = global().lock().unwrap();
    state
        .apps
        .get(&app_id)
        .map(|r| r.state_change_delivered)
        .unwrap_or(false)
}

/// `mbx_<N>` where `<N>` is a decimal application id. Everything else
/// (`mbx_0`, terminal queues, IOSC queues) is ignored.
pub fn parse_mbx_app_id(name: &str) -> Option<u16> {
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

/// Byte-for-byte reproduction of
/// `amt_tclPowerMessage::amt_tclPowerMessage` (`0x003882f8` in procmap
/// Ghidra). The body is exactly 0x20 bytes:
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
pub fn encode_power_message(
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

/// Parse a `PowerMessage` body. Used by the libosal Post hook, which has to
/// look at the content before it hits the shared queue (the queue itself
/// only carries an 8-byte ref, which is meaningless to the host).
pub fn parse_power_message(data: &[u8]) -> Option<(u16, u16, u32, u32)> {
    if data.len() < POWER_MESSAGE_LEN {
        return None;
    }
    let message_type = u16::from_le_bytes([data[0x08], data[0x09]]);
    if message_type != MSG_TYPE_POWER {
        return None;
    }
    Some((
        u16::from_le_bytes([data[0x00], data[0x01]]),
        u16::from_le_bytes([data[0x14], data[0x15]]),
        u32::from_le_bytes([data[0x18], data[0x19], data[0x1a], data[0x1b]]),
        u32::from_le_bytes([data[0x1c], data[0x1d], data[0x1e], data[0x1f]]),
    ))
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
        assert_eq!(
            u32::from_le_bytes([body[0x04], body[0x05], body[0x06], body[0x07]]),
            0x20
        );
        assert_eq!(u16::from_le_bytes([body[0x08], body[0x09]]), MSG_TYPE_POWER);
        assert_eq!(body[0x0b], 0x40);
        assert_eq!(
            u16::from_le_bytes([body[0x14], body[0x15]]),
            PWR_PROXY_START_CONF
        );
    }

    #[test]
    fn round_trips_power_message() {
        let body = encode_power_message(0x400, 0x109, PWR_STATE_CHANGE_REQ, 2, 3, 7);
        let (sender, power_type, data1, data2) =
            parse_power_message(&body).expect("body parses");
        assert_eq!(sender, 0x400);
        assert_eq!(power_type, PWR_STATE_CHANGE_REQ);
        assert_eq!(data1, 2);
        assert_eq!(data2, 3);
    }

    #[test]
    fn rejects_non_power_message() {
        let body = vec![0u8; POWER_MESSAGE_LEN];
        assert!(parse_power_message(&body).is_none());
    }
}