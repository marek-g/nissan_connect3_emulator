use crate::emulator::emulator::{ProcessFactory, ProcessHandle, ProcessSpec};
use std::sync::{Arc, Mutex, OnceLock};

static PROCESS_LAUNCHER: OnceLock<ProcessLauncher> = OnceLock::new();

#[derive(Clone)]
struct ProcessLauncher {
    factory: ProcessFactory,
    handles: Arc<Mutex<Vec<ProcessHandle>>>,
    envs: Vec<(String, String)>,
}

pub fn install(
    factory: ProcessFactory,
    handles: Arc<Mutex<Vec<ProcessHandle>>>,
    envs: Vec<(String, String)>,
) {
    if PROCESS_LAUNCHER
        .set(ProcessLauncher {
            factory,
            handles,
            envs,
        })
        .is_err()
    {
        log::debug!("process launcher: already installed");
    }
}

pub fn spawn_process(app_name: &str, filename: &str, cmdline: &str) -> Result<(), String> {
    let Some(launcher) = PROCESS_LAUNCHER.get() else {
        return Err("process launcher is not installed".to_string());
    };

    let elf_path = resolve_process_path(filename);
    let args = process_args(&elf_path, cmdline);
    log::info!(
        "OSAL: spawning process app='{}' path={} args={:?}",
        app_name,
        elf_path,
        args
    );

    let spec = ProcessSpec::new(elf_path)
        .args(args)
        .envs(launcher.envs.clone());
    let handle = launcher.factory.spawn_process(spec);
    launcher
        .handles
        .lock()
        .unwrap()
        .push(handle);
    Ok(())
}

fn resolve_process_path(filename: &str) -> String {
    let filename = filename.trim();
    if filename.starts_with('/') {
        return filename.to_string();
    }

    format!("/opt/bosch/processes/{}", filename)
}

fn process_args(elf_path: &str, cmdline: &str) -> Vec<String> {
    let mut args: Vec<String> = cmdline
        .split_whitespace()
        .map(str::to_string)
        .filter(|arg| !arg.is_empty())
        .collect();

    if args.is_empty() {
        args.push(elf_path.to_string());
    }

    args
}