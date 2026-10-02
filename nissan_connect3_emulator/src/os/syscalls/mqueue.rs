//! POSIX message queues (mq_* syscalls #274-#282), following Linux 2.6.32
//! `ipc/mqueue.c` semantics.
//!
//! The guest (libosal, via glibc 2.8 librt) uses:
//! - `mq_open("/<name>", O_RDWR|O_CREAT|O_EXCL, 0770, &attr)` to create a queue
//!   (`OSAL_s32MessageQueueCreate`), and `mq_open("/<name>", O_RDWR, 0770, NULL)`
//!   to open an existing one (`u32GetValidMqHandle`)
//! - `mq_send`/`mq_timedsend` (absolute CLOCK_REALTIME timeout) to post
//! - `mq_receive`/`mq_timedreceive` (absolute timeout) to wait
//! - `mq_notify` with SIGEV_THREAD: glibc passes `sigev_value` pointing at a
//!   cookie `{_function, sigval, ...}` and `sigev_signo` = netlink fd. We do not
//!   emulate glibc's rt-thread/netlink plumbing, so when the notification fires
//!   we spawn a guest thread that calls `_function(sigval)` directly (the kernel
//!   hands the cookie to glibc which does exactly this, minus the netlink hop)
//!
//! Blocking send/receive: the guest thread is parked with
//! `BlockReason::MqSend`/`MqReceive`; a peer operation (or the deadline)
//! completes it and stores the syscall result in `GuestThread::pending_result`,
//! which the scheduler installs into R0 on the next switch-in.

use crate::emulator::context::Context;
use crate::emulator::memory_map::{MQ_NOTIFY_EXIT_STUB, STACK_SIZE};
use crate::emulator::thread::{BlockReason, GuestThread, ThreadAction, ThreadStatus};
use crate::emulator::utils::{pack_u32, read_string, unpack_u32};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};
use unicorn_engine::unicorn_const::Prot;
use unicorn_engine::{RegisterARM, Unicorn};

const O_ACCMODE: u32 = 0o0003;
const O_CREAT: u32 = 0o0100;
const O_EXCL: u32 = 0o0200;
const O_NONBLOCK: u32 = 0o0400;

// include/linux/mqueue.h + include/linux/ipc_namespace.h
const MQ_PRIO_MAX: u32 = 32768;
const DFLT_MSGMAX: i64 = 10;
const DFLT_MSGSIZEMAX: i64 = 8192;
const NAME_MAX: usize = 255;

// arm _NSIG
const SIGMAX: i32 = 64;

const EAGAIN: u32 = -11i32 as u32;
const EBADF: u32 = -9i32 as u32;
const EBUSY: u32 = -16i32 as u32;
const EEXIST: u32 = -17i32 as u32;
const EINVAL: u32 = -22i32 as u32;
const EMSGSIZE: u32 = -90i32 as u32;
const ENAMETOOLONG: u32 = -36i32 as u32;
const ENOENT: u32 = -2i32 as u32;
const ETIMEDOUT: u32 = -110i32 as u32;

#[derive(Clone, Copy)]
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

pub struct MqState {
    pub queues: HashMap<u32, MqQueue>,
    pub name_to_id: HashMap<String, u32>,
    /// queue id -> blocked guest tids (senders and receivers share the list,
    /// distinguished by their BlockReason)
    pub waiters: HashMap<u32, Vec<u32>>,
    next_id: u32,
}

/// mq handles must not collide with file-system fds (glibc's mq_close() uses
/// the plain close syscall, which dispatches on the number); file fds are
/// allocated from 0 upwards (MountFileSystem::get_unique_fd), so start well
/// above any realistic simultaneous fd count
const MQ_HANDLE_BASE: u32 = 1000;

impl MqState {
    pub fn new() -> Self {
        Self {
            queues: HashMap::new(),
            name_to_id: HashMap::new(),
            waiters: HashMap::new(),
            next_id: MQ_HANDLE_BASE,
        }
    }

    fn alloc_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }
}

/// outcome of converting an absolute CLOCK_REALTIME timeout
enum Deadline {
    /// no timeout passed - block indefinitely
    Infinite,
    /// the timespec is invalid (kernel: prepare_timeout -> -EINVAL)
    Invalid,
    At(Instant),
}

