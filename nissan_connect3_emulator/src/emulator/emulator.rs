use crate::emulator::process::Process;
use crate::emulator::scheduler;
use crate::emulator::thread::Wake;
use crate::os::file_system::MountFileSystem;
use crate::os::syscalls::namespace::SystemNamespace;
use crate::rtos;
use std::error::Error;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub type ProcessResult = Result<(), Box<dyn Error + Send + Sync + 'static>>;
pub type ProcessHandle = JoinHandle<ProcessResult>;

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

/// Cloneable handle that can create process host threads without borrowing the
/// whole [`Emulator`]. The RTOS backend needs this because dynamically started
/// processes are spawned after `run_processes` has already begun.
#[derive(Clone)]
pub struct ProcessFactory {
    file_system: Arc<Mutex<MountFileSystem>>,
    namespace: Arc<Mutex<SystemNamespace>>,
    next_thread_id: Arc<AtomicU32>,
}

impl ProcessFactory {
    fn new(
        file_system: Arc<Mutex<MountFileSystem>>,
        namespace: Arc<Mutex<SystemNamespace>>,
        next_thread_id: Arc<AtomicU32>,
    ) -> Self {
        Self {
            file_system,
            namespace,
            next_thread_id,
        }
    }

    pub fn namespace(&self) -> Arc<Mutex<SystemNamespace>> {
        self.namespace.clone()
    }

    pub fn spawn_process(&self, spec: ProcessSpec) -> ProcessHandle {
        let process = Process::new(
            self.file_system.clone(),
            self.namespace.clone(),
            self.next_thread_id.clone(),
        );
        let namespace = self.namespace.clone();

        std::thread::spawn(move || -> ProcessResult {
            let wake = Arc::new(Wake::new());
            namespace.lock().unwrap().register_process(&wake);

            let result = (|| -> ProcessResult {
                let elf_filepath = spec.elf_filepath;
                let program_args = spec.program_args;
                let program_envs = spec.program_envs;

                let mut unicorn = process.setup(&elf_filepath, program_args, program_envs)?;
                let result = scheduler::run_process_loop(&mut unicorn, &wake);
                drop(unicorn);
                result
            })();

            namespace.lock().unwrap().unregister_process(&wake);
            result
        })
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
    pub fn run_processes(&self, specs: Vec<ProcessSpec>) -> ProcessResult {
        let factory = ProcessFactory::new(
            self.file_system.clone(),
            self.namespace.clone(),
            self.next_thread_id.clone(),
        );
        let rtos_config = rtos::RtosBootConfig::from_env_with_default_envs(&specs);
        let handles = Arc::new(Mutex::new(Vec::with_capacity(specs.len())));

        if rtos_config.is_enabled() {
            rtos::RtosQueueInteraction::bootstrap(&factory.namespace());
        }

        for spec in specs {
            let handle = factory.spawn_process(spec);
            handles.lock().unwrap().push(handle);
        }

        let rtos_queues = if rtos_config.is_enabled() {
            let startup_messages = if rtos_config.queue_boot_enabled() {
                rtos_config.startup_messages()
            } else {
                Default::default()
            };

            Some(rtos::RtosQueueInteraction::start_with_startup_messages(
                factory.namespace(),
                rtos_config.interaction_config(),
                startup_messages,
            ))
        } else {
            None
        };
        let rtos_service =
            rtos::RtosBootService::new(factory, handles.clone(), rtos_config).start();

        let mut first_error = None;
        if rtos_service.join().is_err() {
            first_error = Some(Box::<dyn Error + Send + Sync>::from(
                "the RTOS boot host thread panicked",
            ) as Box<dyn Error + Send + Sync + 'static>);
        }

        loop {
            let batch: Vec<ProcessHandle> = {
                let mut guard = handles.lock().unwrap();
                if guard.is_empty() {
                    break;
                }
                std::mem::take(&mut *guard)
            };

            for handle in batch {
                match handle.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        if first_error.is_none() {
                            first_error = Some(e);
                        }
                    }
                    Err(_) => {
                        if first_error.is_none() {
                            first_error = Some(Box::<dyn Error + Send + Sync>::from(
                                "a process host thread panicked",
                            ));
                        }
                    }
                }
            }
        }

        if let Some(rtos_queues) = rtos_queues {
            rtos_queues.stop();
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
