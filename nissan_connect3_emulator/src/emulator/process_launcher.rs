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
    // r799n34b: argv[0] is the filename AS GIVEN in the start message (the
    // unit's baselx execs the PROC_LOCATION string, e.g. "procsql_out.out",
    // not the resolved path).  procsql derives its OSAL IPC names from its
    // argv ("sql:evt:"+argv[0]) and libosal rejects names >= 32 chars with
    // 0x7200b, so a full path here makes procsql's sem/event create assert
    // (n287: sql::sem_t::init "invalid != handle", process death at main).
    let args = process_args(filename.trim(), cmdline);
    // The real head unit boots with the CRYPTNAV map medium already ACTIVE
    // before the map engine registers its DAPI service: the registration
    // snapshots the service state and a request answered while the entry is
    // still REGISTERED makes DAPIAPP drop the registration. Emulate that
    // procbaselx-style staggering by deferring the map-engine spawn on a host
    // thread until DAPIAPP's device manager reports the medium active. The
    // guest must never be blocked here - the emulator is serialized, so a
    // host-side sleep inside the guest call would freeze every process (the
    // medium could then never come up at all).
    if elf_path.contains("procmapengine")
        && !crate::libs::dapi::MAP_MEDIUM_ACTIVE.load(std::sync::atomic::Ordering::SeqCst)
    {
        let launcher = launcher.clone();
        std::thread::Builder::new()
            .name("map-engine-spawn".into())
            .spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
                while !crate::libs::dapi::MAP_MEDIUM_ACTIVE
                    .load(std::sync::atomic::Ordering::SeqCst)
                    && std::time::Instant::now() < deadline
                {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                log::info!(
                    "OSAL: deferred map-engine spawn starting at t={}ms (map medium active: {})",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    crate::libs::dapi::MAP_MEDIUM_ACTIVE.load(std::sync::atomic::Ordering::SeqCst)
                );
                if let Err(err) = launch(&launcher, "map-engine", &elf_path, args) {
                    log::warn!("OSAL: deferred map-engine spawn failed: {err}");
                }
            })
            .map_err(|err| err.to_string())?;
        return Ok(());
    }
    // r799n36: PROCSDSTS staggering.  The unit boots LINUX ORDER with real
    // inter-process spacing (seconds apart): PROCHMI (PROC02) has created its
    // mailbox mbx_265 long before PROCSDSTS (PROC07) starts, and sdsts' SCD
    // bring-up - which `scd_bCreateQueue`s its partner apps' queues when they
    // do not exist yet - therefore never touches it.  Here all eight processes
    // exec within ~1 ms, so sdsts sometimes won the race and prochmi's own
    // mq_open(O_CREAT|O_EXCL) met EEXIST -> "Application 0x0109: Queue could
    // not be created" -> no HMI startup at all (n300_2; earlier n295).  Defer
    // the sdsts spawn on a host thread (never sleep in the guest call!) until
    // the queue exists in the shared namespace, 15 s timeout.
    if elf_path.contains("procsds_ts") {
        let launcher = launcher.clone();
        let app_name = app_name.to_string();
        std::thread::Builder::new()
            .name("sdsts-spawn".into())
            .spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                let mut owned = false;
                loop {
                    owned = launcher
                        .factory
                        .namespace()
                        .lock()
                        .unwrap()
                        .mq
                        .queue_id_for_name("mbx_265")
                        .is_some();
                    if owned || std::time::Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                log::info!(
                    "OSAL: deferred PROCSDSTS spawn starting at t={}ms (mbx_265 already created: {owned})",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                );
                if let Err(err) = launch(&launcher, &app_name, &elf_path, args) {
                    log::warn!("OSAL: deferred PROCSDSTS spawn failed: {err}");
                }
            })
            .map_err(|err| err.to_string())?;
        return Ok(());
    }
    launch(launcher, app_name, &elf_path, args)
}

fn launch(
    launcher: &ProcessLauncher,
    app_name: &str,
    elf_path: &str,
    args: Vec<String>,
) -> Result<(), String> {
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

fn process_args(name_for_argv0: &str, cmdline: &str) -> Vec<String> {
    let mut args: Vec<String> = cmdline
        .split_whitespace()
        .map(str::to_string)
        .filter(|arg| !arg.is_empty())
        .collect();

    if args.is_empty() {
        args.push(name_for_argv0.to_string());
    }

    args
}