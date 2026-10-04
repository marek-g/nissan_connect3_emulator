//! Host-side RTOS terminal/queue simulator.
//!
//! The real Triton side bootstraps the shared OSAL queues before Linux process
//! managers run and consumes terminal messages sent by Linux. In the emulator
//! there is no RTOS guest, but the shared SystemNamespace is visible to every
//! Linux process. This module pre-creates the boot queues, posts the RTOS's
//! `0xe` terminal-ready notification toward Linux, and drains the Linux-to-RTOS
//! terminal queue so guest messages are not left unread.

use crate::common::queues::{
    LI_TERM_MQ, OSAL_CB_HDR_LI_MAIN, OSAL_CB_HDR_MAXMSG, OSAL_CB_HDR_MSGSIZE, OSAL_CB_HDR_TE,
    TE_TERM_MQ, TERM_MQ_MAXMSG, TERM_MQ_MSGSIZE,
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

        namespace.mq.rtos_open_or_create(TE_TERM_MQ, TERM_MQ_MAXMSG, TERM_MQ_MSGSIZE);
        namespace.mq.rtos_open_or_create(LI_TERM_MQ, TERM_MQ_MAXMSG, TERM_MQ_MSGSIZE);
        namespace
            .mq
            .rtos_open_or_create(OSAL_CB_HDR_LI_MAIN, OSAL_CB_HDR_MAXMSG, OSAL_CB_HDR_MSGSIZE);
        namespace
            .mq
            .rtos_open_or_create(OSAL_CB_HDR_TE, OSAL_CB_HDR_MAXMSG, OSAL_CB_HDR_MSGSIZE);

        if namespace.mq.host_post(LI_TERM_MQ, vec![0x0e], 0) {
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
        namespace.mq.host_receive(TE_TERM_MQ)
    };

    match message {
        Some(message) => {
            let words = message_words(&message.data);
            log::info!(
                "RTOS terminal received {} command 0x{:02x} (len {}, data [{}, {}, {}, {}])",
                TE_TERM_MQ,
                words[0] & 0xff,
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

fn message_words(data: &[u8]) -> [u32; 4] {
    let mut words = [0u32; 4];
    for (index, word) in words.iter_mut().enumerate() {
        let offset = index * 4;
        if offset + 4 <= data.len() {
            *word = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
        }
    }
    words
}
