use crate::emulator::context::Context;
use crate::emulator::mmu::Mmu;
use crate::emulator::thread::Thread;
use crate::file_system::MountFileSystem;
use crate::os::SysCallsState;
use std::error::Error;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub struct Process {
    mmu: Arc<Mutex<Mmu>>,
    file_system: Arc<Mutex<MountFileSystem>>,
    sys_calls_state: Arc<Mutex<SysCallsState>>,
    threads: Arc<Mutex<Vec<Thread>>>,
    next_thread_id: Arc<AtomicU32>,
}

impl Process {
    pub fn new(file_system: Arc<Mutex<MountFileSystem>>) -> Self {
        let mmu = Arc::new(Mutex::new(Mmu::new()));
        let sys_calls_state = Arc::new(Mutex::new(SysCallsState::new()));
        Self {
            mmu,
            file_system,
            sys_calls_state,
            threads: Arc::new(Mutex::new(Vec::new())),
            next_thread_id: Arc::new(AtomicU32::new(1)),
        }
    }

    pub fn run(
        &mut self,
        elf_filepath: String,
        program_args: Vec<String>,
        program_envs: Vec<(String, String)>,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        let thread_id = self.next_thread_id.fetch_add(1, Ordering::Relaxed);
        let context = Context {
            mmu: self.mmu.clone(),
            file_system: self.file_system.clone(),
            sys_calls_state: self.sys_calls_state.clone(),
            threads: self.threads.clone(),
            next_thread_id: self.next_thread_id.clone(),
            thread_id,
        };

        let (emu_main_thread, main_thread_handle) =
            Thread::start_elf_file(context, elf_filepath, program_args, program_envs);

        self.threads.lock().unwrap().push(emu_main_thread);

        main_thread_handle.join().unwrap()?;

        Ok(())
    }
}
