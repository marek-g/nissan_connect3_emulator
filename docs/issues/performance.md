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

## RESOLVED: glVertexAttribPointer client-array churn leaked driver memory

- **Location:** `nissan_connect3_emulator/src/gpu/mod.rs` (`route_vertex_attrib_pointer`)
- **Problem:** Every guest `glVertexAttribPointer` with a client pointer read a fresh
  64 KB `Vec` and minted a throwaway host VBO (`GenBuffers` + `BufferData`, never
  deleted) ~15x per HMI swap. This leaked driver-side buffers and churned the host
  allocator, growing process RSS ~200 KB/swap (~190 MB/10 min).
- **Fix:** one reusable scratch read buffer + one dedicated host VBO per distinct
  guest pointer (content-hashed; unchanged geometry skips `BufferData` entirely).
  RSS now flat over multi-minute runs (`GPU: resource telemetry` watchdog).

## RESOLVED: forensic range hooks forced per-instruction dispatch in bare runs

- **Location:** `nissan_connect3_emulator/src/libs/prochmi.rs` (`GUI_MAINLOOP_TRACE_*`),
  `nissan_connect3_emulator/src/libs/procmapengine.rs` (AIL administration tracer)
- **Problem:** Two diagnostic fetch-sequence tracers registered unconditionally as
  bounded `add_code_hook` ranges - 64 KiB over `bGUIMainloop` and 64 KiB over the
  AIL administration module. A bounded range makes Unicorn emit a per-instruction
  hook check in every TB intersecting it (AGENTS/r743: the emulator's main hook
  cost), throttling the GUI thread itself. Measured: after removing the
  bGUIMainloop tracer, the loop logged 5x more iterations per wall second
  (fix19: 121 -> fix20: 643 `bGUIMainloop` lines at equal run length).
- **Fix (r751):** both tracers register only under their trace env flags
  (`EMU_PROCMAP_TRACE` / `EMU_PROCHMI_NAV_TRACE` / lua trace). All other
  remaining single-address Unicorn code hooks were migrated to `trap_site`
  (`bGUIMainloop`, the 14-entry GUI composition observer list, `OSAL_IOOpen`
  entry, dapi eval return tap, `OSAL_ProcessSpawn`, `CreateNewRawDataBlock`);
  the two dapi sites whose first instruction is PC-relative (`ADD r12,pc,#0x700`,
  `BAL`) structurally reject the svc patch and stay narrow fallback hooks by
  design.  Legacy `add_code_hook` registrations on the normal path: **0**
  (only `add_mem_hook` watchpoints, the svc intr hook, and rejected-site
  fallbacks remain).

## RESOLVED: read_string one-byte FFI + O(#mounts) fd->mount scan
- `read_string` (src/emulator/utils.rs) now reads 256-byte chunks with a
  byte-wise fallback when the chunk straddles an unmapped page.
- `MountFileSystem::get_mount_point(_mut)` resolves the fd through the
  existing `file_data` map's new `mount_index` field (recorded at open/dup2)
  instead of probing every mount's `is_open`; the linear scan remains only as
  fallback for fds with no record (driver fds). `get_unique_fd` rides the
  same O(1) lookup now.
- Added `pretty_env_logger::init_timed()`: logs carry ms wall timestamps -
  required to profile the minutes-long map pipeline (the plain init printed
  none). NB `formatted_builder()` alone never reads RUST_LOG (silent runs).
