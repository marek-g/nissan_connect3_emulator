use crate::common::queues;
use crate::emulator::emulator::{ProcessFactory, ProcessHandle, ProcessSpec};
use crate::rtos::interaction::{
    RtosInteractionConfig, RtosQueueMessageFormat, RtosStartupQueueMessage,
};
use std::collections::VecDeque;
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
        Self {
            command,
            path: path.into(),
        }
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
    start_queue: String,
    queue_boot: bool,
    start_message_format: RtosQueueMessageFormat,
    direct_spawn: bool,
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
            start_queue: queues::OSAL_CB_HDR_LI_MAIN.to_string(),
            queue_boot: true,
            start_message_format: RtosQueueMessageFormat::Callback,
            direct_spawn: false,
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
            Err(_) => true,
        };

        if let Ok(value) = std::env::var("EMU_RTOS_START") {
            cfg.commands = value
                .split(':')
                .filter(|path| !path.is_empty())
                .map(StartProcessCommand::start_proc)
                .collect();
        }

        if let Ok(value) = std::env::var("EMU_RTOS_START_QUEUE") {
            if !value.is_empty() {
                cfg.start_queue = value;
            }
        }

        cfg.queue_boot = env_flag("EMU_RTOS_QUEUE_BOOT", cfg.queue_boot);
        cfg.direct_spawn = env_flag("EMU_RTOS_DIRECT_SPAWN", cfg.direct_spawn);
        if let Ok(value) = std::env::var("EMU_RTOS_START_MESSAGE_FORMAT") {
            match value.as_str() {
                "callback" | "cb" | "syscallback" => {
                    cfg.start_message_format = RtosQueueMessageFormat::Callback
                }
                "terminal" | "term" => cfg.start_message_format = RtosQueueMessageFormat::Terminal,
                _ => {}
            }
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

    pub fn queue_boot_enabled(&self) -> bool {
        self.enabled && self.queue_boot && !self.commands.is_empty()
    }

    pub fn interaction_config(&self) -> RtosInteractionConfig {
        RtosInteractionConfig::default()
    }

    pub fn startup_messages(&self) -> VecDeque<RtosStartupQueueMessage> {
        self.commands
            .iter()
            .map(|command| {
                let path = command.path();
                let mut message = match self.start_message_format {
                    RtosQueueMessageFormat::Callback => RtosStartupQueueMessage::start_proc(
                        self.start_queue.clone(),
                        command.command(),
                        path,
                    ),
                    RtosQueueMessageFormat::Terminal => {
                        RtosStartupQueueMessage::terminal_start_proc(
                            self.start_queue.clone(),
                            command.command(),
                            path,
                        )
                    }
                };
                message.format = self.start_message_format;
                message
            })
            .collect()
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
        Self {
            factory,
            handles,
            config,
        }
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

        if !self.config.direct_spawn {
            if self.config.queue_boot {
                log::debug!("RTOS boot service direct spawn disabled; startup commands are queued by RTOS interaction service");
            }
            return;
        }

        for command in self.config.commands() {
            log::info!(
                "RTOS direct start-process command 0x{:02x} path {} payload {:02x?}",
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

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => match value.as_str() {
            "0" | "off" | "false" | "FALSE" => false,
            "1" | "on" | "true" | "TRUE" => true,
            _ => default,
        },
        Err(_) => default,
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
