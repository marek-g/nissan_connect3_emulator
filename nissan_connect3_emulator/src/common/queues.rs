//! Shared message-queue model used by both the guest OS POSIX mqueue layer and
//! the host-side RTOS simulator.
//!
//! The state here is deliberately independent of Unicorn/guest memory: it can be
//! manipulated by guest syscall handlers (which copy bytes from their own VM) and
//! by the host RTOS thread (which only uses host-provided payloads).

use std::collections::HashMap;

pub const TE_TERM_MQ: &str = "TE_TERM_MQ";
pub const LI_TERM_MQ: &str = "LI_TERM_MQ";
pub const OSAL_CB_HDR_LI_MAIN: &str = "OSAL_CB_HDR_LI_MAIN";
pub const OSAL_CB_HDR_TE: &str = "OSAL_CB_HDR_TE";
pub const NOIOSC_CB_HDR_LI_PREFIX: &str = "NOIOSC_CB_HDR_LI_";
pub const DP_MASTER: &str = "DpMaster";

pub const DEFAULT_READY_QUEUE: &str = "NOIOSC_CB_HDR_LI_0";

pub const TERM_MQ_MAXMSG: i64 = 10;
pub const TERM_MQ_MSGSIZE: i64 = 0x50;
pub const OSAL_CB_HDR_MAXMSG: i64 = 0xf0;
pub const OSAL_CB_HDR_LI_MAIN_MAXMSG: i64 = 0xf0;
pub const OSAL_CB_HDR_TE_MAXMSG: i64 = 0x78;
pub const OSAL_CB_HDR_MSGSIZE: i64 = 0x50;

const MQ_HANDLE_BASE: u32 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueCreator {
    Guest,
    Rtos,
}

#[derive(Clone, Copy, Debug)]
pub enum MqNotify {
    SigevNone,
    Signal { signo: i32, sigval: u32 },
    Thread { function: u32, sigval: u32 },
}

pub struct MqMessage {
    pub data: Vec<u8>,
    pub priority: u32,
}

pub struct MqQueue {
    pub name: String,
    pub maxmsg: i64,
    pub msgsize: i64,
    /// O_NONBLOCK was passed by any opener (the kernel checks it per fd; the
    /// firmware only ever opens with O_RDWR)
    pub nonblock: bool,
    /// creator that currently owns the queue object. The RTOS simulator creates
    /// boot queues before the Linux processes run, so readiness checks must not
    /// treat a mere queue name as proof that Linux/libosal has initialized.
    pub creator: QueueCreator,
    /// number of successful guest `mq_open` calls; used to distinguish a queue
    /// only pre-created by the RTOS simulator from one actually opened by Linux
    pub guest_open_count: u32,
    /// sorted by ascending priority (index 0 = lowest); pop from the back for
    /// the highest priority, FIFO within equal priorities (msg_insert in
    /// ipc/mqueue.c)
    pub messages: Vec<MqMessage>,
    pub open_count: u32,
    pub unlinked: bool,
    /// guest tid that registered the notification (kernel: notify_owner)
    pub notify_owner: Option<u32>,
    pub notify: Option<MqNotify>,
}

impl MqQueue {
    fn new(name: String, maxmsg: i64, msgsize: i64, nonblock: bool, creator: QueueCreator) -> Self {
        Self {
            name,
            maxmsg,
            msgsize,
            nonblock,
            creator,
            guest_open_count: if creator == QueueCreator::Guest { 1 } else { 0 },
            messages: Vec::new(),
            open_count: 1,
            unlinked: false,
            notify_owner: None,
            notify: None,
        }
    }
}

pub struct MqState {
    pub queues: HashMap<u32, MqQueue>,
    pub name_to_id: HashMap<String, u32>,
    /// queue id -> blocked guest tids (senders and receivers share the list,
    /// distinguished by their BlockReason)
    pub waiters: HashMap<u32, Vec<u32>>,
    /// bytes staged by a blocked sender, keyed by tid: `(queue_id, data, priority)`.
    /// The sender copies its message out of its own (private) address space when
    /// it blocks; whichever host thread later frees a slot moves these staged
    /// bytes into the queue, so a sender in one VM never needs its memory read by
    /// another process' VM.
    pub staged: HashMap<u32, (u32, Vec<u8>, u32)>,
    next_id: u32,
}

impl MqState {
    pub fn new() -> Self {
        Self {
            queues: HashMap::new(),
            name_to_id: HashMap::new(),
            waiters: HashMap::new(),
            staged: HashMap::new(),
            next_id: MQ_HANDLE_BASE,
        }
    }

    pub fn alloc_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    pub fn queue_id_for_name(&self, name: &str) -> Option<u32> {
        self.name_to_id.get(&canonical_mq_name(name)).copied()
    }

    pub fn guest_open_count_for_name(&self, name: &str) -> Option<u32> {
        self.queue_id_for_name(name)
            .and_then(|id| self.queues.get(&id))
            .map(|queue| queue.guest_open_count)
    }

    pub fn has_guest_ready_queue(&self, configured: &str) -> bool {
        self.queues
            .values()
            .any(|queue| queue.guest_open_count > 0 && is_boot_ready_queue(&queue.name, configured))
    }

