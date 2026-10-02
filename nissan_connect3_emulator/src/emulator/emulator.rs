use crate::emulator::process::Process;
use crate::emulator::scheduler;
use crate::os::file_system::MountFileSystem;
use crate::os::syscalls::namespace::SystemNamespace;
use std::error::Error;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use unicorn_engine::unicorn_const::uc_error;

/// One program to launch: its ELF image plus argv/envp.
pub struct ProcessSpec {
    pub elf_filepath: String,
    pub program_args: Vec<String>,
    pub program_envs: Vec<(String, String)>,
}

impl ProcessSpec {
    pub fn new(elf_filepath: impl Into<String>) -> Self {
        Self {
            elf_filepath: elf_filepath.into(),
            program_args: vec![],
            program_envs: vec![],
        }
    }

    pub fn args(mut self, args: Vec<String>) -> Self {
        self.program_args = args;
        self
    }

    pub fn envs(mut self, envs: Vec<(String, String)>) -> Self {
        self.program_envs = envs;
        self
    }
}

pub struct Emulator {
    file_system: Arc<Mutex<MountFileSystem>>,
    /// shared across every process this emulator runs (the "kernel" IPC state)
    namespace: Arc<Mutex<SystemNamespace>>,
    /// single monotonic counter so guest thread ids are unique across processes
    next_thread_id: Arc<AtomicU32>,
}

impl Emulator {
    pub fn new(file_system: MountFileSystem) -> Result<Emulator, uc_error> {
        Ok(Self {
            file_system: Arc::new(Mutex::new(file_system)),
            namespace: Arc::new(Mutex::new(SystemNamespace::new())),
            next_thread_id: Arc::new(AtomicU32::new(1)),
        })
    }

    /// Set up every process (each gets its own VM + address space but shares the
    /// file system and kernel namespace) and run them cooperatively until all have
    /// exited.
    pub fn run_processes(
        &self,
        specs: Vec<ProcessSpec>,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        let mut unicorns = Vec::with_capacity(specs.len());
        for spec in specs {
            let process = Process::new(
                self.file_system.clone(),
                self.namespace.clone(),
                self.next_thread_id.clone(),
            );
            let unicorn =
                process.setup(&spec.elf_filepath, spec.program_args, spec.program_envs)?;
            unicorns.push(unicorn);
        }
        scheduler::run_all(unicorns)
    }

    /// Run a single process to completion (a convenience over `run_processes`).
    pub fn run_process(
        &self,
        elf_filepath: String,
        program_args: Vec<String>,
        program_envs: Vec<(String, String)>,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        self.run_processes(vec![ProcessSpec::new(elf_filepath)
            .args(program_args)
            .envs(program_envs)])
    }
}
