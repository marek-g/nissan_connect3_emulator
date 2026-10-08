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

## Parallel-process model: one host thread + one Unicorn VM per process

The threading architecture is documented in [`docs/threading.md`](../threading.md).
Its load-bearing invariants (a saved `Unicorn::Context` is per-VM and never
crosses a thread; guest memory is private, so cross-process IPC completion runs
in the waiter's own VM) are enforced by the code in `emulator/` - the main trap
to avoid is reintroducing any direct read/write of another process' guest memory.

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

## RTOS boot backend bypasses guest `OSAL_ProcessSpawn`

- **Locations:** `nissan_connect3_emulator/src/rtos/boot.rs`, `nissan_connect3_emulator/src/emulator/emulator.rs:41-105`, `nissan_connect3_emulator/src/main.rs:104-121`
- **Problem:** The new RTOS backend logs the Linux OSAL `0x1a` start-process payload but actually launches `prochmi_out.out` through `ProcessFactory` as another top-level guest process. The target is not a child of `procbaselx`, `fork`/`execve`/waitpid semantics are bypassed, and the guest-side `vSysCallbackHandler`/`OSAL_ProcessSpawn` code is never exercised.
- **Fix:** Either implement guest `OSAL_ProcessSpawn`/`fork`/`execve` interception so spawned processes become children of `procbaselx`, or inject a callback-header message that causes `vSysCallbackHandler` to run inside `procbaselx`.

## main.rs: hardcoded absolute paths and commented-out process selection

- **Location:** `nissan_connect3_emulator/src/main.rs:17-102`
- **Problem:** Firmware paths (`/home/marek/Ext/reverse_engineering/...`) and the target process are hardcoded; which process runs is selected by commenting lines out.
- **Fix:** take the firmware root + target binary as CLI arguments (e.g. `clap` or plain `env::args`).

## Missing PWR-proxy service (BSP layer not shipped in guest firmware)

- **Locations:** new service to add under `nissan_connect3_emulator/src/rtos/`;
  consumes shared `mbx_*` queues from `crate::common::osal_queues::OsalQueueService`;
  previous workaround lived in `nissan_connect3_emulator/src/libs/libosal_linux/message.rs`
  (disabled in commit `16dde50`) and hooked `ail::bPostIpcMessage` stub in
  `dapi.rs:202` + `procmapengine.rs:364` (removed in commit `ef271f5`).
- **Problem:** Every guest binary contains only the libail *client* side of the
  Bosch power handshake (`PWR_PROXY_START_CONF received, PWR_APP_INITIALIZED
  sent`, `STATE_CHANGE_REQ from %s to %s`, `CVM_SIGNAL_CHANGED to %s`). None
  contains a `PWR_PROXY_START_REQ` sender nor any code that emits
  `PWR_PROXY_START_CONF` — verified by grepping every `/opt/bosch/processes/*`
  and `/usr/lib/*.so`. The power proxy is below Linux on real hardware (BSP /
  PMU daemon). Without it `procmapengine`'s AE_400 thread blocks indefinitely
  in `ail_bIpcMessageWait(mbx_1024, ...)` and never calls `vStartApp`, so
  `s32InitAppMapEngine` never runs and DAPI is never asked for map blocks.
- **Fix (implemented):** `src/rtos/pwr_proxy.rs` holds proxy policy; delivery
  happens inside the recipient's own `OSAL_s32MessageQueueWait` hook (see
  `pwr_proxy_service_handle` in `libosal_linux::message`). Order of events on
  real hardware, and now in the emulator:
  1. App opens `mbx_<app_id>` and `mbx_0`; the hook observes the queue-open
     and queues `PWR_PROXY_START_CONF` in the proxy's pending list.
  2. The app's first Wait on `mbx_<app_id>` is answered with that pending
     message: the hook heap-allocates 0x20 bytes in the recipient's own
     guest VM, mem_writes the encoded `PowerMessage`, then emits the 8-byte
     OSAL message ref `[1, content_ptr]` into the caller's out-buffer and
     returns 8. Content lives in the recipient's VM because each process has
     its own Unicorn VM + message pool; only the recipient can produce a
     pointer its own code can dereference.
  3. Post to `mbx_0` is observed (never intercepted): we read the sender's
     own OSAL message pool, extract `PowerType` and `Sender`, and hand the
     tuple to the proxy. On `PWR_APP_INITIALIZED` the proxy immediately
     enqueues `STATE_CHANGE_REQ` and `CVM_SIGNAL_CHANGED` for that specific
     app (per-app promotion, not gated on all apps acking).
  4. The next Wait on that `mbx_<app_id>` hands the pending message back
     via the same content-injection path.