fn read_deadline(unicorn: &Unicorn<'_, Context>, addr: u32) -> Deadline {
    if addr == 0 {
        return Deadline::Infinite;
    }

    let mut buf = [0u8; 4];
    unicorn.mem_read(addr as u64, &mut buf).unwrap();
    let secs = unpack_u32(&buf) as i64;
    unicorn.mem_read(addr as u64 + 4, &mut buf).unwrap();
    let nsecs = unpack_u32(&buf) as i64;

    if secs < 0 || nsecs < 0 || nsecs >= 1_000_000_000 {
        return Deadline::Invalid;
    }

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let now_nanos = now.as_secs() as i128 * 1_000_000_000 + now.subsec_nanos() as i128;
    let abs_nanos = secs as i128 * 1_000_000_000 + nsecs as i128;

    match abs_nanos - now_nanos {
        delta if delta < 0 => Deadline::At(Instant::now()), // already expired
        delta => Deadline::At(Instant::now()
            + Duration::new(delta as u64 / 1_000_000_000, (delta % 1_000_000_000) as u32)),
    }
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

/// mq_open(name, oflag, mode, attr) - ipc/mqueue.c SYSCALL_DEFINE4(mq_open)
pub fn mq_open(
    unicorn: &mut Unicorn<'_, Context>,
    name_addr: u32,
    oflag: u32,
    _mode: u32,
    attr_addr: u32,
) -> u32 {
    let name = read_string(unicorn, name_addr);
    log::trace!(
        "{:#x}: [{}] [SYSCALL] mq_open(name = \"{}\", oflag: {:#x}, mode: {:#x}, attr: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        name,
        oflag,
        _mode,
        attr_addr,
    );

    // kernel: mqueue names start with '/' and contain no other '/'
    let res = if !name.starts_with('/') || name[1..].contains('/') {
        EINVAL
    } else if name.len() > NAME_MAX {
        ENAMETOOLONG
    } else if (oflag & O_ACCMODE) == 3 {
        // (O_RDWR | O_WRONLY) is not a valid access mode (do_open)
        EINVAL
    } else {
        let attr = if attr_addr != 0 {
            let mut buf = [0u8; 8];
            unicorn.mem_read(attr_addr as u64 + 4, &mut buf).unwrap();
            Some((unpack_u32(&buf[0..4]) as i64, unpack_u32(&buf[4..8]) as i64))
        } else {
            None
        };

        let data = unicorn.get_data();
        let mut state = data.sys_calls_state.lock().unwrap();

        if let Some(&id) = state.mq.name_to_id.get(&name) {
            // entry already exists (mq_open: -EEXIST only when O_EXCL is set,
            // otherwise the existing queue is just opened)
            if oflag & O_CREAT != 0 && oflag & O_EXCL != 0 {
                EEXIST
            } else {
                let queue = state.mq.queues.get_mut(&id).unwrap();
                queue.open_count += 1;
                queue.nonblock |= oflag & O_NONBLOCK != 0;
                id
            }
        } else if oflag & O_CREAT != 0 {
            // mqueue_get_inode: attr validation + defaults
            let (maxmsg, msgsize) = match attr {
                Some((m, s)) => (m, s),
                None => (DFLT_MSGMAX, DFLT_MSGSIZEMAX),
            };
            if maxmsg <= 0 || msgsize <= 0 {
                return EINVAL;
            }

            let id = state.mq.alloc_id();
            state
                .mq
                .queues
                .insert(
                    id,
                    MqQueue {
                        name: name.clone(),
                        maxmsg,
                        msgsize,
                        nonblock: oflag & O_NONBLOCK != 0,
                        messages: Vec::new(),
                        open_count: 1,
                        unlinked: false,
                        notify_owner: None,
                        notify: None,
                    },
                );
            state.mq.name_to_id.insert(name, id);
            id
        } else {
            ENOENT
        }
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] mq_open => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        res,
    );

    res
}

/// find a guest thread blocked in mq_receive on `queue_id`
fn find_blocked_receiver(unicorn: &Unicorn<'_, Context>, queue_id: u32) -> Option<(u32, u32, u32)> {
    let threads = unicorn.get_data().threads.lock().unwrap();
    threads.iter().find_map(|t| {
        if let ThreadStatus::Blocked(BlockReason::MqReceive {
            queue_id: qid,
            msg_ptr,
            prio_ptr,
            ..
        }) = t.status
        {
            if qid == queue_id {
                return Some((t.id, msg_ptr, prio_ptr));
            }
        }
        None
    })
}

