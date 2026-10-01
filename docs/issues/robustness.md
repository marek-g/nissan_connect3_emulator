# Robustness: Panics on Guest-Controlled Input

The emulator must never panic because of something the guest program does. Every item below crashes the whole host process from a syscall handler or callback triggered by guest behavior.

## panic! on every unimplemented syscall (highest impact)

- **Location:** `nissan_connect3_emulator/src/os/syscalls/hook_syscall.rs:225-241`
- **Problem:** Any guest call to an unhandled syscall number crashes the whole emulator. Line 229 even does a 1-second host sleep for `mq_open` and *then* still panics. This is the main reason new binaries can't be run at all.
- **Fix:** return `-ENOSYS` (and log the number); never panic from a syscall handler.

## Other panics in syscall handlers on guest-controlled input

| Location | Problem | Fix |
|---|---|---|
| `os/syscalls/fcntl.rs:183` | `panic!("unsupported command")` for unknown fcntl commands | return `-EINVAL` |
| `os/syscalls/resource.rs:49` | panics on any getrlimit resource except RLIMIT_STACK — glibc probes several at startup | return `-EINVAL`/`-ENOSYS` for unhandled resources |
| `os/syscalls/mman.rs:154` | panics on unaligned mmap | return `-EINVAL` |
| `os/syscalls/socket.rs:66` | `String::from_utf8(...).unwrap()` on socket payload | handle non-UTF8 gracefully |
| `file_system/tmp_file_system.rs:311`, `file_system/proc_file_system.rs:130` | `todo!()` in ioctl | return `-ENOTTY`/`-ENOSYS` |
| `file_system/std_file_system.rs:130,145` | `mem_write().unwrap()` on guest address | propagate the error |
| `emulator/utils.rs:8-20` (`load_binary`) | `panic!("Cannot load file")` on open failure; `vec![0u8; size as usize]` from `get_length()` (a `u64`, 0 for unknown fds) can over-allocate for special files | return a `Result`; validate/bound the length before allocating |
| `emulator/elf_loader.rs:126,154,171,182` | `.expect()` on malformed/failed ELF loads | return `Err` for bad input |
| `emulator/thread.rs:43-45, 135-137` | `.map_err(|e| format!(...)).unwrap()` in `Result`-returning fns — the error is converted to a `String` then immediately unwrapped; defeats the function's `Result` contract | propagate with `?` |
| `emulator/context.rs:18` + `mmu.rs:79,109,132,172` | `threads: Weak<...>` forces `.upgrade().unwrap()` in hot paths; if the owning `Process` is dropped before a callback fires this panics mid-syscall/MMU | hold an `Arc` (break the cycle elsewhere) or handle `None` gracefully |

## Execution errors swallowed into Ok(())

- **Location:** `nissan_connect3_emulator/src/emulator/thread.rs:296-311`
- **Problem:** When `emu_start` returns `Err`, the loop logs "Execution error", breaks, and then `emu_thread_loop` returns `Ok(())` — callers believe the program succeeded.
- **Fix:** capture the last `uc_error` and return it from `emu_thread_loop`.

## Fault callbacks do unbounded heavy work + magic address

- **Location:** `nissan_connect3_emulator/src/emulator/thread.rs:394-457`
- **Problem:** `callback_mem_error`/`callback_mem_rw` are near-identical; each runs `dump_context` (3× `disasm(…,200)` + stack + full MMU dump), so a guest loop dereferencing a bad pointer causes unbounded expensive logging. `pc - 100` underflows (lines 449, 454) and a magic address `0x484e93ec` is hardcoded (line 457).
- **Fix:** merge the two callbacks, rate-limit/cap the dump, use checked PC arithmetic, drop the hardcoded address.

## futex waiter leaks and panics on dead threads

- **Location:** `nissan_connect3_emulator/src/os/syscalls/futex.rs:73-120`
- **Problem:**
  - Waiters of dead threads are never removed from `futex_waiters`.
  - `receiver.recv().unwrap()` (line 83) panics if all senders are dropped.
  - `sender.send(()).unwrap()` (line 119) panics on a dead waiter.
  - The `timeout` argument is ignored — FUTEX_WAIT blocks forever.
- **Fix:** clean up waiters when threads die; use `Option`-based channel handling; honor the timeout (or document it as unsupported and return `-ENOSYS` for timed waits).

## writev swallows errors and reports partial success

- **Location:** `nissan_connect3_emulator/src/os/syscalls/uio.rs:24-46`
- **Problem:** Unbounded `iovcnt*8` allocation, per-iovec alloc + `mem_read().unwrap()`, and `Err(_) => {}` (line 43) swallows write errors while still returning a partial byte count as success.
- **Fix:** bound `iovcnt`, propagate read/write errors as proper errno.

## read/write: unbounded per-syscall allocation sized by guest length

- **Location:** `nissan_connect3_emulator/src/os/syscalls/unistd.rs:133, 169`
- **Problem:** `vec![0u8; length as usize]` on every read/write: a huge guest-supplied length triggers multi-GB allocations/OOM. Plus the pointless `file_system.clone()` Arc bump (lines 134/171) and double lock (`is_open` then `read`). Lines 176-178 also needlessly write back into the guest's source buffer after `write`.
- **Fix:** cap length against a bounded reusable buffer; remove the redundant `is_open` pre-check and the write-back.

## getdents64 loses iteration state on error path

- **Location:** `nissan_connect3_emulator/src/os/syscalls/unistd.rs:404`
- **Problem:** The `not_enough_space && res.len() == 0` branch discards the pending entry list, losing readdir iteration state for subsequent calls.
- **Fix:** restore `sys_calls_state` on the EINVAL path.
