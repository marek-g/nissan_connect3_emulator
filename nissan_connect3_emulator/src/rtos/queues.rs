//! Host-side RTOS terminal/queue simulator.
//!
//! The real Triton side bootstraps the shared OSAL queues before Linux process
//! managers run and consumes terminal messages sent by Linux. In the emulator
//! there is no RTOS guest, but the shared SystemNamespace is visible to every
//! Linux process. This module pre-creates the boot queues, posts the RTOS's
//! `0xe` terminal-ready notification toward Linux, and drains the Linux-to-RTOS
//! terminal queue so guest messages are not left unread.

use crate::common::osal_queues::{
    message_command, message_words, OsalQueueService, LI_TERM_MQ, TE_TERM_MQ,
};
use crate::os::syscalls::namespace::SystemNamespace;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct RtosQueueSimulator {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl RtosQueueSimulator {
    /// Pre-create the queues that the RTOS owns and send the RTOS's startup
    /// terminal-ready notification. This should run before the Linux processes
    /// start, matching `FUN_8013f2b0` on the real side.
    pub fn bootstrap(namespace: &Arc<Mutex<SystemNamespace>>) {
        let mut namespace = namespace.lock().unwrap();

        OsalQueueService::bootstrap_rtos_queues(&mut namespace.mq);
        if OsalQueueService::post_rtos_terminal_ready(&mut namespace.mq) {
            log::info!("RTOS terminal posted startup notification 0x0e to {}", LI_TERM_MQ);
        }

        namespace.notify_waiters();
    }

    pub fn start(namespace: Arc<Mutex<SystemNamespace>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let running = stop.clone();
        let poll_namespace = namespace.clone();

        let handle = thread::spawn(move || {
            while !running.load(Ordering::Relaxed) {
                if !poll_once(&poll_namespace) {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });

        Self { stop, handle }
    }

    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.join();
    }
}

fn poll_once(namespace: &Arc<Mutex<SystemNamespace>>) -> bool {
    let message = {
        let mut namespace = namespace.lock().unwrap();
        OsalQueueService::receive_rtos_terminal_message(&mut namespace.mq)
    };

    match message {
        Some(message) => {
            let words = message_words(&message.data);
            let command = message_command(&message.data).unwrap_or(0);
            log::info!(
                "RTOS terminal received {} command 0x{:02x} (prio {}, len {}, data [{}, {}, {}, {}])",
                TE_TERM_MQ,
                command,
                message.priority,
                message.data.len(),
                words[0],
                words[1],
                words[2],
                words[3]
            );
        }
        None => return true,
    }

    namespace.lock().unwrap().notify_waiters();

    true
}
