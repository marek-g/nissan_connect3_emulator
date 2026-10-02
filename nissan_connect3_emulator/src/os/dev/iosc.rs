//! Emulation of the Bosch IOSC inter-process communication driver (`/dev/iosc`).
//!
//! On the target, `libiosclib_so.so` opens `/dev/iosc` once (`iosc_init`) and
//! issues `ioctl(fd, 0x534f00XX, &arg)` for every primitive (mutexes, events,
//! semaphores, ringbuffers, shared memory). The kernel driver keeps the IPC
//! state shared across processes. In this emulator all guest threads share one
//! address space, so the primitives are backed by ordinary Rust state in
//! [`IoscState`] and blocking operations use the same BlockReason /
//! pending_result machinery as the POSIX message queues.

use crate::emulator::context::Context;
use crate::emulator::thread::{BlockReason, ThreadAction, ThreadStatus};
use crate::emulator::utils::{pack_u32, unpack_u32};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use unicorn_engine::unicorn_const::Prot;
use unicorn_engine::{RegisterARM, Unicorn};

// ioctl command numbers (type 'OS' = 0x534f), recovered from libiosclib_so.so
const IOSC_SHARED_MALLOC: u32 = 0x534f0000;
const IOSC_CREATE_MUTEX: u32 = 0x534f000a;
const IOSC_CREATE_SEMAPHORE: u32 = 0x534f0002;
const IOSC_OBTAIN_SEMAPHORE: u32 = 0x534f0004;
const IOSC_RELEASE_SEMAPHORE: u32 = 0x534f0005;
const IOSC_SENDTO_RINGBUFFER: u32 = 0x534f0008;
const IOSC_ENTER_MUTEX: u32 = 0x534f000c;
const IOSC_LEAVE_MUTEX: u32 = 0x534f000d;
const IOSC_CREATE_EVENT: u32 = 0x534f000e;
const IOSC_WAIT_FOR_EVENT: u32 = 0x534f0010;
const IOSC_SET_EVENT: u32 = 0x534f0011;

// /dev/iosc file descriptors live in a reserved range so they never collide
// with filesystem fds (allocated from 0) or mq handles (from 1000)
const IOSC_FD_BASE: u32 = 2000;

/// IOSC failure codes surfaced by the driver
const EINVAL: u32 = (-22) as i32 as u32;
const ETIMEDOUT: u32 = (-110) as i32 as u32;

#[derive(Default)]
struct IoscMutex {
    locked: bool,
    waiters: Vec<u32>,
}

#[derive(Default)]
struct IoscEvent {
    value: u32,
    waiters: Vec<u32>,
}

#[derive(Default)]
struct IoscSemaphore {
    count: u32,
    waiters: Vec<u32>,
}

/// Shared state for the emulated IOSC driver. Lives in SysCallsState so every
/// guest thread (and, later, every process) sees the same objects.
pub struct IoscState {
    /// open `/dev/iosc` file descriptors
    pub fds: HashSet<u32>,
    next_fd: u32,
    /// next handle handed out by create_event / create_semaphore (unique across
    /// both so a handle always names exactly one object)
    next_handle: u32,
    mutexes: HashMap<u32, IoscMutex>,
    events: HashMap<u32, IoscEvent>,
    semaphores: HashMap<u32, IoscSemaphore>,
}

impl IoscState {
    pub fn new() -> Self {
        Self {
            fds: HashSet::new(),
            next_fd: IOSC_FD_BASE,
            next_handle: 0,
            mutexes: HashMap::new(),
            events: HashMap::new(),
            semaphores: HashMap::new(),
        }
    }

    fn alloc_fd(&mut self) -> u32 {
        self.next_fd += 1;
        self.next_fd
    }

    fn alloc_handle(&mut self) -> u32 {
        self.next_handle += 1;
        self.next_handle
    }
}

impl Default for IoscState {
    fn default() -> Self {
        Self::new()
    }
}

/// open("/dev/iosc") - iosc_init stores the returned fd in a global that all
/// subsequent iosc_* calls use.
pub fn open_iosc(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let fd = {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        state.iosc.alloc_fd()
    };
    log::trace!(
        "{:#x}: [{}] [IOSC] open(/dev/iosc) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        fd,
    );
    fd
}

/// close() interception for /dev/iosc fds
pub fn close_iosc(unicorn: &mut Unicorn<'_, Context>, fd: u32) -> u32 {
    let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
    state.iosc.fds.remove(&fd);
    log::trace!(
        "{:#x}: [{}] [IOSC] close({:#x}) => 0",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        fd,
    );
    0
}

