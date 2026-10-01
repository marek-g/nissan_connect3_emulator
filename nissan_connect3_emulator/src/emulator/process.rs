use crate::emulator::context::{Context, ContextInner};
use crate::emulator::mmu::Mmu;
use crate::emulator::scheduler::run as run_scheduler;
use crate::emulator::thread::add_mem_fault_hooks;
use crate::file_system::MountFileSystem;
use crate::os::hook_syscall;
use crate::os::SysCallsState;
use std::error::Error;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use unicorn_engine::unicorn_const::{Arch, Mode};
use unicorn_engine::Unicorn;

pub struct Process {
    mmu: Arc<Mutex<Mmu>>,
    file_system: Arc<Mutex<MountFileSystem>>,
    sys_calls_state: Arc<Mutex<SysCallsState>>,
    threads: Arc<Mutex<Vec<crate::emulator::thread::GuestThread>>>,
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
        let context = Context {
            inner: Arc::new(ContextInner::new(
                self.mmu.clone(),
                self.file_system.clone(),
                self.sys_calls_state.clone(),
                self.threads.clone(),
                self.next_thread_id.clone(),
            )),
        };

        // the single VM shared by all guest threads
        let mut unicorn =
            Unicorn::new_with_data(Arch::ARM, Mode::LITTLE_ENDIAN, context)
                .map_err(|e| format!("Unicorn error: {:?}", e))?;
        unicorn.add_intr_hook(hook_syscall).unwrap();
        add_mem_fault_hooks(&mut unicorn);

        run_scheduler(&mut unicorn, &elf_filepath, program_args, program_envs)
    }
}
