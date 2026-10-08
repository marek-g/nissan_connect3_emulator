# Correctness Bugs

Bugs where the emulated guest behaves wrongly (wrong syscall semantics, data corruption, hangs).

## IOSC message-queue setup fails → `u32CreateMsgQueue` asserts (procvoice reboots)

- **Location:** libosal `OSAL_s32MessageQueueCreate` (+0x46c) → `u32CreateMsgQueue` (0x4850bc90, +0x60) → `u32MapErrorCodeIOSC` (0x48509070, +0xc4) → `OSAL_vAssertFunction`; the IOSC primitives live in `nissan_connect3_emulator/src/os/syscalls/iosc.rs`.
- **State:** `/dev/iosc` is emulated and SIGSEGV delivery now works (see below), so `procvoice_out.out` runs through early init and then faults in the message-queue path. The delivered SIGSEGV backtrace pinpoints it: an IOSC operation inside `u32CreateMsgQueue` returns a value that `u32MapErrorCodeIOSC` does not map to the success code (0x72000), tripping the assert → reboot. A null-deref in `OSAL_s32IOClose` (libosal+0x19FB4, reading address 0x5) is the immediate fault; it is downstream of that failed IOSC op.
- **Fix:** determine which IOSC primitive in `u32CreateMsgQueue` fails and why — either the `/dev/iosc` ioctl emulation returns a wrong value for that command, or the queue name/id handling differs from the real driver. Trace the actual ioctl(s) issued by `u32CreateMsgQueue` (enable ioctl trace logging) and align the emulator's response with the recovered protocol.

## Cross-process futex (no `FUTEX_PRIVATE_FLAG`) can stall a waiter

- **Location:** `nissan_connect3_emulator/src/os/syscalls/futex.rs` (`wake_waiters`), surfaced by the `futex without FUTEX_PRIVATE_FLAG not implemented` error log.
- **Problem:** with the parallel one-VM-per-process model, `wake_waiters` only marks threads in the *current* VM runnable. A `FUTEX_WAKE` issued by one process for a futex word whose waiters live in another process (the non-`PRIVATE`, shared-memory futex case) does not wake them; the waiter sleeps until its timeout. Same-process (pthread) futexes are unaffected. The guest logs `futex without FUTEX_PRIVATE_FLAG not implemented` when this path is hit.
- **Fix:** route `FUTEX_WAKE` on a non-private futex through the namespace (map the shared futex word to waiters across processes, e.g. back such futexes with a host-side word in shared memory and a namespace waiter set), mirroring the mq/iosc doorbell + owner re-check pattern.

## procmap entry-thread Wait loop exits after first START_CONF → STATE_CHANGE_REQ never delivered

- **Location:** procmap `ail_tclAppInterface::vAppEntry` at Ghidra 0x0065eb40. `bDispatchCCAMessages` (0x0066c8e8) returns non-zero on the START_CONF dispatch, so the `do { Wait; dispatch; } while (dispatch != 0);` loop should re-enter `Wait`. Observation from `/tmp/opencode/pwr_run_final.log`: procmap calls `OSAL_s32MessageQueueWait(mbx_1024, ...)` exactly once; the second Wait is never issued, so the `STATE_CHANGE_REQ` and `CVM_SIGNAL_CHANGED` messages our proxy queues on the ack get no delivery.
- **Impact:** vtable+0x1c (`vOnNewAppState`) is only invoked from `bHandleMsgPowerMessage` case 0x10 (STATE_CHANGE_REQ) at 0x00667d38; without the second dispatch procmap never calls `StartMapEngine`, so the DAPI `DapiGetBlockIDs` / map-block request chain is not exercised.
- **Fix (to determine):** trace `bDispatchCCAMessages` return path (0x0066c944/0x0066c948/0x0066c980) with a per-thread counter; verify `r6` at the return point equals 1 (continue). Alternative: deliver `STATE_CHANGE_REQ` and `CVM_SIGNAL_CHANGED` directly to the body-thread's queue `MQB_1024` before it enters its own Wait — this matches the "forward to body thread" branch in the real dispatch and lets vtable+0x1c run on the body-thread stack.