/// true if `fd` is an open /dev/iosc descriptor
pub fn is_iosc_fd(unicorn: &Unicorn<'_, Context>, fd: u32) -> bool {
    unicorn
        .get_data()
        .sys_calls_state
        .lock()
        .unwrap()
        .iosc
        .fds
        .contains(&fd)
}

/// ioctl dispatch for /dev/iosc. `addr` points at the per-command argument
/// struct built by libiosclib on the guest stack.
pub fn ioctl(unicorn: &mut Unicorn<'_, Context>, fd: u32, request: u32, addr: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [IOSC] ioctl(fd={:#x}, request={:#x}, addr={:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        fd,
        request,
        addr,
    );

    let res = match request {
        IOSC_SHARED_MALLOC => shared_malloc(unicorn, addr),
        IOSC_CREATE_MUTEX => create_mutex(unicorn),
        IOSC_CREATE_SEMAPHORE => create_semaphore(unicorn, addr),
        IOSC_OBTAIN_SEMAPHORE => obtain_semaphore(unicorn, addr),
        IOSC_RELEASE_SEMAPHORE => release_semaphore(unicorn, addr),
        IOSC_ENTER_MUTEX => enter_mutex(unicorn, addr),
        IOSC_LEAVE_MUTEX => leave_mutex(unicorn, addr),
        IOSC_CREATE_EVENT => create_event(unicorn),
        IOSC_WAIT_FOR_EVENT => wait_for_event(unicorn, addr),
        IOSC_SET_EVENT => set_event(unicorn, addr),
        IOSC_SENDTO_RINGBUFFER => {
            log::warn!("[IOSC] sendto_ringbuffer not implemented yet");
            0
        }
        other => {
            log::warn!("[IOSC] unhandled ioctl request {:#x}", other);
            EINVAL
        }
    };

    log::trace!(
        "{:#x}: [{}] [IOSC] ioctl({:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().thread_id(),
        request,
        res,
    );
    res
}

// ---- argument-struct helpers ---------------------------------------------

fn read_u32(unicorn: &Unicorn<'_, Context>, addr: u32) -> u32 {
    let mut b = [0u8; 4];
    unicorn.mem_read(addr as u64, &mut b).unwrap();
    unpack_u32(&b)
}

fn read_i32(unicorn: &Unicorn<'_, Context>, addr: u32) -> i32 {
    read_u32(unicorn, addr) as i32
}

/// convert an IOSC timeout (milliseconds, -1 = infinite) into a deadline
fn timeout_to_deadline(ms: i32) -> Option<Instant> {
    if ms < 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(ms as u64))
    }
}

// ---- shared memory --------------------------------------------------------

