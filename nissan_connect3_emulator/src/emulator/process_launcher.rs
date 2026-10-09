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
    // The real head unit's boot timing has DAPIAPP's map medium (CRYPTNAV)
    // already ACTIVE before the map engine's DAPI service registration runs.
    // The registration's registry entry permanently snapshots the service
    // state at that moment and the client never re-registers, so an early
    // map engine start stamps it REGISTERED and every later request fails
    // temp-unavailable. Hold the spawn (this is the OSAL spawn boundary the
    // real procbaselx drives) until the medium is genuinely up.
    if elf_path.contains("procmapengine") {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(70);
        while !crate::libs::dapi::MAP_MEDIUM_ACTIVE
            .load(std::sync::atomic::Ordering::SeqCst)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        log::info!(
            "OSAL: map-engine spawn released (map medium active: {})",
            crate::libs::dapi::MAP_MEDIUM_ACTIVE.load(std::sync::atomic::Ordering::SeqCst)
        );
    }
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