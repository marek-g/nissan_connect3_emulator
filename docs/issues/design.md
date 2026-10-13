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

## GPU display model composites the Map and HMI layers (fixed)

- **Locations:** `nissan_connect3_emulator/src/gpu/mod.rs`.
- **Problem (fixed):** both guests ran against two *shared* GL contexts
  (`SDL_GL_SHARE_WITH_CURRENT_CONTEXT=1`), so `glGen*` handed IDs out of one
  common pool and prochmi's and procmap's objects were interchangeable. The
  HMI drew straight into the SDL window (framebuffer 0 of the shared
  namespace) while the Map target got an FBO bound only at context-switch
  time; any explicit `glBindFramebuffer(_*, 0)` from a guest escaped to the
  window and trampled the other layer. Nothing was ever composed.
- **Fix:** the two contexts are created unshared, so each guest has a private
  ID space (both now legitimately own e.g. FBO 1). Each target gets its own
  private off-screen "default framebuffer" (RGBA8 colour texture +
  DEPTH_COMPONENT16 renderbuffer); `glBindFramebuffer(_*, 0)` from a guest is
  rewritten to that target's private FBO, and
  `glGetIntegerv(GL_*_FRAMEBUFFER_BINDING)` reports 0 back so guests keep the
  illusion of a default surface. Guest names that collide with the private
  FBO's numeric ID are re-pointed at a fresh host object through a per-target
  alias table (`glBindFramebuffer`/`glDeleteFramebuffers` translate). On
  eglSwapBuffers the GPU backend keeps each target's pixels private: Map
  swaps ReadPixels the Map surface into `MAP_SURFACE_BYTES`; HMI swaps
  ReadPixels the HMI surface, CPU-composites it over the latest Map pixels,
  and presents the merged 800x480 image to the SDL window with a GLES-2
  fullscreen-quad shader (ES has no glDrawPixels). The merge uses only the
  HMI layer's real per-pixel alpha: transparent pixels reveal the map,
  opaque pixels (including intentionally black widgets) occlude it. Black
  is *not* treated as a colorkey - prochmi legitimately paints opaque black
  UI that must stay visible.
- **Remaining gap:** prochmi's HMI layer currently clears to fully opaque
  black and its `GUI_GL_OpenGL::mixLayers` does not include the map, so at
  this UI state the opaque black covers procmap's map layer and only the
  widgets render. Making the map show through requires wiring prochmi's
  layer list to `MAP_View1` (see below), not inventing a colorkey. The
  libsvg-layer registry is still not shared between processes. Historical
  notes on that gap follow.

## SVG layer composition between procmapengine and prochmi is not wired (historical)

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

## RESOLVED for the popup-background case (commit 782bbce + snapshot refresh)

The gap closed differently than the two options above: prochmi does consume
an external map through `GUI_GL_OpenGL::mixLayers(LayerCopy*)`, but only
when its own `GUI_DM_EAManager` state machine runs a full EA show→hide
cycle. The missing input was procmap's CCA/FI replies, which the emulator
now injects onto the `GUI_UTIL_Queue` that `clGUIWidgetEngine::bCheckMsgBox`
polls (`post_lsync_map_announcement` in prochmi.rs):
SetLayerNames("MAP_View1") → ViewStatusChanged(VISIBLE) → SetView(0,0,800,480).
That drives the natural `getLayers` → `svgGetLayerByName` (fake handle) →
`setLayersVisible` flow. The snapshot itself is produced by the natural
`updateEAHide` state 5 → `GUI_GL_LayerSync::copyLayer` →
`GUI_GL_LayerCopy::copy`/`performCopy`, which builds a real 800×480 RGBA
`GUI_GL_Texture` from the map surface copy (`write_svg_surface_status`
layout: base@0, byte pitch@8 and @0x10 — performCopy divides +0x10 by 4 for
the width — format enum 1=RGBA@0x0c). `mixLayers` then receives a non-null
LayerCopy and draws the map under the GUI views; the window shows the map
behind the popup (verified via `mapI_*`/`mapK_*` captures and
`EMU_GPU_PROBE_ALPHAS=1`: base/widget layers are alpha-0 outside the art).
Notes:
- The emulated hide trigger pokes the EAWStatus slot command + timer-expiry
  state directly; guest-calling `requestEAHide`/`EAManager::update` from the
  bCheckMsgBox or DisplayManager::update hooks corrupts the GUI thread
  (stub-return re-entrancy into an address that is itself hooked →
  FETCH_PROT into vtable data). Do not reintroduce guest calls from those
  hooks.
- The snapshot is frozen after the hide completes; the mixLayers hook
  re-uploads the live `MAP_SURFACE_BYTES` into the snapshot texture every
  30 frames via `gpu::upload_rgba_texture`.
- Remaining: the map is only visible where prochmi's base-layer art is
  transparent (~5% of the FM-radio popup frame); the live scanout-level
  compositing of a moving map in other HMI screens is still open, as is the
  app-level stall where mixLayers stops after ~120-200 frames.
    It sidesteps the need for prochmi to know about the map layer at all.

## RESOLVED: host-side GL work must never run on a guest context

- **Symptom:** after the LayerSync snapshot paths were added, every post-hide
  `GUI_GL_OpenGL::mixLayers` pass rendered pure black (whole HMI + popup gone).
- **Root cause:** prochmi's mix pass draws quads *without* `glBindTexture` or
  `glUseProgram` - on real hardware each process has its own EGL context and
  its producer passes bound the texture/program earlier *in the same context*,
  so the state persists. Our per-swap host work (window blit program/texture/
  VBO, readbacks, snapshot refresh/probes) ran on the HMI context and clobbered
  exactly that state.
- **Fix:** dedicated unshared `comp_context` (sole renderer to the SDL window,
  owns the composite program/texture/VBO); all helpers touching a guest
  context save/restore framebuffer and TEXTURE_2D bindings. Rule going
  forward: *never* leave any GL state changed on `hmi_context`/`map_context`.
- **Result:** full HMI page renders (FM1, presets, Menu, compass) and the
  "Starting navigation." popup shows center-screen.
- **Remaining:** map visible only in the small left strip (map-window region);
  "Starting navigation." popup never dismissed (procmap waits on something -
  likely an IRMC/GPS event).

## OPEN: procmapengine's CCA GetBlockIDs (svc 0x26) reaches DAPI's map task but gets no answer

- **Goal:** `CcaGetBlockIDs` must return 0 so the "Starting navigation." popup
  is dismissed and the map renders behind the HMI.
- **What works now (verified in /tmp/opencode/run_r75.log):**
  - procmap's own `ServiceRegister` (class 0x42) is processed by DAPIAPP
    (`conf-result r0=1`, regid assigned, entry added with state ACTIVE when the
    map medium is already up).
  - The medium-ACTIVE flag comes from DAPIAPP's own status consumer
    (`0xb3a448/0xb3a4c0`) and it *does* propagate to client entries
    (`0xb3a50c`) - but only when a fresh status message arrives *after* the
    registration, so a registration that lands before the flip stays REGISTERED
    (state 1) and the request is refused with ServiceDataError 0xb.
  - A request that passes the state gate is dispatched
    (`0xb4357c`, vtable+0x20 = `dap_tclDapiApp::vOnNewMessage` 0x823998), turned
    into a `dap_tclJob` (svc 0x26 -> job type 4 via
    `enGetJobTypeFromCCAServiceId` 0xb53eac) and handed to DAPI's map task:
    `vSendToTask thread-idx=5 task=0xfad... job=...` with no 0x213 drop.
- **Where it stops:** nothing ever answers. procmap waits, times out, retries
  (2nd/3rd attempts then hit `id=6 unknown-register` because DAPIAPP's error
  funnel `FUN_00b43100` auto-unregisters the client on a failed request), and
  `CcaGetBlockIDs` returns 1.
- **Next step:** find DAPI's job-type-4 worker thread and what it blocks on
  (its queue/device reads). Note: never block inside a guest hook; the
  in-line spawn wait that used to "wait for the medium" froze the whole
  emulator - the medium only comes up while DAPIAPP keeps running.

### Update (register-id and message ordering)

- The worker does answer, with an error: `dap_map_tclWorker::enProcessJob`
  (0x84b5c8) reads the register-id from the request (message +0x16, job +0x12)
  and returns error 6 "unknown register" for the wildcard 0xffff
  (`dap_map_tclWorker::vReportError` 0x844e78, dap_map_worker.cpp line 374).
  procmapengine's emulated client reference therefore now remembers the handle
  taken from the conf, and the bridge re-posts a wildcard request that it held
  back behind the registration with the handle filled in.
- **Still stopping on:** DAPIAPP processes a request before the REGISTER of the
  same service was handled, and before the *stale* deregistration procmapengine
  replays for a handle from a previous session. `fwl_List<ail_tclServiceRegistry>::nRemove`
  (0xb369b0) matches through `ail_tclServiceRegistry::operator!=`, i.e. not by
  handle, so that late deregistration deletes the entry created moments earlier
  and every later request is answered with `id=6 unknown-register`. Simply
  dropping the stale deregistration is worse: procmapengine only registers the
  service in reaction to it, so no REGISTER is emitted at all.
- **Open question:** whether `operator!=` compares the register-id after all
  (then delivering the stale deregistration *after* the conf would be a no-op
  and correct), or whether the deregistration has to be answered locally in the
  emulated ail layer without ever reaching DAPIAPP.

### Update (registration ordering fixed, worker error remains)

- DAPIAPP resolves a deregistration's removal key from its own live entry, so
  the stale deregistration cannot be neutralised by ordering it after the conf.
  It has to run while no entry exists: the bridge now also holds the map-data
  *registration* until the conf of a pending deregistration has been delivered
  to procmapengine (procmapengine waits for that conf before it registers, so
  the deregistration may not simply be dropped).
- With that, the ail layer accepts the request (register-id matches, the
  `id=6 unknown-register` errors are gone) and all three attempts reach
  `dap_map_tclWorker::enProcessJob`. Each is answered with an error via
  `dap_map_tclWorker::vReportError` (0x844e78).
- **Next step:** the hook's register reading is unreliable there (code and
  "func" print the same value), so decode the real arguments of `vReportError`
  and find which check in the worker fails now that the request is valid.

### Update (the real root cause is out-of-order processing, not ordering at post time)

- Runs are not reproducible: in one the registration hold fires and the request
  is accepted and dispatched (worker error follows), in the next run the same
  binary never holds the registration and the request dies with `id=6` again.
  The difference is *when DAPIAPP happens to process* the stale deregistration:
  two DAPIAPP threads wait on `mbx_7`, take messages in FIFO order and finish
  them in whatever order they like, so a deregistration queued before a
  registration can still delete the entry that registration creates.
- Holding a message at post time cannot fix that: the message is already in the
  right order in the queue, it is the *processing* that is inverted. The
  `MBX_IN_SERVICE` heuristic (same thread coming back to `Wait` means it is
  done) does not hold, because DAPIAPP hands the message to another worker
  thread and returns to `Wait` while processing continues.
- `dap_map_tclWorker::vReportError` arguments are now decoded correctly:
  `(this, code, text, line, func)`; the hook prints both strings.
