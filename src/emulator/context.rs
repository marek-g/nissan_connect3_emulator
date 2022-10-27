use crate::emulator::mmu::Mmu;
use crate::file_system::MountFileSystem;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub struct Context {
    pub mmu: Mmu,

    pub file_system: Rc<RefCell<MountFileSystem>>,

    pub sys_calls_state: SysCallsState,
}

pub struct SysCallsState {
    // state for getdents syscall - list of files in folder to process
    pub get_dents_list: HashMap<u32, Vec<String>>,
}

impl SysCallsState {
    pub fn new() -> Self {
        Self {
            get_dents_list: HashMap::new(),
        }
    }
}
