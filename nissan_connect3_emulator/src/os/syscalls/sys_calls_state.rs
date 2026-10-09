use crate::os::syscalls::signal::SignalState;
use std::collections::{HashMap, HashSet};

const OSAL_MESSAGE_POOL_CHUNK: u32 = 0x1000;
const OSAL_MESSAGE_POOL_SLOTS: u32 = 4096;

/// Per-process syscall state. System-wide IPC objects (POSIX message queues and
/// the IOSC driver) do NOT live here - they live in the shared
/// [`SystemNamespace`](crate::os::syscalls::namespace::SystemNamespace) so that
/// every process sees the same objects.
pub struct SysCallsState {
    // state for getdents syscall - list of files in folder to process
    pub get_dents_list: HashMap<u32, Vec<String>>,

    // maps futex `uaddr` to list of blocked guest thread ids waiting on that address
    pub futex_waiters: HashMap<u32, Vec<u32>>,

    // descriptors returned by inotify_init(); reads on them block instead of EOFing
    pub inotify_fds: HashSet<u32>,

    // descriptors returned by socket(); currently host-less emulated sockets
    pub socket_fds: HashSet<u32>,
    pub next_socket_fd: u32,

    // signal dispositions + blocked mask (rt_sigaction / rt_sigreturn / SIGSEGV delivery)
    pub signals: SignalState,

    // fallback allocator for OSAL type-2 message-pool messages
    pub osal_messages: OsalMessagePool,
}

pub struct OsalMessagePool {
    base: u32,
    chunk: u32,
    slots: u32,
    used: Vec<bool>,
    dynamic: HashMap<u32, u32>,
    freed: Vec<(u32, u32)>,
}

impl SysCallsState {
    pub fn new() -> Self {
        Self {
            get_dents_list: HashMap::new(),
            futex_waiters: HashMap::new(),
            inotify_fds: HashSet::new(),
            socket_fds: HashSet::new(),
            next_socket_fd: 0x1000,
            signals: SignalState::new(),
            osal_messages: OsalMessagePool::new(),
        }
    }
}

impl OsalMessagePool {
    fn new() -> Self {
        Self {
            base: 0,
            chunk: OSAL_MESSAGE_POOL_CHUNK,
            slots: OSAL_MESSAGE_POOL_SLOTS,
            used: Vec::new(),
            dynamic: HashMap::new(),
            freed: Vec::new(),
        }
    }

    pub fn base(&self) -> u32 {
        self.base
    }

    pub fn chunk_size(&self) -> u32 {
        self.chunk
    }

    pub fn slot_count(&self) -> u32 {
        self.slots
    }

    pub fn set_base(&mut self, base: u32) {
        if self.base != 0 {
            return;
        }

        self.base = base;
        self.used = vec![false; self.slots as usize];
    }

    pub fn take_slot(&mut self) -> Option<u32> {
        let index = self
            .used
            .iter_mut()
            .position(|used| {
                if *used {
                    false
                } else {
                    *used = true;
                    true
                }
            })?;

        Some(index as u32)
    }

    pub fn mark_dynamic(&mut self, content: u32, size: u32) {
        self.dynamic.insert(content, size);
    }

    /// Hand back a previously released emulated-message block that can hold
    /// `size` bytes. Reusing the already-mapped guest block keeps QEMU's
    /// memory-section count flat under sustained CCA traffic (each fresh
    /// `heap_alloc` maps a new section and QEMU aborts past 4096).
    pub fn take_freed(&mut self, size: u32) -> Option<u32> {
        let index = self
            .freed
            .iter()
            .position(|(_, freed)| *freed >= size)?;
        let (content, _) = self.freed.remove(index);
        self.dynamic.insert(content, size);
        Some(content)
    }

    pub fn release(&mut self, content: u32) -> bool {
        if let Some(size) = self.dynamic.remove(&content) {
            if self.freed.len() < 1024 {
                self.freed.push((content, size));
            }
            return true;
        }

        if self.base == 0 || content < self.base {
            return false;
        }

        let offset = content - self.base;
        if offset % self.chunk != 0 {
            return false;
        }

        let index = offset / self.chunk;
        if index >= self.slots {
            return false;
        }

        let slot = &mut self.used[index as usize];
        if !*slot {
            return false;
        }

        *slot = false;
        true
    }
}
