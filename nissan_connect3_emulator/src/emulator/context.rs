use crate::emulator::mmu::Mmu;
use crate::emulator::thread::{GuestThread, ThreadAction};
use crate::os::file_system::MountFileSystem;
use crate::os::SysCallsState;
use std::cell::Cell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Context {
    pub inner: Arc<ContextInner>,
}

impl std::ops::Deref for Context {
    type Target = ContextInner;

    fn deref(&self) -> &ContextInner {
        &self.inner
    }
}

pub struct ContextInner {
    pub mmu: Arc<Mutex<Mmu>>,
    pub file_system: Arc<Mutex<MountFileSystem>>,
    pub sys_calls_state: Arc<Mutex<SysCallsState>>,
    pub threads: Arc<Mutex<Vec<GuestThread>>>,
    pub next_thread_id: Arc<AtomicU32>,

    pub instruction_tracing: Arc<AtomicBool>,
    pub hooked_libraries: Arc<Mutex<HashSet<String>>>,

    // scheduler-owned state - accessed only from the single scheduler host thread,
    // so plain Cell is used instead of atomics/mutexes
    current_thread_id: Cell<u32>,
    action: Cell<ThreadAction>,
    process_exit_code: Cell<Option<i32>>,
    last_run_index: Cell<usize>,
}

impl ContextInner {
    pub fn new(
        mmu: Arc<Mutex<Mmu>>,
        file_system: Arc<Mutex<MountFileSystem>>,
        sys_calls_state: Arc<Mutex<SysCallsState>>,
        threads: Arc<Mutex<Vec<GuestThread>>>,
        next_thread_id: Arc<AtomicU32>,
    ) -> Self {
        Self {
            mmu,
            file_system,
            sys_calls_state,
            threads,
            next_thread_id,
            instruction_tracing: Arc::new(AtomicBool::new(false)),
            hooked_libraries: Arc::new(Mutex::new(HashSet::new())),
            current_thread_id: Cell::new(0),
            action: Cell::new(ThreadAction::None),
            process_exit_code: Cell::new(None),
            last_run_index: Cell::new(0),
        }
    }

    /// id of the guest thread currently running on the vCPU
    pub fn thread_id(&self) -> u32 {
        self.current_thread_id.get()
    }

    pub fn set_thread_id(&self, id: u32) {
        self.current_thread_id.set(id);
    }

    /// consume the action requested by a syscall handler (if any)
    pub fn take_action(&self) -> ThreadAction {
        self.action.replace(ThreadAction::None)
    }

    pub fn set_action(&self, action: ThreadAction) {
        self.action.set(action);
    }

    pub fn process_exit_code(&self) -> Option<i32> {
        self.process_exit_code.get()
    }

    pub fn set_process_exit_code(&self, code: i32) {
        self.process_exit_code.set(Some(code));
    }

    pub fn last_run_index(&self) -> usize {
        self.last_run_index.get()
    }

    pub fn set_last_run_index(&self, index: usize) {
        self.last_run_index.set(index);
    }
}
