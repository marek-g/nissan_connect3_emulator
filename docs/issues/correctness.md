# Correctness Bugs

Bugs where the emulated guest behaves wrongly (wrong syscall semantics, data corruption, hangs).

## IOSC `/dev/iosc` device emulation — implemented, pending end-to-end verification

- **Location:** `nissan_connect3_emulator/src/os/syscalls/iosc.rs`; wired into `fcntl::open_internal`, `ioctl::ioctl`, `unistd::close`, and the scheduler deadline handling.
- **State:** The `/dev/iosc` character device is now emulated. `open("/dev/iosc")` hands out a reserved fd (verified at runtime: returns a valid fd); `ioctl(fd, 0x534f00XX, &arg)` dispatches the recovered command set — shared-malloc (0x00), create/obtain/release semaphore (0x02/0x04/0x05), enter/leave mutex (0x0c/0x0d), create/wait/set event (0x0e/0x10/0x11) — backed by shared Rust state in `IoscState`, with blocking ops using the `BlockReason`/`pending_result` machinery (same as mqueue).
- **Remaining / verification gap:** With this in place `procvoice_out.out` gets through early init (opens `/dev/iosc`, spawns threads, sizes the message pool) but still dies during message-queue setup — at the null-deref in `OSAL_s32IOClose` (libosal+0x19FB4) described in the next entry, with a corrupted return address (bad function pointer from a half-initialised object). No IOSC ioctls are observed before that fault, so the blocking primitives still need an end-to-end run to confirm. Root-causing whether the null-deref is downstream of an IOSC failure (or independent) is part of the signal-delivery work below.

## Memory fault kills the guest thread; no signal delivery

- **Location:** `nissan_connect3_emulator/src/emulator/scheduler.rs` (emu_start error path), `emulator/thread.rs` (`on_mem_fault`)
- **Problem:** Any unmapped access exits the whole guest thread (`Exited(1)`); when all threads are gone `run()` returns `Ok(())`. On real hardware the kernel would deliver SIGSEGV, so apps that install handlers (libosal has assert/signal machinery) never get to react. Observed in a verified run: main thread faults on a read of address `0x5` at libosal+0x19FB4 (null deref, LR inside the main executable) and dies; remaining threads continue until the reboot path.
 - **Fix:** decide the signal policy — deliver SIGSEGV to the guest (signal emulation with a proper sigreturn frame), or first root-cause the NULL at libosal+0x19FB4 in case it is an artifact of an earlier emulation gap; keep thread-death as an approximation of default signal handling only if that holds up.