/// find a guest thread blocked in mq_timedsend on `queue_id`
fn find_blocked_sender(unicorn: &Unicorn<'_, Context>, queue_id: u32) -> Option<(u32, u32, u32, u32)> {
    let threads = unicorn.get_data().threads.lock().unwrap();
    threads.iter().find_map(|t| {
        if let ThreadStatus::Blocked(BlockReason::MqSend {
            queue_id: qid,
            msg_ptr,
            msg_len,
            priority,
            ..
        }) = t.status
        {
            if qid == queue_id {
                return Some((t.id, msg_ptr, msg_len, priority));
            }
        }
        None
    })
}

fn remove_waiter(unicorn: &Unicorn<'_, Context>, queue_id: u32, tid: u32) {
    let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
    if let Some(list) = state.mq.waiters.get_mut(&queue_id) {
        list.retain(|&waiter| waiter != tid);
        if list.is_empty() {
            state.mq.waiters.remove(&queue_id);
        }
    }
}

fn set_runnable_with_result(unicorn: &Unicorn<'_, Context>, tid: u32, result: u32) {
    let mut threads = unicorn.get_data().threads.lock().unwrap();
    if let Some(thread) = threads.iter_mut().find(|t| t.id == tid) {
        if matches!(thread.status, ThreadStatus::Blocked(_)) {
            thread.status = ThreadStatus::Runnable;
            thread.pending_result = Some(result);
        }
    }
}

/// take the message from the queue (highest priority first)
fn pop_message(unicorn: &Unicorn<'_, Context>, queue_id: u32) -> Option<MqMessage> {
    let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
    state.mq.queues.get_mut(&queue_id)?.messages.pop()
}

/// insert a message; if the queue just became non-empty and a notification is
/// registered, take it (kernel: __do_notify fires only on empty -> not empty
/// with no synchronous receiver waiting, then unregisters)
fn insert_message_and_take_notify(
    unicorn: &Unicorn<'_, Context>,
    queue_id: u32,
    data: Vec<u8>,
    priority: u32,
) -> Option<MqNotify> {
    let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
    let queue = state.mq.queues.get_mut(&queue_id)?;
    insert_message(&mut queue.messages, data, priority);
    if queue.messages.len() == 1 {
        if queue.notify_owner.take().is_some() {
            return queue.notify.take();
        }
    }
    None
}

fn has_free_slot(unicorn: &Unicorn<'_, Context>, queue_id: u32) -> bool {
    let state = unicorn.get_data().sys_calls_state.lock().unwrap();
    match state.mq.queues.get(&queue_id) {
        Some(queue) => queue.messages.len() < queue.maxmsg as usize,
        None => false,
    }
}

/// deliver a registered notification (called with no locks held)
fn fire_notification(unicorn: &mut Unicorn<'_, Context>, notify: MqNotify) {
    match notify {
        MqNotify::SigevNone => {}
        MqNotify::Signal { signo, sigval } => {
            // signal delivery is not implemented yet - the notification is
            // consumed (kernel unregisters after firing) but the signal is lost
            log::warn!(
                "mq_notify SIGEV_SIGNAL fired (signo = {}, sigval = {:#x}) - signal delivery not implemented",
                signo,
                sigval,
            );
        }
        MqNotify::Thread { function, sigval } => {
            spawn_notify_thread(unicorn, function, sigval);
        }
    }
}

