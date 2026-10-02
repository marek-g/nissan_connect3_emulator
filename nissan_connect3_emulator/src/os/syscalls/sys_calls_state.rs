use crate::os::syscalls::iosc::IoscState;
use crate::os::syscalls::mqueue::MqState;
use crate::os::syscalls::signal::SignalState;
use std::collections::HashMap;

pub struct SysCallsState {
    // state for getdents syscall - list of files in folder to process
    pub get_dents_list: HashMap<u32, Vec<String>>,

    // maps futex `uaddr` to list of blocked guest thread ids waiting on that address
    pub futex_waiters: HashMap<u32, Vec<u32>>,

    // POSIX message queues (mq_* syscalls)
    pub mq: MqState,

    // Bosch IOSC IPC driver (/dev/iosc): mutexes, events, semaphores, shared mem
    pub iosc: IoscState,

    // signal dispositions + blocked mask (rt_sigaction / rt_sigreturn / SIGSEGV delivery)
    pub signals: SignalState,
}

impl SysCallsState {
    pub fn new() -> Self {
        Self {
            get_dents_list: HashMap::new(),
            futex_waiters: HashMap::new(),
            mq: MqState::new(),
            iosc: IoscState::new(),
            signals: SignalState::new(),
        }
    }
}
