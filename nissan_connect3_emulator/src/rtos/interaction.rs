//! Queue-level interaction between the simulated RTOS and Linux processes.
//!
//! The RTOS side does not have a guest VM in this emulator. It still owns the
//! TEngine side of the shared OSAL queues, so this service mirrors the RTOS
//! task flow found in `triton_mid_raw.bin`:
//!
//! * `FUN_8013f2b0` creates `TE_TERM_MQ` and `LI_TERM_MQ`, posts the initial
//!   terminal-ready word `0x0e` to `LI_TERM_MQ`, then consumes `TE_TERM_MQ`.
//! * `FUN_80153a54` creates `OSAL_CB_HDR_LI_MAIN` and `OSAL_CB_HDR_TE`, then
//!   consumes `OSAL_CB_HDR_TE` and ignores callback-header types other than 7.
//!
//! Startup queue messages are enabled by default and pre-queued before Linux process
//! spawn so waiting guest processes can consume them. The mapped RTOS terminal task
//! itself does not send `0x1a`; the emulator uses the Linux OSAL callback layout as the
//! bridge command.

use crate::common::osal_queues::{
    callback_message_command, make_callback_command_message, make_terminal_command_message,
    message_command, message_words, OsalMessage, OsalQueueService, LI_TERM_MQ, OSAL_CB_HDR_TE,
    OSAL_CB_MESSAGE_MAIN, RTOS_TERMINAL_READY, TE_TERM_MQ,
};
use crate::os::syscalls::namespace::SystemNamespace;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RtosQueueMessageFormat {
    /// `[0, 0, command, payload...]`: libosal callback/system command layout.
    Callback,
    /// `[command, 0, 0, 0, payload...]`: RTOS terminal word-command layout.
    Terminal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtosStartupQueueMessage {
    pub queue_name: String,
    pub command: u8,
    pub payload: Vec<u8>,
    pub description: String,
    pub format: RtosQueueMessageFormat,
}

impl RtosStartupQueueMessage {
    pub fn new(
        queue_name: impl Into<String>,
        command: u8,
        payload: Vec<u8>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            queue_name: queue_name.into(),
            command,
            payload,
            description: description.into(),
            format: RtosQueueMessageFormat::Callback,
        }
    }

    pub fn terminal(
        queue_name: impl Into<String>,
        command: u8,
        payload: Vec<u8>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            queue_name: queue_name.into(),
            command,
            payload,
            description: description.into(),
            format: RtosQueueMessageFormat::Terminal,
        }
    }

    pub fn start_proc(queue_name: impl Into<String>, command: u8, path: impl Into<String>) -> Self {
        let path = path.into();
        Self::new(queue_name, command, path.as_bytes().to_vec(), path.clone())
    }

    pub fn terminal_start_proc(
        queue_name: impl Into<String>,
        command: u8,
        path: impl Into<String>,
    ) -> Self {
        let path = path.into();
        Self::terminal(queue_name, command, path.as_bytes().to_vec(), path.clone())
    }

    fn encoded(&self) -> Vec<u8> {
        match self.format {
            RtosQueueMessageFormat::Callback => {
                make_callback_command_message(self.command, &self.payload)
            }
            RtosQueueMessageFormat::Terminal => {
                make_terminal_command_message(self.command, &self.payload)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct RtosInteractionConfig {
    /// Post a duplicate terminal-ready message after Linux replies. The real RTOS
    /// only sets an internal ready flag, so this is opt-in bring-up behavior.
    pub ack_terminal_ready: bool,
    /// Wait for Linux terminal ready on TE_TERM_MQ before sending optional
    /// startup queue messages. This only affects explicitly enabled startup
    /// queue messages; it is not part of the mapped RTOS terminal task flow.
    pub wait_for_terminal_handshake: bool,
    pub startup_handshake_timeout: Duration,
}

impl Default for RtosInteractionConfig {
    fn default() -> Self {
        Self {
            ack_terminal_ready: env_flag("EMU_RTOS_TERMINAL_ACK"),
            wait_for_terminal_handshake: env_flag_or_default("EMU_RTOS_WAIT_TE_READY", true),
            startup_handshake_timeout: env_duration_ms(
                "EMU_RTOS_TE_READY_TIMEOUT_MS",
                Duration::from_millis(1500),
            ),
        }
    }
}

pub struct RtosQueueInteraction {
    stop: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
}

impl RtosQueueInteraction {
    pub fn bootstrap_with_startup_messages(
        namespace: &Arc<Mutex<SystemNamespace>>,
        startup_messages: VecDeque<RtosStartupQueueMessage>,
    ) {
        let mut namespace = namespace.lock().unwrap();

        let ready_posted = OsalQueueService::rtos_terminal_bootstrap(&mut namespace.mq);
        if ready_posted {
            log::info!(
                "RTOS terminal task posted ready notification to {}",
                LI_TERM_MQ
            );
        }

        OsalQueueService::rtos_callback_bootstrap(&mut namespace.mq);

        if !startup_messages.is_empty() {
            log::info!(
                "RTOS startup queued {} pre-spawn start-process message(s)",
                startup_messages.len()
            );
            for message in startup_messages {
                post_startup_message_locked(&mut namespace, &message);
            }
        }

        namespace.notify_waiters();
    }

    pub fn start_with_startup_messages(
        namespace: Arc<Mutex<SystemNamespace>>,
        config: RtosInteractionConfig,
        startup_messages: VecDeque<RtosStartupQueueMessage>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));

        let terminal_namespace = namespace.clone();
        let terminal_stop = stop.clone();
        let terminal_handle = thread::spawn(move || {
            let mut state = TerminalTaskState {
                config,
                startup_messages,
                linux_terminal_ready: false,
                started_at: Instant::now(),
            };

            while !terminal_stop.load(Ordering::Relaxed) {
                terminal_task_poll_once(&terminal_namespace, &mut state);
                thread::sleep(Duration::from_millis(5));
            }
        });

        let callback_namespace = namespace.clone();
        let callback_stop = stop.clone();
        let callback_handle = thread::spawn(move || {
            while !callback_stop.load(Ordering::Relaxed) {
                callback_header_task_poll_once(&callback_namespace);
                thread::sleep(Duration::from_millis(5));
            }
        });

        Self {
            stop,
            handles: vec![terminal_handle, callback_handle],
        }
    }

    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        for handle in self.handles {
            let _ = handle.join();
        }
    }
}