/// spawn a guest thread that runs the SIGEV_THREAD notification function with
/// `sigval` in R0; when it returns, the thread exits (kernel: glibc's rt
/// thread calls the user function and then terminates)
fn spawn_notify_thread(unicorn: &mut Unicorn<'_, Context>, function: u32, sigval: u32) {
    let tid = unicorn.get_data().next_thread_id.fetch_add(1, Ordering::Relaxed);

    // fresh stack for the notification thread
    let mmu_arc = unicorn.get_data().mmu.clone();
    let stack_base = mmu_arc
        .lock()
        .unwrap()
        .heap_alloc(unicorn, STACK_SIZE, Prot::READ | Prot::WRITE, "mq-notify");
    unicorn
        .mem_write(stack_base as u64, &vec![0u8; STACK_SIZE as usize])
        .unwrap();

    // snapshot the current (caller) CPU state, set up the child registers on
    // the shared vCPU, snapshot again, then restore the caller state
    let parent_context = unicorn.context_init().unwrap();
    unicorn
        .reg_write(RegisterARM::SP as i32, (stack_base + STACK_SIZE - 8) as u64)
        .unwrap();
    unicorn.reg_write(RegisterARM::R0 as i32, sigval as u64).unwrap();
    unicorn
        .reg_write(RegisterARM::LR as i32, MQ_NOTIFY_EXIT_STUB as u64)
        .unwrap();
    let child_context = unicorn.context_init().unwrap();
    unicorn.context_restore(&parent_context).unwrap();

    {
        let threads = unicorn.get_data().threads.clone();
        threads.lock().unwrap().push(GuestThread {
            id: tid,
            status: ThreadStatus::Runnable,
            cpu_context: Some(child_context),
            pc: function,
            pending_result: None,
        });
    }

    log::info!(
        "========== mq_notify thread [{}] (function = {:#x}, sigval = {:#x}) ==========",
        tid,
        function,
        sigval,
    );
}

/// hand the sender's message directly to a blocked receiver (kernel:
/// pipelined_send) - returns true if a receiver was completed
fn complete_blocked_receiver(
    unicorn: &mut Unicorn<'_, Context>,
    queue_id: u32,
    msg_ptr: u32,
    msg_len: u32,
    priority: u32,
) -> bool {
    let Some((tid, dst_ptr, prio_ptr)) = find_blocked_receiver(unicorn, queue_id) else {
        return false;
    };

    remove_waiter(unicorn, queue_id, tid);

    let mut buf = vec![0u8; msg_len as usize];
    unicorn.mem_read(msg_ptr as u64, &mut buf).unwrap();
    unicorn.mem_write(dst_ptr as u64, &buf).unwrap();
    if prio_ptr != 0 {
        unicorn.mem_write(prio_ptr as u64, &pack_u32(priority)).unwrap();
    }
    set_runnable_with_result(unicorn, tid, buf.len() as u32);
    true
}

/// give a freed slot to a blocked sender (kernel: pipelined_receive - the
/// sender's message is re-read from its still-valid buffer and inserted)
fn complete_blocked_sender(unicorn: &mut Unicorn<'_, Context>, queue_id: u32) {
    let Some((tid, msg_ptr, msg_len, priority)) = find_blocked_sender(unicorn, queue_id) else {
        return;
    };

    remove_waiter(unicorn, queue_id, tid);

    let mut buf = vec![0u8; msg_len as usize];
    unicorn.mem_read(msg_ptr as u64, &mut buf).unwrap();
    // no notification here - the kernel's pipelined_receive does not call
    // __do_notify
    {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        if let Some(queue) = state.mq.queues.get_mut(&queue_id) {
            insert_message(&mut queue.messages, buf, priority);
        }
    }
    set_runnable_with_result(unicorn, tid, 0);
}

