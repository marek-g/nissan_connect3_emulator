use crate::emulator::context::{Context, ContextInner};
use crate::emulator::mmu::Mmu;
use crate::emulator::scheduler::setup_process;
use crate::emulator::thread::add_mem_fault_hooks;
use crate::os::file_system::MountFileSystem;
use crate::os::hook_syscall;
use crate::os::syscalls::namespace::SystemNamespace;
use crate::os::SysCallsState;
use std::error::Error;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use unicorn_engine::unicorn_const::{Arch, Mode};
use unicorn_engine::Unicorn;

pub struct Process {
    mmu: Arc<Mutex<Mmu>>,
    file_system: Arc<Mutex<MountFileSystem>>,
    sys_calls_state: Arc<Mutex<SysCallsState>>,
    namespace: Arc<Mutex<SystemNamespace>>,
    threads: Arc<Mutex<Vec<crate::emulator::thread::GuestThread>>>,
    next_thread_id: Arc<AtomicU32>,
}

impl Process {
    pub fn new(
        file_system: Arc<Mutex<MountFileSystem>>,
        namespace: Arc<Mutex<SystemNamespace>>,
        // shared across every process so guest thread ids are globally unique
        next_thread_id: Arc<AtomicU32>,
    ) -> Self {
        let mmu = Arc::new(Mutex::new(Mmu::new()));
        let sys_calls_state = Arc::new(Mutex::new(SysCallsState::new()));
        Self {
            mmu,
            file_system,
            sys_calls_state,
            namespace,
            threads: Arc::new(Mutex::new(Vec::new())),
            next_thread_id,
        }
    }

    /// Create this process' VM (it owns its Context and, through it, clones of
    /// the shared mmu/threads/namespace), load the ELF into it and register the
    /// main thread. The returned Unicorn is self-contained and ready to schedule.
    pub fn setup(
        &self,
        elf_filepath: &str,
        program_args: Vec<String>,
        program_envs: Vec<(String, String)>,
    ) -> Result<Unicorn<'static, Context>, Box<dyn Error + Send + Sync + 'static>> {
        let context = Context {
            inner: Arc::new(ContextInner::new(
                self.mmu.clone(),
                self.file_system.clone(),
                self.sys_calls_state.clone(),
                self.namespace.clone(),
                self.threads.clone(),
                self.next_thread_id.clone(),
            )),
        };

        let mut unicorn =
            Unicorn::new_with_data(Arch::ARM, Mode::LITTLE_ENDIAN, context)
                .map_err(|e| format!("Unicorn error: {:?}", e))?;
        unicorn.add_intr_hook(hook_syscall).unwrap();
        // mem-fault hooks (unmapped/prot) deliver SIGSEGV to the guest. They add
        // per-access overhead, so they're opt-in via EMU_MEM_HOOKS.
        if std::env::var("EMU_MEM_HOOKS").map(|v| v == "1").unwrap_or(false) {
            add_mem_fault_hooks(&mut unicorn);
        }

        setup_process(&mut unicorn, elf_filepath, program_args, program_envs)?;
        Ok(unicorn)
    }
}