- **Payload format** (0x20 bytes, matches Ghidra's `amt_tclPowerMessage`
  ctor at procmap 0x003882f8):
  `[0x00:u16 sender, 0x02:u16 target, 0x04:u32 len(0x20), 0x08:u16 kind(2),
  0x0a:u16 length_low, 0x0b:u8 flags(0x40 for PowerMessage), 0x0c:u32
  reserved, 0x10:u32 reserved, 0x14:u16 power_type, 0x16:u16 pad, 0x18:u32
  power_data1, 0x1c:u32 power_data2]`. Verified against live posts made by
  procmap/DAPI/prochmi.
- **Non-goals:** no periodic broadcasts, no per-queue special-casing in the
  libosal hooks. Real libail handles everything once the queue has the right
  messages on it.

## SVG layer composition between procmapengine and prochmi is not wired

- **Locations:** `nissan_connect3_emulator/src/libs/prochmi.rs:1549`
  (`install_svg_map_surface_hooks`), `nissan_connect3_emulator/src/gpu/mod.rs:70`
  (`MAP_SURFACE_BYTES`), `nissan_connect3_emulator/src/os/dev/svg_resource.rs`.
- **Current state after commit e7b9e0f (revert of g_bCreateDefaultView
  clearing):** procmapengine's natural boot path now executes
  `MDBC_CreateDefaultView` → `JobRCCreateView::Execute` →
  `rl_tclWindow_ContextHandler_Platform::bCreateWindow`. Diagnostic trace in
  commit ea6299e confirms the call chain runs end-to-end:
  - `bCreateWindow` is entered with `view_type=1, w=800, h=480`.
  - `svgCreateResourceSurface` × 3 returns nonzero handles (0x1, 0x10001, 0x1)
    — `libsvg-resource.so` is loaded at procmap+0x9003e000 and works.
  - `svgCreateLayerContextTriple` returns handle `0x11000000` and
    `svgSetLayerName(layer, "MAP_View1", 9)` runs — `libsvg-layer.so` is
    loaded even though we do not hook it in `os/mod.rs::add_library_hook`.
  - `svgApplyLayerInSync(0)` runs; `eglCreateWindowSurface` branch is reached
    with a valid layer handle. The GPU backend returns a Map FBO for that
    handle and `handle_surface_swap(Map)` reads 800×480 RGBA into
    `MAP_SURFACE_BYTES` on every swap (131 swaps per boot).
- **Problem:** prochmi never calls `svgGetLayerByName("MAP_View1")` (or
  `svgGetLayerStatus` / `svgGetSurfaceStatus`), so the emulator's
  `write_map_surface_to_guest` is never invoked; procmap's rendered pixels
  never reach prochmi's GL texture. The `install_svg_map_surface_hooks`
  table in prochmi.rs is a no-op on the natural path — it only fires if the
  guest happens to hit those PLT stubs, and it does not. On the HMI we
  currently observe a small grey rectangle in the middle of the screen that
  is NOT the map surface (it is prochmi's own HMI background widget at
  (76,184)–(722,300)); the actual map surface never appears.