/// iosc_shared_malloc_with_id: arg struct is
/// `{ result_ptr, id, size, out_ptr }` - the driver allocates `size` bytes of
/// zeroed shared memory and writes its base address to `*result_ptr`.
fn shared_malloc(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> u32 {
    let result_ptr = read_u32(unicorn, addr);
    let size = read_u32(unicorn, addr + 8);
    if size == 0 {
        return 0;
    }

    let base = {
        let mmu_arc = unicorn.get_data().mmu.clone();
        let base = mmu_arc.lock().unwrap().heap_alloc(
            unicorn,
            size,
            Prot::READ | Prot::WRITE,
            "iosc-shm",
        );
        base
    };
    unicorn
        .mem_write(base as u64, &vec![0u8; size as usize])
        .unwrap();
    if result_ptr != 0 {
        unicorn
            .mem_write(result_ptr as u64, &pack_u32(base))
            .unwrap();
    }
    base
}

// ---- events ---------------------------------------------------------------

/// iosc_create_event: no argument struct; the driver assigns and returns a
/// handle (the id passed by the caller is ignored).
fn create_event(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let handle = {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        let h = state.iosc.alloc_handle();
        state.iosc.events.insert(h, IoscEvent::default());
        h
    };
    handle
}

/// iosc_set_event(event_id, value): arg struct `{ event_id, value }`. A
/// non-zero value sets the event and wakes one waiter.
fn set_event(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> u32 {
    let event_id = read_u32(unicorn, addr);
    let value = read_u32(unicorn, addr + 4);

    let woken_tid = {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        match state.iosc.events.get_mut(&event_id) {
            Some(event) => {
                event.value = value;
                if value != 0 {
                    event.waiters.pop()
                } else {
                    None
                }
            }
            None => return EINVAL,
        }
    };

    if let Some(tid) = woken_tid {
        set_runnable_with_result(unicorn, tid, 0);
    }
    0
}

/// iosc_wait_for_event: arg struct `{ event_id, _, _, _, timeout }`. Blocks
/// until the event is set (then consumes it), returning 0.
fn wait_for_event(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> u32 {
    let event_id = read_u32(unicorn, addr);
    let deadline = timeout_to_deadline(read_i32(unicorn, addr + 16));

    // fast path: already set - consume and return
    {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        if let Some(event) = state.iosc.events.get_mut(&event_id) {
            if event.value != 0 {
                event.value = 0;
                return 0;
            }
        } else {
            return EINVAL;
        }
    }

    // slow path: block
    match deadline {
        Some(d) if d <= Instant::now() => return ETIMEDOUT,
        _ => {}
    }
    let tid = unicorn.get_data().thread_id();
    {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        if let Some(event) = state.iosc.events.get_mut(&event_id) {
            event.waiters.push(tid);
        }
    }
    unicorn
        .get_data()
        .set_action(ThreadAction::Block(BlockReason::IoscEvent {
            id: event_id,
            deadline,
        }));
    0 // overwritten by pending_result when the wait completes
}

// ---- mutexes --------------------------------------------------------------

/// iosc_create_mutex: no argument struct; the driver assigns and returns a new
/// mutex handle (positive on success). Mirrors create_event.
fn create_mutex(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let handle = {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        let h = state.iosc.alloc_handle();
        state.iosc.mutexes.insert(h, IoscMutex::default());
        h
    };
    handle
}

/// iosc_enter_mutex(mutex_id, timeout): arg struct `{ mutex_id, timeout }`.
fn enter_mutex(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> u32 {
    let mutex_id = read_u32(unicorn, addr);
    let deadline = timeout_to_deadline(read_i32(unicorn, addr + 4));

    // fast path: free (or new) - take it
    {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        match state.iosc.mutexes.get_mut(&mutex_id) {
            Some(m) if m.locked => {}
            _ => {
                state
                    .iosc
                    .mutexes
                    .entry(mutex_id)
                    .or_insert_with(IoscMutex::default)
                    .locked = true;
                return 0;
            }
        }
    }

    // slow path: held - block (or time out)
    match deadline {
        Some(d) if d <= Instant::now() => return ETIMEDOUT,
        _ => {}
    }
    let tid = unicorn.get_data().thread_id();
    {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        if let Some(m) = state.iosc.mutexes.get_mut(&mutex_id) {
            m.waiters.push(tid);
        }
    }
    unicorn
        .get_data()
        .set_action(ThreadAction::Block(BlockReason::IoscMutex {
            id: mutex_id,
            deadline,
        }));
    0 // overwritten by pending_result when the wait completes
}

/// iosc_leave_mutex(mutex_id): arg struct `{ mutex_id }`. Releases and hands the
/// lock to one waiting thread.
fn leave_mutex(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> u32 {
    let mutex_id = read_u32(unicorn, addr);
    let woken_tid = {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        match state.iosc.mutexes.get_mut(&mutex_id) {
            Some(m) => {
                m.locked = false;
                match m.waiters.pop() {
                    Some(tid) => {
                        m.locked = true; // hand the lock to the woken thread
                        Some(tid)
                    }
                    None => None,
                }
            }
            None => return EINVAL,
        }
    };
    if let Some(tid) = woken_tid {
        set_runnable_with_result(unicorn, tid, 0);
    }
    0
}

// ---- semaphores -----------------------------------------------------------

/// iosc_create_semaphore(id, initial_count): arg struct `{ id, count }`. The
/// driver registers the semaphore under `id` and returns it.
fn create_semaphore(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> u32 {
    let id = read_u32(unicorn, addr);
    let count = read_i32(unicorn, addr + 4);
    if count < 0 {
        return EINVAL;
    }
    {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        state.iosc.semaphores.insert(
            id,
            IoscSemaphore {
                count: count as u32,
                ..Default::default()
            },
        );
    }
    id
}

/// iosc_obtain_semaphore(id, _, timeout): arg struct `{ id, _, timeout }`.
/// Blocks until the count is non-zero, then decrements it.
fn obtain_semaphore(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> u32 {
    let sem_id = read_u32(unicorn, addr);
    let deadline = timeout_to_deadline(read_i32(unicorn, addr + 8));

    // fast path: available now
    {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        match state.iosc.semaphores.get_mut(&sem_id) {
            Some(sem) if sem.count > 0 => {
                sem.count -= 1;
                return 0;
            }
            None => return EINVAL,
            _ => {}
        }
    }

    match deadline {
        Some(d) if d <= Instant::now() => return ETIMEDOUT,
        _ => {}
    }
    let tid = unicorn.get_data().thread_id();
    {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        if let Some(sem) = state.iosc.semaphores.get_mut(&sem_id) {
            sem.waiters.push(tid);
        }
    }
    unicorn
        .get_data()
        .set_action(ThreadAction::Block(BlockReason::IoscSemaphore {
            id: sem_id,
            deadline,
        }));
    0 // overwritten by pending_result when the wait completes
}

/// iosc_release_semaphore(id, value): arg struct `{ id, value }`. Increments the
/// count and wakes one waiting obtainer.
fn release_semaphore(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> u32 {
    let sem_id = read_u32(unicorn, addr);
    let value = read_i32(unicorn, addr + 4).max(0) as u32;

    let woken_tid = {
        let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
        match state.iosc.semaphores.get_mut(&sem_id) {
            Some(sem) => {
                sem.count += value;
                if sem.count > 0 {
                    sem.waiters.pop()
                } else {
                    None
                }
            }
            None => return EINVAL,
        }
    };
    if let Some(tid) = woken_tid {
        set_runnable_with_result(unicorn, tid, 0);
    }
    0
}

// ---- blocking completion (scheduler / timeout path) -----------------------

/// mark a blocked thread runnable and install its syscall result in R0 on the
/// next switch-in (mirrors mqueue::set_runnable_with_result)
fn set_runnable_with_result(unicorn: &Unicorn<'_, Context>, tid: u32, result: u32) {
    let mut threads = unicorn.get_data().threads.lock().unwrap();
    if let Some(thread) = threads.iter_mut().find(|t| t.id == tid) {
        if matches!(thread.status, ThreadStatus::Blocked(_)) {
            thread.status = ThreadStatus::Runnable;
            thread.pending_result = Some(result);
        }
    }
}

/// resolve a timed-out (or otherwise expired) blocked IOSC wait, re-checking the
/// object state and installing the syscall result. Called by the scheduler from
/// wake_expired once the thread's deadline passes. Each arm removes `tid` from
/// the waiter list and re-attempts the operation atomically.
pub fn finish_iosc_wait(unicorn: &mut Unicorn<'_, Context>, tid: u32) {
    let reason = {
        let threads = unicorn.get_data().threads.lock().unwrap();
        threads.iter().find(|t| t.id == tid).and_then(|t| match t.status {
            ThreadStatus::Blocked(reason) => Some(reason),
            _ => None,
        })
    };

    let res: u32 = match reason {
        Some(BlockReason::IoscMutex { id, .. }) => {
            let granted = {
                let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
                match state.iosc.mutexes.get_mut(&id) {
                    Some(m) => {
                        m.waiters.retain(|&w| w != tid);
                        if !m.locked {
                            m.locked = true;
                            true
                        } else {
                            false
                        }
                    }
                    None => false,
                }
            };
            if granted {
                0
            } else {
                ETIMEDOUT
            }
        }
        Some(BlockReason::IoscEvent { id, .. }) => {
            let fired = {
                let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
                match state.iosc.events.get_mut(&id) {
                    Some(e) => {
                        e.waiters.retain(|&w| w != tid);
                        if e.value != 0 {
                            e.value = 0;
                            true
                        } else {
                            false
                        }
                    }
                    None => false,
                }
            };
            if fired {
                0
            } else {
                ETIMEDOUT
            }
        }
        Some(BlockReason::IoscSemaphore { id, .. }) => {
            let granted = {
                let mut state = unicorn.get_data().sys_calls_state.lock().unwrap();
                match state.iosc.semaphores.get_mut(&id) {
                    Some(s) => {
                        s.waiters.retain(|&w| w != tid);
                        if s.count > 0 {
                            s.count -= 1;
                            true
                        } else {
                            false
                        }
                    }
                    None => false,
                }
            };
            if granted {
                0
            } else {
                ETIMEDOUT
            }
        }
        _ => return, // not an IOSC wait - nothing to do
    };

    set_runnable_with_result(unicorn, tid, res);
}