struct TerminalTaskState {
    config: RtosInteractionConfig,
    startup_messages: VecDeque<RtosStartupQueueMessage>,
    linux_terminal_ready: bool,
    started_at: Instant,
}

impl TerminalTaskState {
    fn should_send_startup_messages(&self) -> bool {
        !self.startup_messages.is_empty()
            && (!self.config.wait_for_terminal_handshake
                || self.linux_terminal_ready
                || self.started_at.elapsed() >= self.config.startup_handshake_timeout)
    }
}

fn terminal_task_poll_once(namespace: &Arc<Mutex<SystemNamespace>>, state: &mut TerminalTaskState) {
    let mut notify = false;
    let mut ack_terminal_ready = false;

    {
        let mut namespace = namespace.lock().unwrap();

        while let Some(message) = OsalQueueService::receive_message(&mut namespace.mq, TE_TERM_MQ) {
            notify = true;
            let ready = handle_inbound_terminal_message(&message);
            state.linux_terminal_ready |= ready;
            ack_terminal_ready |= ready;
        }

        if ack_terminal_ready && state.config.ack_terminal_ready {
            if OsalQueueService::post_rtos_terminal_ready(&mut namespace.mq) {
                notify = true;
                log::info!("RTOS terminal ready ack posted to LI_TERM_MQ (opt-in ack)");
            }
        }

        if state.should_send_startup_messages() {
            while let Some(message) = state.startup_messages.front() {
                if !post_startup_message_locked(&mut namespace, message) {
                    break;
                }
                notify = true;
                state.startup_messages.pop_front();
            }
        }

        if notify {
            namespace.notify_waiters();
        }
    }
}

