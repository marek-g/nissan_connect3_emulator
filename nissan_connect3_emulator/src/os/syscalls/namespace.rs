//! System-wide IPC objects shared across every guest process - the emulator's
//! "kernel". Named POSIX message queues and the Bosch IOSC driver state live
//! here (not in the per-process [`SysCallsState`]) so that multiple processes
//! observe the same objects.
//!
//! Blocking waiters are recorded as guest thread ids inside the objects; waking
//! a waiter routes through the process that owns it (see the scheduler / wake
//! machinery). Named shared memory and named semaphores will be added here too.

use crate::common::queues::MqState;
use crate::emulator::thread::Wake;
use crate::os::dev::iosc::IoscState;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// A pending wait on a futex word that lives in shared memory. `signaled` is set
/// by a `FUTEX_WAKE` from any process; the owning process' host thread reaps the
/// entry (and makes its guest thread runnable) the next time it runs.
struct SharedFutexWaiter {
    key: (String, u32),
    signaled: bool,
}

/// Result of asking whether a shared-futex waiter has been woken.
pub enum SharedFutexPoll {
    /// a `FUTEX_WAKE` reached it - complete with success (0)
    Woken,
    /// still waiting
    Waiting,
    /// already reaped (should not normally be observed)
    Gone,
}

pub struct SystemNamespace {
    /// Bosch IOSC IPC driver (`/dev/iosc`): mutexes, events, semaphores.
    pub iosc: IoscState,
    /// POSIX message queues (named, system-wide).
    pub mq: MqState,
    /// Named shared memory (`/dev/shm/*`): host-backed buffers shared by every
    /// process so that `mmap(MAP_SHARED)` of the same file maps the same bytes.
    pub shm: ShmState,
    /// Waiters on shared-memory futex words, keyed by a small id stored in the
    /// owning thread's `BlockReason::FutexWaitShared`.
    futex_slab: HashMap<usize, SharedFutexWaiter>,
    /// Pending waiter ids per shared word, in arrival order (FIFO wake).
    futex_by_key: HashMap<(String, u32), VecDeque<usize>>,
    next_futex_waiter: usize,
    /// One doorbell per running guest process. Each process runs on its own host
    /// thread and parks on its doorbell when none of its guest threads can run;
    /// a host thread that opens a resource (mq message, iosc mutex/event/
    /// semaphore, shared futex wake) rings every doorbell so each parked process
    /// re-checks the objects it is blocked on. This is the only cross-thread
    /// coordination needed - guest state stays private to each process' host thread.
    pub wakes: Vec<Arc<Wake>>,
}

impl SystemNamespace {
    pub fn new() -> Self {
        Self {
            iosc: IoscState::new(),
            mq: MqState::new(),
            shm: ShmState::new(),
            futex_slab: HashMap::new(),
            futex_by_key: HashMap::new(),
            next_futex_waiter: 0,
            wakes: Vec::new(),
        }
    }

    /// Enqueue a wait on a shared-memory futex word; returns the waiter id that
    /// the caller records in the thread's `BlockReason::FutexWaitShared`.
    pub fn futex_wait_shared(&mut self, key: (String, u32)) -> usize {
        let id = self.next_futex_waiter;
        self.next_futex_waiter += 1;
        self.futex_slab.insert(id, SharedFutexWaiter { key: key.clone(), signaled: false });
        self.futex_by_key.entry(key).or_default().push_back(id);
        id
    }

    /// Signal up to `val` pending waiters of a shared futex word. Returns how
    /// many were marked; the caller rings the process doorbells (via
    /// `notify_waiters`) so the owners reap them.
    pub fn futex_wake_shared(&mut self, key: &(String, u32), val: usize) -> usize {
        let mut count = 0;
        if let Some(queue) = self.futex_by_key.get_mut(key) {
            while count < val {
                match queue.pop_front() {
                    Some(id) => {
                        if let Some(waiter) = self.futex_slab.get_mut(&id) {
                            waiter.signaled = true;
                            count += 1;
                        }
                    }
                    None => break,
                }
            }
            if queue.is_empty() {
                self.futex_by_key.remove(key);
            }
        }
        count
    }

    /// Has the waiter been woken? On `Woken` the entry is reaped.
    pub fn futex_poll_shared(&mut self, id: usize) -> SharedFutexPoll {
        match self.futex_slab.get_mut(&id) {
            None => SharedFutexPoll::Gone,
            Some(waiter) if waiter.signaled => {
                self.futex_remove_locked(id);
                SharedFutexPoll::Woken
            }
            Some(_) => SharedFutexPoll::Waiting,
        }
    }

    /// Drop a waiter whose deadline passed (timeout): remove it from the pending
    /// queue and the slab so a later wake does not target a reaped id.
    pub fn futex_remove_shared(&mut self, id: usize) {
        self.futex_remove_locked(id);
    }

    fn futex_remove_locked(&mut self, id: usize) {
        if let Some(waiter) = self.futex_slab.remove(&id) {
            if let Some(queue) = self.futex_by_key.get_mut(&waiter.key) {
                queue.retain(|&x| x != id);
                if queue.is_empty() {
                    self.futex_by_key.remove(&waiter.key);
                }
            }
        }
    }

    /// Register a process' doorbell so other host threads can wake it.
    pub fn register_process(&mut self, wake: &Arc<Wake>) {
        self.wakes.push(wake.clone());
    }

    /// Remove a process' doorbell once its host thread is finished.
    pub fn unregister_process(&mut self, wake: &Arc<Wake>) {
        self.wakes.retain(|w| !Arc::ptr_eq(w, wake));
    }

    /// Ring every process' doorbell (call after mutating a shared IPC object).
    /// Must be called with the namespace lock held or released - it only takes
    /// each doorbell's own (independent) lock, so it never deadlocks against a
    /// caller that still holds the namespace lock.
    pub fn notify_waiters(&self) {
        for wake in &self.wakes {
            wake.notify();
        }
    }
}

/// Host-backed buffers for named shared memory, keyed by `/dev/shm` path.
///
/// Each entry is a single native (host) allocation that every process maps into
/// its own address space via `Unicorn::mem_map_ptr`. Because all guest mappings
/// alias the same host bytes, writes made through one process' mapping are
/// immediately visible to the others - the coherent cross-process shared memory
/// that `shm_open`/`sem_open` rely on. The `Arc` keeps the allocation alive for
/// as long as any process holds a mapping (the namespace outlives every process).
pub struct ShmState {
    buffers: HashMap<String, Arc<Vec<u8>>>,
}

impl ShmState {
    pub fn new() -> Self {
        Self {
            buffers: HashMap::new(),
        }
    }

    /// Return the shared host buffer for `path`, creating it on first use. The
    /// buffer is sized to `size` (a multiple of the page size) and seeded from
    /// `initial` (the file's current content, zero-filled up to `size`). A clone
    /// of the owning `Arc` is returned so the caller can take a stable pointer.
    pub fn get_or_create(&mut self, path: &str, size: usize, initial: &[u8]) -> Arc<Vec<u8>> {
        if let Some(buf) = self.buffers.get(path) {
            return buf.clone();
        }
        let mut buf = vec![0u8; size];
        let n = size.min(initial.len());
        buf[..n].copy_from_slice(&initial[..n]);
        let arc = Arc::new(buf);
        self.buffers.insert(path.to_string(), arc.clone());
        arc
    }
}
