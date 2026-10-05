use crate::os::syscalls::signal::SignalState;
use std::collections::{HashMap, HashSet};

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
        }
    }
}
