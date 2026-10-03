use crate::emulator::process::Process;
use crate::emulator::scheduler;
use crate::emulator::thread::Wake;
use crate::os::file_system::MountFileSystem;
use crate::os::syscalls::namespace::SystemNamespace;
use std::error::Error;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};

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
    pub fn new(file_system: MountFileSystem) -> Emulator {
        Self {
            file_system: Arc::new(Mutex::new(file_system)),
            namespace: Arc::new(Mutex::new(SystemNamespace::new())),
            next_thread_id: Arc::new(AtomicU32::new(1)),
        }
    }

    /// Launch every process in parallel: each gets its own host thread and its
    /// own Unicorn VM (its own address space) while sharing the file system and
    /// the kernel namespace, so guest threads of different processes run on
    /// separate cores and their IPC (message queues / IOSC / shared memory)
    /// works. Blocks until every process has exited.
    pub fn run_processes(
        &self,
        specs: Vec<ProcessSpec>,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        let mut handles = Vec::with_capacity(specs.len());

        for spec in specs {
            let process = Process::new(
                self.file_system.clone(),
                self.namespace.clone(),
                self.next_thread_id.clone(),
            );
            let namespace = self.namespace.clone();

            let handle = std::thread::spawn(move || -> Result<
                (),
                Box<dyn Error + Send + Sync + 'static>,
            > {
                // Each process parks on this doorbell; peers ring it on IPC
                // activity. It is `Sync`, so registering it is the only state
                // this host thread shares with the others.
                let wake = Arc::new(Wake::new());
                namespace.lock().unwrap().register_process(&wake);

                let result = (|| {
                    // The VM and the guest-thread list are built here, on this
                    // host thread, so the (non-Send) CPU contexts stay local.
                    let mut unicorn =
                        process.setup(&spec.elf_filepath, spec.program_args, spec.program_envs)?;
                    let r = scheduler::run_process_loop(&mut unicorn, &wake);
                    drop(unicorn);
                    r
                })();

                namespace.lock().unwrap().unregister_process(&wake);
                result
            });
            handles.push(handle);
        }

        let mut first_error = None;
        for handle in handles {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
                Err(_) => {
                    if first_error.is_none() {
                        first_error = Some(
                            Box::<dyn Error + Send + Sync>::from("a process host thread panicked"),
                        );
                    }
                }
            }
        }

        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Run a single process to completion (a convenience over `run_processes`).
    pub fn run_process(
        &self,
        elf_filepath: &str,
        program_args: Vec<String>,
        program_envs: Vec<(String, String)>,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        self.run_processes(vec![ProcessSpec::new(elf_filepath)
            .args(program_args)
            .envs(program_envs)])
    }
}
