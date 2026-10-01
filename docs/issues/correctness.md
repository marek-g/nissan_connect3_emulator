# Correctness Bugs

Bugs where the emulated guest behaves wrongly (wrong syscall semantics, data corruption, hangs).

## FUTEX_WAIT always blocks (missing return)

- **Location:** `nissan_connect3_emulator/src/os/syscalls/futex.rs:62-64`
- **Problem:** `if val_read != val { -11i32 as u32; }` computes the EAGAIN value but does not `return`, so FUTEX_WAIT blocks regardless of the word's current value. Any futex-based lock (glibc pthread mutexes, etc.) misbehaves.
- **Fix:** `if val_read != val { return -11i32 as u32; }`.

## TmpFileSystem: unchecked length arithmetic → underflow/OOM

- **Location:** `nissan_connect3_emulator/src/file_system/tmp_file_system.rs:245-268`
- **Problem:** `pos` is per-fd while `data` is shared via `Arc<Mutex>`. If another fd truncates the file after a seek, `data.len() - pos` underflows in debug or wraps in release (`bytes_to_read` becomes huge → OOM/slice panic). Real race across emulated threads since the fs mutex is only held per-syscall.
- **Related:** lines 245/249 take the lock twice for one operation.
- **Fix:** saturating math / clamp `pos` to `len`, return 0 at EOF; use a single lock guard per operation.

## MountFileSystem: infinite loop on 0-byte IO

- **Location:** `nissan_connect3_emulator/src/file_system/mount_file_system.rs:264-281, 296-318`
- **Problem:** `while bytes_to_read > 0 { ... bytes_to_read -= bytes as usize }` never handles a 0-byte result. If the host file shrinks between `get_length` and the read (TOCTOU), or a write returns 0, the loop spins forever.
- **Fix:** `if bytes == 0 { return Err(()) }` (or break on read EOF).

## mmap: unsigned underflow when offset > file length

- **Location:** `nissan_connect3_emulator/src/os/syscalls/mman.rs:174`
- **Problem:** `length.min(file_system.get_length(fd) as u32 - off_t)` wraps in release mode when the offset is past EOF → `buf.resize(huge)` → OOM abort on a plain `mmap(fd, len, offset)`.
- **Related:** lines 169-180 take 5 separate mutex locks + unwraps for one mmap.
- **Fix:** checked subtraction returning `-EINVAL`/`-ENOMEM`; single lock scope.

## Mount resolution: string-prefix matching without component boundary

- **Location:** `nissan_connect3_emulator/src/file_system/mount_file_system.rs:49, 80, 18-23`
- **Problem:**
  - Matching is raw `str::starts_with`, so mount `/var/lib` also claims `/var/libfoo/...`.
  - Line 49 sorts lexicographically, not by length; it only happens to work because a true string prefix sorts before its extension.
  - The "must be sorted longest to shortest" requirement leaks into `main.rs:16` (stale comment — `new()` re-sorts anyway).
- **Fix:** match on `Path` components (`path.strip_prefix(mount)` with component-boundary check); ordering becomes irrelevant and the sort + caller requirement disappear.

## FileInfo stores mount-relative path → broken openat/getdents, inode collisions

- **Location:** `nissan_connect3_emulator/src/file_system/mount_file_system.rs:133-138, 184-197, 352-356`
- **Problem:** Because the translated (relative) path is what gets stored:
  - `fcntl.rs:95` (`openat` with dirfd) and `unistd.rs:362` (getdents) join entries onto it and re-resolve, producing wrong global paths for any non-root mount (e.g. `/var/volatile/...` becomes `/sub/entry` on the root fs).
  - Inodes are keyed by relative path (`get_inode_for_filepath`, line 191), so identical relative paths on different mounts collide in `st_ino`.
  - The `inodes` map grows without bound (every fstat inserts an entry, never removed).
- **Fix:** store the global path in `MountFsFileData`; key inodes by `(mount_point, path)` or use host inos.

## mmu_clone_map aliases parent memory (no COW)

- **Location:** `nissan_connect3_emulator/src/emulator/mmu.rs:391-409`
- **Problem:** The child Unicorn maps the parent's live `data` buffers directly, so cloned threads read/write the same backing memory and corrupt each other — deviates from `clone()` copy-on-write semantics.
- **Fix:** give the child its own buffers with copy-on-write (or at least an initial copy).

## Thread pause/resume are no-ops but MMU remaps depend on them

- **Location:** `nissan_connect3_emulator/src/emulator/thread.rs:241-253`, `nissan_connect3_emulator/src/emulator/mmu.rs:79-154`
- **Problem:** `Thread::pause`/`resume` are commented-out no-ops, yet `mmu.map/unmap/mem_protect` call `pause_all_threads` before `mem_map_ptr`/`mem_unmap`. Other concurrently-running guest threads therefore race with live memory remapping (Unicorn UB while a VM runs).
- **Fix:** implement real pause (`is_paused` + `emu_stop`) or explicitly constrain the emulator to single-threaded guests.

## Guest panic crashes the host

- **Location:** `nissan_connect3_emulator/src/emulator/process.rs:57`
- **Problem:** `main_thread_handle.join().unwrap()?` — any panic in the emulated thread (unimplemented syscall, malformed ELF `.expect`, bad access) makes `join()` return `Err`, and `.unwrap()` re-panics the host instead of returning a clean error.
- **Fix:** map `JoinError` to a normal `Err` so a crashing guest program is reported, not fatal to the process.

## Child stack pointer computed with unchecked arithmetic

- **Location:** `nissan_connect3_emulator/src/emulator/thread.rs:198`
- **Problem:** `reg_read(SP) as u32 - STACK_BASE + stack_ptr` underflows/wraps (release mode) if `SP < STACK_BASE`, yielding a garbage `child_stack` → memory corruption on `clone()`.
- **Fix:** use checked/saturating subtraction and validate the result.

## stat implemented as open + fstat + close(fd).unwrap()

- **Location:** `nissan_connect3_emulator/src/os/syscalls/stat.rs:24-34, 69-79, 106-118`
- **Problem:** Every path-based stat allocates an fd (side effects on some fs), is TOCTOU-prone, panics if close fails, and pollutes the inode table.
- **Fix:** add a `stat_path`/lstat-style method to the `FileSystem` trait (no open) and drop the unwrap.

## OsFileSystem: no host-path containment check, panics on relative path

- **Location:** `nissan_connect3_emulator/src/file_system/os_file_system.rs:49, 106, 129, 194-203`
- **Problem:**
  - `path_transform_to_real` panics if a non-absolute path ever reaches it.
  - `host_path.join(...)` has no defensive check that the result stays under `host_path` (only "works" because upstream `absolutize()` strips `..` — fragile cross-module invariant).
  - `metadata().unwrap()` (lines 106/129) panics if the host file vanishes between open and stat.
  - Line 49 `to_str().unwrap()` panics on non-UTF8 host filenames.
- **Fix:** normalize then assert `starts_with(host_path)` and return `Err` instead of panicking; make metadata failures return `None`.

## devfs/procfs: exists("/cmdline") true but open fails

- **Location:** `nissan_connect3_emulator/src/file_system/dev_file_system.rs:45-48, 56-59`, `nissan_connect3_emulator/src/file_system/proc_file_system.rs`
- **Problem:** Dead `/cmdline` special-cases make `exists("/cmdline")` return true while `open("/cmdline")` fails — a real inconsistency for guests that stat-before-open.
- **Fix:** either implement the file or remove the special cases so exists/open agree.
