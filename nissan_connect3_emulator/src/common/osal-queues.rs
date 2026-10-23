#![allow(dead_code)]

//! OSAL message-queue abstraction used by the host-side RTOS simulation and the
//! guest-side libosal bridge.
//!
//! This module is the guest-visible semantic layer above the low-level POSIX
//! queue state in [`crate::common::queues`]. The RTOS side must not call into
//! libosal; it should use this service and the shared queue namespace instead.
//! The libosal hooks call the same service so guest OSAL queue traffic and the
//! RTOS simulation see the same queue objects.

use crate::common::queues::{canonical_mq_name, MqMessage, MqState};

pub use crate::common::queues::{
    DP_MASTER, LI_TERM_MQ, NOIOSC_CB_HDR_LI_PREFIX, OSAL_CB_HDR_LI_MAIN,
    OSAL_CB_HDR_LI_MAIN_MAXMSG, OSAL_CB_HDR_MAXMSG, OSAL_CB_HDR_MSGSIZE, OSAL_CB_HDR_TE,
    OSAL_CB_HDR_TE_MAXMSG, TERM_MQ_MAXMSG, TERM_MQ_MSGSIZE, TE_TERM_MQ,
};

pub const RTOS_TERMINAL_READY: u8 = 0x0e;
pub const OSAL_CB_MESSAGE_MAIN: u32 = 7;
pub const OSAL_CB_MESSAGE_LOCAL: u32 = 6;
pub const OSAL_START_PROC_COMMAND: u8 = 0x1a;

pub const OSAL_MQ_PRIORITY_MAX: u32 = 7;
pub const OSAL_DEFAULT_MAXMSG: i64 = 0x100;
pub const OSAL_DEFAULT_MSGSIZE: i64 = 0x50;
pub const DP_MASTER_MSGSIZE: i64 = 0x38;

pub const OSAL_MQ_HANDLE_MAGIC: u32 = 0x7568_6d71;
pub const OSAL_MQ_HANDLE_KIND_OFFSET: usize = 0x04;
pub const OSAL_MQ_HANDLE_INFO_OFFSET: usize = 0x0c;

pub const OSAL_MQ_INFO_IN_USE_OFFSET: usize = 0x08;
pub const OSAL_MQ_INFO_TYPE_OFFSET: usize = 0x0a;
pub const OSAL_MQ_INFO_IOS_INDEX_OFFSET: usize = 0x0c;
pub const OSAL_MQ_INFO_LOCAL_INDEX_OFFSET: usize = 0x30;
pub const OSAL_MQ_INFO_NAME_OFFSET: usize = 0x38;
pub const OSAL_MQ_INFO_NAME_SIZE: usize = 0x20;

pub const OSAL_QUEUE_TYPE_LOCAL: u32 = 1;
pub const OSAL_QUEUE_TYPE_CALLBACK: u32 = 2;
pub const OSAL_QUEUE_TYPE_IOSC: u32 = 3;

