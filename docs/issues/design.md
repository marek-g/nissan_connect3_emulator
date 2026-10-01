# Design & Maintainability Issues

Structural problems: inconsistent abstractions, duplication, hardcoded data, build/config issues.

## Inconsistent error model across the codebase

- **Locations:** `emulator/` module-wide, `os/syscalls/mod.rs:22-35`, all syscall modules
- **Problem:** Mixed error types — `uc_error`, `&'static str` (`elf_loader.rs::load_elf`), `Box<dyn Error + Send + Sync>`, raw `-1i32 as u32` in syscalls. The `SysCallError` trait is used by only 4 of ~20 call sites, and futex uses its own magic `-11`. Guest-failure vs emulator-failure is indistinguishable at the boundaries.
- **Fix:** a single error type (e.g. a `thiserror` enum) for emulator errors plus one errno-mapping layer for syscalls; would remove dozens of magic numbers.

## ~900 hardcoded addresses in the OSAL hook table

- **Locations:** `nissan_connect3_emulator/src/os/libosal_linux/mod.rs:137-1126`, `nissan_connect3_emulator/src/os/libtrace/mod.rs:34-170`
- **Problem:** The table mixes functions with data symbols (`CRC32TAB`, `szErrorString_*`, `__bss_start`), inserts a code hook at `0x00000000` (line 154), and has duplicate keys that silently overwrite each other (`0x4856adf4` twice, `0x48572d3c` four times; libtrace `0x00009874` at lines 103 and 164). The same address `base+0x34A5C` is hooked twice with different handlers (`init.rs:11`, `trace.rs:12`). Line 32 `.unwrap()`s each `add_code_hook`.
- **Fix:** parse the ELF symbol table at load time (the comments already reference `rabin2 -E`), filter to FUNC symbols, and deduplicate.

## Duplicated dev/proc filesystem shims

- **Locations:** `nissan_connect3_emulator/src/file_system/dev_file_system.rs`, `nissan_connect3_emulator/src/file_system/proc_file_system.rs`
- **Problem:** Near-identical delegation shims over `TmpFileSystem`; the trace-log + lock + unwrap boilerplate is copy-pasted into every syscall function across `os/syscalls/`.
- **Fix:** extract a common delegating base/helper; centralize the log+lock+unwrap wrapper.

## Duplicated hook registration in Thread

- **Locations:** `nissan_connect3_emulator/src/emulator/thread.rs:47-59` vs `147-159`
- **Problem:** The 5 hook registrations are duplicated verbatim between `start_elf_file` and `clone`, each with `.unwrap()`.
- **Fix:** factor into a helper that returns `Result`.

## Misleading Result / by-value parameters in the public API

- **Locations:** `nissan_connect3_emulator/src/emulator/emulator.rs:12, 20`
- **Problem:** `Emulator::new` returns `Result<_, uc_error>` but never produces an error (no Unicorn created there); `run_process` takes `elf_filepath: String` by value.
- **Fix:** drop the misleading `Result`; take `&str`.

## Interpreter and non-PIE executables can overlap at address 0

- **Locations:** `nissan_connect3_emulator/src/emulator/elf_loader.rs:165`, `nissan_connect3_emulator/src/emulator/memory_map.rs:1`
- **Problem:** Interpreter loads at `interp_address = 0` while a non-PIE `ET_EXEC` also uses `EXE_LOAD_ADDRESS = 0`, so they can overlap (works only for PIE).
- **Fix:** give the interpreter a distinct base.

## mem_align_up unchecked arithmetic near u32::MAX

- **Location:** `nissan_connect3_emulator/src/emulator/utils.rs:39-42`
- **Problem:** `address + align - 1` overflows/wraps near `u32::MAX`.
- **Fix:** checked/wrapping-safe alignment math.

## Root workspace manifest typo

- **Location:** `Cargo.toml:2`
- **Problem:** `rosolver="2"` is an unused manifest key (typo of `resolver = "2"`), so the workspace silently defaults to resolver v1 despite edition 2021 members.
- **Fix:** rename to `resolver = "2"`.

## Clippy errors in unicorn-engine bindings

- **Location:** `unicorn_engine/bindings/rust/` (clippy: `this public function might dereference a raw pointer but is not marked unsafe`)
- **Problem:** 2 hard clippy errors block `cargo clippy -p nissan_connect3_emulator` for the workspace.
- **Fix:** mark the relevant FFI wrapper functions `unsafe fn`.

## main.rs: hardcoded absolute paths and commented-out process selection

- **Location:** `nissan_connect3_emulator/src/main.rs:17-102`
- **Problem:** Firmware paths (`/home/marek/Ext/reverse_engineering/...`) and the target process are hardcoded; which process runs is selected by commenting lines out.
- **Fix:** take the firmware root + target binary as CLI arguments (e.g. `clap` or plain `env::args`).
