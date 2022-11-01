use crate::emulator::mmu::Mmu;
use crate::emulator::thread::Thread;
use crate::file_system::MountFileSystem;
use crate::os::SysCallsState;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};

pub struct Context {
    pub mmu: Arc<Mutex<Mmu>>,
    pub file_system: Arc<Mutex<MountFileSystem>>,
    pub sys_calls_state: Arc<Mutex<SysCallsState>>,
    pub threads: Arc<Mutex<Vec<Thread>>>,
    pub next_thread_id: Arc<AtomicU32>,

    pub thread_id: u32,
}
