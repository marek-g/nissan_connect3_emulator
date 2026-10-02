use crate::os::syscalls::signal::SignalState;
use std::collections::HashMap;

/// Per-process syscall state. System-wide IPC objects (POSIX message queues and
/// the IOSC driver) do NOT live here - they live in the shared
/// [`SystemNamespace`](crate::os::syscalls::namespace::SystemNamespace) so that
/// every process sees the same objects.
pub struct SysCallsState {
    // state for getdents syscall - list of files in folder to process
    pub get_dents_list: HashMap<u32, Vec<String>>,

    // maps futex `uaddr` to list of blocked guest thread ids waiting on that address
    pub futex_waiters: HashMap<u32, Vec<u32>>,

    // signal dispositions + blocked mask (rt_sigaction / rt_sigreturn / SIGSEGV delivery)
    pub signals: SignalState,
}

impl SysCallsState {
    pub fn new() -> Self {
        Self {
            get_dents_list: HashMap::new(),
            futex_waiters: HashMap::new(),
            signals: SignalState::new(),
        }
    }
}
