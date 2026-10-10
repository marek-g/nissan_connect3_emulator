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