    /// Open an existing queue as a guest opener, or create one if requested.
    /// Returns the queue id and `true` when a new queue was created.
    pub fn guest_open_or_create(
        &mut self,
        name: &str,
        maxmsg: i64,
        msgsize: i64,
        create: bool,
        exclusive: bool,
        nonblock: bool,
    ) -> MqGuestOpen {
        let name = canonical_mq_name(name);

        if let Some(&id) = self.name_to_id.get(&name) {
            if create && exclusive {
                return MqGuestOpen::AlreadyExists;
            }

            let queue = self
                .queues
                .get_mut(&id)
                .expect("name_to_id points to a queue");
            queue.open_count += 1;
            queue.guest_open_count += 1;
            queue.nonblock |= nonblock;
            return MqGuestOpen::Opened(id);
        }

        if !create {
            return MqGuestOpen::NotFound;
        }

        if maxmsg <= 0 || msgsize <= 0 {
            return MqGuestOpen::InvalidAttrs;
        }

        let id = self.alloc_id();
        self.queues.insert(
            id,
            MqQueue::new(name.clone(), maxmsg, msgsize, nonblock, QueueCreator::Guest),
        );
        self.name_to_id.insert(name, id);
        MqGuestOpen::Created(id)
    }

    /// Pre-create (or open) a queue from the host-side RTOS simulator.
    pub fn rtos_open_or_create(&mut self, name: &str, maxmsg: i64, msgsize: i64) -> u32 {
        let name = canonical_mq_name(name);
        if let Some(&id) = self.name_to_id.get(&name) {
            let queue = self
                .queues
                .get_mut(&id)
                .expect("name_to_id points to a queue");
            queue.open_count += 1;
            return id;
        }

        let id = self.alloc_id();
        self.queues.insert(
            id,
            MqQueue::new(name.clone(), maxmsg, msgsize, false, QueueCreator::Rtos),
        );
        self.name_to_id.insert(name, id);
        id
    }

    /// Host-side post used by the RTOS simulator. The message length is padded
    /// to the queue's configured message size because libosal/OSAL receives
    /// fixed-size terminal/callback messages.
    pub fn host_post(&mut self, name: &str, mut data: Vec<u8>, priority: u32) -> bool {
        let Some(id) = self.queue_id_for_name(name) else {
            return false;
        };

        let Some(queue) = self.queues.get_mut(&id) else {
            return false;
        };
        if queue.messages.len() >= queue.maxmsg as usize {
            return false;
        }

        if (data.len() as i64) < queue.msgsize {
            data.resize(queue.msgsize as usize, 0);
        }
        if (data.len() as i64) > queue.msgsize {
            data.truncate(queue.msgsize as usize);
        }

        insert_message(&mut queue.messages, data, priority);
        true
    }

    /// Host-side non-blocking receive used by the RTOS simulator.
    pub fn host_receive(&mut self, name: &str) -> Option<MqMessage> {
        let id = self.queue_id_for_name(name)?;
        self.pop_message(id)
    }

    pub fn insert_message(&mut self, queue_id: u32, data: Vec<u8>, priority: u32) -> bool {
        if let Some(queue) = self.queues.get_mut(&queue_id) {
            insert_message(&mut queue.messages, data, priority);
            return true;
        }
        false
    }

    /// insert a message; if the queue just became non-empty and a notification is
    /// registered, take it (kernel: __do_notify fires only on empty -> not empty
    /// with no synchronous receiver waiting, then unregisters)
    pub fn insert_message_and_take_notify(
        &mut self,
        queue_id: u32,
        data: Vec<u8>,
        priority: u32,
    ) -> Option<(u32, MqNotify)> {
        let queue = self.queues.get_mut(&queue_id)?;
        insert_message(&mut queue.messages, data, priority);
        if queue.messages.len() != 1 {
            return None;
        }

        let owner = queue.notify_owner.take()?;
        let notify = queue.notify.take()?;
        Some((owner, notify))
    }

    pub fn pop_message(&mut self, queue_id: u32) -> Option<MqMessage> {
        self.queues.get_mut(&queue_id)?.messages.pop()
    }

    pub fn has_free_slot(&self, queue_id: u32) -> bool {
        self.queues
            .get(&queue_id)
            .map(|queue| queue.messages.len() < queue.maxmsg as usize)
            .unwrap_or(false)
    }

    pub fn queue_nonblock(&self, queue_id: u32) -> bool {
        self.queues
            .get(&queue_id)
            .map(|queue| queue.nonblock)
            .unwrap_or(false)
    }

    /// remove `tid` from a queue's waiter list
    pub fn remove_waiter(&mut self, queue_id: u32, tid: u32) {
        if let Some(list) = self.waiters.get_mut(&queue_id) {
            list.retain(|&waiter| waiter != tid);
            if list.is_empty() {
                self.waiters.remove(&queue_id);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MqGuestOpen {
    Opened(u32),
    Created(u32),
    AlreadyExists,
    NotFound,
    InvalidAttrs,
}

pub fn canonical_mq_name(name: &str) -> String {
    name.strip_prefix('/').unwrap_or(name).to_string()
}

pub fn is_boot_ready_queue(name: &str, configured: &str) -> bool {
    let name = canonical_mq_name(name);
    let configured = canonical_mq_name(configured);

    matches!(
        name.as_str(),
        "NOIOSC_CB_HDR_LI_0" | "OSAL_CB_HDR_LI_MAIN" | "TE_TERM_MQ" | "LI_TERM_MQ"
    ) || name.starts_with(NOIOSC_CB_HDR_LI_PREFIX)
        || name == configured
}

/// insert a message keeping the kernel's priority ordering (ipc/mqueue.c
/// msg_insert: higher priority towards the back of the vector)
fn insert_message(messages: &mut Vec<MqMessage>, data: Vec<u8>, priority: u32) {
    let pos = messages
        .iter()
        .rposition(|m| m.priority >= priority)
        .map(|i| i + 1)
        .unwrap_or(messages.len());
    messages.insert(pos, MqMessage { data, priority });
}
