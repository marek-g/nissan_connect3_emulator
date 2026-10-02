# Correctness Bugs

Bugs where the emulated guest behaves wrongly (wrong syscall semantics, data corruption, hangs).

## IOSC message-queue setup fails → `u32CreateMsgQueue` asserts (procvoice reboots)

- **Location:** libosal `OSAL_s32MessageQueueCreate` (+0x46c) → `u32CreateMsgQueue` (0x4850bc90, +0x60) → `u32MapErrorCodeIOSC` (0x48509070, +0xc4) → `OSAL_vAssertFunction`; the IOSC primitives live in `nissan_connect3_emulator/src/os/syscalls/iosc.rs`.
- **State:** `/dev/iosc` is emulated and SIGSEGV delivery now works (see below), so `procvoice_out.out` runs through early init and then faults in the message-queue path. The delivered SIGSEGV backtrace pinpoints it: an IOSC operation inside `u32CreateMsgQueue` returns a value that `u32MapErrorCodeIOSC` does not map to the success code (0x72000), tripping the assert → reboot. A null-deref in `OSAL_s32IOClose` (libosal+0x19FB4, reading address 0x5) is the immediate fault; it is downstream of that failed IOSC op.
- **Fix:** determine which IOSC primitive in `u32CreateMsgQueue` fails and why — either the `/dev/iosc` ioctl emulation returns a wrong value for that command, or the queue name/id handling differs from the real driver. Trace the actual ioctl(s) issued by `u32CreateMsgQueue` (enable ioctl trace logging) and align the emulator's response with the recovered protocol.
