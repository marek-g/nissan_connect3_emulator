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

pub struct SystemNamespace {
    /// Bosch IOSC IPC driver (`/dev/iosc`): mutexes, events, semaphores.
    pub iosc: IoscState,
    /// POSIX message queues (named, system-wide).
    pub mq: MqState,
}

impl SystemNamespace {
    pub fn new() -> Self {
        Self {
            iosc: IoscState::new(),
            mq: MqState::new(),
        }
    }
}
