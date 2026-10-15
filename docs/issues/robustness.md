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

## Startup flakiness: AE pump (tid77) never leaves its IO-control cycle in starved runs

~3 of 4 PROCNAV boots end with tid77 parked in a repeating `0x5036e`/`0x2016e`/`0x1026e`
(object `0x6e`) cycle instead of progressing to its `0x2d0564` mailbox wait; the AE pump
then never consumes `mbx_19` and every tid77-keyed probe goes silent (0 hits). The same
runs show tid74/75/76/78-82 parked normally, so it is one task's bring-up racing the
registry/trace IO answers, not a global crash. Suspect: the `0x5036e` (IO control) answer
semantics differ from the device when the target device is absent, sending the pump into a
retry loop. Reproduce: `run_r326.log` (all-alive but starved) vs `run_r345.log` (good).
Fixing this collapses the good-run lottery and stabilizes regression testing.

## Boot-rate lottery: DAPI cryptnav open loop stretches ~12x under host GPU load

* r711/r711b/r711c (and earlier r704) reached 340-470 s of wall time without
  ever emitting `MOUpd copy-active`: the same ~71-78 `DAPI filemgmt open`
  cryptnav-probe iterations that complete in ~150 s in lucky runs (r705-710)
  spread over ~440 s, ~27-35 s between successive open pairs, so the map
  object update (and every downstream street probe) misses the run window.
* Correlation: `nvidia-smi` showed 57 % GPU utilization by a co-tenant
  (`gb10_inference`) during the failing runs; GB10's iGPU is shared with the
  emulator's host GL, and the DAPI worker iterations block on guest GL calls
  that queue behind the co-tenant.  Nothing in the r709-r711 diffs touches
  the boot path (`eglgp`/`stubmd`/synth-shader lines are all simply never
  reached).
* Mitigations used: longer run windows (470 s), run retries.  Candidate real
  fixes: make `gpu_void_clear` round-trips cheap (avoid a host round-trip per
  trivial GL call from DAPI's worker threads - batch or answer lock-step for
  calls whose result needs no backend state), or detect the DAPI-side
  stall and re-prioritize; tracked here so a failing run gets retried
  before being read as a regression.
