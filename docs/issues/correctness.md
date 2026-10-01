# Correctness Bugs

Bugs where the emulated guest behaves wrongly (wrong syscall semantics, data corruption, hangs).

## mq_open unimplemented → OSAL message queue creation fails → reboot loop

- **Location:** `nissan_connect3_emulator/src/os/syscalls/hook_syscall.rs` (fallback arm, ARM #274), libosal `u32CreateMsgQueue`/`OSAL_s32MessageQueueCreate`
- **Problem:** libosal's `u32CreateMsgQueue` calls `mq_open` (ARM #274) which returns `-ENOSYS`; the failure maps to an IOSC error, hits `OSAL_vAssertFunction`, and the firmware reset handler reboots ("rebooting", write to `/sys/devices/platform/dexcep/trigger_exception`). The guest never reaches its main loop. This is the current blocker for running the navigation app.
- **Fix:** implement POSIX message queues per `fs/mqueue.c` (mq_open #274, mq_getattr #275, mq_setattr #276, mq_notify #277, mq_unlink #278, mq_close #279, mq_timedsend #280, mq_receive #281, mq_timedreceive #282), or stub `u32CreateMsgQueue` at the OSAL hook level as a stopgap.

## Memory fault kills the guest thread; no signal delivery

- **Location:** `nissan_connect3_emulator/src/emulator/scheduler.rs` (emu_start error path), `emulator/thread.rs` (`on_mem_fault`)
- **Problem:** Any unmapped access exits the whole guest thread (`Exited(1)`); when all threads are gone `run()` returns `Ok(())`. On real hardware the kernel would deliver SIGSEGV, so apps that install handlers (libosal has assert/signal machinery) never get to react. Observed in a verified run: main thread faults on a read of address `0x5` at libosal+0x19FB4 (null deref, LR inside the main executable) and dies; remaining threads continue until the reboot path.
- **Fix:** decide the signal policy — deliver SIGSEGV to the guest (signal emulation with a proper sigreturn frame), or first root-cause the NULL at libosal+0x19FB4 in case it is an artifact of an earlier emulation gap; keep thread-death as an approximation of default signal handling only if that holds up.

## CLONE_CHILD_CLEARTID not implemented

- **Location:** `nissan_connect3_emulator/src/os/syscalls/sched.rs` (`clone`)
- **Problem:** glibc passes `CLONE_CHILD_CLEARTID`; on child exit the kernel zeroes `*child_tidptr` and does a futex wake there (`do_exit → clear_child_tid`, `kernel/exit.c`). Skipping it leaves stale tid values that pthread join/cleanup code relies on.
- **Fix:** store `child_tidptr` in `GuestThread`; on thread exit write 0 to it and wake futex waiters at that address.
