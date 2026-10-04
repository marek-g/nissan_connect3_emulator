use crate::common::queues;
use crate::emulator::emulator::{ProcessFactory, ProcessHandle, ProcessSpec};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const LINUX_OSAAL_START_PROC_COMMAND: u8 = 0x1a;
pub const DEFAULT_READY_QUEUE: &str = queues::DEFAULT_READY_QUEUE;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartProcessCommand {
    command: u8,
    path: String,
}

impl StartProcessCommand {
    pub fn new(command: u8, path: impl Into<String>) -> Self {
        Self { command, path: path.into() }
    }

    pub fn start_proc(path: impl Into<String>) -> Self {
        Self::new(LINUX_OSAAL_START_PROC_COMMAND, path)
    }

    pub fn command(&self) -> u8 {
        self.command
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Linux OSAL callback payload layout:
    ///
    /// ```c
    /// struct osal_callback_message {
    ///     uint8_t unknown0[2];
    ///     uint8_t command;
    ///     uint8_t payload[];
    /// };
    /// ```
    ///
    /// `vSysCallbackHandler` reads the command byte at offset 2 and treats offset
    /// 3 as the start of the path payload.
    pub fn payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.path.len());
        out.extend_from_slice(&[0, 0, self.command]);
        out.extend_from_slice(self.path.as_bytes());
        out.push(0);
        out
    }
}

#[derive(Clone, Debug)]
pub struct RtosBootConfig {
    enabled: bool,
    commands: Vec<StartProcessCommand>,
    ready_queue: String,
    ready_timeout: Duration,
    initial_delay: Duration,
    default_envs: Vec<(String, String)>,
}

impl Default for RtosBootConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            commands: vec![StartProcessCommand::start_proc(
                "/opt/bosch/processes/prochmi_out.out",
            )],
            ready_queue: DEFAULT_READY_QUEUE.to_string(),
            ready_timeout: Duration::from_secs(30),
            initial_delay: Duration::ZERO,
            default_envs: Vec::new(),
        }
    }
}

impl RtosBootConfig {
    pub fn from_env_with_default_envs(specs: &[ProcessSpec]) -> Self {
        let mut cfg = Self::default();
        cfg.default_envs = specs
            .first()
            .map(|spec| spec.program_envs.clone())
            .unwrap_or_default();

        cfg.enabled = match std::env::var("EMU_RTOS") {
            Ok(value) => !matches!(value.as_str(), "0" | "off" | "false" | "FALSE"),
            Err(_) => std::env::var_os("EMU_PROCESSES").is_none(),
        };

        if let Ok(value) = std::env::var("EMU_RTOS_START") {
            cfg.commands = value
                .split(':')
                .filter(|path| !path.is_empty())
                .map(StartProcessCommand::start_proc)
                .collect();
        }

        if let Ok(value) = std::env::var("EMU_RTOS_READY_QUEUE") {
            if !value.is_empty() {
                cfg.ready_queue = value;
            }
        }

        cfg.ready_timeout = env_duration_ms("EMU_RTOS_READY_TIMEOUT_MS", cfg.ready_timeout);
        cfg.initial_delay = env_duration_ms("EMU_RTOS_START_DELAY_MS", cfg.initial_delay);

        cfg
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn commands(&self) -> &[StartProcessCommand] {
        &self.commands
    }
}

pub struct RtosBootService {
    factory: ProcessFactory,
    handles: Arc<Mutex<Vec<ProcessHandle>>>,
    config: RtosBootConfig,
}

impl RtosBootService {
    pub fn new(
        factory: ProcessFactory,
        handles: Arc<Mutex<Vec<ProcessHandle>>>,
        config: RtosBootConfig,
    ) -> Self {
        Self { factory, handles, config }
    }

    pub fn start(self) -> JoinHandle<()> {
        thread::spawn(move || self.run())
    }

    fn run(&self) {
        if !self.config.enabled {
            log::debug!("RTOS boot service disabled");
            return;
        }

        if self.config.commands.is_empty() {
            log::debug!("RTOS boot service has no start-process commands");
            return;
        }

        if self.config.initial_delay > Duration::ZERO {
            thread::sleep(self.config.initial_delay);
        }

        if !self.wait_until_ready() {
            log::warn!(
                "RTOS boot service did not observe ready queue {}; starting configured processes anyway",
                self.config.ready_queue
            );
        }

        for command in self.config.commands() {
            log::info!(
                "RTOS boot: start-process command 0x{:02x} path {} payload {:02x?}",
                command.command(),
                command.path(),
                command.payload()
            );

            let spec = ProcessSpec::new(command.path()).envs(self.config.default_envs.clone());
            let handle = self.factory.spawn_process(spec);
            self.handles.lock().unwrap().push(handle);
        }
    }

    fn wait_until_ready(&self) -> bool {
        let namespace = self.factory.namespace();
        let deadline = Instant::now() + self.config.ready_timeout;

        while Instant::now() < deadline {
            if self.ready_observed(&namespace.lock().unwrap().mq) {
                log::debug!(
                    "RTOS boot service observed ready queue(s) around {}",
                    self.config.ready_queue
                );
                return true;
            }

            thread::sleep(Duration::from_millis(10));
        }

        false
    }

    fn ready_observed(&self, mq: &queues::MqState) -> bool {
        mq.has_guest_ready_queue(&self.config.ready_queue)
    }
}

fn env_duration_ms(name: &str, default: Duration) -> Duration {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .map(Duration::from_millis)
            .unwrap_or(default),
        Err(_) => default,
    }
}
