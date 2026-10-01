# Robustness: Panics on Guest-Controlled Input

The emulator must never panic because of something the guest program does. Every item below crashes the whole host process from a syscall handler or callback triggered by guest behavior.

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
