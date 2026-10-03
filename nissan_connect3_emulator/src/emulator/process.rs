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

/// A guest process: the state shared with the other processes (the file system
/// and the "kernel" namespace) plus the id counter. The process' *private* state
/// (its address space, syscall state and guest-thread list) is created lazily in
/// [`Process::setup`], which runs on that process' own host thread - the
/// guest-thread list holds raw Unicorn CPU contexts that are not `Send`, so it
/// must never be built on, or moved from, another thread.
pub struct Process {
    file_system: Arc<Mutex<MountFileSystem>>,
    namespace: Arc<Mutex<SystemNamespace>>,
    next_thread_id: Arc<AtomicU32>,
}

impl Process {
    pub fn new(
        file_system: Arc<Mutex<MountFileSystem>>,
        namespace: Arc<Mutex<SystemNamespace>>,
        // shared across every process so guest thread ids are globally unique
        next_thread_id: Arc<AtomicU32>,
    ) -> Self {
        Self {
            file_system,
            namespace,
            next_thread_id,
        }
    }

    /// Create this process' VM (its own address space and Context, sharing the
    /// file system and namespace), load the ELF into it and register the main
    /// guest thread. Must run on the host thread that will run the process: it
    /// builds the `Unicorn` and the guest-thread list here so that the non-`Send`
    /// CPU contexts never cross a thread boundary.
    pub fn setup(
        &self,
        elf_filepath: &str,
        program_args: Vec<String>,
        program_envs: Vec<(String, String)>,
    ) -> Result<Unicorn<'static, Context>, Box<dyn Error + Send + Sync + 'static>> {
        let context = Context {
            inner: Arc::new(ContextInner::new(
                Arc::new(Mutex::new(Mmu::new())),
                self.file_system.clone(),
                Arc::new(Mutex::new(SysCallsState::new())),
                self.namespace.clone(),
                Arc::new(Mutex::new(Vec::new())),
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