/// mq_timedsend(mqdes, msg_ptr, msg_len, msg_prio, abs_timeout) -
/// ipc/mqueue.c SYSCALL_DEFINE5(mq_timedsend). Also serves as mq_send
/// (glibc passes a NULL timeout).
pub fn mq_timedsend(
    unicorn: &mut Unicorn<'_, Context>,
    mqdes: u32,
    msg_ptr: u32,
    msg_len: u32,
    msg_prio: u32,
    timeout_addr: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] mq_timedsend(mqdes = {:#x}, msg_ptr: {:#x}, msg_len: {:#x}, msg_prio: {:#x}, timeout: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        mqdes,
        msg_ptr,
        msg_len,
        msg_prio,
        timeout_addr,
    );

    if msg_prio >= MQ_PRIO_MAX {
        return EINVAL;
    }

    let deadline = read_deadline(unicorn, timeout_addr);

    {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        let Some(queue) = state.mq.queues.get_mut(&mqdes) else {
            return EBADF;
        };
        if (msg_len as i64) > queue.msgsize {
            return EMSGSIZE;
        }
        if queue.messages.len() >= queue.maxmsg as usize {
            // queue is full
            if queue.nonblock {
                return EAGAIN;
            }
            match deadline {
                Deadline::Invalid => return EINVAL,
                Deadline::At(at) if at <= Instant::now() => return ETIMEDOUT,
                _ => {}
            }

            // block until a receiver frees a slot (or the deadline passes)
            let tid = unicorn.get_data().thread_id();
            state.mq.waiters.entry(mqdes).or_default().push(tid);
            drop(state);

            unicorn.get_data().set_action(ThreadAction::Block(BlockReason::MqSend {
                queue_id: mqdes,
                msg_ptr,
                msg_len,
                priority: msg_prio,
                deadline: match &deadline {
                    Deadline::At(at) => Some(*at),
                    _ => None,
                },
            }));
            return 0; // overwritten by pending_result when the wait completes
        }
    }

    // there is a free slot: either hand the message straight to a waiting
    // receiver (pipelined_send) or insert it into the queue
    if !complete_blocked_receiver(unicorn, mqdes, msg_ptr, msg_len, msg_prio) {
        let mut buf = vec![0u8; msg_len as usize];
        unicorn.mem_read(msg_ptr as u64, &mut buf).unwrap();
        if let Some(notify) = insert_message_and_take_notify(unicorn, mqdes, buf, msg_prio) {
            fire_notification(unicorn, notify);
        }
    }

    log::trace!(
        "{:#x}: [{}] [SYSCALL] mq_timedsend => 0",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
    );

    0
}

/// mq_timedreceive(mqdes, msg_ptr, msg_len, msg_prio, abs_timeout) -
/// ipc/mqueue.c SYSCALL_DEFINE5(mq_timedreceive). ARM has no separate
/// mq_receive syscall: glibc's mq_receive() calls this one with a NULL timeout.
pub fn mq_timedreceive(
    unicorn: &mut Unicorn<'_, Context>,
    mqdes: u32,
    msg_ptr: u32,
    msg_len: u32,
    prio_ptr: u32,
    timeout_addr: u32,
) -> u32 {
    do_mq_timedreceive(unicorn, mqdes, msg_ptr, msg_len, prio_ptr, timeout_addr)
}

fn do_mq_timedreceive(
    unicorn: &mut Unicorn<'_, Context>,
    mqdes: u32,
    msg_ptr: u32,
    msg_len: u32,
    prio_ptr: u32,
    timeout_addr: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] mq_timedreceive(mqdes = {:#x}, msg_ptr: {:#x}, msg_len: {:#x}, prio_ptr: {:#x}, timeout: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        mqdes,
        msg_ptr,
        msg_len,
        prio_ptr,
        timeout_addr,
    );

    let deadline = read_deadline(unicorn, timeout_addr);

    {
        let state = unicorn.get_data().sys_calls_state.lock().unwrap();
        let Some(queue) = state.mq.queues.get(&mqdes) else {
            return EBADF;
        };
        // the buffer must be able to hold any message (kernel check)
        if (msg_len as i64) < queue.msgsize {
            return EMSGSIZE;
        }
    }

    if let Some(message) = pop_message(unicorn, mqdes) {
        unicorn.mem_write(msg_ptr as u64, &message.data).unwrap();
        if prio_ptr != 0 {
            unicorn.mem_write(prio_ptr as u64, &pack_u32(message.priority)).unwrap();
        }
        // there is now a free slot - a blocked sender may proceed
        complete_blocked_sender(unicorn, mqdes);

        let res = message.data.len() as u32;
        log::trace!(
            "{:#x}: [{}] [SYSCALL] mq_timedreceive => {:#x}",
            unicorn.reg_read(RegisterARM::PC).unwrap(),
            unicorn.get_data().thread_id(),
            res,
        );
        return res;
    }

    // queue is empty
    let nonblock = {
        let state = unicorn.get_data().sys_calls_state.lock().unwrap();
        state.mq.queues.get(&mqdes).map(|q| q.nonblock).unwrap_or(false)
    };
    if nonblock {
        return EAGAIN;
    }
    match deadline {
        Deadline::Invalid => return EINVAL,
        Deadline::At(at) if at <= Instant::now() => return ETIMEDOUT,
        _ => {}
    }

    // block until a sender posts a message (or the deadline passes)
    let tid = unicorn.get_data().thread_id();
    {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        state.mq.waiters.entry(mqdes).or_default().push(tid);
    }

    unicorn.get_data().set_action(ThreadAction::Block(BlockReason::MqReceive {
        queue_id: mqdes,
        msg_ptr,
        msg_len,
        prio_ptr,
        deadline: match &deadline {
            Deadline::At(at) => Some(*at),
            _ => None,
        },
    }));

    0 // overwritten by pending_result when the wait completes
}

