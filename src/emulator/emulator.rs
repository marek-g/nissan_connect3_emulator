use crate::emulator::process::Process;
use crate::file_system::MountFileSystem;
use std::error::Error;
use std::sync::{Arc, Mutex};
use unicorn_engine::unicorn_const::uc_error;

pub struct Emulator {
    file_system: Arc<Mutex<MountFileSystem>>,
}

impl Emulator {
    pub fn new(file_system: MountFileSystem) -> Result<Emulator, uc_error> {
        Ok(Self {
            file_system: Arc::new(Mutex::new(file_system)),
        })
    }

    pub fn run_process(
        &mut self,
        elf_filepath: String,
        program_args: Vec<String>,
        program_envs: Vec<(String, String)>,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        let mut process = Process::new(self.file_system.clone());
        process.run(elf_filepath, program_args, program_envs)
    }
}
