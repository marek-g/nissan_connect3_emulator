# Correctness Bugs

Bugs where the emulated guest behaves wrongly (wrong syscall semantics, data corruption, hangs).

## IOSC message queues unimplemented → most processes fail at queue creation

- **Location:** libosal `OSAL_s32MessageQueueCreate`/`u32CreateMsgQueue` (IOSC path) → `libiosclib_so.so` `iosc_enter_mutex`/`iosc_create_event`/`iosc_create_semaphore`/`iosc_shared_malloc_with_id` → `/dev/iosc`
- **Problem:** `s32CheckForIOSCQueue` classifies every queue as IOSC unless its name starts with `"NOIOSC"`, so the default IPC path is the IOSC kernel driver (`/dev/iosc`), not POSIX mqueue. In the emulator `iosc_enter_mutex` fails, hits `OSAL_vAssertFunction`, and the reset handler reboots (observed with `procvoice_out.out`). The POSIX mqueue syscalls are now implemented (they only serve the `"NOIOSC*"` local-queue fallback used e.g. by `procmapengine.out`), but IOSC itself is not emulated.
- **Fix:** emulate the IOSC primitives. Either hook the `iosc_*` client functions in `libiosclib_so.so` (like the existing libosal/libtrace hooks) and back them with cross-process shared state, or emulate the `/dev/iosc` character device (open + ioctrl/read/write protocol). Needed before any non-NOIOSC process can reach its main loop.

## Memory fault kills the guest thread; no signal delivery

- **Location:** `nissan_connect3_emulator/src/emulator/scheduler.rs` (emu_start error path), `emulator/thread.rs` (`on_mem_fault`)
- **Problem:** Any unmapped access exits the whole guest thread (`Exited(1)`); when all threads are gone `run()` returns `Ok(())`. On real hardware the kernel would deliver SIGSEGV, so apps that install handlers (libosal has assert/signal machinery) never get to react. Observed in a verified run: main thread faults on a read of address `0x5` at libosal+0x19FB4 (null deref, LR inside the main executable) and dies; remaining threads continue until the reboot path.
- **Fix:** decide the signal policy — deliver SIGSEGV to the guest (signal emulation with a proper sigreturn frame), or first root-cause the NULL at libosal+0x19FB4 in case it is an artifact of an earlier emulation gap; keep thread-death as an approximation of default signal handling only if that holds up.

## CLONE_CHILD_CLEARTID not implemented

- **Location:** `nissan_connect3_emulator/src/os/syscalls/sched.rs` (`clone`)
- **Problem:** glibc passes `CLONE_CHILD_CLEARTID`; on child exit the kernel zeroes `*child_tidptr` and does a futex wake there (`do_exit → clear_child_tid`, `kernel/exit.c`). Skipping it leaves stale tid values that pthread join/cleanup code relies on.
- **Fix:** store `child_tidptr` in `GuestThread`; on thread exit write 0 to it and wake futex waiters at that address.