/// mq_close(mqdes) - ipc/mqueue.c mqueue_flush_file (open count handling)
pub fn mq_close(unicorn: &mut Unicorn<'_, Context>, mqdes: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] mq_close(mqdes = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        mqdes,
    );

    let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
    let Some(queue) = state.mq.queues.get_mut(&mqdes) else {
        return EBADF;
    };

    queue.open_count = queue.open_count.saturating_sub(1);
    // a queue is removed once unlinked and no longer open (mqueue_unlink +
    // the last close)
    let remove_now = queue.unlinked && queue.open_count == 0;
    let name = queue.name.clone();
    if remove_now {
        state.mq.name_to_id.remove(&name);
        state.mq.queues.remove(&mqdes);
    }

    log::trace!(
        "{:#x}: [{}] [SYSCALL] mq_close => 0",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
    );

    0
}

/// mq_unlink(name) - ipc/mqueue.c SYSCALL_DEFINE1(mq_unlink)
pub fn mq_unlink(unicorn: &mut Unicorn<'_, Context>, name_addr: u32) -> u32 {
    let name = read_string(unicorn, name_addr);
    log::trace!(
        "{:#x}: [{}] [SYSCALL] mq_unlink(name = \"{}\") [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        name,
    );

    let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
    let Some(&id) = state.mq.name_to_id.get(&name) else {
        return ENOENT;
    };

    let res = if let Some(queue) = state.mq.queues.get_mut(&id) {
        queue.unlinked = true;
        if queue.open_count == 0 {
            state.mq.name_to_id.remove(&name);
            state.mq.queues.remove(&id);
        }
        0
    } else {
        ENOENT
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] mq_unlink => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        res,
    );

    res
}

/// mq_getattr(mqdes, attr) / mq_setattr(mqdes, new, old) - ipc/mqueue.c
/// SYSCALL_DEFINE3(mq_getsetattr): new == NULL -> get only, old == NULL -> set
/// only
pub fn mq_getsetattr(
    unicorn: &mut Unicorn<'_, Context>,
    mqdes: u32,
    new_attr_addr: u32,
    old_attr_addr: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] mq_getsetattr(mqdes = {:#x}, new: {:#x}, old: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        mqdes,
        new_attr_addr,
        old_attr_addr,
    );

    // read the new flags first (guest memory access outside the state lock)
    let new_flags = if new_attr_addr != 0 {
        let mut buf = [0u8; 4];
        unicorn.mem_read(new_attr_addr as u64, &mut buf).unwrap();
        Some(unpack_u32(&buf))
    } else {
        None
    };

    // only mq_flags is writable (the kernel ignores the rest)
    let attr_out = {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        let Some(queue) = state.mq.queues.get_mut(&mqdes) else {
            return EBADF;
        };
        if let Some(flags) = new_flags {
            queue.nonblock = flags & O_NONBLOCK != 0;
        }
        if old_attr_addr != 0 {
            let mut out = [0u8; 32];
            out[0..4].copy_from_slice(&pack_u32(if queue.nonblock { O_NONBLOCK } else { 0 }));
            out[4..8].copy_from_slice(&pack_u32(queue.maxmsg as u32));
            out[8..12].copy_from_slice(&pack_u32(queue.msgsize as u32));
            out[12..16].copy_from_slice(&pack_u32(queue.messages.len() as u32));
            Some(out)
        } else {
            None
        }
    };

    if let Some(out) = attr_out {
        unicorn.mem_write(old_attr_addr as u64, &out).unwrap();
    }

    log::trace!(
        "{:#x}: [{}] [SYSCALL] mq_getsetattr => 0",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
    );

    0
}

