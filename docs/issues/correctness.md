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

## 0x305 is the DAPIAPP CCA map-data service (svc 0x26), not PROCNAV

Full-run trace (r166) settles the long-standing `DAPI ServiceDataError
code=0x305` question and overturns the PROCNAV-centric theory of earlier
sessions:

- PROCNAV (thread [18]) is healthy and never involved: it completes its startup
  registration once and runs a normal main loop; it names `mbx_0` zero times and
  performs zero message receives. It is not the nav responder.
- The road-network / map-data answer is DAPIAPP's own CCA **service 0x26**
  ("job type 4"), served by `dap_map_tclWorker` inside DAPIAPP. The registry
  gate and medium-up ordering pass (`MAP_MEDIUM_ACTIVE` fires; `svcdata-PATH
  r9=0x26` dispatches to handler `0x823998`; `enProcessJob result=0xffff` = the
  worker's success code; `u16SendResponse` sends the answer).
- Only ~2 map requests reach the worker per boot: `vSendToTask jobtype=4` count
  is 2 and `u16SendResponse` count is 2, while the client (thread [36]) waits on
  `DYN4MBX` ~10 times. The unanswered requests never become worker jobs; the
  map workers' `ActionList::bSetBlock` sees `capacity=0`. The client then times
  out and DAPI answers `0x305` (`amt_tclServiceDataError::vSetErrorData`,
  lr=0xb52520).

So the remaining blocker is upstream of the worker: why most svc-0x26 requests
are not turned into dispatched jobs. Prime suspects, both tied to worker/action
list initialisation and the medium-active ordering noted in `dapi.rs`:
- the client registry entry `0x9035ab94` staying at state 1 (not ACTIVE), which
  `dapi.rs` already flags as making later requests fail; and
- map-worker `ActionList` capacity left at 0.

Next concrete step is RE on `DAPIAPP.OUT` (already in Ghidra) at `lr=0xb52520`
(the 0x305 site) and at the `ActionList` capacity setter / worker init, to see
the exact branch that drops the request.
