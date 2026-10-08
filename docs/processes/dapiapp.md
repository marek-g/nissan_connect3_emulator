# DAPIAPP.OUT

External map-data server. Serves map blocks (and other map media) over the
OSAL message queue `mbx_7` for clients that register against service id `0x26`
(`PORT_CCA_SERVICE_ID_DAPI`, `procmapengine` CCA client). Client requests a set
of block IDs via opcode `2`; the DAPI server answers with opcode `7` (result)
or `8` (error).

## Binary

Stock: `/var/opt/bosch/dynamic/processes/DAPIAPP.OUT` (16 MB ARM ELF, dynamic).
Symlinked to `/opt/bosch/processes/DAPIAPP.OUT`.

The emulator's firmware root mount (`/`) already contains this file at the
stock path, and the extra `/var/opt/bosch/dynamic/processes` overlay makes the
symlink resolve. `DAPIAPP.OUT` imports `libosal_linux_so.so` and `libtrace.so`
via PLT; the dynamic linker resolves them from `/opt/bosch/processes/` and
`/usr/lib/` respectively. `libasound.so.2` is not present on the SD card
either — the loader prints a series of `NoSuchFileOrDirectory` warnings for
each search-path variant, then continues without it (audio is optional).

## Launch

The RTOS boot service enqueues `StartProcessCommand` payloads on
`OSAL_CB_HDR_LI_MAIN`. `procbaselx_out.out` runs `vStartProc(path, pid)` for
each entry, which calls `OSAL_ProcessSpawn` and lands in
`emulator::process_launcher::spawn_process`. Add the DAPI process to
`EMU_RTOS_START`, or edit the default list in `src/rtos/boot.rs`:

```sh
EMU_RTOS_START=/opt/bosch/processes/procmapengine.out:\
/opt/bosch/processes/DAPIAPP.OUT:\
/opt/bosch/processes/prochmi_out.out
```

## Startup trace (observed 2026-10-08, `dapi_run3.log`)

The main thread (host tid 17 in the current run) executes:

1. `_start` → glibc init → `vStartApp` (0x8137e0).
2. `vStartApp` creates the process-control queue `OSAL_CB_HDR_LI_MAIN`,
   spawns `SIG_HDR3` and `CB_HDR1` callbacks, then calls
   `_Z28bLBS_OnlyInitForTEngineRh` and `s32InitApp07`.
3. `s32InitApp07` runs `scd_init`, `amt_bInit`, and `bOnInit`.
4. `dap_tclDapiApp::bOnInit` (0x8235a8) is entered by thread 29 after
   receiving `PWR_PROXY_START_CONF` on `mbx_7`. It walks
   `bGetConfigFromReg` → `bGetCommonConfig` → `bGetDeviceConfig` →
   `bGetRNWConfig` → `bGetTMCConfig` → `bGetMapConfig` →
   `bGetRegionSelectionConfig` → `bGetLisaConfig` and then ten calls to
   `bInitThread`, which spawns `DAPDATAS`, `DAPDATAM`, `DAPDEVM`, and
   `DYN1` … `DYN7`. Each `bInitThread` returns `r0 = 0x1` in the emulator,
   which is the success value.
5. `dap_tclDataServer::u16Init` runs after `bOnInit`. `bOnInit` itself
   returns `0x1`.
6. After `bOnInit` returns, `vStartApp` posts two semaphores and calls
   `vExitApp07` (Ghidra 0x813f10), which only tears down the trace context.
   The main thread then returns from `main`, glibc calls `exit_group`, and
   the process is torn down unless the emulator keeps the process alive on
   the strength of its other guest threads.

## Open issues

- **Process lifetime**: `vStartApp` returning does not mean the DAPI service
  shuts down on real hardware, because OSAL worker threads (`DAPDATAS`,
  `DAPDATAM`, `DAPDEVM`, `DYN1` … `DYN7`) live in a detached OSAL thread
  table and are not joined by the main thread. In the emulator the scheduler
  treats the process as done when *all* guest threads are
  `ThreadStatus::Exited` (`src/emulator/scheduler.rs::all_exited`), so the
  DAPI worker threads must actually be created as guest threads and stay
  runnable.
- `procmapengine` must actually call `DapiGetBlockIDs` so the DAPI queue
  gets traffic. In the current run it never enters that path; the map-data
  worker loop polls `job_type=0x83` (a different internal job type) and the
  DAPI-side call trace hooks are not fired.
- The synthetic `MAP PWR_STATE_CHANGE_REQ` periodic message injected in
  `src/libs/libosal_linux/message.rs` (`mbx_1024`) drives procmapengine's
  power state machine but does not advance it to a state where a DAPI
  request would be issued.