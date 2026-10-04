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
    DP_MASTER, LI_TERM_MQ, NOIOSC_CB_HDR_LI_PREFIX, OSAL_CB_HDR_LI_MAIN, OSAL_CB_HDR_MAXMSG,
    OSAL_CB_HDR_MSGSIZE, OSAL_CB_HDR_TE, TE_TERM_MQ, TERM_MQ_MAXMSG, TERM_MQ_MSGSIZE,
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

pub const RTOS_INTERCEPTED_OSAL_QUEUES: &[&str] = &[
    TE_TERM_MQ,
    LI_TERM_MQ,
    OSAL_CB_HDR_TE,
    OSAL_CB_HDR_LI_MAIN,
];

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
        Self { name: name.into(), kind, maxmsg, msgsize }
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
        Self { data: message.data, priority: mq_priority_to_osal_priority(message.priority) }
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

    pub fn rtos_post_message(mq: &mut MqState, name: &str, data: Vec<u8>, osal_priority: u32) -> bool {
        mq.host_post(name, data, osal_priority_to_mq_priority(osal_priority))
    }

    pub fn post_terminal_message(mq: &mut MqState, name: &str, command: u8) -> bool {
        Self::rtos_post_message(mq, name, make_terminal_message(command), OSAL_MQ_PRIORITY_MAX)
    }

    pub fn post_rtos_terminal_ready(mq: &mut MqState) -> bool {
        Self::post_terminal_message(mq, LI_TERM_MQ, RTOS_TERMINAL_READY)
    }

    pub fn receive_rtos_terminal_message(mq: &mut MqState) -> Option<OsalMessage> {
        let message = mq.host_receive(TE_TERM_MQ)?;
        Some(OsalMessage::from_mq_message(message))
    }

    pub fn is_rtos_intercepted_queue(name: &str) -> bool {
        matches!(
            canonical_mq_name(name).as_str(),
            TE_TERM_MQ | LI_TERM_MQ | OSAL_CB_HDR_TE | OSAL_CB_HDR_LI_MAIN
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

        let spec = spec_for_name(&name)
            .unwrap_or_else(|| default_queue_spec(&name, OSAL_DEFAULT_MSGSIZE));
        mq.rtos_open_or_create(&spec.name, spec.maxmsg, spec.msgsize)
    }

    pub fn guest_post(mq: &mut MqState, name: &str, data: Vec<u8>, osal_priority: u32) -> bool {
        let name = canonical_mq_name(name);
        let requested_msgsize = data.len() as i64;
        let spec = spec_for_name(&name)
            .unwrap_or_else(|| default_queue_spec(&name, OSAL_DEFAULT_MSGSIZE.max(requested_msgsize)));

        let id = match mq.queue_id_for_name(&name) {
            Some(id) => id,
            None => mq.rtos_open_or_create(&name, spec.maxmsg, spec.msgsize.max(requested_msgsize)),
        };

        if mq.queues.get(&id).map(|queue| queue.msgsize).unwrap_or(0) < requested_msgsize {
            return false;
        }
        if !mq.has_free_slot(id) {
            return false;
        }

        let priority = osal_priority_to_mq_priority(osal_priority);
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

    pub fn pop_guest_message(mq: &mut MqState, queue_id: u32, max_len: usize) -> Option<OsalMessage> {
        let fits = mq
            .queues
            .get(&queue_id)
            .and_then(|queue| queue.messages.last())
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
            OSAL_CB_HDR_MAXMSG,
            OSAL_CB_HDR_MSGSIZE,
        ),
        OsalQueueSpec::new(
            OSAL_CB_HDR_TE,
            OsalQueueKind::OsalCallbackTe,
            OSAL_CB_HDR_MAXMSG,
            OSAL_CB_HDR_MSGSIZE,
        ),
    ]
}

pub fn spec_for_name(name: &str) -> Option<OsalQueueSpec> {
    let name = canonical_mq_name(name);
    let kind = queue_kind(&name);

    match kind {
        OsalQueueKind::RtosTerminalInbound | OsalQueueKind::RtosTerminalOutbound => {
            Some(OsalQueueSpec::new(name, kind, TERM_MQ_MAXMSG, TERM_MQ_MSGSIZE))
        }
        OsalQueueKind::OsalCallbackMain | OsalQueueKind::OsalCallbackTe => {
            Some(OsalQueueSpec::new(name, kind, OSAL_CB_HDR_MAXMSG, OSAL_CB_HDR_MSGSIZE))
        }
        OsalQueueKind::DatapoolMaster => Some(OsalQueueSpec::new(
            name,
            kind,
            OSAL_DEFAULT_MAXMSG,
            DP_MASTER_MSGSIZE,
        )),
        OsalQueueKind::LinuxLocalCallback => {
            Some(OsalQueueSpec::new(name, kind, OSAL_CB_HDR_MAXMSG, OSAL_CB_HDR_MSGSIZE))
        }
        OsalQueueKind::Unknown => None,
        OsalQueueKind::Iosc => {
            Some(OsalQueueSpec::new(name, kind, OSAL_DEFAULT_MAXMSG, OSAL_DEFAULT_MSGSIZE))
        }
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

pub fn message_command(data: &[u8]) -> Option<u8> {
    data.first().copied()
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