- **Open:** enforce one-at-a-time *processing* per mailbox queue (only release
  the next message once the previous one's handling has visibly completed), or
  find why DAPIAPP's deregistration handler keys the removal on its live entry
  instead of the handle in the message.

### Update: the ApplicationInfoStatus synthesis is load-bearing, not the culprit

`bRegisterAsync` defers the ServiceRegister until the process directory reports
the server application as running, and no process tells procmapengine that about
DAPIAPP, so we inject one `ApplicationInfoStatus(app 0x0007)`. Suppressing it
(EMU_NO_APP_INFO_SYNTH=1) removes not only the duplicate registration but the
whole CCA exchange: no ServiceRegister, no deregistration, no GetBlockIDs and no
error-reply flood at all - procmapengine never registers the map-data service.
The synthesis therefore cannot simply be deleted; the flood is downstream of the
failing answer, and one status message cannot explain a second registration.
Removing our syntheses is not the way out of this one.

### Update: what the registry actually contains when the request is answered

The list at `app+0x58` (0xf614c8) that the registry hooks talk about is the real
`ail_tclServiceRegistry` of DAPIAPP's application object - the data-request scan
walks exactly that pointer. Walking it while a GetBlockIDs request is answered
(run_r90 line 5426) shows eight entries, all of them DAPIAPP's own services:

    [ffff,0008,0007,ffff] [ffff,0056,0007,ffff] [ffff,0007,0007,ffff]
    [ffff,0012,0007,ffff] [ffff,0011,0007,ffff] [ffff,0026,0007,ffff]
    [ffff,003c,0007,ffff] [ffff,0006,0007,ffff]

The search tuple of the request is (regid=0x0001, svc=0x0026, client=0x0400,
sub=0xfffe), so nothing can match and `id=6 unknown-register` is the correct
answer: at that moment DAPIAPP does not hold a client registration for
procmapengine at all. Earlier in the same run an `ADD [0001,0026,0400,fffe]` on
the same list is followed later by a `REMOVE` of exactly that entry, with the
only observed `ServiceRegister` posted in between - who adds it, and what removes
it again, is still open and is now the thing to determine (the ADD is reached from
a region Ghidra has no function for, so name it and look at the callers).

`MAP_DATA_CLIENT_ENTRY_LIVE` from the registry hooks was reverted for a different
reason than assumed there: it is not a scratch list, but it also cannot help,
because the message that must be kept away from the entry is posted before the
entry exists.

### Update: the error is emitted from the ADMIN_OPERATION_LOCKED branch

`ail_bHandleMsgServiceRegister` (0xb439c4) is now fully read: it looks the service
up in its own list (`vt+0xd8`), assigns a register-id (`vt+0x24`), takes the list
lock, and then adds two entries to `this+0x58` - its own `(0xffff, service, 0x0007,
0xffff)` if missing, with state `service != 0xfffe`, and the client entry
`(assigned-id, service, client-app, client-sub)` whose state is *copied* from the
found own entry. Only then does it post the success conf. So the `vAdd` we watched
is the client entry being created, and the state copy is what makes a client entry
born "not available" when the medium is down.

The `REMOVE` of that entry comes from `ail_vPostServiceDataError6AndUnregister`
(fragment at 0xb40400, `nRemove` call at 0xb404e4), whose trace strings say
`ADMIN_OPERATION_LOCKED` and `(InterfaceState!=INITIALIZED) couldn't send
AMT_C_U16_ERROR_UNKNOWN_REG_ID`. The error we chase is therefore raised while the
dispatch object is not in state INITIALIZED, and the same path unregisters the
client on the way out. Next question: what leaves DAPIAPP's interface state short
of INITIALIZED here, since that would explain both the unmatchable register-id and
the teardown, without any message ordering being wrong at all.

### Update: neither holding nor dropping the deregistration at post time helps

The bridge now drops a deregistration of the map-data service whose handle
differs from the one DAPIAPP has registered (`MAP_DATA_LIVE_REGISTER_ID`, taken
from the registry list hooks), which is the destructive case identified in
`ail_vPostServiceDataError6AndUnregister`. In the run it never fires: the
deregistration is posted while DAPIAPP has no registration of the service yet, so
the bridge cannot know that the message will be processed only after one exists.
Both post-time strategies (hold while an entry is live, drop when the handle does
not match) fail for that one reason.

What is left is to keep the *registration* from being processed before the pending
deregistration was dealt with, which needs a reliable observation of "the
deregistration has been answered" rather than the conf-based heuristic that armed
wrongly before (a conf of an unrelated registration cleared it).

### Update: the registration obstacle was the ADMIN_OPERATION_LOCKED branch (patched)

Clearing the two request words that select that branch (`hook_clear_admin_lock_branch`,
see the comment there for why this is the documented exception to the fidelity rule)
makes the first map-data request survive its own registration: the registry entry
stays, no `ServiceDataError(6)` is produced, the job reaches the CCA dispatch
(`0xb43578` -> `0x823998`) and the map worker, and the `mbx_1024` flood drops from
223787 to 367 messages. The post-time bridge heuristics (`HELD_MESSAGES` for the data
request, dropping the mismatching deregistration) were not what fixed it.

Next obstacle, now visible for the first time: `dap_map_tclWorker::enProcessJob`
reports `0x306` at `Worker.cpp:374` (the shared tail that reports any result different
from -1). With function id `0x0103` the worker takes the branch of
`u16SendUniqueIdList` that first has to load the RNW/RS regulation-profile outlines
(`bIsRnwRegProfOutlinesValid_Locked` is false, `u16CheckDatasetIdLocked` fails and
`vHandleDatasetIdProtected` runs), so the error comes from that load - i.e. the map
database/dataset is still not usable at this point, which is the stage the emulator
has to reach next.

### Update: the map worker runs, dataset id fixed, next gate is the RNW outline load

`MAP_DATA_FAKE_DATASET_ID` was `1` while the inserted card declares
`DATASET_ID{ '1758962541' }` in `CRYPTNAV/DATA/DATASET.CFG`. `dap_map_tclWorker`
validates the id of every request, so the request was rejected with `0x306`. Using
the card's id makes the worker process the request to completion
(`enProcessJob` result `0xffff`, no `vReportError`).

The answer is nevertheless an error, now `0x305`. Reading `u16SendUniqueIdList`
(`Worker.cpp`): the branch that actually generates the unique-id list requires
`bIsRnwRegProfOutlinesValid_Locked() && bIsRsRegProfOutlinesValid_Locked() &&
u16CheckDatasetIdLocked() == 0xffff`; otherwise it first tries to load the
regulation-profile outlines (`u16ProcessLoadRegProfOutlines` /
`u16ProcessLoadRsRegProfOutlines`), and that load fails with `0x305`. So the map
database is not in the state the worker needs: the RNW/RS regulation-profile
outlines are not loaded. That is the next thing to produce (database loading on the
medium, not CCA traffic).

Two facts about the flood were measured while chasing this: every flood message is
posted *and* waited by procmapengine thread 57 (posts 218073 / waits 218071 in one
run), i.e. the client takes the unanswered answer out of its mailbox and puts it
back, and DAPI re-sends it because no conf comes back (`dap_tclJob::u16GetErrorCode`
returns the stored code for a job whose opcode byte is 8, and the CCA layer turns
that into the re-sent `amt_tclServiceDataError`). Both sides stop as soon as the
answer is a real result.

### Update: 0x305 comes from a road-network request DAPIAPP has no peer for

`dap_map_tclWorker::u16ProcessLoadRegProfOutlines` does not read the outlines
itself: it checks the dataset id (`0x306` if the medium rejects it), then creates a
sub-job (job type 6, action `0x84`) carrying `dap_rnwfi_tclMsgGetRegProfOutlineMethodStart`
and forwards it through the worker's comm container; the `0x305` we see is the
result of *that* request. DAPIAPP only contains the rnw *message types*
(`dap_rnwfi_tclMsg...`), no worker that answers them, so the peer is another
application.

That application (PROCNAV.OUT) is not present anywhere in the firmware image: the
dynamic partition's checksum list (`var/opt/bosch/dynamic/system/dynamic.md5`) covers
`processes/DAPIAPP.OUT` but no PROCNAV, and `/opt/bosch/processes` has no such entry.
The map card ships `CRYPTNAV/DNL/BIN/NAV/COMMON/PROCNAV.OUT`, but that file is not an
ELF - it starts with the magic `ULI `, i.e. the card stores compressed/installed
artefacts (its `VERSION.TXT` names the same build as ours, NAV_13.2C5P10), and the
unit installs/decompresses them before running. Starting the card file directly gets
a `spawn process pid=4` with no further activity, which is consistent with the loader
rejecting the container.

Related observation from the same run: the dynamic FFS is writable in our setup, yet
every `datapool/*.dat` open fails even with `O_CREAT` (`.../datapool/fff0/DpInternData.dat
flags=0xa4800 failed: NoSuchFileOrDirectory`), so process pools cannot be created.
Independent of the nav chain, that is worth fixing.

Next question, then, is how to satisfy the road-network side: either find the
component that unpacks the card's `ULI` payload (and the install path it writes to),
or emulate the road-network service on the DAPI level so the outline request is
answered without PROCNAV.

### Update: the road-network worker is inside DAPIAPP, and it has the data

`DAPDEVMMBX`/`DAPDATAMMBX`/`DAPDATASMBX` are not external peers: the threads using
them (`DAPDEVM...`, `DAPDATAM...`, `DYN6`, `DYN7`) are DAPIAPP's own worker threads,
and `u16SendResponse` is never used on that path. So the `GetRegProfOutline` sub-job
is served in-process and answered with `0x305` by DAPI itself - no missing process.

Tracing reads of the navdata tree (`NAVDATA_FDS` + a read log) shows the reads are
complete, so nothing fails on I/O. DAPIAPP reads `MEDIUM.CFG`, `DATASET.CFG`,
`POI_MAPPING.DAT`, `tp_meta.dat` and then the whole road-network root file
`data/connect/rnw/NAV_ROOT.DAT` (61136 bytes, magic `CPRNAV_2`) in three requests
(16 KiB at 0, 11108 at 0x4C, 42704 at 0x4800) and *still* fails. It never opens
anything under `CRYPTNAV/DATA/CONNECT/RNW/CCP/<CC>/*.PTH`, although `DATASET.CFG`
declares a `PTH` database (`'PTH' | '/RNW/' | '' | '1.2' | '1'`) next to `RNW`
(`16.12`) and `MAP` (`10.23`), plus 19 `REGION_CONFIG` entries with profile ids -
which is what "regulation profile outlines" refers to. The next thing to find is the
check that rejects the medium before those files are ever opened (format version,
region/profile lookup, or a signature/`CHECK_SIGNATURE` gate).

### Update: signature/CID is not what produces 0x305; PROCNAV is loaded but silent

Two things settled:

* The SDX/signature chain never runs here. Nothing in a run issues the cryptcard
  `0x410` CID ioctl, opens `SDX_META.DAT` or logs any `verify`/`BPCL`/`SDX` string,
  so `0x305` cannot be a signature verdict. The CID substitution path is still worth
  having (`ioctl.rs` answers `0x410` on `/dev/cryptcard[2]` with the card's real CID
  from `cid.txt`, `5d5342303031364712e055a86c013301`) - it is the honest way to make
  the partition signature pass when the chain does start, instead of the known
  "Map modification enabler" patch of `BPCL_EC_DSA_Verify`.
* PROCNAV is the road-network server (its image contains `dap_rnw_if_tclloader.cpp`,
  `fi_tcl_RegProfOutline`, `dap_rnwfi_tclMsgGetMapBlocksMethodStart/Result`) and it
  now loads: mapped at its prelinked base `0x8000-0x1476fff`, `Start program` with
  entry `0x5ea418`, ld.so resolves its `libiosclib_so.so`. It is statically linked
  (no `DT_NEEDED`), so our libosal hooks do not apply to it - only syscall-level
  emulation does.
  It nevertheless performs no IPC: no `mq_open`, no `/dev/registry`, no `/dev/iosc`
  open, nothing attributed to its image after the mapping. So the `0x305` path is
  still unhandled, and the request that has no reader is visible in the queue
  statistics: `mbx_0` gets 58 posts and 0 waits, and one of the payloads carries
  `f60a80`, the same object DAPI logs as its svcdata dispatch target.
  First thing to find next: what PROCNAV's main thread waits on before it opens its
  mailboxes (it is silent in a way that futex/poll waits, which we do not log, would
  explain - add temporary logging there rather than guessing).

### Update: why PROCNAV stops right after it starts (syscall trace)

With a per-process syscall trace (`NAVBIN syscall` in `hook_syscall.rs`) PROCNAV is
not silent at all - it makes exactly ten syscalls and dies:

```
brk, uname, mmap2 x2, #33, then #983045 (0xF0005, a stub svc),
then four calls of "syscall #0" from pc=0x62ef90, then
[16] thread exited with code 1  and  Execution error: FETCH_PROT at 0x7ff3cfe0
```

The `#0` calls are not Linux syscalls. The call site is a two-instruction ARM stub:

```
0x62ef88  push {r4, lr}
0x62ef8c  svc  #6
0x62ef90  pop  {r4, pc}
```

so the call number is the *immediate* of `svc`, not R7 (R7 is whatever the caller
left behind - here 0, which is why we reported the unimplemented syscall #0). Its
caller passes `r0 = sp`, `r1` = a version-like id (`0x50312`, `0x3010a`, `0x60112`)
and `r2` = a code pointer (`0x5e8ab4`, `0x5f2824`), which reads like a component
registration handshake (id+version+entry), not a filesystem or IPC operation.
Around `0x62efac` the words are themselves ARM encodings (`0xe92d4008`,
`0xe59f1010`, `0xe58d0000`), i.e. this area holds generated stub fragments.

So PROCNAV needs the service ABI that `svc #imm` selects (LX monitor / dual-OS call
layer that statically linked processes use instead of `libtrace_dualos`), and it
aborts as soon as that call is unanswered. Next step is to dump every distinct
`svc #imm` in the image, find the table the immediate indexes, and answer those
calls at syscall level (return a value, do not freeze on the first one).

### Update: PROCNAV's blocker is the LX monitor call `svc #6`

Answering the shim (see previous section) gets PROCNAV past the point where it used
to exit, and shows what the call actually is. All five calls in a boot are `#6`,
with `r0` pointing at a struct on the stack and `r1` a packed version:

```
#6 r0=0x7ff3ce10 r1=0x50312    r2=0x5e8ab4 r3=0x5e8bbc
#6 r0=0x7ff3cdec r1=0x5c022a   r2=0x5ea67c r3=0x5ea668
#6 r0=0x7ff3cdd8 r1=0x103010b  r2=0x60d674 r3=0x5f2a4c
#6 r0=0x7ff3cde4 r1=0x63040a   r2=0x5ea6a0 r3=0x0
#6 r0=0x7ff3cdc8 r1=0x101030b  r2=0x5f2a3c r3=0x0
```

The shims live in one block next to `DT_INIT`/`DT_FINI` (`0x5ea39c-0x5ea3d4`:
`svc #5`, `svc #9`, `svc #9` - the second one entered after `orr r1, r1, #0x40000000` -
and `svc #4`), so they are the statically linked replacement for `libtrace_dualos`
and the call id is the instruction immediate, which is why they surfaced as
"unimplemented syscall #0" (R7 happens to be 0).

Return value does not matter for surviving the calls (0 and 1 both walk through all
five), but with 1 the process then faults writing at `0x5ea818` (`WRITE_UNMAPPED`)
and exits with code 1, while with 0 it exits immediately. That reads like `#6` is a
"attach/allocate this resource" call whose result is a handle or a pointer the caller
then writes through - returning a small integer is wrong either way. Next: answer
`#6` with an address of a real mapped guest region, dump what PROCNAV stores there
and let the struct layout tell us what the monitor is expected to provide; the
`orr r1, #0x40000000` variant of `svc #9` is probably a feature query and will need
the same treatment.

### Update: monitor SWI ABI and what call #6 actually has to do

The monitor image (`Firmware/D605/triton_mid.bin`, stripped 64-byte `triton_dualos`
header, based at `0x80000000`) starts with an ARM vector table whose SWI entry is
`0x800eaa04`:

```
mrs  ip, spsr ; tst ip, #0x20        ; Thumb or ARM?
ldrhne ip, [lr, #-2] ; bicne ip, ip, #0xff00      ; Thumb: id = imm8
ldreq  ip, [lr, #-4] ; biceq ip, ip, #0xff000000  ; ARM:   id = imm24
cmp  ip, #4 ; blt  out_of_range
cmp  ip, #0x1b ; bgt out_of_range
ldr  lr, [table_ptr] ; ldr lr, [lr, ip, lsl #2] ; cmp lr,#0 ; bxne lr
```

So ids `0..3` stay with the OS and `4..0x1b` are monitor services - exactly the
`#4/#5/#6/#9` shims in PROCNAV. The table pointer itself lives in RAM beyond the
image (`0x80531ff0`, filled at init), so the individual handlers are not reachable
statically from the file image.

Handing the guest a real mapped page as the result (instead of 0/1) proves the
result is not an output buffer: the page stays all zero, and PROCNAV feeds the
returned value back as `r3` on the next call:

```
#6 r0=0x7ff3ce10 r1=0x50312   r2=0x5e8ab4 r3=0x5e8bbc
#6 r0=0x7ff3ce18 r1=0x3010a   r2=0x5e8b38 r3=0x5e8bbc
#6 r0=0x7ff3ce18 r1=0x60112   r2=0x5e8b44 r3=0x90002000   <- our handle echoed back
#6 r0=0x7ff3ce10 r1=0x50312   r2=0x5f2824 r3=0x90002000   <- then FETCH_PROT
```

The fourth call dies with `PC = 0x7ff3cfe0` (the stack, NX) and `LR = 0x5e8bd8`,
which is the shape of `pop {r4, pc}` restoring a link slot the callee never wrote.
That is the signature of a *callback* service: the monitor is expected to execute
`r2` (component entry point, `r1` = its version, `r0` = its argument struct) in the
caller's context and return what that function returns. We currently return without
invoking anything, so the caller's frame is left half-filled.

Next step is therefore not more guessing about the return value but calling `r2` as a
guest function, which the emulator can do with the same call-stub machinery
`prochmi.rs` uses (`heap_alloc` + a stub that restores registers and jumps back).

### Update: `#6` is a completion handoff, and PROCNAV now survives bootstrap

The `svc #6` sites are reached through a wrapper (`0x62ef94`):

```
push {r4, lr}
ldr  r1, [pc, #0xc]   ; component version
ldr  r2, [sp, #4]     ; entry point supplied by the caller
mov  r0, sp           ; pointer to the caller's own frame
bl   0x62ef88         ; push {r4, lr} ; svc #6 ; pop {r4, pc}
```

so `r2` is caller-supplied and `r0` is its stack frame. What `r2` points at differs per
component, and the two shapes need different answers:

* `0x5e8ab4`, `0x60d674` start with `cmp r0, #0` and then store `[sp, #4]` into
  `[r4, #0x18]` - a *completion* path that consumes the service result, so the monitor
  has to resume there in place with the result in `R0` (success = 0).
* `0x5ea67c` starts with `str r0, [sp]` - an ordinary function body, so it has to be
  called with the caller state saved and returned through a stub.

`hook_syscall.rs` now distinguishes them by decoding the first instruction (`cmp r0,#0`
=> resume in place, otherwise invoke through the `[lx-monitor-call-stub]` page). Calling
everything, or resuming everything, leaves PROCNAV dead; with the split, thread 18 no
longer exits with code 1 during bootstrap at all.

### Update: recovered monitor SWI table (PROCNAV stays the RNW data source)

The monitor's own service table is now recovered (see
`docs/reverse_engineering/triton_mid_swi_services.md`): the table base is `0x8052f678`
behind the variable at `0x80531ff0` (sole writer `0x80008a08`), registered through
`FUN_8002077c(id, handler)`. `svc #6` is handler `0x80037184`, a **yield**: it stamps the
timeout taken from `r1` into `[TCB+0x20]` and tails into the reschedule entry; `svc #9`
(`0x800373a8`) is a per-thread array access, and ids `0xb..0x1b` are never registered by
this image (they belong to the other core/components).

So the `svc #6` sites in PROCNAV are coroutine handoffs: the wrapper at `0x62ef94` passes
its own frame in `r0`, a timeout in `r1` and a coroutine body/continuation in `r2`. We do
not switch a TCB, we run that body inline (host scheduler does the switching), which is
why the entry-point shape decides between resuming in place and calling through a stub.
PROCNAV stays the RNW data source - replacing it with a service simulator is off the
table, because we need the real binary for diagnostics once we start editing map data.

### Update: PROCNAV is blocked, not spinning

`run_quantum` gives a lone thread `count = 0`, i.e. `emu_start` runs until the thread
blocks or exits, so a user-space spin loop would look like a frozen process from the
outside. To tell the two apart, `EMU_QUANTUM_TIMEOUT_US` adds a wall-clock budget per
quantum and dumps PC/LR/SP when it stops a run without treating it as an error.

With the budget enabled (`EMU_QUANTUM_TIMEOUT_US=1000000`) the run is byte-for-byte the
same as before and not a single spin-watch sample appears: thread 18 is never inside
`emu_start`. PROCNAV therefore sits in our own `Blocked` state after the three `svc #6`
handoffs - it is waiting on an emulator-side object whose wake side we do not implement,
which is also why it never reaches `mq_open` or the registry device.

### Update: PROCNAV neither parks nor is inside a bounded run

With the park report added, the log shows park lines only for DAPIAPP, procbaselx,
prochmi and procmapengine - never for `/opt/bosch/navbin/PROCNAV.OUT`. Together with the
spin watch (`EMU_QUANTUM_TIMEOUT_US=500000`) producing no sample either, PROCNAV's host
thread is neither parked in the scheduler nor stopped by a wall-clock budget inside
`emu_start`: its loop keeps getting `run_quantum == true` while the quantum itself
finishes essentially instantly, so the thread stays `Runnable` and nothing is ever
logged. That is a host-side livelock, not a guest spin loop and not a blocked thread,
and it is what has to be chased next (log the PC at quantum entry for that process).

### Update: null-PC resume is fatal, not a quantum (livelock fixed)

The stall was PROCNAV being rescheduled with `pc = 0x0` (`lr = 0x60c9e8`): `emu_start`
with `begin == until == 0` returns immediately, so the loop counted 43469 empty quanta
without logging anything. `run_quantum` now refuses to start such a thread and
terminates it with the SIGSEGV default action (139), which turns the livelock into a
single clear error line.

Giving the mid-function entries the frame their prologue implies (`r4` and the return
link above a `0x10` locals area, entered with `sp` below it) makes the `str r0, [sp]`
site (`0x5ea67c`) behave, but the last site still lands on a null link: those entries are
epilogues of *other* functions, so an inline call cannot restore their frames - the
handoff needs a per-handoff context save (the monitor's TCB role) so the yielder is
resumed by the body returning, rather than us re-entering the body.

### Update: per-handoff context frame (the monitor's TCB role)

`lx_invoke_monitor_callback` now allocates one `[lx-monitor-handoff]` block per handoff
(a handoff body can hand off again, so they cannot share a block) with three separated
areas - trampoline at +0, the entry's own frame at +0x20, the save table at +0x40 - and
enters the mid-function entry with SP at the locals, `r4` and the return link above them.
The link is a trampoline that points the shared resume stub at this handoff's save table;
the stub's `add r12, sp, #0x34` matches the table layout `R0..R11, R14, SP, resume PC`.

Two bugs this exposed, both worth remembering: the save area must not sit directly above
the body frame (the body's `add sp, #0x10 ; pop {r4, pc}` pops it as its own saved
frame, which is how the null PC used to appear), and the trampoline's literal must be
addressed with `ldr sp, [pc, #4]` for a word at +0xC - with `#8` it read the next page
word (zero), sent the stub to SP 0 and produced a null PC again.

With both fixed PROCNAV takes no null PC at all, `ServiceDataError code=0x305` is gone and
the `mbx_1024` flood collapses from ~1M messages to 30. It still ends by exiting with
code 1 after the handoffs and never reaches `mq_open` or the registry device.

### Update: the synthesized frame is wrong for the mid-function entries

The handoff now dies with `FETCH_UNMAPPED` at `PC=0x90006000`, `LR=0x90005000` (that
handoff's trampoline), `SP=0x90005038`, `R0=0x90004000` (the previous handoff's block),
`R1=0x103010b`, `R2=0x60d674`. `SP` sits `0x18` above the synthesized entry SP, so the
body's epilogue popped one word past our saved link: the frame whose `r4`/`lr` it pops
is `0x18` (or generally `0x10 + n`) below the entry point, not `0x10`.

That frame is not ours to invent - for the entries starting with `cmp r0, #0` the body
belongs to the *same function as the yielder*, so its frame is the yielder's own frame
and the correct SP is the yielder's SP at the moment of the `svc`. Only the entries that
start with a real prologue (`str r0, [sp]`) need a synthesized frame. The hybrid is
therefore: prologue-shaped entry => synthesized frame; mid-function entry => resume in
place with the caller's registers and SP, R0 = result - and before resuming, validate the
link slot that epilogue will pop instead of letting the thread branch to whatever is
there (the scheduler's null-PC guard is the backstop, not the answer).

### Update: the "frame" the body pops is the handoff control block, not a C frame

Logging what the mid-function entry will pop before resuming it:

```
0x5e8ab4 resumes with sp=0x7ff3ce08: pops r4=0x0     pc=0x5e8ab4
0x60d674 resumes with sp=0x90002ff8: pops r4=0xe320f000 pc=0x90003040
```

At the first site the slot holds `r4 = 0` and `pc = 0x5e8ab4` - the continuation
address again, which is also exactly what `r2` carried. So the slots at `sp + 0x10` /
`sp + 0x14` are the handoff **control block** the wrapper builds (`mov r0, sp`), not a
`push {r4, lr}` frame, and the second slot is where the app stores the resume address it
passes in `r2`. Resuming the continuation with that block in place therefore jumps back
to the same continuation: the loop we kept chasing (null PC, FETCH_UNMAPPED, livelock) is
the same bug in three costumes.

The monitor's actual service is to take the block - register the coroutine, remember the
resume address, and later run it with the context it records there (`[r0+0x10] = 0` reads
as "no context saved yet"). Both of our approximations (inline call with a synthesized
frame, resume in place with the caller SP) only re-enter the body, so nothing can ever
complete the handshake and PROCNAV still ends in `0x305` with the `mbx_1024` flood back at
~1M. Next step is to model the control block: parse `[r0 + 0x00..0x20]`, keep a per-block
coroutine record in the emulator, and make the *record* drive the resume instead of
trusting whatever the stack slot happens to contain.

### Update: the coroutine's resume stub lives on the app stack (NX stack is the blocker)

Dumping the control block and decoding each entry gives the full picture. The words
around `r0` are just the caller's frame (no fixed layout), but they contain the address
the epilogue will branch to:

```
site 1  r0=0x7ff3ce10: .. [r0+0x0c]=0x5e8ab4 (epilogue) .. [r0+0x14]=0x7ff3cfe0
        0x5e8ab4: cmp r0,#0 / ldrge r0,[sp,#4] / strge r0,[r4,#0x18] / pop-equivalent
site 2  r0=0x7ff3cdd4: [r0+0x08]=0x5ea67c: str r0,[sp] ; bl 0x5f2a50 ; subs r4,r0,#0 ...
site 3  r0=0x90003000: [r0-0x04]=0x60c9e8 ; [r0+0x04]=0x60d674: cmp r0,#0 ... add sp,#0x10 ; pop {r4,pc}
```

`r2` is the *caller's saved link*, and the code there is the yielder function's epilogue -
i.e. the app tail-yields: ending the coroutine is a normal return to a landing stub the
monitor put on the coroutine's stack. At site 1 that landing stub is `0x7ff3cfe0`, a
**stack address**, which matches the `FETCH_PROT` at `0x7ff3cfe0` seen long ago: the real
LX system has executable coroutine stacks and the monitor writes its resume/exit trampoline
into the frame before starting the coroutine, while our stacks are mapped NX.

Concrete next step, no guessing left: at each handoff, write our resume trampoline
(`ldr sp,[pc,#4] ; b <stub> ; nop ; <save table>`) into the landing slot the frame points
at, make that page executable (add PROT_EXEC to the stack mapping, or map a small
executable landing area and point the slot at it), and enter the epilogue with the
caller's SP as it already is - no synthesized frame at all.

### Update: per-entry frame geometry decoded, one coroutine still needs a real stack

`lx_entry_frame_size` reads the entry code and commits on the first instruction that
picks a stack discipline: `push`/`bl`/`bx` or a call means the entry brings its own frame
(`locals = 0`), while touching `[sp, #imm]` / popping means the frame belongs to us and its
size is the `add sp, sp, #imm` that precedes the `pop` (prefix matching is required, since
the reads are conditional: `ldrge r0, [sp, #4]`). The sites come out as:

```
0x5e8ab4  locals = 0x00   (mov lr, pc ; bx r3 - returns through lr)
0x5ea67c  locals = 0x00   (str r0,[sp] ; bl ... ; subs r4, r0, #0 - sets up its own)
0x60d674  locals = 0x10   (cmp r0,#0 ; ldrge r0,[sp,#4] ; add sp,#0x10 ; pop {r4, pc})
```

That removes the guessed frame size and there is no null PC any more, but the third site
still ends in `FETCH_UNMAPPED` at `0x90006000` with `SP = 0x90005038`, one block page
below its own trampoline. The reason is visible in its control block: its `r0` was
`0x90003000`, i.e. *our previous handoff block* - the body of handoff 2 yielded again, so
by handoff 3 the yielder's frame lives inside a block we allocated and reuse per handoff.
Per-handoff blocks are therefore still the wrong unit: a coroutine needs one stack
allocated when the coroutine starts and reused for all of its handoffs, which is what
`[TCB+0xe0]` context frames are on the real monitor.

### Update: executable stacks make PROCNAV fault-free (and it now hangs instead)

Two fidelity fixes: the process stack is mapped `PROT_READ|WRITE|EXEC` (the LX runtime
keeps its monitor-return stub on the app stack, `0x7ff3cfe0`), and each handoff gets a
real 64 KiB executable coroutine stack plus the same landing stub at its bottom - because
the epilogues that end a coroutine branch to a landing address *on that stack*, which is
exactly what `FETCH_UNMAPPED`/`FETCH_PROT` at `block + 0x1000` had been reporting.

Result: PROCNAV no longer takes any execution error, no null PC, no exit. The control
blocks of the nested handoffs now point into their parent's coroutine stack with clean
zero slots, i.e. the nesting is consistent. What changed is the failure mode: after the
third handoff thread 18 goes completely silent - no syscall, no park report, no
spin-watch sample with `EMU_QUANTUM_TIMEOUT_US=1000000`, no `Execution error`, no exit -
so its host thread is neither in `emu_start`, nor parked, nor terminating; it is stuck
somewhere between quanta (or on a host lock). Attaching gdb to confirm is blocked by
`ptrace_scope`, so the next step is a host-side heartbeat inside `run_process_loop`
(log the loop tick per process) to tell "stuck in a call" from "spinning in the loop".

### Update: `svc #6` is a coroutine *switch* - map coroutines onto guest threads

With quantum tracing per handoff, the handoff loop is fully explained. Two facts decide
it:

* Every entry resumes with the same `sp` and `lr = tramp`, and the slot its epilogue pops
  contains the continuation's own address (`continuation 0x5ea67c resumes with
  sp=0x90013eb0: pops r4=0x76573c pc=0x5ea67c`) or zero (`0x60d674: pops r4=0x0 pc=0x0`).
  Resuming in place therefore re-enters the same body: an endless loop. Giving the body a
  fresh stack only moves the loop into our pages (`pc` bouncing over the zero-filled
  pages between the handoff block and the coroutine stack).
* The monitor's id-6 handler does not return to the caller either: per the recovered
  handler `0x80037184`, it stamps the timeout into `[TCB+0x20]` and **switches the stack to
  the context frame at `[TCB+0xe0]`** before rescheduling. The popped `r4`/`pc` belong to
  whoever resumes the coroutine, not to the caller.

So `svc #6` neither calls a function nor returns - it suspends the current coroutine and
transfers control, and the epilogue is the *exit path* used when the coroutine is resumed
for the last time. Neither of our inline approximations can express that. The faithful
mapping is available in the emulator already: create a **guest thread** per coroutine
(entry `r2`, arguments `r0..r3`, its own stack, link = a coroutine-exit handler that wakes
the parent with the child's result) and block the yielding thread on it, reusing the
existing `Blocked`/`pending_result` machinery. That is the next step.

#### Implementation pointers for the coroutine threads

`mqueue.rs:222-260` (`mq_notify`) is the exact template: allocate a stack, snapshot the
caller context with `context_init`, write SP/R0/LR on the shared vCPU, `context_init`
again for the child, restore the caller, then push a `GuestThread { status: Runnable,
pc: function, .. }`. For `svc #6`: child `pc = r2` with `r0..r3` passed through and `LR`
set to a coroutine-exit stub; the yielding thread parks until that child terminates and
takes the child's final `R0` as the result of the yield. Check `BlockReason` for a
join/child-exit variant (only futex/sleep/mq/signal variants were seen so far) - if there
is none, add one and complete it in `advance_blocked` when the child thread is `Exited`.

#### Coroutines as guest threads: what it revealed

Mapping `svc #6` onto a spawned `GuestThread` (+ `BlockReason::CoroutineExit`, resumed from
the exit hook with the child's final R0) replaced the endless in-place loop with visible
structure: 22 coroutines spawned, 7 yielders resumed, no safety timeouts. The remaining
failure is now a single instruction, reached from an OSAL async-IO path (the containing
function is `OSAL_s32IOReturnAsync`):

```
5ea804: bl  5ecda0        ; second init/complete call
5ea808: subs r4, r0, #0   ; r4 = its return value
5ea810: bge 5ea824        ; success path
5ea814: mov r0, #0
5ea818: str r4, [r0]      ; <-- WRITE_UNMAPPED: stores the error code through NULL
```

Two facts to chase next: the coroutine bodies finish with `0x1` without making a *single*
syscall, and their epilogue pops a zero link (`[31] cannot resume at pc=0x0`), so they are
mid-function continuations of the yielder that expect the yielder's frame and a *success*
result. Their `r1` arguments (`0x103010b`, `0x63040a`, `0x101030b`) look like packed OSAL
message ids, and the callee failing at `0x5ec554`/`0x5ecda0` reads a table at `[obj, #0x9c]`
that nothing has filled - the registry/portal the real monitor's id-4 task table would have
created at startup (`0x80009a6c`, the message pump) is what is still missing.

The callee that fails is a static tail-call wrapper adding a static argument:

```
5f2aa0: ldr r2, [pc, #8]      ; r2 = &0x0122f578 (pool descriptor in .bss)
5f2aa4: b   5f61ec            ; pool_alloc(obj, size, &descriptor)
5f61ec: push {...} ; bic r7, r2, #7 ; ldr r0, [r7, #32]   ; pool root at 0x0122f598
```

so the table append at `0x5ec554` returns `-0x210000` because the *pool root word* at
`0x0122f598` is still zero: PROCNAV's own allocator pools are never initialized. That is
the same missing bootstrap as the monitor's id-4 task table (`0x80009a6c`), and it is the
next thing to supply - either by running PROCNAV's pool-init routine or by handing it the
backing memory it expects.

### `svc #6` resolved: asynchronous service call with a monitor-built return slot

Reading the shims together with the code they hand over settles the ABI:

```text
60c9d0: push {r3, lr} ; ldr r1, [pc]      ; r1 = service id (0x0103010b, 0x0010000a, ...)
        str r0, [sp]  ; ldr r2, [sp, #4]  ; r0 = argument block, r2 = completion address
        add r0, sp, #0; bl 62ef88         ; svc #6
60d674: cmp r0, #0 ; ldrge r0, [sp, #4]   ; status, then the out-parameter from the block
        strge r0, [r4, #24]               ; ... stored into the OSAL object being created
        add sp, sp, #0x10 ; movge r0, #0 ; pop {r4, pc}
```

So `r0` is the argument block (the result travels back through it), `r1` is a packed
service id, `r2` is the completion that consumes the result, and the completion's epilogue
pops its return address from `block + N + 4*skipped`, where `N` is the `add sp, sp, #N` it
performs and `skipped` the registers before `pc` in its pop list. That slot is the monitor's
to fill; putting the thread-exit trap there is what ends the coroutine instead of unwinding
back into the shim that yielded (which hands the same completion back forever). The
completion keeps the yielding frame and the yielder's registers, and the yielder parks.

Result: PROCNAV's syscall count `navsys` went from 6 to 12 and the execution errors from
10 to 2 - `0x5ea818` (`str r4, [0]`, an error path inside a completion that called
something still unimplemented) and `0x122f560` (`FETCH_PROT`, one completion whose return
address is taken by something other than a pop).

### The service wrappers, and the stack geometry that is still wrong

Every `svc #6` comes out of a thin wrapper that copies its arguments into a stack block and
hands over its own return address:

```text
5f282c: push {lr} ; sub sp, sp, #20 ; stmib sp, {r0,r1,r2,r3}
        add r0, sp, #4         ; argument block
        ldr r2, [sp, #20]      ; completion = return address into the caller
        ldr r1, [pc, #8]       ; service id (0x0063040a here)
        bl  62ef88             ; push {r4,lr} ; svc #6 ; pop {r4,pc}
        add sp, sp, #20
        pop {pc}
```

The completion therefore continues the *caller* of the wrapper, mid-function, while SP is
still inside the wrapper's frame. The return slot has to be computed from the wrapper's
frame as well: `block + (wrapper frame - block offset) + unwind + 4*skipped`, where `skipped`
is the number of registers before `pc` in the epilogue that finally ends the outer function
(`0x5ea820: pop {r3,r4,r5,r6,r7,r8,r9,sl,fp,pc}` -> 9 registers). Two symptoms show the
current decoder missing that:

* `0x122f560: [21] FETCH_PROT` with `sp=0x7ff3cdc4`, right after
  `[21] quantum 23: pc=0x62ef90 lr=0x5f2848 sp=0x7ff3cdbc` - a completion that issued its
  own service call popped a return address that was not the exit trap.
* the linear "first pop after the entry" scan can stop at an inner function's epilogue
  instead of the one that ends the outer function.

Fix: take the *outermost* epilogue (the pop whose list matches the `push {..., lr}` that
starts the function containing the completion) and add the issuing wrapper's frame size,
decoded from the wrapper at `lr - 8` of the `svc`.

### Measured geometry of the two service idioms

`EMU`-style stack dump taken at the `svc` (window starts at `sp - 0x20`):

```text
# wrapper 0x60c9d0 (create pool), works today
service 0x103010b block=0x7ff3cdbc sp=0x7ff3cdb4 lr=0x60c9e8 slot=0x7ff3cdd0
  sp+0x00 0x122f578   descriptor        sp+0x04 0x60c9e8   lr
  sp+0x08 0x7ff3cdc8  = block (K = 8)   sp+0x0c 0x60d674   completion
  sp+0x10..0x1c 0 0 0 0                 sp+0x20 0x47376a9c canary
# wrapper 0x5f51e0 (id 0x5c022a), the one that ends in FETCH_PROT
service 0x5c022a block=0x7ff3cddc sp=0x7ff3cdd0 lr=0x5f51f8 slot=0x7ff3cdf0
  sp+0x00 0x47376a9c canary             sp+0x04 0x5f51f8   lr
  sp+0x08 0                            sp+0x0c 0x121b884
  sp+0x14 0x76573c  sp+0x18 0x5ea67c  sp+0x1c 0x5ea668  sp+0x20 0x47376a9c
  sp+0x24 0x5ea418  ...  sp+0x38 0x5e8ad8 0x1 0x5ea41c 0x0 0x5e8ab4 0x2
```

`R2` (the completion) is `0x5ea67c`, *not* the `[block + 4]` value `0x5f51f8`, and `0x5ea67c`
is the middle of a function whose epilogue pops ten registers. So the two idioms differ:
the create-pool wrapper's completion is the function whose frame ends right above the
block, while `0x5ea67c` belongs to a function whose frame is far below (`0x5ea418` /
`0x5e8ad8` / `0x5e8ab4` entries are its own saved registers, still on the stack). Resuming
there with `sp = block` makes its `pop {r3..fp, pc}` read the wrong slots by four bytes -
which is what produces the branch to `0x122f560`.

### The request block: first word points at the caller's own result slot

The `tkse_*` wrappers all look like

```text
60c9d0: push {r3, lr}
        ldr  r1, [pc, #16]      ; service id literal next to the wrapper
        str  r0, [sp]           ; block[0] = the argument the caller passed
        ldr  r2, [sp, #4]       ; block[4] = return address into the caller = continuation
        add  r0, sp, #0         ; block
        bl   62ef88             ; svc #6
        pop  {r3, pc}
```

and their callers hand over a pointer into their own frame, e.g. `0x60d630` does
`add r0, sp, #4` before `bl 0x60c9d0`, then reads the result with `ldrge r0, [sp, #4]`.
So `block[0]` *is* the address the continuation loads: the service writes its result to
`*[block]`, not into the block. `0x5f2a3c` confirms it from the other side - it passes
`sp` itself and reads back with `ldrge r0, [sp]`, and indeed `block[0] == block + 8`
there. Sizes come along in `block[8]` (`0x1000000` = the 16 MB pool `0x5f2a5c` asks for).

Handing out a block of the requested size moved PROCNAV past `0x5ea818` into the
allocator's own list linking at `0x5f5ed4`, which computes
`r1 = r7 * [desc + 0x24] + (block - 8)` and stores there. The descriptor's stride and
limit words (`+0x24`, `+0x20`) are only zeroed by `0x60d630`, so the pool service has to
initialise the descriptor rather than only hand back a pointer.

### The pool object (0x0b) services: create returns a chunk size, grow returns the memory

The `0x5f5d54` allocator and its OSAL glue (`0x5f2a28`/`0x5f2a4c`/`0x5f2a54`, plus the
same trio `0x60d600`/`0x60d624`/`0x60d630`) split the work over three `svc #6` services:

- `0x0103010b` create: `0x60d630` zeroes `[desc+0x18]`/`[desc+0x20]`, stores the caller's
  first argument (pool size, e.g. `0x1000000`) at `[desc+0x1c]`, the grow thunk at
  `[desc+0x24]` and the free thunk at `[desc+0x28]`, then calls the wrapper. The
  continuation stores the result into `[desc+0x18]`, and the allocator multiplies that
  word by chunk counts (`end = p + n * chunk - 8`) and thresholds against `chunk >> 2`:
  the result is the **bytes-per-chunk granularity**, not a buffer. Returning a pointer
  there is what made `0x5f5ed4` write to `mla`-garbage.
- `0x0101030b` grow: the allocator tail-calls `[desc+0x24]`, which packs
  `[pointer to result slot, chunk count, context, continuation]` into the `0x60c988`
  wrapper. The continuation reads the pointer back from the word `block + 0` points at
  and maps negative statuses to NULL (`movlt r0, #0`), so refusal must be a negative
  status. The allocator initializes the region itself - end sentinel at
  `p + count * chunk - 8`, both nodes threaded - so the region must be exactly
  `count * chunk` mapped bytes.
- `0x0102010b` free: acknowledges a returned region; the emulator never reclaims heap.

Block layout of a grown region: header at `p` (`[p]` = region end sentinel address, also
the in-use-list next), flags in `[p+4]` low bits (1 allocated, 2 block, 4 end), free
list entries at `p+8`/`p+0xc` stored as slots (walkers dereference the slot, so
`[node - 8]` is the in-use-list word). The user pointer is `p + 8`; `free` walks
neighbours through the in-use list, never below `p`.

### The bank/directory-service object (0x0a): resolve hands back the receive context

`0x5ea668` is PROCNAV's directory-service registration. It issues four `0x0a`
commands through the `tkse_*` wrappers that each build a request block on the
stack and trap (`0x62ef88` = `svc #6`); the dispatcher keys on `id >> 16`
(`0x8004829c`). The real leaves validate and walk kernel bank/DS state the
monitor image never initialises (`0x8052e295`, `*0x80516fc4 = 0`), so each
command is served to its *observable* contract, the value the caller reads back:

- `0x0008030a` (cmd 8): `0x5ea6d4` checks only `cmp r0, #0`. Served status 0.
- `0x0063040a` (cmd 63): `0x5ea69c` stores the returned word into a global
  (`0x5ea6a0`) and does not dereference it here. Served status 0.
- `0x0201020a` (cmd 201): `0x5ea71c` reads the result as a path *length*; a zero
  makes the caller take the same branch as an unregistered entry and use its
  statically linked default path (`0x5ea72c -> 0x5ea734`). Served status 0.
- `0x0203020a` (cmd 203): the only command whose *output* is dereferenced. The
  wrapper passes `r0 = 0` (self handle) and `r1 = &obj->0x24` as an output slot;
  the continuation (`0x5ea7d4`) runs `0x5f2ab4 -> 0x5f5af0`, which reads
  `[obj->0x24 + 0x1c]` (byte count) and `[obj->0x24 + 0x2c]` (`u16` element
  count) and walks the buffer only when the count is non-zero. At boot nothing
  has been received, so the truthful object the bank manager hands back has an
  empty list; a freshly mapped, zero-filled page gives a zero count and
  `0x5f5af0` returns 0 without touching the buffer. `complete()` therefore
  allocates one zero page and writes its address through `block + 4`.

Object identity, checked against the run: the out-slot `0x122f4cc` is
`obj + 0x24` where `obj = 0x122f4a8`, and `0x5f2ab4`'s argument is
`0x5ea858 - 0x98 = 0x122f540 - 0x98 = 0x122f4a8` - the same object the reader
dereferences, so the pointer written at `block + 4` is exactly the one read.

Result: PROCNAV completes its OSAL bring-up (spawns Timer/MAXCB_Timer/AE_109,
opens `mbx_265`/`mbx_0`) and parks on a `MonitorYield` instead of dying at
`0x5f5af8`. The `0x64` (queue) and `0x6e` objects its peers then drive are the
next decode surface; they currently fall to the status-0 catch-all.

### Corrected svc #6 object model (Ghidra pass): 0x12 is the message service, 0x13/0x6e are sem/event banks, 0x64 is a loadable overlay

A full dispatch/registration sweep of `triton_mid_raw.bin` corrects the earlier
"object 0x0a is the queue" reading and settles the reuse question:

- **Dispatch (CONFIRMED).** `0x80037184 -> 0x800371b4 -> 0x8003c2b8`. Only
  consumer of CB `0x80511634`; there is **no second id table**. `CB+0` = max id
  (>= 0x77 at runtime), `CB+8` = table base (`kmalloc`'d at boot by `0x8003b214`,
  all zero in the image). Entry = `base + (id-1)*0x38`; **handler = `entry+0x18`**
  (the prior-pass "entry+0x04" was off by one slot; `0x8004ef9c` proves the
  layout: +0x08 id, +0x0c prio, +0x10 aux buf, +0x14 last-ret, +0x18 handler).
  Handler contract (via `0x8010cf4c`): `f(r0=req block, r1=full id, r2=cont,
  r3=0x60 scratch)`, `r0`=status; outputs written through the request block via
  `0x8002f69c(ptr,size,dir,tid)` (dir 4 read guest, dir 6 write guest).
  Generic multiplexer `0x8003b060/0x8003b130` = a 64-entry vector at `obj+0x14`
  (count `obj+0x18`) keyed on `id>>16`.

- **Object 0x12 = the real message service (CONFIRMED).** Handler `0x80042c40`,
  registered `0x800439f0` (keys `TEV_MsgEvt/TEV_MsgBrk/TotalMsgMax/MaxMsgSz`);
  message FIFO lives in a per-process **kernel bank** (self bank via
  `0x80043a7c(0,&obj)`), sorted list at `bank+0x6c`. Send = `0x000012`
  (`0x800425e0`, wakes receiver via `0x8003cfec`); **blocking receive = `0x010012`**
  (parks via `0x8003cdc8(mask,0xffffffff) -> 0x8003ed48`, timeout = infinite,
  returns message type; `0xffcf0000` = timeout). Addressed by **integer type id
  (1..0x1e) + 32-bit source mask**, never by name. `0x00050312` (what PROCNAV
  calls at boot) is *unimplemented in this build* (falls to `0x800436a0`).

- **Object 0x13 = semaphore bank (CONFIRMED), object 0x6e = event bank
  (CONFIRMED from the guest).** Handler `0x800151ec` (0x13). PROCNAV's
  `0x10030313` is a **blocking timed take** (rb[0]=handle, rb[1]=count,
  rb[2]=timeout ticks; parks at `0x8003efb4`) -> that is the `object 19, <ticks>`
  idle wait, NOT a message wait. `0x10220213` = give/release, `0x10000113` =
  create, `0x10200113` = delete. The event bank 0x6e (`0x70000|idx`) is a pure
  1-bit wakeup. Confirmed by `libosal_linux_so.so` `u32CreateMsgQueue`
  (`0x4850bc90`): each OSAL queue is `event(0x6e) + semaphore(0x13) + slot idx`,
  name kept in a 32-byte user-space field (251 slots), FIFO in user-space shared
  pools (`s32MqueueTableCreate` `0x4850fa18`).

- **Objects 0x64 and 0x6e have NO registration in the static image (NOT FOUND).**
  Every xref of `0x8003b5a8`/`0x800db104` enumerated; no `mov r0,#0x64/#0x6e`
  feeds a registrar. The image holds an overlay-name table
  (`sysprogosal.uout`, `sysprogosalutil.uout`, `sysprogdnl.uout`, ... at
  `0x80120ac8`) -> these kernel objects are installed when a `sysprog*` overlay
  is loaded. PROCNAV's workhorse `0x64` (181 calls, cmds 0x2,0x3,0xa,0x16,0x17,
  0x1b,0x43,0x48,0x4d...) is therefore the OSAL-utility overlay service. No
  `sysprog*.uout` exists in the firmware tree or the SD card, so the handler
  cannot be recovered statically; it must be read live from
  `table_base + 0x64*0x38 + 0x18` in a run where the monitor's own init built
  the table (our host-side interception never builds it).

### Why the nav request still 0x305s, and the reuse fork it forces

PROCNAV is now alive and sits in its main loop (timed take on a 0x13 semaphore);
`mbx_0` accumulates posts with **zero waits** (116/0), DAPIAPP keeps logging
`DAPI ServiceDataError code=0x305`. Cause: the two IPC worlds do not share a
substrate:

- libosal processes (DAPIAPP/prochmi/procmapengine) `OSAL_s32MessageQueue*` are
  hooked to the host `common::queues::MqState` (`mbx_*`), keyed by **name**.
- PROCNAV is statically linked with its own OSAL: queue FIFO in a **user-space
  shared pool**, sync on kernel **0x6e event / 0x13 semaphore**, plus the 0x12
  kernel **message bank** (keyed by type+mask) and the 0x64 overlay object. It
  never touches `MqState`, so the DAPIAPP post is invisible to it.

Reuse answer, sharpened: the shared `MqState` in `src/common` is the right
substrate for the *libosal/name-keyed* queues, but it is NOT what PROCNAV's
static OSAL reads. Making PROCNAV serve `0x305` needs one of:
  (A) bridge PROCNAV's 0x12 bank send/receive (and the 0x6e/0x13 that wake it)
      onto `MqState`, which requires the (name <-> bank/type/mask) mapping that
      lives in PROCNAV's user-space OSAL table; or
  (B) treat the OSAL queues as genuinely cross-process user-space shared memory
      (the `s32MqueueTableCreate` pool), have libosal use it too, and use
      0x6e/0x13 only to signal across processes - closer to the real design but
      a larger change that also reworks the libosal hook.
Both are blocked on first decoding PROCNAV's own OSAL queue table layout and
object 0x64, which needs a live table dump (the monitor's real init) rather
than static RE.

### Reframe: PROCNAV is gated in its registration phase, not missing an `mbx_0` wait

Pointer-deref added to the monitor request-block dump (`describe()`/
`dereferenced_names`) surfaced the names carried behind the pointer args of
PROCNAV's `svc #6` (thread [18]). It shows PROCNAV never names `mbx_0` and does
**zero** message receives: across a full run only 4 calls to the message object
0x12 exist (2 boot-only `0x50312`), `mbx_0` wait count is 0, and every `0x64`
(`0x1b0464`/`0x30164`/...) is a *create/register* of a named kernel object:

- OSAL objects it builds at startup: `LBS_REGISTRY`, `LBS_FUEL_SERVICE_TREE`,
  `LBS_MOVIE_SERVICE_DESC`, `GD_ThreadCommonCommunication`,
  `RDB_DEBUGINFO_SEM`, `wdb_safecnt_XX`... (registry / LBS / DB service objects)
- **Gating strings**: `_LICENSE` + `00000000` (an empty licence), `NAVEND`,
  `SPM_LCN2KAI_DownloadWorkaround`, `ON_TENGINE`, `MAXPOOLSIZE`,
  `LIBTKSE_FIRST_MEMALLOC_BLOCK`.

Conclusion: the earlier model ("PROCNAV should wait on `mbx_0`, so bridge
DAPIAPP's post to it") is wrong. PROCNAV is stuck *earlier*, in its
SPM/licence/registry bring-up, blocked on a semaphore/event that a peer or the
authorization path should release, so it never reaches the point of opening its
request queue and becoming the road-network server. The `0x305` is downstream of
that, not a name-mapping gap. The name-keyed C-bridge would therefore connect
nothing.

Next real question (supersedes the queue bridge): which semaphore/event/SPM
handshake is `0x10030313` / the `_LICENSE`/`NAVEND`/`SPM_LCN2KAI_*` sequence
waiting on, and who signals it on real hardware (map-card authorization via
`/dev/cryptnav`/`/dev/prm`, the `sysprogdnl` DNL overlay, or a peer process).

## The residual `0x305` is a missing vehicle position (CCP), not a data gap

After the `DYN<n>MBX` IOSC bridge made the IPC work, `0x305` dropped to a
steady residual (see correctness.md): 2 requests still fail, and they are the
road-network *area/coverage* requests, i.e. the ones that depend on where the
car is. Investigation (r168 + Ghidra) pinned the cause to a **missing position
feed**, end to end:

- `procmapengine` renders and requests map areas relative to the vehicle's
  **Car-Current-Position (CCP)**: `PortControl_CCAClientHandler_Navi::vHandleNavPosition`,
  `AddCCP`/`GetCCP` (`AddCCP: Updated CCA 1st TS..`, `GetCCP: Pos (%d/%d/%f) OUT OF BOUNDS`),
  and `MapEngine_GetCurrentGlobalMapPosition` (view `+0x2b8`=Long, `+0x2bc`=Lat,
  int grid units, degrees = unit * `8.381903e-08`).
- In the whole run there are **zero** `NavPosition`/`CCP`/`GlobalMapPosition`
  events - nothing ever delivers a position to the map engine, so the derived
  area requests resolve to "no data" and DAPI's `dap_map_tclWorker::enProcessJob`
  answers `0x305`. Confirmed empirically: seeding the map *view* centre to Krakow
  (`view+0x2b8/+0x2bc`, the `RENDER_CONTROL_GET_VIEW_RESULT` hook) did **not**
  change the CCA request count or the `0x305` - the block requests are driven by
  CCP, not the view centre. (That experiment was reverted; it also perturbed the
  steady-state `mbx_1024` loop.)
- The position *producer* is **`PROCNAV`** (pid 4). PROCNAV is the server of the
  `posfi` NavPosition service (`rtf_tclServerSideOutChannel` + `posfi_tclMsgNavPositionStatus`,
  `pc_tclMapMatching::vExtractBestSections_SetPosition`, map-matching) and the
  **client** of the combined-sensor service: it installs handlers for
  `cmb_sensorfi_tclMsgGpsDataStatus` / `GpsTightCouplingDataStatus` /
  `GpsExtDataStatus` via `pc_tclSensorMsgHandler`. GPS in -> map-match -> NavPosition out.
- PROCNAV spawns, takes its start-proc callback, then parks in
  `MonitorYield { deadline }` and only runs periodically; it never publishes a
  NavPosition because it never receives a `cmb_sensorfi` GPS fix.
- Nobody in the launched process set serves `cmb_sensorfi`: `procsds` is Speech
  (TTS/Recognizer), and `prochmi`/`procsmartphone` are FI *clients* (they merely
  statically link the same FI lib). "cmb" = combined sensor (GPS + gyro + vehicle
  speed for dead-reckoning). The GPS sensor server is either a Triton-side
  component or a Linux process outside our launch set; no `/dev/gps|nmea|gpt` node
  exists in the rootfs (created at runtime).

So the faithful fix (Option A, chosen) is to establish a position source:

1. Find how the combined-sensor `cmb_sensorfi` GPS property is provided and
   delivered to PROCNAV: the service/function id PROCNAV registers for
   (`cmb_sensorfi` RelUpReg), and whether the provider is a Triton service (svc #6)
   or a process to spawn + feed a GPS device.
2. Deliver a GPS fix for Krakow (~50.0614 N, 19.9456 E; grid unit = deg * 11.93e6)
   through that channel so PROCNAV map-matches and publishes NavPosition, which
   then flows to procmapengine's CCP and makes the area requests resolve.
3. If PROCNAV is parked *before* it even subscribes to the sensor service, resolve
   the earlier SPM/licence/registry bring-up first (see previous section) - GPS
   will not be consumed until PROCNAV reaches its main loop that registers the
   sensor handler.

Cheaper but non-faithful alternative (rejected for now): have the emulator answer
the `posfi`/`statfi` CurrentPosition/NavPosition service directly with Krakow
(infrastructure partly in `dapi.rs`). Keeps the GUI moving but the position is a
fabrication rather than something PROCNAV computes from a GPS feed.

### Addendum: the GPS/CCP path is gated behind the un-implemented 0x64 overlay

Before GPS can even be consumed, PROCNAV must reach its main loop that registers
the `posfi` NavPosition service and subscribes the `cmb_sensorfi` GPS property.
r168 shows PROCNAV (thread [18]) never gets there: it loops on Triton monitor
service `0x1b0464` - **object `0x64`**, the OSAL-util overlay (`sysprogosal`,
which implements OSAL message queues / IOSC / the ail-IPC substrate) - and the
monitor answers every call with `completed without a result` (catch-all), mixed
with timed `0x13`/`0x0b` bank waits (a bring-up retry cadence, e.g.
`0x10200113` = object 0x13, 4128 ticks). Because 0x64 is a no-op catch-all,
PROCNAV's OSAL message-queue operations silently do nothing, so it never
establishes the IPC it needs (service register, property subscribe) and never
publishes a NavPosition.

So the real root of the residual `0x305`, and of PROCNAV's silence in general, is
the **missing 0x64 (and 0x6e) OSAL-util overlay**, not GPS and not the queue
name-mapping. GPS injection is downstream and moot until the 0x64 overlay is
emulated (or the specific OSAL operations PROCNAV drives through 0x64 are given
real backing). The recovered `sysprogosal` overlay (blob0) now decodes in Ghidra
(1319 functions once analysis is triggered - see
`docs/reverse_engineering/triton_mid_swi_services.md`), but it has no named
symbols and 0x64's command switch has not yet been mapped.

Empirical 0x64 op catalog (r168, PROCNAV thread [18]). Dispatch key is
`cmd = service >> 16` (the `[15:8]` byte is a sub-arg, low byte `0x64` is the
object); every one is currently answered by the catch-all with r0=0:

| service | cmd | sub | x | request-block flavour |
|---|---|---|---|---|
| `0x1b0464` | 0x1b | 0x04 | 53 | named-object work; `w0->"wdb_safecnt_00"` (watchdog safe counters) |
| `0x30164`  | 0x03 | 0x01 | 49 | `w4->"ALWAYS"` - message-route/condition |
| `0x430064` | 0x43 | 0x00 | 19 | block reads all-zero at the completion point |
| `0xa0064`  | 0x0a | 0x00 | 12 | all-zero |
| `0x160064` | 0x16 | 0x00 | 10 | all-zero |
| `0x170264` | 0x17 | 0x02 |  9 | pointers + two sizes (`0x610`,`0x698`) - a copy/pack |
| `0x200164` | 0x20 | 0x01 |  4 | ASCII path/name (`"122f764"`, `"22f764"`) - named lookup |
| `0x210264` | 0x21 | 0x02 |  2 | `(ptr, 0x7d0, ...)` with a `-1` sentinel |

So 0x64 keys on `cmd = r1>>16` and PROCNAV alone drives cmds up to `0x43`, i.e.
the OSAL message/pool/event/sem/IOCTL surface (~68 arms), not one call. Faithful
support means decoding `sysprogosal`'s per-cmd contract (via its dense OSAL log
format strings - see the RE doc; the immediate `100`/`0x64` is the log channel,
not the object id) and implementing each in `complete()` (like
`SERVICE_CREATE_POOL`/`SERVICE_MBK_*` already do), driven by the request block and
writing a real result before the continuation runs. This is
necessary-but-not-sufficient: after 0x64 works, PROCNAV still needs the
`cmb_sensorfi` GPS source to publish NavPosition.

**Update**: the 0x64 command surface is now decoded - see
`docs/reverse_engineering/triton_mid_swi_services.md` "Object 0x64 fully decoded".
The prime unlock is **cmd `0x1b` = `OSAL_s32SemaphoreCreate`** (PROCNAV's most
frequent 0x64 op): the catch-all returns 0 but never writes the created handle
back into the request block, so PROCNAV stalls on a null semaphore before it can
register `posfi` / subscribe the sensor. Narrow first implementation: back a
semaphore object + fill `block[1]` with its handle in `complete()`.

**Implemented & validated (r169, `complete_osal_sem_create`)**: cmd `0x1b` now
returns a real handle (a `[monitor-osal-sem]` control block mirroring
`FUN_00036734`'s layout: type/initial/max/state/name) written into `block[1]`,
resolved through a process-global `name -> handle` registry so a named semaphore
is shared across processes (OSAL named objects are process-global). r169 vs r168:
53 creates served, `0x305` unchanged (2 -> 2), 0 exec errors / panics / exits, and
PROCNAV advances into its semaphore post/release path (cmd `0x20` 4 -> 14, `0x21`
2 -> 12).

**All PROCNAV-hit 0x64 commands served faithfully (r170-r172).** The earlier
prediction that the remaining 0x64 commands were "no-ops / regression risk" was
**wrong**: answering each to its decoded contract (per-task context set/get
`0x03`/`0x02`, dense per-task id `0x16`, registry lookup `0x17` -> not-found for
`id < 1`, status 0 for trace/IOSC/flag/post/release, and the whole **OSAL Event**
family PROCNAV then reaches - `0x31` EventCreate via `complete_osal_event_create`,
`0x33`/`0x34` delete/close, `0x48` the stub that returns 3) pushed PROCNAV *past*
the service-registration churn, not into a regression. r170 vs r169 and r171/r172
vs r170: `0x305` unchanged (2), exec-fault 5, 0 panics / 0 exits / 0 hook failures
throughout, and PROCNAV advanced through previously-unreached 0x64 commands.

**0x64 is fully cleared; PROCNAV now parks on a real data wait.** PROCNAV now runs
its init to completion and stops yielding (last monitor activity near the tail),
going quiet on a **blocking take of object `0x13`** (continuations `0x5e9be8` /
`0x5e9bfc` / `0x5e9c44`, "4130 ticks"). That is the correct idle-on-data-wait
state, not a stall: nothing ever posts that semaphore because no GPS/CCP producer
runs. Object `0x6e` (`0x1026e` / `0x2016e`, "SPM_LCN2KAI_DownloadWorkaround") and
`0x13` are periodic/startup OSAL traffic PROCNAV continues past - not read-back
gates. A secondary gate: DAPI's `DeviceManager subscribe-event8` returns `r0 =
0xffff` (failure) - see correctness.md.

The real next blocker is therefore a **source/trigger**, not a command: PROCNAV
never receives the combined-sensor GPS (`cmb_sensorfi`) so it never map-matches or
publishes `NavPosition` / feeds the CCP. Next work item = a faithful `cmb_sensorfi`
GPS source + the `posfi` registration trigger, and/or unblocking the DAPI
`subscribe-event8` `0xffff` path (both still "necessary, not sufficient" - see the
DYN4MBX bridge and this chain).

**Object `0x6e` is the OSAL IO service and is now served (r174/r175).** The
earlier note above called `0x1026e`/`0x2016e` "periodic/startup OSAL traffic
PROCNAV continues past" - true only in the sense that PROCNAV survived it: the
catch-all's handle `0` made every client-side `OSAL_s32IOControl` fail its magic
check *before* the svc, so PROCNAV's registry traffic never reached the monitor
at all and the SCD lookup fell back to mode 0 (static map, which needs registry
enumeration the catch-all cannot answer). The wrappers/clients are now fully
decoded (see `docs/reverse_engineering/triton_mid_swi_services.md` "Object 0x6e
fully decoded") and `complete_osal_io` serves the object: opens of
`/dev/registry/...` hand back real handle objects (magic `0xD0E1D0E1`, path at
`+0x20`), registry query/set run against the shared registry (`MAXPOOLSIZE` =
`0xFA000` resolves), the `0xa`/`0xb` probes pass and PROCNAV **reaches SCD mode
1**, with the `0xb` service->app lookup wired to the registry (`SERVICEID` ->
`APPID`, service `0xb` -> app `10041`). r175 vs r174: no crashes, GPU/quantum
volume unchanged, PROCNAV still parks on the same object `0x13` data wait.
Remaining gaps on this object are *data*, not ABI: the `PROCESS\NAVAPP` and
`FC_*/APP_CONFIG` keys live in the encrypted region files (`ni2k_*/NAVAPP.ERG`)
and are absent from the emulator registry (the client handles the failed opens
with defaults, exactly as on a unit without those keys), and the `0xb` lookup
only fires once something drives PROCNAV's commcon - the `cmb_sensorfi` GPS
provider (next work item) is still the trigger.

### Sensorfi provider slice - recon notes (r175, post-0x6e)

With `0x6e` served, PROCNAV's boot-time traffic was re-catalogued (r175, 45 s):
every one of the 51 `0x5036e` ioctls is answered (42 trace class-mask `0x3`,
7 probes, 1 `MAXPOOLSIZE` query, 1 probe-resolution) - there are **no unknown
registry ioctls left**. The SCD `0xb` service lookup is *lazy*: nothing calls
`scd_bGetAppIdFromServiceId` during bring-up, and within PROCNAV's static image
there are no callers of the `scd_*` API at all (only `.dynsym` exports - the
runtime binder that calls `scd_init` lives outside the analyzed text, reached
through symbol resolution like the rest of the component framework). So the GPS
unlock is not an ioctl gap either: after init PROCNAV idles in its DS message
pump - waits on object `0x13` (`0x10200113`/`0x10220213`/`0x10030313`, ~4130
tick cadence) backed by the bank object the `0x0203020a` resolve handed out
(`[monitor-mbk]` page, currently truthful-empty), plus the `0x200164`/`0x210264`
queue get/put churn around it. It publishes NavPosition only once a message
arrives there.

Concrete plan for the provider (next slice):

1. Ground the bank wire format: `0x5f2ab4` -> `0x5f5af0` walk over the bank
   object (`+0x1c` = byte count, `+0x2c` = u16 element count), the element
   header, and what the reader consumes per element; that page is the injection
   point the emulator already controls.
2. Ground the real *post* path: find the sender-side tkse service (object `0x13`
   / `0x0a` send or post ids) in the Linux processes' libtkse wrappers - if any
   launched process ever posts to a Triton bank, the emulator learns the exact
   message framing for free; otherwise frame it from the reader side.
3. Ground the payload: locate `pc_tclSensorMsgHandler` (via the
   `cmb_sensorfi_tclMsgGpsDataStatus` / `...GpsTightCouplingDataStatus` strings)
   and extract the ServiceID/FunctionID + struct layout of the GPS status
   message PROCNAV must receive to drive map-matching -> `posfi` NavPosition.
4. Implement `src/rtos/sensorfi.rs` (gated `EMU_SENSORFI=1`): after PROCNAV's DS
   registration, post a Krakow fix (~50.0614 N, 19.9456 E, grid unit =
   deg * 11.93e6) into its bank and wake the parked `0x13` wait, mimicking app
   `10041` (PROCMW/SENSOR). Also expect the `0x64` cmd `0x17` app-id registry
   lookup (currently "not modelled -> not found" for id 1) to be asked for app
   `10041` once commcon resolves the publisher - serve it from the registry.

#### Bank object wire format (decoded from PROCNAV `0x5f5af0`, r2 = the page the
`0x0203020a` resolve hands out through `block + 4`, stored by PROCNAV at `obj->0x24`):

```
struct bank {            // the [monitor-mbk] page
    u32 slots[];         // +0x00: fixed-size slots, stride 0x20, one per message;
                         // slot[0] = class/magic word, valid == 0x70000001;
                         // slot[+8]  = u32 (offset/length into the data array)
    u32 data_off;        // +0x1c: offset from the bank pointer to the parallel
                         // data array (data[i] at bank + data_off + 0*i / +0x14*i)
    u32 size;            // +0x28: span used for range checks (see 0x5f5b64)
    u16 count;           // +0x2c: number of entries the walk iterates
};
```

`0x5f5af0` returns 0 when `count == 0` (the truthful-empty page the emulator hands
out today - hence PROCNAV's silent park), otherwise walks `count` 32-byte slots,
matches `slot[0] == 0x70000001` and accumulates `slot[+8] + obj->0x3c + data[i]`.
So the emulator-side GPS injection point is confirmed: fill one slot with the
`cmb_sensorfi` class word + payload into the data array + set `count`, then let
the parked `0x13` wait complete. Open sub-questions: what class id `0x70000001`
is (the matched reader looks class-specialised - find the dispatcher and the
`cmb_sensorfi` class id near the `pc_tclSensorMsgHandler` strings), whether the
slot stride/magic hold for posted-but-unconsumed frames written by a sender
(the real post path fills this from the other side - look for the sender-side
tkse wrapper services on object `0x13`/`0x0a` in the launched processes), and
the meaning of `obj->0x3c` (a per-object base added to every hit).

#### Correction: object `0x13` is the TSE object family, and the PROCNAV park is
*idle*, not a bank wait (r175 disassembly of the wrapper clusters)

The service literals ending `0x13` resolve to named TSE wrappers, and the
`>> 16` field the yield model reads as "ticks" is actually the **command**:

| service | wrapper | block | notes |
|---|---|---|---|
| `0x10000113` | `tkse_cre_sem` `0x6233c0` | `[0]=&semid` | PROCNAV creates `wdb_safecnt_21` this way (catch-all returns sem id `0`) |
| `0x10010113` | `tkse_del_sem` `0x623408` | `[0]=sem` | |
| `0x10020213` | `tkse_sig_sem` `0x623428` | `[0]=sem` | |
| `0x10030313` | `tkse_wai_sem` `0x623450` | `[0]=sem, [4]=?, [8]=timeout` | the *last* PROCNAV wait: sem `0` (= its own `CreateOsalLock` result), timeout `0xffffffff`, continuation `0x5e9b60` = `LockOsal` inside the OSAL **async-IO** job-queue code (`0x5e99xx-0x5e9cxx`, `nFindJobQueue`/`OSAL_bInitAsyncIO`) |
| `0x10320413` | `tkse_snd_mbf` `0x6233e0` | `[4..]=msg,len,...` | mailbox-full send = the physical FI/OSAL queue post |
| `0x10200113`/`0x10210113`/`0x10220213`/`0x10240513` | `tkse_cre_flg`/`del_flg`/`set_flg`/`wai_flg` `0x5f5220-0x5f5288` | `set_flg [0]=flg,[4]=pat`; `wai_flg [0]=flg,[1]=pat,[2]=cyc,[3]=mode,[4]=to` | flags; `set_flg` was PROCNAV's final monitor call |

Re-reading the tail of r175 with this table: PROCNAV's last acts are event
`NAVEND` create + delete/close (`0x31/0x33/0x34` on 0x64 - normal bring-up), a
`/dev/trace` close (`0x2016e`), then the async-IO worker's `LockOsal`/`UnlockOsal`
- and then *silence*: no svc #6, no LX monitor calls, no exits (heap telemetry
runs to the end). It is not parked on a bank and not blocked on data - the main
threads are idle/sleeping while the commcon member registration that would call
`scd_bGetAppIdFromServiceId` (and eventually `tkse_snd_mbf`) has simply not been
started. Note `yield_thread` misreads `service >> 16` as a tick timeout for
these services (it is the cmd), so every TSE call gets a spurious ~4 s deadline
park - harmless today but it must not be extended to real sem/flag semantics
until the decode uses `(cmd, block)` instead.

Next-step order follows from this: (a) model object `0x13` per-(cmd, block) -
named sems/flags with real wait/sig, replacing the deadline-park for the TSE
cmds; (b) find what is supposed to trigger PROCNAV's commcon member registration
for service `0xb` (the DAPI `subscribe-event8` `0xffff` failure in
correctness.md is the prime suspect); (c) only then does the bank/`snd_mbf`
injection of a GPS `cmb_sensorfi` status (service `0xb`, func `5`, opcode `6`,
TypeId `0x00050600`, payload `cmb_fi_tcl_GpsBigdata`) have a receiver.

#### The sensorfi stack is runtime-bound; with one PROCNAV thread it never starts (r175)

`pc_tclSensorCommunications`/`pc_tclSensorMsgHandler` (and the whole `scd_*` API)
are only referenced through `.dynsym`/EXTERNAL - the position/sensor ("pc")
layer is bound at runtime by the component framework, not called directly from
PROCNAV's static text, and there is no component DLL tree under
`CRYPTNAV/DNL/BIN/NAV/COMMON` (only `PROCNAV.OUT`, `DAPIAPP.OUT`,
`PROCDLSAVER.OUT`). r175 proves the binder never ran: no yield has a
continuation inside `scd_bGetAppIdFromServiceId` (`0x34f012..0x34f0c2`) or any
`pc_*` ctor region, and the scheduler's park dump shows PROCNAV with exactly
**one** thread (`threads: :blocked(MonitorYield { deadline })`) while its code
clearly builds an async-IO job-queue worker (`OSAL_bInitAsyncIO` + `LockOsal`
sem, `s32PrepAsyncTask`), a DR/position pipeline and CCP pumps - threads that
would exist only after the component bring-up. So the sensorfi unlock is NOT a
message-injection problem yet: nothing has created the receiver.

Two live leads for who runs the binder (tracked in correctness.md next):

1. `s32PrepAsyncTask` (`0x5e9af8`) calls `CreateOsalLock` + `OSAL_bInitAsyncIO`
   - i.e. the async-IO/OSAL task-creation surface (OSAL_s32TaskCreate on the
   monitor or via `tkse`/IOSC svc-0x26). The DAPI-side `svc-0x26` service
   registry (`dapi.rs`, `MAP_MEDIUM_ACTIVE`) is emulated - PROCNAV's native
   equivalent may be the same IOSC that must tell PROCNAV "system ready", and
   the DAPI `subscribe-event8` `0xffff` gate (target = PRM app `0x0a` in the
   unlaunched `procbaselx`) is the visible symptom of the same hole. Spawning
   `procbaselx` (or emulating its PRM announcements on IOSC) is therefore the
   first thing to try.
2. `PROCDLSAVER.OUT` / `procbaselx` are in the firmware launch set but not in
   our `EMU_RTOS_START`; the real startup ordering (StartConfig*.reg) may put
   the binder before PROCNAV's consumer threads are created.

### PROCNAV FC bring-up completes: component threads live, main loop at NAVEND (r191-r204)

The registration-phase gating is over. With SCD pinned to mode 0 (refusing the
`0xa`/`0xb` lookup ioctls with `-1` + last-error `0x72011`), the subkey-enumeration
ioctl `0x3eb` served, and the NAVAPP app family fabricated in `registry_compat.reg`
(APPID / MAXMCOUNT / MAXMSIZE / AIF_CONFIG / APP_CONFIG / SERVICES / APP_THREAD +
`FC_LBS\APP_CONFIG\ON_TENGINE`), every FC component passes its startup gate:
NAVI/STAT/TEA/APP03(TIMA)/WDB `bInitInstance` ok, `scd_OpenQueue` success per app,
AE/AB worker threads created and parked on their message receives, TIMA fully up,
and `vStartApp` idling at the NAVEND event wait (OSAL `0x35`, parked). Two fixes
outside the registry made it work: the OSAL `0x1c` named-semaphore open (lazy
`AIL_TRACE_SEMAPHORE`) and the scheduler THUMB-mode bug (`uc_emu_start` forces ARM
on an even begin, killing every odd-entry FC worker thread at its first
instruction). Full chain documented in
`docs/reverse_engineering/triton_mid_swi_services.md` ("PROCNAV FC bring-up reaches
its main loop").

Consequences for the open threads here:

- The "sensorfi stack is runtime-bound; binder never ran" analysis above is
  superseded: the component binder *did* run (AE threads exist); the sensorfi
  receiver threads are now reachable. GPS injection (`EMU_SENSORFI=1`,
  `src/rtos/sensorfi.rs`) is unblocked - next milestone is verifying NavPosition
  publish and the CCP/`0x305` effect with a Krakow-fixed position.
- FC_LBS is deferred by `ON_TENGINE=0`; if LBS proves needed for a feature, the
  value flips to 1 and its `APP_CONFIG` tree will surface real requirements
  (iterate like AIF_CONFIG did).
- Unknown `APP_THREAD\<name>` misses are benign only where the caller tolerates
  them (TIMA defaults); a component that requires its named thread fails
  `bInitInstance` with "failed to find key <NAME>_PRIO" - add the pair to the
  compat file when seen.

## Narrow-range code hooks silently never fire (unicorn-engine 2.1.5)
- **Status:** worked around (r377/r378, `xp-watch` in `src/libs/procnav.rs`)
- **Symptom:** observation probes added at `0x2ebd6c`, `0x297f54`, `0x2981e6`,
  `0x2989c4`, `0x2c4780`, `0x30b98a` (2-4 byte ranges) registered `Ok` but got
  zero hits for the whole run, while the code demonstrably executed (yield
  continuations in the monitor log named `0x2989c4` 2180 times).
- **Isolation (r372-r376):** a *page-range* hook over the same pages fired for
  every fetch of the exact addresses (`TARGET ... fetched`); swapping the
  process start order changed nothing; all registrations reported success and
  share one closure type, so the failure is inside the engine, not the hook
  list or the guest.
- **Engine internals:** unicorn 2.x decides hook dispatch at *translation*
  time (`accel/tcg/translator.c` `HOOK_EXISTS_BOUNDED` per instruction,
  `translator.c:151` records only TB-granular regions). A narrow range that
  only overlaps the middle of an already-translated large Thumb TB never gets
  its per-instruction check emitted, and nothing re-translates it later.
  The exact cache-history trigger for our runs was not pinned down further.
- **Workaround:** hook whole 4 KiB pages (`XP_PAGES`) and dispatch on the
  exact fetch address inside the callback (`XP_TARGETS`/`xp_watch`). Costs a
  table lookup per instruction inside five hot pages; budget counters keep
  the log bounded. Live narrow hooks elsewhere keep working - only convert
  probes that must see mid-TB addresses.

## Fabricated posfi NavPositionStatus: full claim+route chain works (r390-r424)
- **Status:** working slice (r424): `vHandleNavPosition` + `vProcessNavPositionStatus`
  execute once per fabricated message on the engine body thread (tid62).
- **Routing ground truth** (procmapengine.out, Ghidra VA == runtime VA):
  - `bDispatchCCAMessages` (0x66c8e8): class 0x45 + `param_2 == 0xffffffff`
    (body-thread pass) claims via `ail_bHandleMsgServiceData` (0x66c128).
  - op byte at content+0x1a selects the branch: op 6 => `bIsServerMessage`
    (0x386c60) TRUE => walk of the server-reference list at
    `*(app_iface+0x50)`; predicate `ail_tclServiceReference::bIsDataSet`
    (0x66e3d0, observed live at lr 0x66c580) compares the message tuple
    **(svc u16@+0x14, sub u16@+0x00, reg-id u16@+0x16, u16@+0x0e)** against
    reference `{+0 svc, +2 sub, +4 reg-id, +8 target-sub}`, msg value 0xffff
    is a per-slot wildcard.  A truncated 0x00ff in msg+0x0e silently kills
    the match (r420).
  - Claim tail: `(**(app_iface) + 0x20)(app_iface, msg)` =
    `map_tclPortControl_Platform::vOnNewMessage` (0x3a4048), which for class
    'E' + `bIsClientMessage` (0x386c08; TRUE for op in {0..5,0xd,0x10,0x11})
    FALSE looks up the client handler in a `bpstl::map<u16,handler*>` at
    `platform+0xa0` keyed by the message u16 at **content+0x00** via
    `operator[]` (0x39ef80 - a *missing* key inserts a null-valued node, so
    one wrong lookup poisons that key for the rest of the run).
  - bpstl::map node layout (read off operator[]): map+0 -> header node,
    header+4 = root, node+8 = left, node+0xc = right, node+0x10 = key u16,
    node+0x14 = value.
  - Handler registration (platform ctor 0x39f268): map keys **7 = DAPI,
    1 & 0x16 = Navi, 0x17 = WDB** (heap objects, stored at platform+0xb0/
    b4/b8).  The posfi server's sub-id is therefore 0x16.
  - Handler virtual +0x18 = `PortControl_CCAClientHandlerBase::
    vDispatchMessage` (0x592454): optional `vCheckPropertyRegistration` for
    handlers whose service vector matches svc, then a static (svc,fn) table
    `PortControl_CCAClientHandler_Navi::_msgMap` (0x713ffc, group at +4 ->
    0x713f14, 12-byte entries {svc u16, fn u16, fnptr u32, (off<<1|flag)
    u32}): **(0x1e, 0x10) -> vHandleNavPosition @ 0x5487b8**.  After the
    handler returns, `bDelete` frees the message.
  - `vHandleNavPosition` parses the wire into a `posfi_tclMsgNavPosition
    Status` and tail-calls the embedded `map_tclNavPropertyHandler`
    (this+0x44) `.vProcessNavPositionStatus` (0x59216c).
- **Fabrication recipe** (all host-side, no guest patching): link a 24-byte
  fwl_List node `{prev=0,next=0,item={svc 0x1e, sub 0x16, regid 0x17,
  state 0, target 0xffff...}}` into `*(0xa0fc70+0x50)` head (guest
  `fwl_List::vAdd` no-ops in hook context - its operator_new fails), and
  post messages into MQB_1024 with envelope sub=0x16, dst=0x400, src=1,
  class 0x45, svc 0x1e, regid 0x17, fn 0x10, op 6, +0x0c/+0x0e = 0xffff.
  Host posts must also raise the consumer's IOSC event
  (`iosc::signal_events`) because the two-stage wait
  (`OSAL_s32MessageQueueWait` -> `u32GetFromMessageQueue`) needs it; and the
  one-shot fixup in the CCA_DISPATCH hook must restore the engine's own
  body-queue handle into app+0x78 (that field is shared with the entry
  thread's forward target - the early repair seeds it with the entry queue).
- **Open:** lat/lon of the parsed `posfi_tclMsgNavPositionStatus` (probe
  object at 0x9020a8f4-style buffer) do not yet move `UpdateMapDataRequest`
  off the default window - parse offsets and the validity gate in
  vHandleNavPosition still need mapping (next step).
- **r425/r426 (wire layout + window gate):** the fi_tcl payload is a flat
  sequential encoding (no tags): `DataSetId u32`, `Longitude s32`,
  `Latitude s32` (degrees * 1e5), then 1-2-byte e8/e16 section fields read
  cursor-sequentially (all reads boundary-checked, `fi_tclInContext::
  u32ReadAt`).  A uniform non-zero filler byte is required behind the
  coordinates: the map-window gate in `vHandleNavPosition` is
  `PositionCalculationState != 0` (field value slot object+0x15c-ish), and a
  sparse dword pattern lands a 0x00 byte exactly there.  r426:
  `UpdateMapDataRequest(4, 0, 5.0, 0, lon±100000, lat±50000)` fires once
  with the Krakow rect on the body thread - the fabricated position now
  drives the engine's map window.  Open: the MDM job queue created by that
  call is still not consumed (no DapiGetBlockIDs for the Krakow job; the
  early default-window request=5 chain still ends in the DAPI 0x305).
- **r428-r432 (kick self-inflicted MDM murder + list watchdog):** the DAPI
  svc-0x26 and Navi kicks both ran inside the bSendCCARequest entry hook;
  `call_guest_function` saves the hijacked registers once (LR = restore
  stub), so queuing a second hijack while the first still runs corrupts
  both frames - the resumed GetBlockIDs job entered bSendCCARequest with
  r1=0 and the READ_UNMAPPED at 0x591b00 killed the map-data mainloop
  (tid63 exited(1)) in EVERY kick-era run (r410-r427).  Fix: at most one
  guest call per hook event (`!register_fired` gate) - tid63 now survives,
  the DAPI 0x26 service registers for real (service_state 0), and the
  request=5 chain runs 100+ times.  Second discovery: the engine's live
  svc-0x26 registration REBUILDS the app_iface+0x50 server-ref list and
  detaches the host-linked posfi node (claim silently dies); the
  handle_service_data hook now runs `ensure_posfi_ref_linked`, a watchdog
  that re-links the node at the tail whenever the forward walk misses it.
  r432: 12 claims + 12 vHandleNavPosition + the Krakow
  `UpdateMapDataRequest(4,0,5.0,0,...)` (trace=133 - earlier r0=5 floods
  hid it behind the 24-entry probe budget) + bbox flips to
  [0x1cdde1..0x1feb21]x[0x4ba05e..0x4d26fe].  Open: the request=4 job is
  queued (NOIOSCM event posts visible) but no PerformMapDataRequest
  request=4 / Krakow-rect DapiGetBlockIDs observed yet (request=5 probes
  saturate the budgets; needs request-type-filtered probes or the view-
  rect path - tid65 keeps re-requesting the default window).
- **r437-r442 (posfi request reaches PerformMapDataRequest; starvation found):**
  tracing the kernel mqueue layer (`MQ trace send/recv/deliver`, filtered to the
  `NOIOSC*` pointer queues) plus gate probes inside `PerformMapDataRequest`
  proves the fabricated position request now travels the whole engine chain:
  `UpdateMapDataRequest(4,0,5.0f,0,Krakow)` -> `CreateMDMJobMapDataRequest`
  (job prio=4 at +0x24) -> `AddJobToMapDataManagerQueue` -> `AddElement` ->
  `OSAL_s32MessageQueuePost(NOIOSCM, &jobptr, 4)` -> **kernel mq id 0x418**
  (msgsize 4, maxmsg 1000, blocking - nothing is ever dropped) -> the MDM
  mainloop's `EnqueueElements` blocks in `OSAL_s32MessageQueueWait` and the
  waiter-wakeup path (`finish_mq_wait`) hands the pointer over -> `GetElement`
  returns it (probe: `job=0x90ba1a70 job_type=0x45 posfi match`) -> `Execute`
  -> `PerformMapDataRequest` with `rect=[1cdde1 1feb21 4ba05e 4d26fe]`
  width=200000 height=100000 (both >= 1000 gates pass) -> **prio-4 branch at
  0x50cb48** (kind 4 = `ms_sMapDataReqCarsor`, Ghidra's `param_2 == 4`), which
  resets that static request (state +0x74=0, +0x1f4/+0x1f8 = 0xffffffff,
  MDMRefCounter +0x200=0), enlarges its rect by detail level and ends with
  `TriggerHandleMapDataRequest(false)` (kind 4 -> bVar4=false, "invisible").
  That queues a `map_tclJobMDMHandleMapDataRequestInvisibleData` job (type 0x6e,
  vtable 0x6a3e78, `Execute` 0x4b6510 -> `HandleMapDataRequest('\0')`), which
  runs - and then `GetHighestPriorityRequest()` (0x5107f4) returns the **kind=5
  AcCam view request** (0x78fcd4, data_type 1, state 1, sentinels 0xefffffff =
  mid-flight) instead of the freshly armed car-cursor request, because the AcCam
  static is scanned first and only counts as finished at state 7.  tid65 keeps
  re-arming the kind=5 request (its `UpdateMapDataRequest(5,...)` flood with the
  default window resets state to 0) and `GetBlockIDs` never completes it (the
  DAPI map-data service still answers the CCA request with an error), so the
  car-cursor request is starved forever.  Remaining blocker is therefore NOT in
  the job plumbing any more: it is `GetBlockIDs`/DAPI map-data provisioning (the
  0x305 `ServiceDataError` from the unanswered `DYN<n>MBX` service replies).
  Probe traps hit on the way (all "budget proved nothing"): the
  `PerformMapDataRequest` bl-site filter had an inner `count < 200` that hid the
  request=4 line, and the `GetElement` probe's 100-entry cap went blind at t~6s.
- **r444 (DAPI's GetBlockIDs answer finally claims and is parsed):** the map-data
  answer failed the AIL claim walk on its *unique id* slot.  `bIsDataSet`
  (0x66e3d0) is `param == 0xffff || this[slot] == param`, so the 0xffff wildcard
  exists only on the message side and reference +8 has to hold the peer's real
  unique id.  The async registration path never fills it:
  `bRegisterAsyncExecute` (0x660f74) sends ServiceRegister with zeros in its
  trailing words (log: `sub12=0xfffe sub14=0x0000`) and
  `vHandleAsyncRegisterConf` (0x665720) only flags the request entry plus calls
  back, so the reference kept unique id 0 while DAPI answers with
  `AIL_NO_UNIQUE_ID` (0xfffe) -> `tuple=(0x26,0x7,0x1,0xfffe)` vs
  `compared=[0x26,0x7,0x1,0x0]`.  `ensure_service_reference_unique_ids()`
  (run from the ServiceData claim hook, next to the posfi-ref watchdog, DAPI refs
  only) now yields `compared=[0x26,0x7,0x1,0xfffe]`, the answer routes through
  the handler map (key 0x7 -> 0xa07a78) into `vHandleGetBlockIDs` (0x53e7ac,
  opcode 8, mdm_ref 0x17) which queues a type-0x6b job the MDM mainloop runs
  (Execute 0x4b4028).  `ServiceDataError` dropped 2 -> 1 and `vOnNewMessage`
  fired for svc 0x26 for the first time.
- **Two findings this opens up:** (a) the answer is only 0x22 bytes, i.e. DAPI
  reports (almost) no blocks for the requested rect; (b) the request rect
  encoding is *not* 1e-5 degrees: the engine's own view request holds
  `[-990231045, -2039..., 506995542, ...]` = lon -83.0 / lat 42.5 in units of
  360/2^32 degrees (1 deg = 11930464.57), while the fabricated position put
  `0x1cdde1` (19.91809e5) into the same slot.  The posfi wire coordinates and
  thus the Krakow rect have to be re-encoded before DAPI can answer them.

## Why the map area in the GUI is black (r462-r465, 2026-10-13)

Two separate things were conflated here; only the first is real.

**The shared map surface works.** procmapengine renders into EGL surface
`0x5f4d4150` ("MAP", 800x480), the emulator reads it back into
`MAP_SURFACE_BYTES`, and `write_map_surface_to_guest` copies it into the guest
buffer that prochmi imports. The HMI-side probe confirms the bytes arrive:
`svg map surface probe base=0x908ea000 midpx=[cc, ce, d9, ff] row240_nonblack=800`.
(Read back through GL the same pixel is `[d9, ce, cc, ff]`, i.e. our upload is
swapping R/B relative to the engine's readback - a real but cosmetic bug.)

**What prochmi actually does** (r464, single occurrence at the emulated EA-hide):

    GUI_DM_EAManager::updateEAHide -> GUI_GL_LayerSync::copyLayer -> getLayers
      -> svg layer_by_name("MAP_View1") = 0x5f4c0001
      -> GUI_GL_LayerCopy::copy(0, 0x5f4c0001)
      -> svg layer_status / layer_error / resource_error
      -> GUI_GL_Texture(this, 0x320, 0x1e0, data=0x908ea000)   <- the map texture
      -> glTexImage2D(..., w=800, h=480, data=0x0)             <- allocation only

So the layer texture is allocated once with a NULL client pointer and never
re-filled, exactly as the old comment in `prochmi.rs` said. `refresh_texture_*`
tried to keep it current but read the texture name through
`manager+0x130 -> *layer_copy -> *texture`, which walks into the LayerCopy
*vtable*, i.e. yielded name 0, and the refresh returned early forever. The
engine's pixels therefore never reached any texture the HMI samples.

r465 fixes the identification: the map texture is recognised from the upload
whose source pointer is inside the shared map surface
(`note_map_layer_texture`), and every 30 frames `refresh_texture_from_map_surface`
now uploads the live surface into it. The probe read-back proves the texture
content is correct: `snapshot-readback tex=24 center=[d9, ce, cc, ff]`.

**Remaining blocker: prochmi never draws that texture.** The frame is still
black in the map region, and the region is *opaque* black (`glClear` colour
0,0,0,1), so no alpha-based compositing can reveal the map either. Two open
questions, in order:

1. Where does the black come from? It is drawn content, not the clear: with the
   clear alpha forced to 0 the frame still had `opaque=380800` and only 3200
   `alpha=0` pixels, arranged as a fine diagonal hatch. Either that hatch is the
   map window itself (dithered transparency), or the page draws an opaque
   background layer that on real hardware sits *above* the map layer.
2. What makes prochmi sample the layer texture at all? `GUI_GL_LayerCopy::copy`
   ran once (during the hide handshake); the per-frame draw path that would bind
   tex 0x18 and blend it has not been located. Needs `prochmi_out.out` in Ghidra
   (`GUI_GL_LayerCopy`, `GUI_GL_OpenGL::mixLayers`, `GUI_DM_EAManager`).

**Rejected approach, do not repeat:** treating pure black as a colour key
(punch-through) does make the map appear behind the widgets, but it also eats
every black label drawn over the map window ("FM1", "Starting navigation.",
"50 m") and looks terrible; limiting the key to flat black leaves a diagonal
hatch of dashes. Both variants were tried and reverted in r464/r465.

## The map is a display-engine layer, not a prochmi texture (r467, 2026-10-13)

`prochmi_out.out` is now in the Ghidra project and analysed; prochmi is ARM (not
Thumb) mode, and its Ghidra VAs equal runtime VAs.

The layer compositing model, verified by decompiling prochmi:

- prochmi's GL output is itself one svg layer (`OpenGL+0xc` is its own layer
  handle, driven by `OpenGL::setGuiLayerVisible` 0x01344598 and
  `OpenGL::ResizeAll` 0x013452f0 via `svgScaleLayer`/`svgEnableLayerSettings`).
  procmap's surface `0x5f4d4150`/layer `0x5f4c0001` ("MAP_View1") is a sibling
  layer *below* it, composited by the display engine - which here is the
  emulator.
- `GUI_GL_OpenGL::mixLayers` 0x01346bb0 clears the HMI layer to
  `GUI_Color32 0x00000000` (premultiplied alpha) so the map shows through the
  hole, and only draws the external layer itself in the hide-snapshot case
  (`if (param_2 != 0) GUI_GL_LayerCopy::draw(param_2)`, call site 0x01346d98,
  `GUI_GL_LayerCopy::draw` 0x01342a28). `getBackground` 0x0133f528 returns that
  param_2.
- The SHOW path that makes the map visible is
  `EAManager::requestEAShow` 0x0133eef4 → `updateEAShow` 0x0133ea84
  (state 0: `LayerSync::requestViewStatus(EA,2)` 0x01343b50; needs a VISIBLE ack
  from the peer, which comes back through
  `clGUIWidgetEngine::onGuiLSyncViewStatusChanged` 0x0133b7b8 →
  `LayerSync::onViewStatusChanged` 0x01343d14 → `EAManager::onViewStatusChanged`
  0x0133e9b8; state 2: `LayerSync::setLayersVisible(EA,1,rect)` 0x013436e4) →
  `svgSetLayerWindowData` 0x00fb64dc, `svgSetLayerOrder` 0x00fb6e30,
  `svgEnableLayer` 0x00fb6e84. `copyLayer`/`performCopy` are only the
  hide-snapshot path (callers: `updateEAHide` case 5, `requestDisplayModeChange`).

r467 measurement: **the SHOW path does run in the emulator** -
`requestViewStatus(ea=0, 2)` → our emulated VISIBLE ack → `onViewStatusChanged` →
`setLayersVisible(ea=0, visible=1, rect=0x38ef6f8)` →
`svgGetLayerByName("MAP_View1") = 0x5f4c0001`. `LayerCopy::draw` and
`LayerCopy::free` never fire (0 hits), i.e. `getBackground` returns 0 and no
snapshot is composited - which is correct, steady-state prochmi should not bind
that texture at all.

So the missing piece is entirely on our side of the svg ABI: `install_svg_map_
surface_hooks` (prochmi.rs) emulates only the *status* calls (by_name,
layer/surface status, errors, apply/wait) and bypasses them; `svgEnableLayer`,
`svgDisableLayer`, `svgSetLayerWindowData` and `svgSetLayerOrder` are not
intercepted, so the display engine is never told - and never emulates - that the
map layer is on and where. Next step: intercept those four (addresses above),
record per-layer `{enabled, x, y, w, h, order}` in the emulator's svg layer
model, and composite `MAP_SURFACE_BYTES` as the layer *below* the HMI layer for
that rect. Related: the HMI layer is cleared opaque (`glClearColor(0,0,0,1)`)
because `OpenGL+0x140` is non-zero, i.e. prochmi believes it owns an opaque
background; once our svg emulation answers its layer configuration calls without
error, the transparent-hole clear (`0x00000000`) is the branch that should run,
and then the map shows through the hole exactly like on the unit.

## The view rectangle comes from the camera parameters, not from the CCP (r472-r474)

`u32SetCameraParameters` (`0x523eac`, Thumb) does run - 60+ times, from
`lr=0x4d7518` on thread 65 (the engine's render/job thread), starting at
t~1.6 s - so the view is driven continuously without any RenFI traffic. Its
parameter block (r1) reads:

```text
  +0x00 u32 viewId                0x0 (second call site: 0x2)
  +0x04 u32 scale/meter           0x32 = 50  (matches the "50 m" scale bar)
  +0x08 u32 heading/flag          0xffffffff (second call site: 0xffff471c)
  +0x0c u32 pitch/tilt pair       0xb400b4 (two u16: 180,180)
  +0x10 u32 0
  +0x14 u32 count                 1 (second call site: 3)
  +0x18 s32 longitude             0xc4f45dfb  = -83.13 deg
  +0x1c s32 latitude              0x1e3b77c6  = +42.54 deg
```

The second call site carries no coordinates at all (`+0x18/+0x1c` = 0). The map
data request struct at `0x78fcd4` holds `[prio=5, type=1, scale=0x3f000000
(0.5f), rect.x1=0xc4f3257b, ...]`, i.e. the rectangle is derived from those
`(-83.13, 42.54)` coordinates - the engine's power-on position.

That is why the view never follows the car: `posfi_tclMsgExtrapolatedPositions`
*does* fill the CCP with Krakow (`0xe29fa46`, `0x23999b77`), but the camera
parameters handed to `u32SetCameraParameters` still carry the power-on
coordinates, so `UpdateMapDataRequest` keeps asking for a rectangle in Michigan
where the Europe-only card (`sd2-GENJ_EU_I.A v2`) can never have blocks, and DAPI
answers `ServiceDataError 0x305`.

RenFI experiment (r472): the fabricated `rendererfi` request (service `0x300`,
function `0x5a`, opcode 2, 31-byte payload) *is* accepted by the AIL layer -
`ail_bHandleMsgServiceData ... svc=0x300 fn=0x5a` and the body-pass dispatch both
show it - but it lands on the handler object the claim walk chose for
sub-id `0x16` (`handle=0x92eec000`, the same object that serves posfi), and
`vHandleRenFIMessages` `0x3a359c` (function entry, so a reliable hook) never
runs. `bIsClientMessage` `0x386c08` never runs either, so the request is dropped
before the RenFI dispatcher: the server-side registration/dispatch for service
`0x300` has to be found before this message can do anything.
