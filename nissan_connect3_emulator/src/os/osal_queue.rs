use crate::common::osal_queues::OsalQueueService;
use crate::emulator::context::Context;
use crate::emulator::thread::{BlockReason, ThreadStatus};
use crate::emulator::utils::pack_u32;
use std::time::Instant;
use unicorn_engine::Unicorn;

/// r799n13: read the queue handle of `name` from the current process's
/// libosal queue table (core double-pointer at libosal .bss 0x485679e0,
/// table at core+0x25220).  The MSGQ block is not a flat stride array
/// (r799n13: names sit at irregular offsets) - scan word-wise for the name
/// bytes (word reads tolerate unmapped gaps, a bulk read does not).  The
/// kernel handle of an entry is at name-0x24; `decode_queue_handle`
/// resolves a handle as info=[handle+0xc], name=[info+0x38], so the
/// self-pointer entries the fixup installs decode back to this name.
/// Returns None when libosal is not mapped or the queue has no entry.
pub(crate) fn guest_table_handle(unicorn: &mut Unicorn<'_, Context>, name: &str) -> Option<u32> {
    let mut w = [0u8; 4];
    unicorn.mem_read(0x4856_79e0, &mut w).ok()?;
    let core_struct = u32::from_le_bytes(w);
    if core_struct == 0 || core_struct > 0xf000_0000 {
        log::warn!("[LIBOSAL-MBX] guest-table lookup {name:?}: core_struct {core_struct:#x} bad");
        return None;
    }
    unicorn.mem_read(core_struct as u64, &mut w).ok()?;
    let core = u32::from_le_bytes(w);
    if core == 0 || core > 0xf000_0000 {
        log::warn!("[LIBOSAL-MBX] guest-table lookup {name:?}: core {core:#x} bad");
        return None;
    }
    let table = core.wrapping_add(0x25220);
    // The MSGQ block is not a flat stride-0x5c array (r799n13: names sit at
    // irregular offsets inside it) - scan word-wise for the name bytes
    // (word reads tolerate unmapped gaps, a bulk read does not).  The kernel
    // handle of an entry is at name-0x24 (mbx_28's live pointer
    // 0x90538490 and the mbx_1024 placeholder both proved the offset).
    let mut w4 = [0u8; 4];
    let mut pos = 0u32;
    while pos < 0x8000 {
        let a = table.wrapping_add(pos);
        if unicorn.mem_read(a as u64, &mut w4).is_ok() {
            if w4 == [0x6d, 0x62, 0x78, 0x5f] {
                // "mbx_"
                let nm = crate::libs::libosal_linux::read_cstr_at(unicorn, a, 0x20);
                log::warn!("[LIBOSAL-MBX] guest-table scan hit {nm:?} @ {a:#x}");
                if nm == name {
                    let addr = a.wrapping_sub(0x24);
                    if unicorn.mem_read(addr as u64, &mut w).is_ok() {
                        let h = u32::from_le_bytes(w);
                        if h != 0 && h != 0xffff_ffff {
                            return Some(h);
                        }
                    }
                    log::warn!(
                        "[LIBOSAL-MBX] guest table entry {name:?} at {a:#x} carries no usable handle"
                    );
                    return None;
                }
            }
        }
        pos += 4;
    }
    log::warn!(
        "[LIBOSAL-MBX] guest-table lookup {name:?}: no name hit in table {table:#x} (0x8000)"
    );
    None
}

