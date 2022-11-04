use std::collections::HashMap;
use std::sync::mpsc::Sender;

pub struct SysCallsState {
    // state for getdents syscall - list of files in folder to process
    pub get_dents_list: HashMap<u32, Vec<String>>,

    // maps futex uaddr to list of waiters for that address
    pub futex_waiters: HashMap<u32, Vec<Sender<()>>>,
}

impl SysCallsState {
    pub fn new() -> Self {
        Self {
            get_dents_list: HashMap::new(),
            futex_waiters: HashMap::new(),
        }
    }
}
