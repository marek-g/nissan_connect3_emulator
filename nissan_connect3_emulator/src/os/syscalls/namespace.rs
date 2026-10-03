//! System-wide IPC objects shared across every guest process - the emulator's
//! "kernel". Named POSIX message queues and the Bosch IOSC driver state live
//! here (not in the per-process [`SysCallsState`]) so that multiple processes
//! observe the same objects.
//!
//! Blocking waiters are recorded as guest thread ids inside the objects; waking
//! a waiter routes through the process that owns it (see the scheduler / wake
//! machinery). Named shared memory and named semaphores will be added here too.

use crate::os::dev::iosc::IoscState;
use crate::os::syscalls::mqueue::MqState;
use std::collections::HashMap;
use std::sync::Arc;

pub struct SystemNamespace {
    /// Bosch IOSC IPC driver (`/dev/iosc`): mutexes, events, semaphores.
    pub iosc: IoscState,
    /// POSIX message queues (named, system-wide).
    pub mq: MqState,
    /// Named shared memory (`/dev/shm/*`): host-backed buffers shared by every
    /// process so that `mmap(MAP_SHARED)` of the same file maps the same bytes.
    pub shm: ShmState,
}

impl SystemNamespace {
    pub fn new() -> Self {
        Self {
            iosc: IoscState::new(),
            mq: MqState::new(),
            shm: ShmState::new(),
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