/// mq_notify(mqdes, notification) - ipc/mqueue.c SYSCALL_DEFINE2(mq_notify).
/// The struct layout (include/asm-generic/siginfo.h sigevent_t):
///   +0 sigev_value, +4 sigev_signo, +8 sigev_notify, +12 _function, +16 _attr.
/// For SIGEV_THREAD glibc puts a cookie {_function, sigval} at sigev_value and
/// a netlink fd in sigev_signo; we read the cookie and ignore the fd.
pub fn mq_notify(unicorn: &mut Unicorn<'_, Context>, mqdes: u32, notif_addr: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] mq_notify(mqdes = {:#x}, notification: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        mqdes,
        notif_addr,
    );

    let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
    let Some(queue) = state.mq.queues.get_mut(&mqdes) else {
        return EBADF;
    };

    if notif_addr == 0 {
        // deregister - silently ignored when the caller is not the owner
        // (kernel comment in mq_notify)
        if queue.notify_owner == Some(unicorn.get_data().thread_id()) {
            queue.notify_owner = None;
            queue.notify = None;
        }
        return 0;
    }

    let mut buf = [0u8; 12];
    unicorn.mem_read(notif_addr as u64, &mut buf).unwrap();
    let value = unpack_u32(&buf[0..4]);
    let signo = unpack_u32(&buf[4..8]) as i32;
    let notify = unpack_u32(&buf[8..12]);

    let kind = match notify {
        0 => MqNotify::SigevNone, // SIGEV_NONE
        1 => {
            // SIGEV_SIGNAL
            if signo < 1 || signo > SIGMAX {
                return EINVAL;
            }
            MqNotify::Signal { signo, sigval: value }
        }
        2 => {
            // SIGEV_THREAD - read the glibc cookie at sigev_value
            let mut cookie = [0u8; 8];
            unicorn.mem_read(value as u64, &mut cookie).unwrap();
            MqNotify::Thread {
                function: unpack_u32(&cookie[0..4]),
                sigval: unpack_u32(&cookie[4..8]),
            }
        }
        _ => return EINVAL,
    };

    if queue.notify_owner.is_some() {
        return EBUSY;
    }

    queue.notify_owner = Some(unicorn.get_data().thread_id());
    queue.notify = Some(kind);

    0
}

/// re-evaluate a guest thread whose MqSend/MqReceive wait expired (called from
/// the scheduler's wake_expired, with no locks held). Mirrors the kernel's
/// wq_sleep timeout path: re-check the queue, then either complete the
/// operation or return -ETIMEDOUT. The result is parked in
/// `GuestThread::pending_result` for the next switch-in.
pub fn finish_mq_wait(unicorn: &mut Unicorn<'_, Context>, tid: u32) {
    let reason = {
        let threads = unicorn.get_data().threads.lock().unwrap();
        threads.iter().find(|t| t.id == tid).and_then(|t| match t.status {
            ThreadStatus::Blocked(reason) => Some(reason),
            _ => None,
        })
    };

    let res: u32 = match reason {
        Some(BlockReason::MqReceive {
            queue_id,
            msg_ptr,
            prio_ptr,
            ..
        }) => {
            remove_waiter(unicorn, queue_id, tid);
            if let Some(message) = pop_message(unicorn, queue_id) {
                unicorn.mem_write(msg_ptr as u64, &message.data).unwrap();
                if prio_ptr != 0 {
                    unicorn
                        .mem_write(prio_ptr as u64, &pack_u32(message.priority))
                        .unwrap();
                }
                complete_blocked_sender(unicorn, queue_id);
                message.data.len() as u32
            } else {
                ETIMEDOUT
            }
        }
        Some(BlockReason::MqSend {
            queue_id,
            msg_ptr,
            msg_len,
            priority,
            ..
        }) => {
            remove_waiter(unicorn, queue_id, tid);
            if has_free_slot(unicorn, queue_id) {
                let mut buf = vec![0u8; msg_len as usize];
                unicorn.mem_read(msg_ptr as u64, &mut buf).unwrap();
                let notify = insert_message_and_take_notify(unicorn, queue_id, buf, priority);
                if let Some(notify) = notify {
                    fire_notification(unicorn, notify);
                }
                0
            } else {
                ETIMEDOUT
            }
        }
        _ => return, // not an mq wait - nothing to do
    };

    set_runnable_with_result(unicorn, tid, res);
}