pub const RTOS_INTERCEPTED_OSAL_QUEUES: &[&str] =
    &[TE_TERM_MQ, LI_TERM_MQ, OSAL_CB_HDR_TE, OSAL_CB_HDR_LI_MAIN];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsalQueueKind {
    LinuxLocalCallback,
    RtosTerminalInbound,
    RtosTerminalOutbound,
    OsalCallbackMain,
    OsalCallbackTe,
    DatapoolMaster,
    Iosc,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OsalQueueSpec {
    pub name: String,
    pub kind: OsalQueueKind,
    pub maxmsg: i64,
    pub msgsize: i64,
}

impl OsalQueueSpec {
    pub fn new(name: impl Into<String>, kind: OsalQueueKind, maxmsg: i64, msgsize: i64) -> Self {
        Self {
            name: name.into(),
            kind,
            maxmsg,
            msgsize,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OsalMessage {
    pub data: Vec<u8>,
    /// OSAL priority, 0 = highest. This is the value OSAL exposes to guest code;
    /// it is inverted from the POSIX mqueue priority used by [`MqState`].
    pub priority: u32,
}

impl OsalMessage {
    pub fn from_mq_message(message: MqMessage) -> Self {
        Self {
            data: message.data,
            priority: mq_priority_to_osal_priority(message.priority),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OsalQueueHandleInfo {
    pub handle: u32,
    pub info: u32,
    pub queue_type: u32,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct OsalQueueService;

impl OsalQueueService {
    pub fn new() -> Self {
        Self
    }

    pub fn bootstrap_rtos_queues(mq: &mut MqState) {
        for spec in rtos_boot_queue_specs() {
            mq.rtos_open_or_create(&spec.name, spec.maxmsg, spec.msgsize);
        }
    }

    pub fn create_rtos_terminal_queues(mq: &mut MqState) {
        mq.rtos_open_or_create(TE_TERM_MQ, TERM_MQ_MAXMSG, TERM_MQ_MSGSIZE);
        mq.rtos_open_or_create(LI_TERM_MQ, TERM_MQ_MAXMSG, TERM_MQ_MSGSIZE);
    }

    pub fn create_rtos_callback_queues(mq: &mut MqState) {
        mq.rtos_open_or_create(
            OSAL_CB_HDR_LI_MAIN,
            OSAL_CB_HDR_LI_MAIN_MAXMSG,
            OSAL_CB_HDR_MSGSIZE,
        );
        mq.rtos_open_or_create(OSAL_CB_HDR_TE, OSAL_CB_HDR_TE_MAXMSG, OSAL_CB_HDR_MSGSIZE);
    }

    pub fn rtos_terminal_bootstrap(mq: &mut MqState) -> bool {
        Self::create_rtos_terminal_queues(mq);
        Self::post_rtos_terminal_ready(mq)
    }

    pub fn rtos_callback_bootstrap(mq: &mut MqState) {
        Self::create_rtos_callback_queues(mq);
    }

    pub fn rtos_post_message(
        mq: &mut MqState,
        name: &str,
        data: Vec<u8>,
        osal_priority: u32,
    ) -> bool {
        let name = canonical_mq_name(name);
        Self::ensure_queue(mq, &name);
        mq.host_post(&name, data, osal_priority_to_mq_priority(osal_priority))
    }

    pub fn post_terminal_message(mq: &mut MqState, name: &str, command: u8) -> bool {
        Self::rtos_post_message(mq, name, make_terminal_message(command), 0)
    }

    pub fn post_rtos_terminal_ready(mq: &mut MqState) -> bool {
        Self::post_terminal_message(mq, LI_TERM_MQ, RTOS_TERMINAL_READY)
    }

    pub fn receive_message(mq: &mut MqState, name: &str) -> Option<OsalMessage> {
        let message = mq.host_receive(name)?;
        Some(OsalMessage::from_mq_message(message))
    }

    pub fn receive_rtos_terminal_message(mq: &mut MqState) -> Option<OsalMessage> {
        Self::receive_message(mq, TE_TERM_MQ)
    }

    pub fn is_rtos_intercepted_queue(name: &str) -> bool {
        matches!(
            canonical_mq_name(name).as_str(),
            TE_TERM_MQ | LI_TERM_MQ | OSAL_CB_HDR_TE | OSAL_CB_HDR_LI_MAIN
            // r799d NOTE: hosting "HMINavServerLib_1" was tried twice and
            // reverted twice.  The consumer's native loop does not survive
            // bridge-served Waits (f263-f265: 16-byte event overflowed the
            // 0x8 wait buffer; f275: even with the byte-identical 8-byte
            // event and parked timeout-0 waits the drain processed 6 Waits
            // instead of 144 and state 6 never closed).  The fabrication
            // delivers pump events by CALLING the guest's own
            // OSAL_s32MessageQueuePost from a host-spawned guest thread
            // (message.rs pump_push_completion) - native semantics kept.
        )
    }

    pub fn guest_intercepts_post(name: &str) -> bool {
        Self::is_rtos_intercepted_queue(name)
    }

    pub fn guest_intercepts_wait(name: &str) -> bool {
        Self::is_rtos_intercepted_queue(name)
    }

    pub fn ensure_queue(mq: &mut MqState, name: &str) -> u32 {
        let name = canonical_mq_name(name);
        if let Some(id) = mq.queue_id_for_name(&name) {
            return id;
        }

        // r799n36 fidelity: an application mailbox (`mbx_<app>` / `MQB_<app>`)
        // is created by its *own* guest process - via the non-IOSC `mq_open
        // (O_CREAT|O_EXCL)` leg of `OSAL_s32MessageQueueCreate` or the IOSC
        // create request - and a host lane materializing it early registers the
        // name in the shared namespace so the owner's O_EXCL create meets
        // EEXIST: assert osalmqueue.cpp:1181 -> "Queue could not be created"
        // killed procmapengine (mbx_1024) and prochmi (mbx_265).  Host-side
        // ensure/pop/post against such a queue therefore must not create it;
        // the sentinel id 0 matches no real queue (ids start at
        // MQ_HANDLE_BASE), so waiters see "empty" and `guest_post` drops the
        // message like a real ENOENT `mq_send` would.
        if name.starts_with("mbx_") || name.starts_with("MQB_") {
            if std::env::var_os("EMU_MQ_NO_AUTOCREATE").is_none() {
                static LOST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                if LOST.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 40 {
                    log::info!(
                        "MQ ensure refused: {name:?} is an app mailbox - not created host-side (owner must mq_open it)"
                    );
                }
            }
            return 0;
        }

        let spec =
            spec_for_name(&name).unwrap_or_else(|| default_queue_spec(&name, OSAL_DEFAULT_MSGSIZE));
        mq.rtos_open_or_create(&spec.name, spec.maxmsg, spec.msgsize)
    }

    /// Unconditional create-or-get.  Only for lanes that carry an explicit
    /// guest *create* request (the RTOS monitor's OSAL queue-create request,
    /// the IOSC leg of `OSAL_s32MessageQueueCreate`) - the guest is the
    /// creator there, so registering the name is correct.
    pub fn force_create(mq: &mut MqState, name: &str) -> u32 {
        let name = canonical_mq_name(name);
        if let Some(id) = mq.queue_id_for_name(&name) {
            return id;
        }
        let spec =
            spec_for_name(&name).unwrap_or_else(|| default_queue_spec(&name, OSAL_DEFAULT_MSGSIZE));
        mq.rtos_open_or_create(&spec.name, spec.maxmsg, spec.msgsize)
    }

    pub fn guest_post(mq: &mut MqState, name: &str, data: Vec<u8>, osal_priority: u32) -> bool {
        let name = canonical_mq_name(name);
        let requested_msgsize = data.len() as i64;

        // r799n36 fidelity: a host post must never CREATE the queue.  On the
        // unit `mq_send` into a not-yet-created mailbox fails with ENOENT and
        // the sender retries later; here the old materialize-on-post silently
        // registered the name in the shared namespace, so procmapengine's own
        // `mq_open("mbx_1024", O_CREAT|O_EXCL)` met EEXIST (assert
        // osalmqueue.cpp:1181 -> "Queue could not be created" -> the whole
        // svc-0x300 map-data channel died).  Host-side posts that arrive
        // before the guest creator is up are dropped, exactly like the real
        // ENOENT, and the synthesizing lanes re-post on their own cadence.
        let id = match mq.queue_id_for_name(&name) {
            Some(id) => id,
            None => {
                use std::sync::{Mutex, OnceLock};
                static DROPPED: OnceLock<Mutex<std::collections::HashSet<String>>> =
                    OnceLock::new();
                let set = DROPPED.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
                if set.lock().unwrap().insert(name.clone()) {
                    log::info!(
                        "osal-queue: host post of {} bytes to {name:?} dropped - queue not (yet) created by its guest app (real mq_send would ENOENT)",
                        data.len(),
                    );
                }
                return false;
            }
        };

        // The host queue table is bookkeeping, not guest-visible mq metadata
        // (the guest's open sizes its own table separately), so a queue first
        // touched via `ensure_queue`'s default may legally grow when a later
        // poster needs more room - e.g. posfi status envelopes on mbx_1024.
        if let Some(queue) = mq.queues.get_mut(&id) {
            if queue.msgsize < requested_msgsize {
                queue.msgsize = requested_msgsize;
            }
        }

        if mq.queues.get(&id).map(|queue| queue.msgsize).unwrap_or(0) < requested_msgsize {
            return false;
        }
        if !mq.has_free_slot(id) {
            return false;
        }

        let priority = osal_priority_to_mq_priority(osal_priority);
        // r786 forensics: EMU_OSAL_MSGLOG=1 dumps every enqueued message
        // (queue + AIL header words) so the destination-entry CL request/
        // answer round can be identified on the bus.
        if std::env::var_os("EMU_OSAL_MSGLOG").is_some() {
            static LOGGED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 4000 {
                let words: Vec<String> = data
                    .chunks_exact(4)
                    .take(10)
                    .map(|c| format!("{:#010x}", u32::from_le_bytes([c[0], c[1], c[2], c[3]])))
                    .collect();
                log::info!(
                    "OSAL-POST {name} len={} class={:#04x} w={}",
                    data.len(),
                    data.get(0x0b).copied().unwrap_or(0xff),
                    words.join(" ")
                );
            }
        }
        mq.insert_message(id, data, priority)
    }

    pub fn guest_wait_nonblock(
        mq: &mut MqState,
        name: &str,
        max_len: usize,
    ) -> Option<OsalMessage> {
        let id = mq.queue_id_for_name(name)?;
        Self::pop_guest_message(mq, id, max_len)
    }

    pub fn pop_guest_message(
        mq: &mut MqState,
        queue_id: u32,
        max_len: usize,
    ) -> Option<OsalMessage> {
        let fits = mq
            .peek_message(queue_id)
            .map(|message| message.data.len() <= max_len)
            .unwrap_or(false);
        if !fits {
            return None;
        }

        let message = mq.pop_message(queue_id)?;
        Some(OsalMessage::from_mq_message(message))
    }
}

pub fn rtos_boot_queue_specs() -> Vec<OsalQueueSpec> {
    vec![
        OsalQueueSpec::new(
            TE_TERM_MQ,
            OsalQueueKind::RtosTerminalInbound,
            TERM_MQ_MAXMSG,
            TERM_MQ_MSGSIZE,
        ),
        OsalQueueSpec::new(
            LI_TERM_MQ,
            OsalQueueKind::RtosTerminalOutbound,
            TERM_MQ_MAXMSG,
            TERM_MQ_MSGSIZE,
        ),
        OsalQueueSpec::new(
            OSAL_CB_HDR_LI_MAIN,
            OsalQueueKind::OsalCallbackMain,
            OSAL_CB_HDR_LI_MAIN_MAXMSG,
            OSAL_CB_HDR_MSGSIZE,
        ),
        OsalQueueSpec::new(
            OSAL_CB_HDR_TE,
            OsalQueueKind::OsalCallbackTe,
            OSAL_CB_HDR_TE_MAXMSG,
            OSAL_CB_HDR_MSGSIZE,
        ),
    ]
}

pub fn spec_for_name(name: &str) -> Option<OsalQueueSpec> {
    let name = canonical_mq_name(name);
    let kind = queue_kind(&name);

    match kind {
        OsalQueueKind::RtosTerminalInbound | OsalQueueKind::RtosTerminalOutbound => Some(
            OsalQueueSpec::new(name, kind, TERM_MQ_MAXMSG, TERM_MQ_MSGSIZE),
        ),
        OsalQueueKind::OsalCallbackMain => Some(OsalQueueSpec::new(
            name,
            kind,
            OSAL_CB_HDR_LI_MAIN_MAXMSG,
            OSAL_CB_HDR_MSGSIZE,
        )),
        OsalQueueKind::OsalCallbackTe => Some(OsalQueueSpec::new(
            name,
            kind,
            OSAL_CB_HDR_TE_MAXMSG,
            OSAL_CB_HDR_MSGSIZE,
        )),
        OsalQueueKind::DatapoolMaster => Some(OsalQueueSpec::new(
            name,
            kind,
            OSAL_DEFAULT_MAXMSG,
            DP_MASTER_MSGSIZE,
        )),
        OsalQueueKind::LinuxLocalCallback => Some(OsalQueueSpec::new(
            name,
            kind,
            OSAL_CB_HDR_MAXMSG,
            OSAL_CB_HDR_MSGSIZE,
        )),
        OsalQueueKind::Unknown => None,
        OsalQueueKind::Iosc => Some(OsalQueueSpec::new(
            name,
            kind,
            OSAL_DEFAULT_MAXMSG,
            OSAL_DEFAULT_MSGSIZE,
        )),
    }
}

pub fn default_queue_spec(name: &str, msgsize: i64) -> OsalQueueSpec {
    let name = canonical_mq_name(name);
    OsalQueueSpec::new(
        name.clone(),
        queue_kind(&name),
        OSAL_DEFAULT_MAXMSG,
        OSAL_DEFAULT_MSGSIZE.max(msgsize.max(1)),
    )
}

pub fn queue_kind(name: &str) -> OsalQueueKind {
    let name = canonical_mq_name(name);

    match name.as_str() {
        TE_TERM_MQ => OsalQueueKind::RtosTerminalInbound,
        LI_TERM_MQ => OsalQueueKind::RtosTerminalOutbound,
        OSAL_CB_HDR_LI_MAIN => OsalQueueKind::OsalCallbackMain,
        OSAL_CB_HDR_TE => OsalQueueKind::OsalCallbackTe,
        DP_MASTER => OsalQueueKind::DatapoolMaster,
        _ if name.starts_with(NOIOSC_CB_HDR_LI_PREFIX) => OsalQueueKind::LinuxLocalCallback,
        _ => OsalQueueKind::Unknown,
    }
}

pub fn is_rtos_terminal_inbound_queue(name: &str) -> bool {
    matches!(queue_kind(name), OsalQueueKind::RtosTerminalInbound)
}

pub fn is_rtos_terminal_outbound_queue(name: &str) -> bool {
    matches!(queue_kind(name), OsalQueueKind::RtosTerminalOutbound)
}

pub fn make_terminal_message(command: u8) -> Vec<u8> {
    let mut data = vec![command];
    data.resize(TERM_MQ_MSGSIZE as usize, 0);
    data
}

pub fn make_terminal_command_message(command: u8, payload: &[u8]) -> Vec<u8> {
    let mut data = vec![0u8; 4];
    data[0] = command;
    data.extend_from_slice(payload);
    data.resize(TERM_MQ_MSGSIZE as usize, 0);
    data
}

pub fn make_callback_command_message(command: u8, payload: &[u8]) -> Vec<u8> {
    let mut data = vec![0, 0, command];
    data.extend_from_slice(payload);
    data.resize(OSAL_CB_HDR_MSGSIZE.max(TERM_MQ_MSGSIZE) as usize, 0);
    data
}

pub fn message_command(data: &[u8]) -> Option<u8> {
    data.first().copied()
}

pub fn callback_message_command(data: &[u8]) -> Option<u8> {
    data.get(2).copied()
}

pub fn message_words(data: &[u8]) -> [u32; 4] {
    let mut words = [0u32; 4];
    for (index, word) in words.iter_mut().enumerate() {
        let offset = index * 4;
        if offset + 4 <= data.len() {
            *word = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
        }
    }
    words
}

pub fn osal_priority_to_mq_priority(priority: u32) -> u32 {
    OSAL_MQ_PRIORITY_MAX.saturating_sub(priority.min(OSAL_MQ_PRIORITY_MAX))
}

pub fn mq_priority_to_osal_priority(priority: u32) -> u32 {
    if priority <= OSAL_MQ_PRIORITY_MAX {
        OSAL_MQ_PRIORITY_MAX - priority
    } else {
        0
    }
}
