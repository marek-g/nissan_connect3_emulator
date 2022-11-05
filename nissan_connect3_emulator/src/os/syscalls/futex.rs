use crate::emulator::context::Context;
use crate::emulator::utils::unpack_u32;
use log::error;
use std::sync::mpsc::channel;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn set_robust_list(unicorn: &mut Unicorn<Context>, head: u32, len: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_robust_list(head = {:#x}, len: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        head,
        len,
    );

    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_robust_list => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}

pub fn futex(
    unicorn: &mut Unicorn<Context>,
    uaddr: u32,
    futex_op: u32,
    val: u32,
    timeout: u32,
    uaddr2: u32,
    val3: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] futex(uaddr = {:#x}, futex_op: {:#x}, val: {:#x}, timeout: {:#x}, uaddr2: {:#x}, val3: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        uaddr,
        futex_op,
        val,
        timeout,
        uaddr2,
        val3,
    );

    if futex_op & 0x80 == 0 {
        // FUTEX_PRIVATE_FLAG - if not set synchronization between processes is needed
        //panic!("futex without FUTEX_PRIVATE_FLAG not implemented");
        log::error!("futex without FUTEX_PRIVATE_FLAG not implemented");
    }

    let res = match futex_op & 0x7F {
        0x00 => {
            // FUTEX_WAIT
            let mut buf = vec![0u8; 4];
            unicorn.mem_read(uaddr as u64, &mut buf).unwrap();
            let val_read = unpack_u32(&buf);

            if val_read != val {
                -11i32 as u32; // EAGAIN
            }

            // wait
            log::trace!(
                "{:#x}: [{}] [SYSCALL] futex - wait",
                unicorn.reg_read(RegisterARM::PC).unwrap(),
                unicorn.get_data().inner.thread_id,
            );

            let (sender, receiver) = channel();
            {
                let data = unicorn.get_data();
                let sys_call_state = &mut data.inner.sys_calls_state.lock().unwrap();
                sys_call_state
                    .futex_waiters
                    .entry(uaddr)
                    .or_insert(Vec::new())
                    .push(sender);
            }
            receiver.recv().unwrap();

            log::trace!(
                "{:#x}: [{}] [SYSCALL] futex - woken up",
                unicorn.reg_read(RegisterARM::PC).unwrap(),
                unicorn.get_data().inner.thread_id,
            );

            0u32
        }
        0x01 => {
            // FUTEX_WAKE - wake at most `val` waiters
            let data = unicorn.get_data();
            let sys_call_state = &mut data.inner.sys_calls_state.lock().unwrap();
            let senders = if let Some(list) = sys_call_state.futex_waiters.get_mut(&uaddr) {
                let mut res = Vec::new();
                loop {
                    if res.len() == val as usize {
                        break;
                    }

                    if let Some(sender) = list.pop() {
                        res.push(sender);
                    } else {
                        break;
                    }
                }

                res
            } else {
                Vec::new()
            };

            let count = senders.len();

            for sender in senders {
                sender.send(()).unwrap();
            }

            count as u32
        }
        op => panic!("unsupported futex operation: {}", op),
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] futex => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res
    );

    res
}
