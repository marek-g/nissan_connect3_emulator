# Performance Issues

Hot-path problems: per-syscall allocations, O(n) lookups, redundant FFI calls, redundant memory copies.

## read_string reads guest memory one byte per FFI call

- **Location:** `nissan_connect3_emulator/src/emulator/utils.rs:76-88`
- **Problem:** Each loop iteration is a separate `unicorn.mem_read`, i.e. O(n) expensive FFI calls for every path/string read in syscalls (every open/stat/readlink pays this).
- **Fix:** read in chunks (e.g. 256 B) and scan for the NUL terminator.

## O(#mounts) fd→mount scan on every fd syscall; quadratic fd allocation

- **Location:** `nissan_connect3_emulator/src/file_system/mount_file_system.rs:60-70, 226-230, 344-350`
- **Problem:** Every `read`/`write`/`seek`/`close` calls `get_mount_point(_mut)(fd)` which walks all mount points invoking `is_open(fd)`. `get_unique_fd()` scans fds from 0 doing the same, so fd allocation is quadratic in the number of open files. The `file_data: HashMap<i32, MountFsFileData>` table already exists and is the natural place to record the owning mount index.
- **Fix:** store the mount index in `MountFsFileData` on open → O(1) lookup; keep a free-fd list.

## stat implemented as open + fstat + close per call

- **Location:** `nissan_connect3_emulator/src/os/syscalls/stat.rs:24-34, 69-79, 106-118`
- **Problem:** Every path-based stat allocates an fd and round-trips through the full open/close machinery.
- **Fix:** add a `stat_path`/lstat-style method to the `FileSystem` trait (no open).

## getdents64: per-entry mutex lock + open/close round-trip

- **Location:** `nissan_connect3_emulator/src/os/syscalls/unistd.rs:354-421`
- **Problem:** For each directory entry it locks the fs and calls `get_file_info_from_filepath`, which itself opens+closes the file and re-resolves the path — O(n) locks + O(n) opens per listing. Remaining entries are then cloned into `sys_calls_state` (line 410).
- **Fix:** fetch all entry metadata in one locked pass (or a `read_dir_with_details` trait method).

## TmpFileSystem read_dir: O(n) scan with prefix match

- **Location:** `nissan_connect3_emulator/src/file_system/tmp_file_system.rs:83-91`
- **Problem:** O(n) scan of all files per call, and the `key.starts_with(&dir_path)` prefix test has no component boundary check (a `/ab/...` entry would be listed under directory `/a`).
- **Fix:** keep a per-directory index or sort keys; use a path-component boundary check.

## MMU: every mapping stores a full zero-filled Vec<u8> forever

- **Location:** `nissan_connect3_emulator/src/emulator/mmu.rs:86, 295`
- **Problem:** `MmuRegion.data` duplicates ~100% of guest memory in the host heap on top of Unicorn's own copy, held for the process lifetime (it exists only to back `mem_map_ptr`).
- **Fix:** back regions with a shared arena, or use Unicorn `mem_map` + copy-on-clone instead of persistent per-region buffers.

## MMU split clones the entire region on every split

- **Location:** `nissan_connect3_emulator/src/emulator/mmu.rs:341`
- **Problem:** `.map(|item| item.clone())` copies the full `data: Vec<u8>`, then lines 359 and 370-373 allocate two more sub-slices → ~3× peak memory + O(n) copy per `mmap`/`mprotect`/`munmap`.
- **Fix:** slice from the original buffer before removing it; avoid the full clone.

## Library paths cloned per thread on every hook update

- **Location:** `nissan_connect3_emulator/src/emulator/mmu.rs:160-168` (called at 185, per thread at 177)
- **Problem:** `get_libraries_and_base_addresses` does `.map(|r| (r.filepath.clone(), ...))`, invoked once per thread from `update_library_hooks_for_all_threads` → O(threads × libraries) string clones in the hook path.
- **Fix:** return references/`&str` or cache; avoid cloning per thread.

## Fresh Capstone handle on every disasm call

- **Location:** `nissan_connect3_emulator/src/emulator/print.rs:46-59`
- **Problem:** Capstone initialization is costly and `dump_context` calls it 3× per fault.
- **Fix:** cache the disassembler (thread-local / `OnceLock`) and reuse it.

## OSAL trace hook builds Capstone per hit + unwraps register reads

- **Location:** `nissan_connect3_emulator/src/os/libosal_linux/mod.rs:105-133`
- **Problem:** Each OSAL entry hook, when `instruction_tracing` is on, builds a fresh `Capstone` instance, disassembles 4 bytes, and unwraps 11 register reads; line 132 `&disasm[0..disasm.len() - 1]` panics if the disassembly string is empty.
- **Fix:** build the engine once (`OnceLock<Capstone>`), drop `.unwrap()`s on the log path, guard the slice.

## pack_u* allocate a Vec<u8> per call

- **Location:** `nissan_connect3_emulator/src/emulator/utils.rs:90-112`
- **Problem:** `pack_u16/u32/i32/i64/u64` return a freshly allocated `Vec<u8>` per call, used repeatedly in loops (`setup_stack`, auxv).
- **Fix:** return fixed `[u8; N]` arrays.

## mmap takes 5 separate mutex locks for one operation

- **Location:** `nissan_connect3_emulator/src/os/syscalls/mman.rs:169-180`
- **Problem:** Five separate lock+unwrap sequences for a single mmap syscall.
- **Fix:** single lock scope covering the whole operation.