- **Why prochmi does not look it up:** prochmi's composition path calls
  `GUI_GL_OpenGL::mixLayers` on its own display-manager layer list; it does
  not query libsvg-layer for external layers by name. On the real device,
  libsvg-layer's registry lives in shared VRAM backed by `/dev/svg_resource`
  mmap, so a separate HMI compositor (or a display-manager widget bound to
  the "MAP_View1" name) can find and blit procmap's surface. We do not model
  that shared registry across processes today: `os/dev/svg_resource.rs`
  keeps a per-unicorn `STATUS` mutex; libsvg-layer's own layer-name table
  lives inside each guest's own memory.
- **Fix (open, no cheating):** two possible non-cheating directions —
  1. Model the shared VRAM region across processes. Have `svg_resource.rs`
     keep one shared buffer keyed by (fd, offset) that both procmap's and
     prochmi's libsvg-layer instances mmap, so `svgGetLayerByName("MAP_View1")`
     called from prochmi finds the layer that procmap registered. Then
     remove the fake-handle bypasses at prochmi.rs:344–365 and route
     `svgGetSurfaceStatus` to a real shm-backed surface (already partially
     in place via `MAP_SURFACE_BYTES`).
  2. If the SVG registry cannot be made shared cleanly, keep a single
     in-emulator SVG layer registry as a *replacement implementation* of
     libsvg-layer.so's public API (the AGENTS.md-allowed path:
     "alternative implementation for functions in shared libraries").
     Route `svgCreateLayerContextTriple` / `svgSetLayerName` in procmap
     there, and let `svgGetLayerByName` / `svgGetLayerStatus` /
     `svgGetSurfaceStatus` in prochmi return real handles backed by
     `MAP_SURFACE_BYTES`. Both processes then agree on the same registry
     without touching procmap/prochmi guest code.
  Either way, once a name→surface lookup exists, prochmi's HMI config
  (currently a static widget layout) needs a widget bound to the map layer
  or `GUI_GL_OpenGL::mixLayers` needs to iterate the SVG layer list; that
  side is inside the guest process and does not require hooks.
- **Diagnostics:** `EMU_PROCMAPENGINE_TRACE_INIT=1` now also prints
  `PROCMAPENGINE bCreateWindow trace ...` lines with per-call return values
  for `svgCreateResourceSurface` / `svgCreateLayerContextTriple` and the
  `eglCreateWindowSurface` branch, so regressions in the SVG setup path are
  visible without loading Ghidra.
- **Additional finding (commit bf2cfb8):** prochmi's own SVG-consuming code
  path is dead in the current UI state. `GUI_GL_LayerSync::getLayers`
  (prochmi+0x1342e38, the only non-debug caller of `svgGetLayerByName`) is
  never reached; `GUI_GL_LayerSync::copyLayer` is never reached; none of the
  fake-handle bypasses in `prochmi.rs::install_svg_map_surface_hooks` fire
  except `svgApplyLayerInSync` (2 hits from prochmi's own HMI-layer commit).
  Therefore *either* alternative (shared VRAM registry or in-emulator
  libsvg-layer implementation) will not close the gap by itself: prochmi's
  HMI widget layout must also learn to reference a layer named
  `"MAP_View1"`, which is inside the guest and currently not the case.
  **Untried paths**:
  - Check whether an HSI/PowerManager state transition (or a specific HMI
    screen / navigation-mode entry) causes prochmi to switch into the mode
    that owns `GUI_GL_LayerSync` and would populate its layer list from SVG.
    `clHmiNavServerHandler` was seen starting in `mode=1819239265` but the
    mode never progresses.
  - Implement `svgMergeAllLayers` / `svgMergeAllLayersFB` (real hardware
    composes SVG layers to `/dev/fb0` from inside libsvg-layer.so's
    background `SVG_Layer_Thread`) in an emulator-owned libsvg-layer hook,
    and route the merged output to the SDL window. This is composition at
    the framebuffer level (outside both processes) - allowed by AGENTS.md.
    It sidesteps the need for prochmi to know about the map layer at all.