fn callback_header_task_poll_once(namespace: &Arc<Mutex<SystemNamespace>>) {
    let mut notify = false;

    {
        let mut namespace = namespace.lock().unwrap();

        while let Some(message) =
            OsalQueueService::receive_message(&mut namespace.mq, OSAL_CB_HDR_TE)
        {
            notify = true;
            handle_inbound_callback_message(&message);
        }

        if notify {
            namespace.notify_waiters();
        }
    }
}

fn post_startup_message_locked(
    namespace: &mut SystemNamespace,
    message: &RtosStartupQueueMessage,
) -> bool {
    let data = message.encoded();
    let accepted =
        OsalQueueService::rtos_post_message(&mut namespace.mq, &message.queue_name, data, 0);

    log::info!(
        "RTOS queue interaction {} format {:?} command 0x{:02x} {} accepted {}",
        message.queue_name,
        message.format,
        message.command,
        message.description,
        accepted
    );
    accepted
}

fn handle_inbound_terminal_message(message: &OsalMessage) -> bool {
    let words = message_words(&message.data);
    let command = message_command(&message.data).unwrap_or(0);

    log::info!(
        "RTOS terminal task received {} command 0x{:02x} (prio {}, len {}, data [{}, {}, {}, {}])",
        TE_TERM_MQ,
        command,
        message.priority,
        message.data.len(),
        words[0],
        words[1],
        words[2],
        words[3]
    );

    match command {
        0 | 2 | 4..=6 | 8 | 0xf..=0x11 => {
            log::debug!("RTOS terminal command 0x{:02x}: RTOS no-op path", command);
        }
        1 => {
            log::debug!(
                "RTOS terminal command 0x01: FUN_80150dac(local_70[0]={:#x}) not emulated",
                words[1]
            );
        }
        3 => {
            log::debug!(
                "RTOS terminal command 0x03: FUN_80151664(local_70[0]+0x34={:#x}) not emulated",
                words[1] + 0x34
            );
        }
        7 => {
            log::debug!(
                "RTOS terminal command 0x07: FUN_80159a2c(local_70[0]={:#x}) not emulated",
                words[1]
            );
        }
        9 => {
            log::debug!(
                "RTOS terminal command 0x09: local_70[0]+0x3c={:#x}, RTOS branch not emulated",
                words[1] + 0x3c
            );
        }
        10 => {
            log::debug!("RTOS terminal command 0x0a: RTOS auStack_60 side effect not emulated");
        }
        RTOS_TERMINAL_READY => {
            log::debug!("RTOS terminal task observed Linux terminal ready flag");
            return true;
        }
        0x12 => {
            log::debug!("RTOS terminal command 0x12: MBX blocking side effect not emulated");
        }
        0x13 => {
            log::debug!("RTOS terminal command 0x13: MBX release side effect not emulated");
        }
        0x14 => {
            log::debug!("RTOS terminal command 0x14: mailbox callback side effect not emulated");
        }
        _ => {
            log::debug!("RTOS terminal command 0x{:02x}: default wait path", command);
        }
    }

    false
}

fn handle_inbound_callback_message(message: &OsalMessage) {
    let words = message_words(&message.data);
    let message_type = words[0];
    let callback_command = callback_message_command(&message.data).unwrap_or(0);

    log::info!(
        "RTOS callback-header task received {} type {} callback-command 0x{:02x} (prio {}, len {}, data [{}, {}, {}, {}])",
        OSAL_CB_HDR_TE,
        message_type,
        callback_command,
        message.priority,
        message.data.len(),
        words[0],
        words[1],
        words[2],
        words[3]
    );

    if message_type == OSAL_CB_MESSAGE_MAIN {
        log::debug!("RTOS callback-header task type 7: RTOS table dispatch not emulated");
    } else if message_type == 6 || message_type == 0xf {
        log::debug!(
            "RTOS callback-header task skipped callback type {} per RTOS wait loop",
            message_type
        );
    } else {
        log::debug!(
            "RTOS callback-header task ignored non-7 callback type {} per RTOS wait loop",
            message_type
        );
    }
}

fn env_flag(name: &str) -> bool {
    env_flag_or_default(name, false)
}

fn env_flag_or_default(name: &str, default: bool) -> bool {
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
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(default)
}