/// Re-evaluate a guest thread blocked inside an intercepted OSAL queue wait.
///
/// This is the Unicorn/threading glue around the pure OSAL queue model: the
/// scheduler stops the VM, so guest-memory delivery happens in the owning
/// process' own address space, exactly like completed POSIX mqueue waits.
pub(crate) fn finish_guest_wait(unicorn: &mut Unicorn<'_, Context>, tid: u32, now: Instant) {
    let reason = {
        let threads = unicorn.get_data().threads.lock().unwrap();
        threads
            .iter()
            .find(|thread| thread.id == tid)
            .and_then(|thread| match thread.status {
                ThreadStatus::Blocked(reason) => Some(reason),
                _ => None,
            })
    };

    let Some(BlockReason::OsalQueueReceive {
        queue_id,
        msg_ptr,
        msg_len,
        prio_ptr,
        deadline,
        materialize,
    }) = reason
    else {
        return;
    };

    let message = {
        let mut state = unicorn.get_data().namespace.lock().unwrap();
        let message = OsalQueueService::pop_guest_message(
            &mut state.mq,
            queue_id,
            if materialize { usize::MAX } else { msg_len as usize },
        );
        if message.is_some() || deadline.map(|deadline| deadline <= now).unwrap_or(false) {
            state.mq.remove_waiter(queue_id, tid);
        }
        // r798 diagnostic: parked waiter vs queue contents (the tid-84
        // mbx_289 stall: posts report success yet this pop returns None).
        if message.is_none()
            && deadline.is_none()
            && state
                .mq
                .queues
                .get(&queue_id)
                .map(|x| x.name == "mbx_289")
                .unwrap_or(false)
            && crate::libs::dapi::emu_t0_elapsed_ms() % 4000 < 300
        {
            static LAST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let bucket = (crate::libs::dapi::emu_t0_elapsed_ms() / 4000) as u32;
            if LAST.swap(bucket, std::sync::atomic::Ordering::Relaxed) != bucket {
                let q = state.mq.queues.get(&queue_id);
                log::info!(
                    "[{tid}] osal-wait recheck q={queue_id} name={:?} pending={} msgsize={} maxmsg={} unlinked={} this_msgsize={}",
                    q.as_ref().map(|x| x.name.clone()),
                    q.as_ref().map(|x| x.messages.len()).unwrap_or(usize::MAX),
                    q.as_ref().map(|x| x.msgsize).unwrap_or(-1),
                    q.as_ref().map(|x| x.maxmsg).unwrap_or(-1),
                    q.as_ref().map(|x| x.unlinked).unwrap_or(true),
                    q.as_ref()
                        .and_then(|x| x.messages.first())
                        .map(|m| m.data.len())
                        .unwrap_or(usize::MAX),
                );
            }
        }
        message
    };

    match message {
        Some(mut message) if materialize => {
            // r799n12: kernel stamp for the service-data reply-queue field.
            // The routing/wire body carries the poster's reply queue at
            // body word 3 (blob offset 8+0x0c); the real kernel stamps it at
            // route time (senders build it as 0xffffffff - the STAT side of
            // procmapengine cannot know its own handle), and the receiving
            // server port stores it into [port+0x20] to post its
            // ServiceData answers (svc-0x1e answer leg: queue=-1 failures).
            // Stamp exactly like the kernel: the handle of `mbx_{src}` from
            // our own queue-handle table.  Only touch bodies that still read
            // the -1 placeholder.
            if message.data.len() >= 0x28
                && message.data[0x14..0x18] == [0xff, 0xff, 0xff, 0xff]
            {
                let src_app = u16::from_le_bytes([message.data[8], message.data[9]]);
                if src_app != 0 {
                    let name = format!("mbx_{src_app}");
                    // r799n13: take the handle from this process's guest
                    // queue table (the value its own OSAL post path can
                    // decode); when the table has no entry, leave the
                    // placeholder untouched rather than inject a handle the
                    // guest cannot resolve.
                    if let Some(handle) = guest_table_handle(unicorn, &name) {
                        message.data[0x14..0x18].copy_from_slice(&handle.to_le_bytes());
                        log::info!(
                            "[{}] [LIBOSAL-MBX] kernel reply-queue stamp: src app {src_app:#x} -> {name:?} handle {handle:#x}",
                            tid
                        );
                    }
                }
            }
            match crate::libs::libosal_linux::message::deliver_snapshot_message(
                unicorn,
                &message.data,
                msg_ptr,
                prio_ptr,
                message.priority,
            ) {
                Some(result) => {
                    log::info!(
                        "[{}] [LIBOSAL-MBX] OSAL queue wait woke queue_id={} with materialized snapshot len={}",
                        tid,
                        queue_id,
                        message.data.len()
                    );
                    // r757 diagnostic: dump the wire header of every CCA blob
                    // that gets materialised for a waiter.  The CCA gateway
                    // hands the message msgid (@body+0x10) to the HSI layer as
                    // the sMsg type, and the HSI-NAV client forwards that type
                    // verbatim as an FI event id - so a message with msgid
                    // 0x81008031 is what releases the wait-widget.  This dump
                    // names the posters of the real 0x2b09-0x2b0b family.
                    let body = &message.data[8.min(message.data.len())..];
                    if body.len() >= 0x1c {
                        let rd16 = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
                        let rd32 = |o: usize| {
                            u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]])
                        };
                        log::info!(
                            "[LIBOSAL-MBX]   wire body: src={:#x} dst={:#x} class={:#x} subid={:#x} msgid={:#x} svc={:#x} regid={:#x} func={:#x}",
                            rd16(0),
                            rd16(2),
                            body[0xb],
                            rd16(0xc),
                            rd32(0x10),
                            rd16(0x14),
                            rd16(0x16),
                            rd32(0x18),
                        );
                        // r799n11c: the svc-0x1e ServiceData answer leg dies on
                        // the reply-queue handle stored from this body (the
                        // server's port slot +0x20 stays -1, so its answer
                        // posts to handle 0xffffffff and fails).  Dump the full
                        // header to locate the reply-queue field the receiver
                        // should bind, and diff against an answered svc-0x300
                        // request that uses the same class-0x45 envelope.
                        if (rd16(0x14) == 0x1e || rd16(0x14) == 0x300) && body.len() >= 0x80 {
                            let words: Vec<String> = (0..0x20usize)
                                .map(|w| format!("{:08x}", rd32(w * 4)))
                                .collect();
                            log::info!(
                                "[LIBOSAL-MBX]   svc{:03x} body words: {}",
                                rd16(0x14),
                                words.join(" ")
                            );
                        }
                    }
                    set_runnable_with_result(unicorn, tid, result);
                }
                None => {
                    log::warn!(
                        "[{}] [LIBOSAL-MBX] failed to materialize snapshot for queue_id={} blob_len={}",
                        tid,
                        queue_id,
                        message.data.len()
                    );
                }
            }
        }
        Some(message) => {
            log::info!(
                "[{}] [LIBOSAL] OSAL queue wait woke queue_id={} len={} data={:02x?}",
                tid,
                queue_id,
                message.data.len(),
                message.data
            );
            if !message.data.is_empty() {
                unicorn.mem_write(msg_ptr as u64, &message.data).unwrap();
            }
            if prio_ptr != 0 {
                unicorn
                    .mem_write(prio_ptr as u64, &pack_u32(message.priority))
                    .unwrap();
            }
            set_runnable_with_result(unicorn, tid, message.data.len() as u32);
        }
        None if deadline.map(|deadline| deadline <= now).unwrap_or(false) => {
            set_runnable_with_result(unicorn, tid, 0)
        }
        None => {}
    }
}

fn set_runnable_with_result(unicorn: &mut Unicorn<'_, Context>, tid: u32, result: u32) {
    let mut threads = unicorn.get_data().threads.lock().unwrap();
    if let Some(thread) = threads.iter_mut().find(|thread| thread.id == tid) {
        if matches!(thread.status, ThreadStatus::Blocked(_)) {
            thread.status = ThreadStatus::Runnable;
            thread.pending_result = Some(result);
        }
    }
}
