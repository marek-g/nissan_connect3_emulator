# Correctness Bugs

Bugs where the emulated guest behaves wrongly (wrong syscall semantics, data corruption, hangs).

## IOSC message-queue setup fails → `u32CreateMsgQueue` asserts (procvoice reboots)

- **Location:** libosal `OSAL_s32MessageQueueCreate` (+0x46c) → `u32CreateMsgQueue` (0x4850bc90, +0x60) → `u32MapErrorCodeIOSC` (0x48509070, +0xc4) → `OSAL_vAssertFunction`; the IOSC primitives live in `nissan_connect3_emulator/src/os/syscalls/iosc.rs`.
- **State:** `/dev/iosc` is emulated and SIGSEGV delivery now works (see below), so `procvoice_out.out` runs through early init and then faults in the message-queue path. The delivered SIGSEGV backtrace pinpoints it: an IOSC operation inside `u32CreateMsgQueue` returns a value that `u32MapErrorCodeIOSC` does not map to the success code (0x72000), tripping the assert → reboot. A null-deref in `OSAL_s32IOClose` (libosal+0x19FB4, reading address 0x5) is the immediate fault; it is downstream of that failed IOSC op.
- **Fix:** determine which IOSC primitive in `u32CreateMsgQueue` fails and why — either the `/dev/iosc` ioctl emulation returns a wrong value for that command, or the queue name/id handling differs from the real driver. Trace the actual ioctl(s) issued by `u32CreateMsgQueue` (enable ioctl trace logging) and align the emulator's response with the recovered protocol.

## Cross-process futex (no `FUTEX_PRIVATE_FLAG`) can stall a waiter

- **Location:** `nissan_connect3_emulator/src/os/syscalls/futex.rs` (`wake_waiters`), surfaced by the `futex without FUTEX_PRIVATE_FLAG not implemented` error log.
- **Problem:** with the parallel one-VM-per-process model, `wake_waiters` only marks threads in the *current* VM runnable. A `FUTEX_WAKE` issued by one process for a futex word whose waiters live in another process (the non-`PRIVATE`, shared-memory futex case) does not wake them; the waiter sleeps until its timeout. Same-process (pthread) futexes are unaffected. The guest logs `futex without FUTEX_PRIVATE_FLAG not implemented` when this path is hit.
- **Fix:** route `FUTEX_WAKE` on a non-private futex through the namespace (map the shared futex word to waiters across processes, e.g. back such futexes with a host-side word in shared memory and a namespace waiter set), mirroring the mq/iosc doorbell + owner re-check pattern.

## 0x305 is the DAPIAPP CCA map-data service (svc 0x26), not PROCNAV

Full-run trace (r166) settles the long-standing `DAPI ServiceDataError
code=0x305` question and overturns the PROCNAV-centric theory of earlier
sessions:

- PROCNAV (thread [18]) is healthy and never involved: it completes its startup
  registration once and runs a normal main loop; it names `mbx_0` zero times and
  performs zero message receives. It is not the nav responder.
- The road-network / map-data answer is DAPIAPP's own CCA **service 0x26**
  ("job type 4"), served by `dap_map_tclWorker` inside DAPIAPP. The registry
  gate and medium-up ordering pass (`MAP_MEDIUM_ACTIVE` fires; `svcdata-PATH
  r9=0x26` dispatches to handler `0x823998`; `enProcessJob result=0xffff` = the
  worker's success code; `u16SendResponse` sends the answer).
- Only ~2 map requests reach the worker per boot: `vSendToTask jobtype=4` count
  is 2 and `u16SendResponse` count is 2, while the client (thread [36]) waits on
  `DYN4MBX` ~10 times. The unanswered requests never become worker jobs; the
  map workers' `ActionList::bSetBlock` sees `capacity=0`. The client then times
  out and DAPI answers `0x305` (`amt_tclServiceDataError::vSetErrorData`,
  lr=0xb52520).

So the remaining blocker is upstream of the worker: why most svc-0x26 requests
are not turned into dispatched jobs. Prime suspects, both tied to worker/action
list initialisation and the medium-active ordering noted in `dapi.rs`:
- the client registry entry `0x9035ab94` staying at state 1 (not ACTIVE), which
  `dapi.rs` already flags as making later requests fail; and
- map-worker `ActionList` capacity left at 0.

Next concrete step is RE on `DAPIAPP.OUT` (already in Ghidra) at `lr=0xb52520`
(the 0x305 site) and at the `ActionList` capacity setter / worker init, to see
the exact branch that drops the request.

### End-to-end 0x305 chain and the real (already largely working) responder

Following the `0x305` value to its origin in the `DAPIAPP.OUT` Ghidra program:

- `amt_tclServiceDataError::vSetErrorData` at `0xb5251c` is fed the code in `r4`
  = `dap_tclJob::u16GetErrorCode` (`0xb54380`). That function does **not create**
  the code: it returns `*actionAddr` when `job+0x15 == 8` and the job has an
  action, else `0xffff` (success). So DAPIAPP only *forwards* a code some other
  party stored in the job's action block. `0x305` is that stored value.
- The jobs that carry it are `job+0x15 == 8`, produced by the map data path; the
  actual map responder is **procmapengine**, whose map-data mainloop queue
  (`procmapengine.rs`, `MAP_DATA_MAINLOOP_QUEUE_STATE`) is where `job_type=0x83`
  elements wait.
- In r166 procmapengine is no longer the blocker it was: the engine starts
  naturally (`natural map engine start result r0=1 state=2`,
  `MAP_ENGINE_NATURALLY_STARTED`), and the map-data path runs
  (`PerformMapDataRequest result r0=1` hundreds of times, `IntGetBlockIDs`,
  `SendCCARequest`, `SendServiceDataMsg`). The medium is ACTIVE at t=0.

Remaining gap, precisely: only ~2 requests reach DAPI's `dap_map_tclWorker`
(`vSendToTask jobtype=4` count 2) and 2 requests come back as `0x305`; the
client `DYN4MBX` waiter (thread [36]) times out on the rest. Two things to
investigate on the procmapengine side, in its map-data mainloop queue:
1. Why the map-data queue leaves `job_type=0x83` requests pending instead of
   draining and answering them (`wait GetElement` idle-wait loop, `queue_state`).
2. What writes `0x305` into a job's action for the type-8 jobs (a procmapengine
   failure result that DAPI then forwards), vs the type-4 map jobs that answer
   `0xffff` (success).

Correction to earlier sessions: the "PROCNAV road-network server / mbx_0" theory
was wrong. PROCNAV is healthy and uninvolved; nav/map data is procmapengine via
the DAPI CCA broker, and that path is now mostly functional - `0x305` is a small
residual, not a total outage.

### Update: the `DYN*MBX` client wait is now bridged (errors 7 -> 2)

Root cause of the "client `DYN4MBX` waiter times out" gap: `DYN<n>MBX` is an
IOSC-class OSAL queue (`queue_type == 3`). Its native `libosal` implementation
traps into the Triton monitor's OSAL-overlay object `0x64` via `svc #6`, which
the emulator serves as a catch-all - so a native `DYN*MBX` wait never receives
(r166: 13 waits, 0 deliveries, while posts pile up), and DAPI answers the outer
client with `0x305`.

Fix (`bridge_dyn_mbx_queue`, `message.rs`): name-gated `Post`/`Wait` bridge onto
the existing host `MqState`. `DYN*MBX` traffic stays within one process (DAPI's
own threads), so the OSAL message-reference pointer is valid in that VM and the
raw bytes are queued/returned unchanged (`materialize = false`).

Do NOT gate this on `queue_type == 3`: the `Dp*` datapool queues and `NOIOSC*`
queues are also IOSC but are handled correctly by native `libosal` in-process,
and intercepting those stalls bring-up (broad `type==3` gate made procbaselx loop
forever on `Dp00000001` and collapsed the run to ~676 lines). The narrow name
gate is what makes it safe.

Validated r168 vs r166: `ServiceDataError` 7 -> 2 (`0x502x4` and `0x501x1` gone,
both were `DYN*MBX` timeouts that now resolve), run length 1.03M -> 1.07M lines,
`DYN4MBX` 20 posts + 27 waits handled, 0 execution errors, 0 exits/panics. Map
flow counters unchanged. Remaining `0x305x2` is a *different, early-startup*
path: on thread [36] right after `amt::bAllocateMessage` ->
`OSAL_s32MessageCreate(pool, size=0x22)` (log lines ~11.5k/12.8k of 1M), before
the responder peer is up - a startup race, not the `DYN*MBX` wait.

### Update: 0x64 fully served; PROCNAV parks on the GPS/CCP wait; DAPI subscribe fails

With the whole OSAL `0x64` surface answered faithfully (see design.md r170-r172),
PROCNAV no longer stalls on service registration: it completes init and goes quiet
on a **blocking take of monitor object `0x13`**, i.e. it is correctly idle waiting
for a GPS/CCP producer that never posts. The CCP / `NavPosition` gap is therefore a
**missing data source** (`cmb_sensorfi` GPS feed + `posfi` trigger), not a monitor
command.

Secondary gate found on the DAPI side: `DAPIAPP.OUT` `DeviceManager subscribe-event8`
returns `r0 = 0xffff` (failure, e.g. `target_app = 0x94b1c490`), so DAPI's device
event subscription - a plausible part of the CCP-delivery chain - is not succeeding.
Investigate why the subscribe returns `0xffff` (target app / device not registered)
alongside the GPS-source work.

### Correction (r204): PROCNAV's idle point is the NAVEND main loop, with FC threads live

The r172 note above ("parks on a blocking take of monitor object 0x13") predates
the FC bring-up. As of r204 PROCNAV runs the full `vStartApp`: SCD static app map
(48 entries), NAVI/STAT/TEA/APP03/WDB instances initialised, AE/AB component
threads alive on their message receives, and the main thread parked at the NAVEND
event wait (OSAL `0x35` on object `0x64`) - the unit's normal idle. The GPS/CCP
data-source conclusion stands (nothing posts position), but the wait point and the
thread picture are different; see design.md's r191-r204 section and
`docs/reverse_engineering/triton_mid_swi_services.md` for the chain.

### r345: GPS status now delivered honestly to STAT; sink still unreached

The host GPS provider posts `GpsDataStatus` snapshots targeting app 22, the kernel
emulation routes them to `mbx_22`, and tid76 (STAT task) consumes them in
`bHandleMsgServiceData` with a **chain match** on svc=11/prop=5/opcode=6. So the wire,
the queue routing, the opcode gate and the component registrations are all correct now.

The remaining gap: a chain hit in the CCA router only destroys the message (the handler
is not called from this path, re-verified through `0x291954 -> 0x2b7f8c -> 0x30b21c`), and
the in-app dispatcher chain (`s32ProcessMessage` `0x4d3c38` -> dispatcher `0x2e5526` ->
sinks registered from the cmb class table `0xa38c68`) has zero call sites. The sink
`0x68ff8c` is a message-class method (typeId `0x764`) driven by the commcon/amt receive
pump, which PROCNAV never starts in the emulator. Next: find why the receive pump
(amt cmds `0x3f`/`0x55` on object `0x64`, `0x13` flag waits) never spins up per channel.

Side note (flakiness): run quality now hinges on the AE pump (tid77) reaching its queue
wait - in starved runs it idles in the `0x5036e`/`0x2016e` IO cycle and never consumes
`mbx_19`; the STAT-path delivery is unaffected by that (r345+), but old good-run checks
based on tid77 counters need to use `probe-sd-hit` on tid76 instead.

### r346: sink gap root cause = empty STAT subscription lists; ERG attack concluded

Twelve new observation probes (`probe-sd-refcmp/regcmp/refres/regres/dlvchk/deliver`,
`probe-regsvc`, `probe-scd`, `probe-msgreg`, `probe-msgconf` in `src/libs/procnav.rs`)
pin the root cause: STAT's ail object (tid76, `0x90069400`) has **both the
ServiceReference (`+0x50`) and ServiceRegistry (`+0x58`) lists empty**, and
`u16RegisterService` (`0x24a39a`) fires **zero times in the whole run** - nothing ever
subscribes to the GPS service, so every `GpsDataStatus` dies in the no-match branch of
`ail_bHandleMsgServiceData` (delivery would otherwise happen at `0xbd8b0` through the
app pump, see the corrected model in git history of this file). The old "HIT destroys
the message" note above is superseded: opcode 6-0xe server messages are matched against
the ServiceReference list via `bIsDataSet` (`0x24ea04`); wildcards only for 0xffff
fields.

Why the subscription never starts: PROCNAV's scd lookups (`PROCESS/NAVAPP/FC_STAT`,
`FC_TEA`, `NAVAPP`, `WDB`, `APP03`, `SENSORFI`, `DATA_SERVER` + `SERVICES/SERVICEID`,
`APP_CONFIG`, `AIF_CONFIG`) all miss because that subtree ships in the **encrypted
region files** (`ni2k_eur/NAVAPP.ERG`), which the emulator cannot decrypt - see
`docs/reverse_engineering/erg_registry_crypto.md` for the format, the
`KDS(entry 0x1216/0x1217) XOR mask` key chain and why the key is unobtainable from our
dumps. Same log also floods `PROCESS/CONFIG/PROCNAVI/SERVICES` misses: PROCNAV resolves
service -> app through scd (`scd_bGetAppIdFromServiceId` `0x34f012`, observed
`svc=0xb/0x44/0x1e/...`), so those SERVICES keys are on the critical path too.

Next step (registry path, no ERG needed): extend `data/registry_compat.reg` with the
FC_STAT/FC_TEA/NAVAPP/WDB/APP03/SENSORFI/DATA_SERVER blocks (layout proven from
procmw.reg: `APPID`, `AIF_CONFIG`, `APP_CONFIG`, `APP_THREAD`,
`SERVICES/<block>/SERVICEID`) and load `StartConfig1/2/3.reg` (ADR3 variants of the
process table, currently not in `DEFAULT_REGISTRY_FILES`).

## Fabricated GPS blob kills the STAT pump (null-store fault at 0x5a43f0)

Symptom (run r348, `EMU_SENSORFI=1`): STAT adapter thread (tid 76, app 22)
processes one provider message, releases the Bosch-Mailbox lock
(`tkse_set_flg`, svc `0x1022xxxx`) and dies immediately:
`Execution error: WRITE_UNMAPPED @ 0x5a43f0` + `exited(1)` (fault-kill, per the
thread.rs FIXME the bare-fault path always reports code 1). TEA (tid 77) dies
the same way; the remaining PROCNAV threads park fine. Without the provider
(run r349) **all PROCNAV threads stay alive** (76-82 blocked in mbx/TSE waits),
so the fault is caused by our 0x44-byte `gps_status_message()` blob, not by
emulator code: a Thumb copy helper (`ldrb/cmp/beq; ldr r0,[r7]; strb r6,[r0]`)
dereferences a NULL result of the amt field-getter chain (`probe-check-getter:
r0=0x109 own=None` just before the crash) - the real
`cmb_sensorfi_tclMsgGpsDataStatus` (>0x100 B, var-data vector @ +0x78) is
required. Fix in the provider (data), not in the guest.

## navbin mount silently empty after /tmp cleanup (run r347, fixed)

`/opt/bosch/navbin` was served from `/tmp/opencode/nissan_emu/navbin`; once
/tmp was wiped, `vStartProc(PROCNAV.OUT)` logged success but no thread ever
started (only 4 "Start program" lines). The mount is now repo-local
(`nissan_connect3_emulator/data/navbin`, override `EMU_NAVBIN_DIR`, binary
gitignored, regenerate with `tools/extract_procnav.sh`) and `main()` logs an
ERROR at startup when `PROCNAV.OUT` is missing.

## GPS messages delivered into real STAT handlers (runs r380-r389)

Two root causes killed every STAT_TASK1 message before dispatch (both fixed):

1. `s32InterpretAndCompareMessage` (0x297f54) switches on `u8GetType()` =
   `u8GetValue(0xb)` (wire byte +0xb) over classes 0x40..0x45; out of range ->
   "Unknown type of message" (RDE 0x43001c) + "error copying" downstream.
2. `bEnableAccess` (0x30b27e) resolves the received OSAL reference through
   service 0x57. The queue model hands the waiter an 8-byte reference
   `{1, content}` in its pool slot, so 0x57 must dereference one `{1, inner}`
   level (real OSAL resolves token -> data); answering the slot itself made
   the class byte read as 0 (monitor.rs, r380: 546 -> 0 unknown-type errors).

Dispatch ground truth (`rtf::tclMessageDispatcher`, `pfFindMessageMap`
0x520f34): per-thread `rtf::trMessageMap` list, 16-byte entries
`{u8 class, u16 svc, u16 fn, u8 op, handler, flags}` probed with
`(0x45, svc, fn, op)` extracted from the message. `STAT_tclThread::
vInitMessageMap` (0x11d08a) consumes **sensorfi as service 0x0a**:
fn 0x8001 op 6/8 (`s32OnUsedDataSetStatus`/`s32OnUsedDataSetError`,
`u16GetValue(0x18) == 0x8001` confirms the wire fn slot at +0x18), fn 0x201
(method result), plus a class-0x44 svc-0x0a wildcard (`s32OnServiceStateNavi`).
Nothing keys on the earlier guessed 0xb/5. Re-keying the fabricated envelope
(+0x14 = 0x000a, +0x16/+0x18 = 0x8001) + ServiceReference node + registry
SERVICEID made `s32SearchAndCallFunction` hit (r389): STAT handler executes
16x (`xp-stat-dataset`, lr 0x4d380e), "MsgDisp: unknown message" gone.

Remaining gap to the position computer (0x68ff8c `pc_tclSensorMsgHandler::
s32OnStatusMsg`): the consumer is `pc_tclPropertyHandlerTemplate<
cmb_sensorfi_tclMsgGpsDataStatus, h>` (client-side, ctor 0x596170) built by
`pc_tclSensorCommunications` (ctor 0x25b5d8) - **all never executed** in
PROCNAV (0 hits, r387); the owning component (position-computer app task,
likely the never-started FC_01/NAVAPP-main cluster: no `s32InitApp01`/
`s32InitAppSTAT` fetches either, tasks self-bootstrap via vEntry) needs
bring-up. Related observations: TEA_TASK1 (tid82) never pumps - its
`NaviThread::vEntry` (0x29e05c) skips the run loop because its message
manager (+0x40) is NULL (its app object is not an rtf::tclCommThread:
`rtf::tclCommThread::s32Initialize` branch probes ct-open/ct-msgman fire only
on tid76/77); the two tolerated startup `0x430010` vEntry nullpointer errors
(tid81/82) belong to these half-initialized apps.

## procbaselx `exit()` at t~56 s broke the HMI compositing (fixed r753, regression from the trap migration)

- **Symptom:** `******* process "my_program" (PID 1) terminated *******`
  (argv0 of `procbaselx_out.out`) at guest t~56 s on every boot, and the
  user-visible HMI compositing degraded right after (prochmi `eglSwapBuffers`
  dropped from 665 to 22 for the rest of a 340 s run, fix22).
- **Root cause:** `s32RunEnv()` (procbaselx `0xad13c`) runs the environment
  by waiting `OSAL_s32EventWait(SpmEvEnd, 1, mode 1, timeout 0xffffffff)` -
  "run until the SPM master says shutdown".  `libs/libosal_linux/event.rs`
  implements that wait host-side and parks the thread with
  `SleepUntil(now+60 s)`; the trap is an *observer*, so on wake the library
  body continued **mid-function** (past the svc-patched first instruction,
  with handler-written state like `IN_WAIT=1`) and answered itself err 0xd02
  + `-1` immediately; three answers reached `main()` and procbaselx exited
  cleanly.  The golden-era narrow `add_code_hook` handed control back at the
  *function entry*: the body then re-ran its own prologue and parked on its
  monitor yield forever (golden `g.txt`: `term=0`, procbaselx alive; its 891
  background d02s came from other threads and were survivable).  So this was
  a regression introduced by the r746-r751 Unicorn-hook -> trap-site
  migration, not a golden-era defect (the golden-era boot death was only the
  TSE loop, thread [20], fixed separately in r752).
- **Fix:** infinite-timeout event waits now re-enter the trap handler on
  wake (`PC = site` before blocking): every 60 s the handler re-checks the
  event bits (a real `OSAL_s32EventPost` finishes the wait through the
  satisfaction branch) and re-arms the park - the waiter stays a sleeper
  forever, exactly like the golden-era monitor-yield park.  Finite waits
  keep the native-timeout behaviour.
- **SPM slave start confirmation:** procbaselx is the SPM *slave* (app id
  `0x1c`); `ahl_tclBaseOneThreadApp::vAppEntry` (`0xe5068`) creates
  `ev_28` and polls it (5000 ms x 5) for the master's (`spsdm`, not
  emulated) answer, whose post is driven by the reply's message-queue
  callback.  The event layer now fakes exactly one confirmation per
  (thread, `ev_<id>`) pair and consumes the bits immediately (an
  always-satisfied wait turns the slave poll loop into a 3.5 kHz host
  spin, run fix23: 1.06 M hits).  `SpmEvEnd` itself is deliberately never
  answered - it marks shutdown.
- **Verified:** fix27/fix28: no `terminated` line in a 340 s run, prochmi
  swaps 1971 in 140 s (fix24: 659 in 340 s), roads and window dumps intact.

## PROCNAV thread [77]: SXM/AGW service-discovery thread exits with code 1 and faults unwinding (open)

- **Seen:** run fix52 (150 s, first run long enough): after
  `registry query .../PROCESS/PROCSXM/FC_SXM/SERVICES/AGW"\"SERVICEID = 0x98`
  PROCNAV thread [77] logs `thread exited with code 1` and then
  `0x35952a: [77] Execution error: Some(READ_UNMAPPED)` (LR 0x34e4e9,
  FP 0, R2 = 0xfba0f168 poison-ish).  One occurrence per run.
- **Not caused by the r755 CCA-directory work:** the responder answers only
  src 0x0109 registrations and procmap/DAPI keep their bespoke paths; no new
  message reaches PROCNAV.  300 s golden-era runs (fix34) show no such exit -
  the SXM branch is schedule/time dependent and only became reachable with
  the changed timing.  Guest-side bug faithfully surfaced (its exit-unwind
  reads an unmapped pointer), unrelated to navigation/map.
- **Impact:** satellite-radio service discovery gives up; navigation path
  unaffected; thread park list shows [77] gone afterwards, no storms.
- **TODO:** when revisiting PROCNAV fidelity, trace the 0x35952a unwind (it
  follows `vHandle...SXM` startup failing without an AGW service) and either
  provide an AGW/SXM directory entry or document the crash as benign.

## HMI "Starting navigation." wait-widget: remaining chain to hide it (open)

- **Solved this round (r755/r756):** CCA directory now answers
  ApplicationInfoRegister (class 0x49 -> ApplicationInfoStatus class 0x50),
  un-hocking `bRegisterAsync` so real ServiceRegisters (0x41) reach the
  unhosted server queues; RegisterConf(0x43)+ServiceStatus(0x44) answers
  drive `hmicca_tclApp::vOnServiceState` for all 32 registrations incl. nav
  svc 0x27 (server app 0x24); ServiceData(0x45) requests for svc 0x27 are
  answered (OPTYPE 9) and AIL-forwards them into
  `hmicca_tclApp::vOnNewMessage` -> `ahl_tclClientPortMngr::enUpdate`.
- **Still open:** the wait-widget survives all of that.  Facts gathered:
  - It is a Lua/widget-toolkit drawn widget (`GUI_WTK_FormatTextExt` +
    `GUI_ContentLanguage` string id 0x25106fc), presented ~t=3.4s; the map
    page itself (compass, 50 m scale) is already behind it.
  - `GUI_Messaging` receiver lists (+0x10/+0x14) stay EMPTY in the whole
    run - GUI event dispatch (vFireInternalEvent/bOnMessage) finds no
    receivers, so event injection (0xcb ALLOW_USER_ACTION etc.) is a dead
    end; the popup is not an event subscriber.
  - `main.lua` is absent even on the unit (golden log: same
    NoSuchFileOrDirectory); production = the `path=memory` bootstrap, so
    the hide path is C++ widget-engine code, string referenced via the
    GUI_ContentLanguage table (no direct xref - find the table index of
    0x25106fc, then the u32-index constants in code = presenter/hider).
  - Likely true trigger is FI-level: `clHmiNavServerHandler::
    vHandleEnaviServerStatus` returns early because the enavi interface
    field +0x7dc (and +0x814 DAPI, +0x15c nav-server) are 0 - the FI link
    between DAPI and PROCNAV (TEA_TASK1 msg-manager NULL, see design.md)
    is the upstream blocker; the svc-0x27 ServiceData payload schema
    (functions 0x2000/0x2002/0x2208) is what its port data must fill.
- **Next experiments:** decode the service-data port read side
  (`ahl_tclServicePort` consumers) to learn what payload Lua/C++ polls;
  find the ContentLanguage table index for the string and its users; wire
  the DAPI<->PROCNAV FI link (TEA_TASK1) which is already on the plan for
  the GPS consumer work.
- **r757 follow-up (runs fix79..fix90) - the FI-event mechanism is fully
  mapped, the producer of the release event is NOT in any binary:**
  - Power pacing works (OFF@~3s -> delayed NORMAL@~6s -> `appstates-onnew
    cur=8 new=3`, dir flush 40 replies).  The framework state byte
    `clHMIMngr+0x670` gets seeded 3+pending by code whose TB predates even
    a load-time MEM_WRITE watch, and 4->3 likewise - both reads (trap and
    guest) agree the byte is 3 at perform time; the broadcast reaching
    `clFi_Navi::vNewAppState` is therefore always 3, never 4 (fix80/82).
    Not worth chasing further: the vNewAppState startup branch ALSO needs
    gate34=1, which ONLY the event branch below ever sets (constructors
    0x14c28fc/0x14c299c write 0).
  - Event router `clHSIMngr` 0x13528a0 (no symbol) routes `event` to FI
    slot `(event>>12)&0xff` via that FI's vtable+0x3c.  FI 8 = Navi:
    `clFi_Navi::ReceiveEventsFromEvtEn` switch table2 covers ids
    0x80108030..0x80108523 (and 0x81008030.. by wrap) - slot 1
    (0x80108031/0x81008031) sets gate34=1 and calls `vStartupEnaviServer`
    directly.  `vHandleEnaviServerStatus` (0x15e2070) hard-guards
    +0x7dc/+0x814/+0x15c/+0x830 - all created only by that startup, so the
    event is the SINGLE gate for hiding the popup.
  - Feeder chain observed live: CCA wire msg -> `hmicca_tclApp::
    vOnNewMessage` -> CCAGW -> `clHSI_CMMngr::bExecuteMessage` (0x1827b04,
    also entered via +4 this-adjusting thunk - untrapped) fans to every
    `clHSI_CM*` client; the clients' default case forwards `sMsg.type`
    VERBATIM as `clHSI_CMBase::bSendHSICommand(cmd=sMsg.type)` -> router.
    Proved live: types 0x2b09/a/b (hmicca-internal msgs at t~2.4s) arrive
    at CMNAVI (0x182b7d4, sMsg+4 = scalar type, ~0x2900..0x2bff family)
    and are forwarded verbatim (caller=0x18641a0).  Named senders exist
    for siblings: `clHSI_CMSystem::vTellNaviAboutMetricsChange` sends
    0x810080a4 (literal at 0x184d974); the CMNAVI literal pool 0x182c8d4
    holds 0x81008030 (slot 0) but NOTHING anywhere holds 0x80108031 /
    0x81008031 (scanned prochmi, PROCNAV_dec, DAPIAPP, full lx001 for LE
    bytes; no movw/movt forms).  => the id is COMPUTED by its producer
    (or arrives as a message type), most plausibly by PROCNAV's
    navif/NAV_IFM machinery once its server is up - which needs the same
    TEA/mbx_19 fix the GPS consumer work needs.
  - PROCNAV already posts real svc-0x27 traffic to the HMI
    (`src=0x24 dst=0x109 class=0x45 svc=0x27 regid=0xec func=0x90000`,
    `msgid=0x4000`) and it IS materialised into mbx_265; the HMI-side
    `hmicca-svcstate` pump enumerates it (`svc=0x27 sub=0x24 id=0xec
    state=0`).  So the wire plumbing works - only the release-message
    producer on the PROCNAV side is missing.
  - **Exact drop point located (r757b, fix91..fix94).** The nav FI link is
    wired up end-to-end EXCEPT the final client routing:
    - `fi-dispatch` (hmicca vOnNewMessage dispatch tail 0xfc68e4) fires for
      three PROCNAV messages `src=0x24 dst=0x109 class=0x45 svc=0x27
      regid=0xec func=0x90000 msgid=0x4000..4002` at t~6.7s.  The listener
      vtable is `clCcaMsg` (_ZTV8clCcaMsg 0x286e950, +8 = vAddMsg
      0xfc9b84), so the messages are QUEUED into the shared `clCcaMsg`
      singleton (obj 0x38ead48), not delivered synchronously.
    - `clCCAGateway::bReadSharedMemory` (0x13c91f8) is the consumer: it
      calls `clCcaMsg::poGetNextMsg` (0xfc9ae8, cca-getnext trap fires 3x)
      and `vDeleteMsg` (0xfc9af4, cca-del trap fires 3x) at the same t~6.7s
      - so all three nav messages ARE consumed and freed.
    - **CORRECTION (fix97..fix99): the routing DOES succeed.** A trap at
      the pvGetHandler return (0x13c931c, `gw-route`) shows it returns
      handler 0x38feab0 (non-null) for all three nav messages: the client
      vector (55 clFiData entries, created from our r755/756 CCA replies)
      contains route [14] svc=0x27/serverApp=0x24/regId=0xec, and matching
      pairs out (sourceSubId@+10 vs message src, registerId@+12 vs
      wire+0xe).  The message IS delivered to the client handler at
      0x13c9340.  So the HMI-side FI plumbing works end-to-end: the reason
      no event appears is that func=0x90000 is simply not the release
      message - it routes to the nav client and nothing more.  No fabrica-
      tion is needed or justified here: PROCNAV must organically send the
      startup message once its enavi server is up.
  - **Decision: unblock PROCNAV next** (who must post to mbx_19/
    TEA_TASK1QUEUE); a 60-120s run with `cm-mngr-msg` uncapped should then
    show whether the startup event appears organically.
- r758 (fix102b..fix113): the PROCNAV side was unblocked, the producer still
  isn't there - and the popup's actual server turned out to be **inside
  prochmi**.  Findings, in order:
  - PROCNAV component bring-up was serialized behind TIMA's registry retry
    loop (every `PROCESS\NAVAPP\TIMA\APP_CONFIG\KTIMA\*` open failed because
    that branch lives in the encrypted `NAVAPP.ERG`).  Seeding the KTIMA
    branches + APP03's copy + `TIMA\APPID` + `HMINAVSERVER` APPID/SERVICES
    in registry_compat (commit 5a7395e) made TIMA (`mbx_3`+`MQB_3`) and WDB
    (`mbx_23`) start, and the SCD resolves `HMINAVSERVER` APPID=0x24/
    SERVICEID=0x27 stably.  The `CONFIG\<APP>\APPID` misses are a benign
    directory enumeration (4x per app, uniform), not a loop.
  - **Correction to r757c's premise:** the three "PROCNAV nav messages"
    (src=0x24 msgid=0x4000..4002 func=0x90000) were OUR OWN r756 echoes
    (msgid 0x4000 = the fabrication's MSG_ID seed; "func=0x90000" was the
    trap misreading the OPTYPE byte at +0x1a as part of the func word).
    PROCNAV never sends nav service traffic in the emulator; it doesn't
    host the NAVI FI server at all.
  - `HMINAVSERVER` strings exist only in prochmi_out.out: `di_hmi_navserver`
    (`clHmiNavServerHandler`, 537 symbols, ctor 0x15e871c, `vStartupEnaviServer`
    is one of its methods) is compiled INTO prochmi.  The "enavi server" the
    popup waits for is this HMI-internal server; app 0x24 (would-be mbx_36)
    is ITS CCA identity.  prochmi's registry reads go through the libosal
    REGISTRY stubs (log level trace - earlier "prochmi never opens the
    branch" observations were log artifacts); with the eur pack now loaded it
    opens APP_CONFIG fine (fix109).
  - Runtime chain probes (commit 38fb9ff): handler constructed t~2.3s
    (thread 52, caller 0x14c28f4); our r755 ServiceStatus for svc 0x27
    reaches `hmicca_tclApp::vOnServiceState` -> `nav-state-dispatch` ->
    `clCCAGateway` vtable+0x18 method (0x13ca4c8): client-port lookup fails
    (that's the design - fallback path), and the vtable+8 fallback sink
    0x134e91c on clHMIMngr (0x38eee60) passes its guard (+0x650 =
    hmicca_tclApp) and tail-calls into hmicca_tclApp's notify tail
    (0xfc8854).  With the state-1 experiment (commit 7554252) the FULL chain
    runs at t~6.1s, yet the router sees NO 0x80108031: FI-8 events observed
    are only 0x810080a4/c4/a8 (other producers).
  - The id is still not present as any constant anywhere (re-scanned
    prochmi/PROCNAV/DAPIAPP/procmapengine/procbaselx/libosal for
    0x80108030/80108000/108030/8030 LE words; the PROCNAV/procmapengine
    hits are misaligned false positives inside Thumb code - 0x8030 is a
    Thumb `strh` encoding).
  - Remaining hypotheses for the producer: (a) the MQServStateChg poller
    (wait loop at prochmi+0x13c230c, consumer side of the state pump) needs
    a second, non-null client-port entry to emit the +1 event - the r757c
    route table entries were built from our fabrications and may lack the
    field that makes `find` (0xfb6e9c) succeed; (b) the id is composed by an
    enavi component (`enavi_tclRegistry`/HmiNavServer) only once the real
    navif ENGINE link (procmapengine/DAPI, svc 0x26 family) is READY, i.e.
    popup release is downstream of the map-engine data path, not of CCA at
    all.  Next: probe the MQServStateChg consumer loop body for the event
    emission site (objdump 0x13c1d00..0x13c2400, coverage gap - decode by
    hand or page-watch it), and check the +0x24 gate byte of the gateway
    method 0x13ca4c8 (its zero branch takes a completely different loop).

## r761 - popup release branch executes end-to-end (fix179..197)

- Gate decoded: `vHandleEnaviServerStatus` (0x15e2070, handler 0x38f99b0)
  releases the wait popup iff `[handler+0x148]==2 && r7==1`, where 2 comes
  from the enavi server-state word `[[IEnavi 0x3a241c8+0x4c]proxy+0xe0]`
  and r7 from the idapi query `0x1614dac(idapi,"/dev/cryptcard",0)`
  (tail 0x171fcf4: needs a card object in the idapi+0x18 vector whose
  +0x80 in {0,2} and whose +0x98..0x9c record vector holds a +0x0c==2
  record; name match via entry+0x58 vtable +0x24).
- enavi-fab (shallow): write proxy+0xe0=2 + guest-call bWriteQueue(qm,0,
  0x81008001); fires 800 ms after the organic `bstartup-reg` hit, on the
  next hsi-fievent-router trap.  The real handler then runs its own
  release branch (0x15e2220: widget query 0x1706710, hide request
  0x15e0418 -> posts 0x81008132/0x81023200 into the engine, progress-bar
  0x1621a24/0x16259d8, cleanup 0x15dfce4/0x15dfb3c/0x15e1a7c) and pops
  cleanly; 0x81008132 is consumed+delivered via the real router (disp
  0x38ca510).  r7==1 is achieved by `fake_cryptcard_into_idapi`: vtable/
  card/record fabricated on an emulator heap page (data only, name-match
  = text gadget 0xfb70f0 `bx lr`), copied idapi vector + appended entry.
- Root cause of the fix177..195 fatal (READ_UNMAPPED at 0x135a1cc,
  q=0x90610d1c): BOTH injections (enavi-fab at router site 0x13528a0 and
  the evt8001 pump at dispatch site 0x1352df4) fire on *function-entry
  `push {...,lr}`* instructions.  The trap defers the push (cave replay)
  to the injected call's return, and the replay stored the callee's
  leftover LR (bWriteQueue bl-return 0x135a3c8 - caught red-handed by the
  0x90610cd0..0x90610d24 dframe write-watch) into the frame's lr slot:
  the owner's epilogue then `pop {...,pc}`-returned into mid-bWriteQueue.
  Fixed by `sp_slack` (32-byte private frame for the injected call, SP
  restored on re-fire) + saving/restoring LR at fire time
  (ENAVI_FAB_SP/LR, EVT8001_PUMP_SP/LR).  Rule: an injection returning
  onto a trap site must restore every register the site instruction
  consumes - entry pushes consume LR, `bl` sites self-heal.
- Still open: the popup remains visible although the hide request ran and
  its event 0x81008132 was delivered (fix197 frames 6.5s..18s show it
  unchanged; GPU REAL-draw count grows uniformly +60/flush, i.e. the
  scene keeps containing the popup).  Next: find the 0x81008132 case in
  the FI-nav consumer and what visibility flag/animation timer it needs
  (suspect a render-timer or an SVG scene-remove step the stub swallows).

## r762 - release branch verified end-to-end; visual hide is NOT its job (fix198..206)

- Fixed a real bug: the enavi-fab/post-wipe probe read the server-state
  proxy from a *hardcoded* IEnavi 0x3a241c8, but that singleton's address
  is heap- and run-variant (fix194/203: [0x3a241c8+0x4c]=0x1000600 - the
  fab wrote state into unrelated text-segment memory and the handler kept
  seeing e0=1).  `live_srv_proxy()` now chains the STATIC handler
  0x38f99b0 +0x7dc -> IEnavi -> +0x4c -> proxy (fix204: write lands,
  readback=0x2, gate sees state148=2 every run).
- Release-branch forensics (fix198..206): the query 0x1706710 is
  vtable+0x40 = u16[widget 0x38fb388+0x98] on the *embedded session
  object* handler+0x19d8 (segment-mapping gotcha: the binary's second LOAD
  maps VA=off+0x10000, third +0x268000 - the old off+0x8000 assumption
  poisoned earlier vtable reads).  A RUNNING report implies a live
  session, so the release-gate sets that word to 1; the handler then takes
  the REAL close path 0x15e23bc (`bl 0x1650478` at 0x15e23cc, r4:=1
  fingerprint) instead of the 0x15e0418 fallback - all traps fire, clean
  pop, no crash (LR-replay fix of r761 holds across runs).
- KEY negative result (fix204/206 frames + popup7e0/paint-marker watches):
  the handler's close branch does NOT repaint the popup away.  The only
  observed organic visual hide is the GUI-event exit-hide: the 0x8d
  startup-event flush (post_gui_internal_events) moved the popup state
  0x1e5 -> 0x1e4 while the widget existed and the paint vanished
  (fix206 popup #1, gone between the 9 s and 12 s frames).  The same flush
  then (re-)paints a second "Starting navigation." from the rodata string
  (paint marker t=12.4 s) - that late paint is what stays visible: after
  it, the fab's close (n=2 run, state148=2/r7=1/close-traps all good,
  fix206 t=13.5 s) leaves the widget on screen.
- fab re-timed to fire only after `startup_events_release_now()` (paint
  marker + 750 ms, 12 s fallback) so the RUNNING report follows the final
  paint like on the real device - ordering is now right, the missing
  piece is a *visual* hide channel after the last paint.
- Next: the proven channel is GUI-internal events via
  post_gui_message_queue (0x8d family = Lua state transitions with
  enter/exit-hide pairs).  Find the GUI event id whose transition's
  exit-hide covers the wait-popup state (0x1e5) *after* navigation is up
  (real device: the server-RUNNING notification ultimately rides the
  CCA/DAPIMgr -> Lua path), and post it through the same emulated queue
  when the fab fires.  Provider-level event fabrication is the sanctioned
  layer; the 0x8d flush already proves the queue injection works.
- fix207 negative result: re-posting GUI event 0x8d through the messaging
  queue right after the release-pop (state148==2, flag FAB_HIDE_EVENT_
  PENDING -> bOnMessage posts 0x8d again, "posted post-fab hide event"
  t=13.5s) does NOT hide the repainted popup - the Lua FSM has consumed
  that event's transition already and ignores the duplicate.
- Open forensic gap before driving the SM directly: the GUI_StateMachine
  performTransition (0x1371390, mid-function) and vDoTransition
  (0x1371170) probes show ZERO hits across fix204..207 while the popup
  demonstrably hid once organically - classic narrow-dead-hook signature
  (AGENTS.md): convert both to page-range xp-watch probes to learn (a) the
  live sm pointer, (b) the transition id that hid popup #1 at ~12 s, and
  (c) the transition that re-entered the paint at 12.4 s.  Then guest-call
  vDoTransition(sm, that-exit-transition) from the safe post-wipe site
  when the fab release completes (honest fakery: the app's own transition
  executor, our provider-level trigger).
- fix208..213 forensics of the close path (release-wfind + popupslot
  write-watch): 0x1650478's "found" branch is taken (wfind noop=false -
  the live current-popup slot [[0x2a13a00]+0x14] holds the wait widget),
  yet the *entire* call writes NOTHING to that slot (zero popupslot WRITE
  events through the close) and mutates no head word of the widget.  The
  path ends in 0x166e718 (list bookkeeping over widget+0x950..0x978) and
  0x164e478(widget,2,proxy) - a message-registration teardown, NOT a
  widget-hide.  Conclusion: the C++ close branch of
  vHandleEnaviServerStatus is bookkeeping; the wait popup's visual
  visibility is decided solely by the Lua/GUI state machine (the 0x1e5
  enter-paint / exit-hide pair; see r757), and its vDoTransition/
  performTransition sites (0x1371170/0x1371390 - both verified real
  function entries, so the zero hits are NOT the dead-hook quirk) are
  never called - the transitions in those two functions are not what hid
  popup #1 either.  The Lua-side show/hide consumer is still unnamed.
- No .lua/plain-script resources exist in lx001.tar.gz (the GUI logic is
  compiled into libgw/GUI_ResourcePack-style binaries on the SD pack -
  next grep target: the SD CRYPTNAV/ resource tree for the
  "wait popup"-owning resource pack and its state-table event ids).

## r764 - the SM page-hook resolves the transition forensics; hide is an async-paint orphan (fix214..220)

- SM-PAGE page-hook (page 0x1371000, trace-only) proves vDoTransition
  (0x1371170) and performTransition (0x1371390) DO run - the earlier
  zero-hit trap_sites were over-filtered (doTransition's arm only matched
  trans 0x1977/state 0x1e5, never the real ids), not dead.  Full startup
  cascade captured (fix214/215): at t~12.03 s the SM walks
  0x1e3 -(trans 0x6eb)-> 0x1e5 (SYSDLG nav-starting-up, popup painted by
  the Lua side at paint-marker t~12.4 s) and LEAVES the sysdlg family
  within ~20 ms (0x1e7/0x1e6/0x1e9/0x1ed/0x1ee/0x1fc/.../0x132).
- ROOT-CAUSE SHAPE of the persistent popup: the SM's exit-pair for 0x1e5
  fires while the widget is not painted yet (Lua paints ~350 ms later,
  asynchronously) - the orphaned paint is never re-hidden.  This is the
  r757 paint-marker trap from the other side.
- fix218 negative result: paced re-posts of GUI event 0x8d after the
  release (via post_gui_message_queue, 4x) produce a fresh identical
  cascade but never re-enter 0x1e5 (leaf rests at 0x13x) - the popup is
  unaffected.  The static transition-table decode at 0x25a9568 matches
  isolated records (0x6eb 0x1e3->0x1e5, 0x2061 0x1e5->0x1e4 event 0x8d,
  0xa02 0x1e6->0x1e8) but decodes garbage for the cascade's other ids -
  the u16 id-mask or a per-region table split breaks the linear model;
  do not trust the decoded protocol beyond individually confirmed records.
- fix219 negative result (mechanism DISABLED in code, kept dormant):
  guest-calling vDoTransition(sm, 0x2061) sets [sm+0xc] pending fine, but
  the chained performTransition(sm) faults READ_UNMAPPED at 0x1371434
  (ldr r1,[r8,#8]): its state-object resolution only works in the real
  event-dispatch context - the organic caller 0x136b3fc reads sm via the
  owner object's vtable+16 accessor and pushes a RUNTIME record pointer
  from the owner's transition list (entry +8), not a static base+id*0x10
  record.  Directly driving the SM from the router site is therefore not
  viable.
- Next attack vector (widget layer, sanctioned guest calls): enumerate
  the wait-widget class vtable of the slot14-registered widget
  ([[0x2a13a00] singleton +0x14], set at the 12.4 s paint; the C++ close
  branch leaves it set - fix212 popupslot watch) and find its vShow/vHide
  pair in the GUI widget base class; the 12.4 s paint ran through a
  vtable+20 command dispatch (0x1363a4c/0x136bf54 read-chain) - the HIDE
  sibling of that command object is the organic lever.  The bOnMessage
  receiver-list enumeration (r755-diag) remains the map for unnamed Lua
  event ids.

## r765 - popup show/hide is executed by the GUI resource interpreter (fix221..230 probes)

- The old "GUI core 0x47dcxxxx frames" were LIBC (memcpy/strlen in
  /lib/libc-2.8.so mapped at 0x47d58000) - the r763 note about a GUI-core lib
  at those VAs was wrong.  Stack-scan backtraces proved stale (the r757
  popup-string hook's chain picked dead frames), so the probes now log LR +
  raw stack words (bl-validated).
- The popup-string reads are all ONE paint: pc = libc memcpy/strlen called
  from the UTF-16/8 text builder 0x1362cb0 (+0x1362d34 variant, r7 = string
  ptr).  The builder's saved-LR (stack [3]) is 0x136bf58 = the GUI resource
  interpreter's virtual tail-call `ldr pc,[r3,#20]` (loop 0x136bc40..,
  0x874-byte frame): `obj = cmd_class_table[word>>28]; call obj->[vtable+20]
  (obj, word, arg)`.  For the popup the command WORD is the string pointer
  itself (0x025186fc>>28 = class 0 = static text) - the interpreter runs the
  compiled per-SM-state action lists (widget trees), so the SHOW is an entry
  action of state 0x1e5 and the HIDE is the 0x1e4 entry (or 0x1e5 exit)
  list - the same interpreter machinery fix214 saw flash by at 12.03 s,
  BEFORE the async paint (orphan mechanism, r764).
- Driver of the interpreter: FUN_01913c7c (called from 0x19141a4/0x1914204/
  0x191f2b0/0x191f444) tail-calls vtable+16 of [obj+0x14]; live interpreter
  singleton r0=0x391c450, r1=0x391ff90 (stable across runs' lifetimes),
  r2 = action-list pointer (heap 0x3a184f8-style for the SM-state lists,
  stack 0x906100c4 for the materialized paint run at t=12431 ms).
- Probe status: trap_site probes `text-builder`/`text-builder2`/`gui-interp`
  (own 4000-hit cap, t>10 s gate) and `gui-cmd-*` are installed; the
  `ldr pc,[r3,#20]` sites (gui-cmd-wrap-exec/gui-cmd-interp) are NOT
  relocatable so they ride legacy narrow hooks (dead-probe risk, AGENTS.md)
  - they saw zero hits while the page-hook-proven builder DID fire; prefer
  extending the builder/gui-interp traps or a page-range hook.
- NEXT (concrete): (1) at a gui-interp hit during the SM-entry-list runs
  (t~12.0x s, r2 = heap list) dump 64 words at r2 to learn the command
  record format; (2) dump the state 0x1e5 object (leaf 0x39ead28 at the
  12.03 s doTransition) and match fields against those r2 values to locate
  its entry/exit action-list pointers; (3) after the enavi-fab release,
  guest-call the interpreter (this=live 0x391c450-equivalent, r1=r2 pair as
  observed) with the 0x1e4 ENTRY list - that re-runs the app's own hide
  actions after the orphaned paint.  This stays "honest fakery" (guest
  calls into the app's own executor, no memory patching).

## r767 - popup is OVERLAY view 917; MenuManager swap chain decoded but destructive (fix237..253)

- Script-object forensics (POPUPCELLS/NODE probes on the popup paint): the
  painted widgets are GUI_Label/GUI_BGImg script objects (class vtables
  0x2872150/0x28721c8, RTTI "9GUI_Label"), [obj+0x1c] = impl widget, and the
  VM dispatcher record (0x391ff90-style) = {sibling, flags 0x10406, argtable,
  ctx, code, nargs} entries whose `code` IS the 0x136bc40 interpreter -
  gui-interp runs are VM method-table invocations ("Text" property id 0x21
  binds the popup label).  GUI_Scripted lifecycle = bOnMessage/vOnDataUpdate
  call Lua by REF (prepareCallByRef, refs at +0x6c..+0xa4), so per-widget
  hide logic lives in Lua refs we cannot name statically.
- Event pacing fixed to match the device: the fabricated internal events are
  staged - 0x8d on the release gate (completes SM wiring + shows the wait
  popup, async paint ~12.4 s), 0x8e/0x8f (the service-ready effects: main
  view change + app-state moves) and the enavi-fab RUNNING write fire only
  at paint+750 ms (enavi_fab_fire_now(), 15 s fallback).  fix241 had all of
  them on one 12 s gate, so the 12.16 s organic view-change destroyed the
  popup's view 285 ms BEFORE its paint -> orphaned pixels no hide channel
  could erase.
- View lifecycle decoded (viewfac probes + perform asm, objdump immediates
  are DECIMAL): GUI_MenuManager (GOT 0x2a13f50) slots = u16 (drawn,pending)
  pairs at +0x12/+0x1a overlay, +0x14/+0x1c alert, +0x16/+0x1e extended,
  +0x18/+0x20 main (vSetView slot k <-> perform loop i = 3-k), u16 object
  slots at +0x00..+0x0f = pGetView(0..3), u8 +0x30 = zombie-panic marker
  (path B sets it).  Popup = overlay view cid 0x395 (917, created on the
  main view 247/0xf7 at paint time); boot view cid 1 destroyed organically
  at ~12.1 s, main = 247.  vSetView is pure slot bookkeeping; the tree
  rebuild only happens inside vPerformViewChange (0x136465c) when
  pending != drawn.
- The 3-stage injection (vLeave(record) -> vSetView(mm, MAIN_CID, main) ->
  vPerformViewChange(mm), MAIN_CID captured from the organic main-block
  pCreateView) executes the whole swap cleanly - destroy(main)/create(247)
  + destroy(917)/create(0) all run, focus-broadcast 0x59 (whose
  vFireInternalEvent receiver==0 faults, no 0x59 listener exists here) is
  skipped via an entry trap window.  BUT the run then breaks: the stale
  aux-slot garbage (0xffff/0x6157, 0xffff/0x6e72 from startup alert churn)
  forced extra destroy/create rounds, and destroying the overlay view
  zeroes pGetView(1) - the object slot the mainloop view-pump (0x1365224)
  polls every frame (r0 0x391ab88 -> 0 across perform, fix252 viewpump
  probe) -> READ_UNMAPPED 0x136524c, thread 52 dies, screen black.
- ⇒ the real device must NOT close sysind overlays via a plain
  bDestroyView+null-slot; the organic close goes through a path that keeps
  the polled view slot valid (candidates: vShuffleOverlay 0x136432c, the
  GUI_LUA "DestroyWidgetInstance" method seen at prepareCall sites - Lua
  tears the WIDGET down while the view object survives - or a Lua
  view-change handler).  The chain is DISABLED in
  sm_hide_fire_or_restore (documented return at the top); capture probes
  (record/globals/view cids/viewpump/focus-skip) stay installed.
- NEXT: (1) probe vShuffleOverlay + GUI_LUA_Interface prepareCall sites for
  a DestroyWidgetInstance/"Visible" SetProperty pattern during the organic
  0x1e5-leave attempts; (2) guest-call the popup label script-object's
  vBeforeUninitialize (0x137a0e0, runs its Lua ref) - it may tear the
  widget tree down WITHOUT nulling the view slot - then trigger a repaint
  of the main view's rect; (3) if that fails, replicate the device's alert
  slot hygiene (zero aux pairs at boot-factory time, before the first
  perform, instead of mid-rebuild) and retry the swap with drawn slots kept
  consistent so i-loop rounds skip.

## r768 - popup hidden via widget visibility data (clean); repaint channel is the gap (fix254..257)

- fix254 lesson: the painted "script objects" (GUI_Label 0x2872150 /
  GUI_BGImg 0x28721c8 vtables) are VM-materialized CELLS, not GUI_Scripted
  widgets - driving GUI_Scripted::vUninitialize over them dereferences
  garbage Lua ref slots and called the HSI router with junk (READ_UNMAPPED
  0x13528a8, thread 52 died).
- GUI_Widget::vDraw (0x137bb88) walks children ([this+0x1c], count +0x20)
  and skips any child whose tagged visibility ContentId at +0x3c resolves
  to 0.  The popup owner chain (captured at paint: both script cells' f6
  = same container): container widget (kids=2: Label+BGImg, vt 0x2872aa8)
  -> overlay-view root widget (kids=1, vt 0x2871fd8, parent 0).  Writing
  the tagged-false literal 0x10000000 over the container's +0x3c (the same
  store GUI_Widget::bSetProperty performs for the visible property; old
  value was the CID ref 0x70000010) removes the popup subtree from any
  further draw pass - zero errors, no slot/queue corruption (sm_hide_fire
  _or_restore stage 1 + PU_* capture probes).
- Repaint gap: GUI_Widget::vInvalidate now runs from the clGUIWidgetEngine
  ::bExecute trap site (0x133c134) - NOT the router site: the callee's
  message machinery re-enters the router and executes the just-consumed
  site instruction with clobbered r0/r1 (fix255 crash).  The bExecute-host
  handler restores the engine `this` into r0 on the injected return
  (fix251 lesson: bGUIMainloop once ran with r0=0 that way).  But the
  post-hide damage produces NO composite pass: prochmi stops issuing
  eglSwapBuffers when idle and the invalidate never schedules a redraw.
  Consistent with the golden-era finding that GUI_Messaging receiver lists
  ([singleton]+0x10/+0x14) stay EMPTY the whole run - the message-driven
  repaint channel is dead in our emulated unit; the ~120 real swaps came
  from Lua data-update bursts during startup cascades only.
- GUI_GUIAPI__vInvalidate (0x134ae6c) is unusable as-is: its MenuManager
  global [0x2a12ae0] holds a LIBC-region value (0x47dc8080) here, NOT the
  mm ([0x2a13f50]); the host-side guard skips those slots (fix257 log).
- NEXT: (1) investigate why vAddReceiver registrations vanish (the empty
  GUI_Messaging lists) - fixing messaging revives repaint-on-damage for
  the WHOLE GUI, not just the popup; (2) meanwhile test whether posting an
  internal event with a registered receiver (any id the FM1 view listens
  to) forces one full redraw pass after the hide; (3) popup is invisible
  in the widget tree - a frame proof needs any composite pass >14 s.

## r769 - MILESTONE: "Starting navigation." popup visually GONE (fix266..268)

- The hide channel (sm_hide_fire_or_restore stage 1 + the 1145-handler loop)
  now runs end-to-end at paint+~1.4 s with ZERO execution errors and the
  post-hide composite frames (shots268 k0006-k0008) show the full FM1 screen
  with NO popup - the wait-popup is finally gone from the display.
- Channel anatomy (all app code, no instruction patching):
  1. Capture at paint: text-builder NODE probe stores the popup script CELLS
     (Label 0x2872150 / BGImg 0x28721c8 vtables) and their owner widget f6
     (+0x18) = the painted popup CONTAINER (kids=2). PU_WIDGET (slot14 of
     the popup singleton) is rejected by the widget-shape guard - it is not
     a GUI_Widget - and correctly skipped.
  2. Hide: store tagged-false (0x10000000) over container+0x3c (the same
     word GUI_Widget::bSetProperty writes for the visible property; old
     value CID 0x70000010). GUI_Widget::vDraw skips invisible children, so
     subsequent renders of that subtree draw nothing.
  3. Render: on the clGUIWidgetEngine::bExecute site (0x133c134 - NOT the
     router site, fix255 re-entrancy) the loop guest-calls, per target:
     GUI_Widget::vInvalidate(container), popup-ctx [vt+0x14](ctx,1)
     (0x1347010, the GUI_GUIAPI__vInvalidate whole-ctx call), then the
     overlay render triple DisplayManager::update() would run: ctx->[8] =
     GUI_GL_OpenGLContext::drawBegin (0x1348980, binds the layer FBO),
     GUI_Widget::callDraw(ovrRoot,0,0) (0x137af7c), ctx->[0xc] = drawEnd
     (0x13488f4) - drawEnd issues the app's own eglSwapBuffers, so the
     erased layer (damage from the +0x14 call, tree now invisible) presents
     WITHOUT the popup. The pGetView-slot targets self-skip: [0x40] of the
     slot objects is a data wrapper (0x209/heap words, not a vtable) and the
     DisplayManager global [0x2a12928] is junk here (code word at +0), so
     update()/mixLayers cannot run at all in our unit - hence driving the
     per-view block manually on the popup's REAL OpenGLContext.
  4. Site hygiene: the trap site's original instruction replays on EVERY
     injected-call return (entry push each leg), so the loop saves
     r0-r3/sp/lr on entry and restores them at loop end (fix267 crash at
     0x133c138 READ_UNMAPPED = SP drift with only r0 restored).
  5. Proof capture: gpu::request_forced_present() -> FORCE_PRESENT_REQ is
     served by the GPU backend thread (command path + 16 ms timeout tick)
     with one handle_surface_swap(Hmi): pure presentation of what the guest
     rendered + EMU_MAP_DUMP_DIR dump, since the idle app issues no further
     swaps itself.
- NEXT: strip the TEMP forensics probes (POPUPCELLS/NODE, viewfac/viewpump
  dumps, eng/v3/parent-chain diagnostics) keeping capture+hide+render+
  present; then multi-minute runs toward real map pixels. DisplayManager/
  OpenGL pool globals staying junk ([0x2a12928]/[0x2a12958]/[0x2a12948]) is
  the tracked root gap for any engine-driven (non-injected) repaint.

## r770 - wait-popup hide channel made default-run capable (no env vars)
- Symptom (user report): with a bare launch (no EMU_PROCHMI_NAV_TRACE) the
  "Starting navigation." popup never disappeared although r769 had proven the
  channel.  Root cause: the channel's DATA PATH was scattered behind
  trace-gated probe code:
  1. EVT8001_REPOST reset (pump consumed 0x8100_8001) lived only in the
     trace-gated "event-read2" label arm -> REPOST stayed 1 forever in
     no-env runs, blocking both the enavi-fab fire gate and the hide arm.
  2. The whole enavi-chain label match (cryptcard answer, session word,
     PU_WIDGET capture, SM_HIDE_STATE arming) sat behind
     `if !nav_trace_enabled() { return 0; }`.
  3. The popup script-cell capture (PU_OBJS/PU_IMPS owners) sat behind the
     same gate in the textbuilder probe.
  4. The hide arm required SM_HIDE_RECORD, captured only by the trace-only
     0x1371000 page hook.
- Fix (prochmi.rs): all data-path side effects now run unconditionally -
  always-on event-read2 REPOST reset before the gate; release-gate/close/
  wfind/pop labels exempt from the gate (rare sites, zero cost); textbuilder
  probe runs the PU_* capture with only the forensic dumps (POPUPCELLS/NODE/
  gui-interp) behind nav_trace_enabled(); arm gate accepts the unconditional
  popup paint marker as "popup really painted" evidence instead of the
  trace-only SM record.  Logging behaviour unchanged.
- Verified (fix270, EMU_PROCHMI_NAV_TRACE unset): paint marker 12460 ms ->
  fab RUNNING 13465 ms -> release-pop n=2 13580 ms -> hide 13789 ms ->
  invalidation loop done 13791 ms, 0 Execution errors, final frames show the
  full FM1 screen without the popup (shots270/window_0006).

## r773 - shared-futex watchdog semantics decoded; OSAL pool semaphores need producers
Findings (runs fix278..fix287, semtrack + EMU_WATCH forensics):
- The "stuck words" the 20 s watchdog repairs (0x4000365c, 0x4010a9ec,
  0x4010c02c, 0x4010c42c/6c/ac, ...) are NOT mutexes: they are OSAL sync-pool
  POSIX semaphores (`WaiSyncObj`/`CreSyncObj`/`LockOsal` in
  libosal_linux_so.so, pool = the 1 MiB `/dev/shm` segment mapped at 0x40000000
  in every process).  `FUTEX_WAIT(word, 0)` there means "sleep while the
  semaphore value is 0" - the waiter waits for a POST, not for an unlock.
- Protocol (disassembled): `vGenerateSyncObjects` (procbaselx boot) seeds
  slot sems to 1; `WaiSyncObj` zeroes the word after a successful acquire
  (libosal+0x14608 `str`); `UnLockOsal` = `sem_post` + counter--.  Healthy
  pool sems (0x400195c8, 0x4010c1ec..0xc4ac, 0x40023890...) cycle
  wait/post normally through the shared-futex registry.
- The STUCK ones are genuine inter-process semaphores whose producer thread
  never runs in the emulator (SCD bring-up handshakes - names `CB00Tim`,
  `_0NN(OSAL`; the waiter stacks are `OSAL_s32SemaphoreWait` from scd/app
  init and the procbaselx SPM worker).  The watchdog's wake-with-success is
  the designed mitigation; it is NOT a lost-wake bug (zero WAKE syscalls are
  ever issued for these words, and `sem_init(1)`...`sem_wait(0)` sequences
  show the token is legitimately consumed elsewhere).
- Consequence to keep in mind: wake-with-success lets two processes enter
  the same guest critical section.  This is the mechanism behind the
  intermittent PROCNAV AE-worker crash
  `scd_tclServiceAppMap::bReadData -> bpstl::less<u16>` READ_UNMAPPED
  (PC 0x35952a, r1 = small integer as node key): two AE workers race in the
  per-process ServiceAppMap Rb_tree that the device serialises with exactly
  these semaphores.  1 crash in ~6 runs (281: 2, 282/284/285/286: 0, 287-1:
  1); killing one AE worker parks a map pump in some runs.
- Fixed here: watchdog consumes a pending post instead of destroying it
  (word 1 -> 0 + wake), errmem ioctls answered (the -1 asserted every
  OSAL_vWriteToErrMem), IOSC `shared_mem_free` (0x534f0001) modelled,
  `stack sp-0x20` dump added to thread-death diagnostics.  A broadcast
  variant (repair every waiter of the word at once, 99f6c76) had to be
  REVERTED (3c66d37): the fake acquire then lets whole cohorts into the
  same critical section and the DAPI map re-open chain dies - see
  design.md r773.
- Remaining root gap (milestone-2 territory): give the stuck semaphores real
  producers - SCD bring-up completes only when the administration/SPM answer
  path delivers (see gw-deliver probes); until then the watchdog papers over
  it and the AE race stays possible.

## r774 - IOSC shared_mem_free is a no-op (bounded leak; refcount upgrade path)
- `0x534f0001` (libiosclib `iosc_shared_mem_free`) answers success but keeps
  the region mapped and the host buffer alive.  Deliberate: the emulated
  guests keep using pool pointers AFTER calling free (DAPI's SPM clients do),
  so the real driver's unmap-on-caller would only manufacture
  READ_UNMAPPED faults on pointers the guest still dereferences.
- Why the leak cannot grow: host buffers are keyed by pool id
  (`IoscState::get_or_create_shared`), the ids are a closed set
  (measured 0x09..0x5b, 42 distinct, ~23 MiB in a 100 s run), re-malloc of a
  known id clones the same Arc, and the guests free almost never (3 frees
  vs ~30 mallocs per 100 s) - these pools live to boot-completion on the
  device too.
- Upgrade path IF the emulator ever must run for many hours: refcount each
  id (map id -> attachment count), unmap the CALLER's region (mmu.unmap of
  the VA read back through *[ppMem]) at free, and drop the host buffer only
  at the last attachment; keep a per-process "attached ids" list because
  each process maps its own VA.  Verify against the DAPI SPM client's
  use-after-free pattern first (it would fault - gate the unmap behind
  EMU_IOSC_STRICT_FREE until the client behaviour is understood).

## r775 - horizon/tilted map view: injected camera elevation was 0 (horizontal)
- Symptom: delivered map pixels showed a grazing 3D "horizon" view (orange
  void + extruded building prisms) instead of the normal top-down map.
- Root cause chain (all runtime-proven, probes kept in procmapengine.rs):
  `ren_trCameraParameters` is copied verbatim (u32SetCameraParameters
  0x524a4c) to `view+0x6c..0x8b`; the u16 at params+0x08 -> `view+0x74` is
  NOT "pitch*10" but the elevation field `Render` converts to euler angle 1
  as `(0xffff - field) * pi/131072` (0 = look along the ground,
  0xffff = straight down = the normal top-down view the view constructor
  itself bakes).  The renfi camera driver (procnav.rs) sent `0` there, so
  every applied camera laid the camera flat and the picture became the
  tilted horizon (and with it the orange off-map void fill + demo artifacts
  that dominate a grazing view).
- Fix: the serializer now puts `0xffffu16` in the elevation slot (heading
  stays 0 = north-up).  Verified (fix297): euler triple reaches `[0,0,0]`
  and published frames are a clean top-down white base + Krakow landcover
  polygons, no horizon, no orange-void flicker (that flicker was the
  Milwaukee demo camera showing off-map void half the time).
- Remaining (unchanged, milestone-2): only the landcover layer is delivered
  yet (2 flat colors); roads/labels blocks still not harvested; engine
  power-on demo camera still writes its 65deg-tilted literals (lr 0x4d7518)
  once, corrected by the next nudge within 700 ms.
- r775b follow-up (run_fix298): with the top-down elevation the delivered
  frames now show the real ROAD network over Krakow (dark road strips, ring
  + radials) - the grazing-camera geometry had been shrinking the harvested
  MO classes; the landcover-only look of fix297 was the usual harvest-lottery
  variance, not a regression.  Styling is still the untextured 2-color
  palette (no labels/textures yet).

## r776 - every watched pool word decoded; r773 "missing producers" mostly REVERTED to parks
With the new OSAL-level semtrack (`hook_osal_trace`, EMU_SEMTRACK=1: OSAL
sem/event create/open/wait/post log name+slot-id+LR, LockOsal/UnLockOsal log
the fixed-lock names at word+0x18), every word the 20 s watchdog repairs is
now identified.  Structures (from libosal decompile): sync slots at pool
+0x10a22c, 1003 x 0x40 B, per-slot name `OSAL_%04d` (what r773 saw as
`_0NN(OSAL`); AMES semaphore-handle table at pool+0x19608 (0x34 B entries,
slot id +0x8, NAME +0x11); NTVE event table at pool+0x20c (0x3c B entries,
sync id +0x10, name +0x18); `CreateOsalLock` fixed locks carry their name at
word+0x18 (SEM_LOCK 0x195c8, SYNC_LOCK 0x10a1ec, EV__LOCK 0x1cc, TSK_LOCK
0x30f4, MQ__LOCK 0x251e0, TIM_LOCK 0x23890, PRC_LOCK 0x2f97c...).  Verdicts:
- 0x4000365c = sync slot 0 = the NTVE event "SpmEvEnd" (pool+0x20c, first
  event).  Its waiter is procbaselx `s32RunEnv()`: after `fc_spmslv_tclApp::
  vStart()` + server start it waits SpmEvEnd with infinite timeout - the
  designed end-of-watch park, woken only by SPM shutdown.  Not stuck.
- 0x4010a9ec (slot 31) = DAPI `90019FE0_LICENSE_TO_KILL`; 0x4010c02c (slot
  120) = procmapengine `900A8AC0_LICENSE_TO_KILL` (format `%08X_LICENSE_TO_
  KILL`).  They belong to `s32RunExeEnvironment` (DAPI 0x813988, also linked
  into procmapengine): create sem(0), `s32InitApp07(h)` + `s32InitAppFc_lbsm
  (h)` - each posts h once when its app finishes/de-participates (the not-in
  -registry branch posts immediately, DAPIAPP.OUT 0x81f200) - then the env
  thread waits once per successful init and only proceeds to `vExitApp07`/
  `vExitAppFc_lbsv` after ALL tokens.  Parking there for the whole uptime is
  the designed steady state; APPID 7 is DAPIAPP itself (registry_compat.reg
  line 14), so the async `dap_tclDapiApp` bring-up path is taken legitimately
  - its answer path is the genuine remaining producer gap (below).
- 0x4010c42c/46c/4ac (slots 136/137/138) = the DAPI AE dispatch channel
  (producer thread [36] posts, workers [72]/[73]/[74] consume): healthy
  wait/post ping-pong for 8 rounds during bring-up (t=15.6..16.7 s), idle
  park afterwards.  The watchdog only repairs the IDLE waits.
- Watchdog wake on a word still 0 is a benign no-op: after the futex returns
  success, glibc/WaiSyncObj re-checks the sem value, finds 0 and re-parks
  (observed: repaired tids never advanced, they just re-armed the 20 s
  repair; no `[14] quantum` lines between repairs).  So the repair can only
  release a waiter when the word is >0 (consumes a real pending post).  The
  r773 note "wake-with-success lets two processes enter the same critical
  section" is therefore wrong for these words - r773's broadcast-regression
  story stays true, but the AE race cannot be watchdog-driven.
- AE crash RESOLVED as the OLD benign guest bug: the run_semtrk2 fault
  `[78] 0x35952a READ_UNMAPPED` (LR 0x34e4e9, R2 garbage node ptr) is the
  exact fix52 fingerprint already documented above ("PROCNAV thread [77]:
  SXM/AGW service-discovery thread exits with code 1 and faults unwinding"):
  the service-discovery thread exits code 1 when its SXM/AGW directory query
  misses (the emulated registry has no PROCSXM service tree), and the guest's
  own exit-unwind then walks a garbage Rb_tree node once.  Confirmed in
  run_semtrk2: `[78] thread exited with code 1` immediately precedes the
  fault, and WDB opened `SXM_SMS_AGW\...` keys just before.  Benign, no
  navigation impact, NOT watchdog-driven and NOT ServiceAppMap write-path
  corruption - r773's "two AE workers race" model is hereby retired (the
  `probe-less-u16` constant already recorded tid 76 faulting there during
  channel-factory init in an earlier era, same story).
- Genuine remaining producer gap (narrowed, unchanged core of r773's last
  bullet): the `dap_tclDapiApp` (APPID 7) / lbsm bring-up answer path - the
  SPM/administration answer exchange (DpSemSrvIf sem, Dp00000001 event) ends
  incomplete, so the LICENSE_TO_KILL tokens never arrive.  All decoded
  waiters simply park (which is their designed steady state until shutdown),
  so nothing observable depends on it today; it stays listed as emulation
  surface completeness (shutdown-path fidelity), not as a milestone-2
  blocker.  r777 removed the watchdog's incentive to touch those words.

## r777 - shared-futex watchdog repairs only real tokens; pool parks sleep in peace
- Policy (scheduler.rs watchdog branch): when the synthetic 20 s window of a
  `watchdog: true` shared-futex wait expires, first PEEK the word
  (`SystemNamespace::futex_peek_shared`, added).  NON-zero word -> unchanged
  dead-owner repair: steal + consume one token + wake exactly this one waiter
  (ps-mutex with dead holder, or pending post whose wake was lost).  ZERO
  word -> the waiter sits on an OSAL pool semaphore whose post has simply not
  been produced yet (decoded parks of r776): re-arm the window
  (`rearm_shared_watchdog`) and leave the waiter REGISTERED in the shared
  registry.  Benefits: (a) no fabricated futex successes - glibc/WaiSyncObj
  rejected them on their value re-check anyway, so guests now behave exactly
  like the device (sleep until a real post); (b) the steal path no longer
  deregisters park waiters for the repair round-trip, closing a window where
  a real peer `FUTEX_WAKE` racing the watchdog could be lost; (c) a real
  shutdown post to `SpmEvEnd`/`LICENSE_TO_KILL` can never be consumed/
  destroyed by a watchdog tick again.
- Verified: 80 s smoke run - zero `repaired word` lines (were ~3/cycle), zero
  errors; 175 s golden run - render-kick burst at t=157.7 s, map publishes up
  to n=11 with nonblack=384000, delivered frame is the top-down road network
  ((23,23,23) roads over (204,206,217) bg, 2-color palette), milestone-2
  intact.
- Kept as-is on purpose: the AE dispatch channel words (slots 136-138) also
  fall into the zero-word re-arm now; a genuinely lost wake there self-heals
  as soon as the producer posts the next message (FUTEX_WAKE path), which is
  again the device behavior.

## r779 - page switches render: GUI_DM_EAManager::isBlocked gate un-faked
- Symptom (milestone 3): a Menu tap (67,335) ran the whole engine chain
  (event 0x95 -> SM transition 0x133->0x11b -> bDestroyView(247)/
  pCreateView(229) -> Lua CreateWidgetInstance), the new view landed in the
  menu-manager slot with the SAME live draw context - but zero widgets
  painted: idle stayed at exactly 2 glDrawArrays/frame (mixLayers quads),
  window pixels after the tap identical.
- Dead ends (measured, probes removed again): bGetStatePath and the LCA /
  [sm+0x18] / dynamic-stack gates all PASS for the menu transition (SM-DEC
  page probe); GUI_GL_OpenGL::drawImageTriangle and
  GUI_GL_OpenGLContext::drawBitmapTriangles never execute - HMI draws come
  from GUI_GL_OpenGL::drawImage (widget pass) driven by
  GUI_GL_OpenGL::mixLayers callers inside GUI_DM_DisplayManager::update()
  (the stale LR 0x1345638 at the libGLESv2 stubs misled for a while).
- Root cause (update() decompile, 0x133d99c): the per-view render block runs
  only when isViewDirty(view) AND GUI_DM_EAManager::isBlocked(view) == 0.
  The live EA record ([ea+0x124] = ea+0xc embedded head) carries state word
  +8 == 0 while the branch treats 2 ("shown") as pass-through, so isBlocked
  returns 1 forever once the radio-card EA exists - after that, NO view ever
  repaints (the visible screen is the last rasterized layer texture, which
  is why boot paints fine and every later page switch stays invisible).
- Fix (page-switch paint pump, libs/prochmi.rs VIEW_PAINT_*): on every main
  view-create (cid>1, t>15 s - the startup/popup phase must stay untouched,
  A/B proved early invalidation kills the click chain), a beat later the
  pump (a) calls GUI_GL_OpenGLContext::invalidateAll on each live slot view's
  draw context ([[slot]+0x28]+0x40, vtable+0x14, guest_call_via_lr with the
  TOUCH_RETURN register wrap), (b) writes the EA record state -> 2 (faked
  media-stack lifecycle data, same sanctioned class as the forced app-state
  gates; all drawing itself stays the engine's), (c) posts the 0x8f internal
  dispatcher nudge + request_forced_present (r768 recipe).  Opt out with
  EMU_NO_VIEW_PAINT.
- Verified: tap at t=60 s -> full MAP OPTIONS page paints (65% window pixel
  change, icons+labels real), d_paint_120 jumps 240 -> 1454 (drawImage) and
  glDrawArrays 240 -> 8066; 30 s no-tap run stays clean (pump never arms at
  boot, milestone-1 popup hide intact).
- Kept probes (bounded): touch-post-dump + touch-draw-rate counters
  (HMI_DRAW_CALLS, DRAW_IMAGE_CALLS via drawImage page hook, VDRAW_CALLS),
  isBlocked observer late-window dump; nav_trace-only SM-DEC/SM-PAGE and
  viewfac probes document the click->transition->view chain.
- r779 follow-up: intra-menu navigation verified too - a second tap on
  "Next Page" (650,400) at t=64 s renders menu page 2 (Show Route On Map /
  Show Waypoints / Options / Guidance & Map Settings / Show Position
  Information / Previous Page), 39% window pixel change - the pump re-arms
  per view-create, so every subsequent page switch self-heals the same way.

## r780 - clickability scan of the main screen + menu item navigation verified
- Scripted single taps (fresh run each, base = main map page) + window-dump
  pixel diffs:
  * Menu (67,335): MAP OPTIONS pages 1+2 render (r779, re-verified).
  * Menu item "Enter Destination" (205,165 on page 1): fires the exit path
    the engine itself uses - SM 0x1e9->0x322, GUI_StateView::vLeave ->
    GUI_MenuManager::vLeaveView(229), view-create cid 294 = NAVIGATION
    submenu, fully rendered, its own Next Page (650,400) renders page 2
    (Address Book / Intersection / My Points Of Interest / Previous Page).
    So menu open AND menu-internal navigation are usable end to end; there
    is no back-to-map control on the FI menu pages themselves (device
    design: leaving = choosing a function item).
  * Compass/recenter button (67,415): tap delivered, zero pixel change, no
    view-create - it is a map-engine command, invisible without a panned
    map; no emulator defect shown.
  * Scale bar / map area (155,455 / 157,137): click passes THROUGH the EA
    radio-card layer (it carries no HMI hit-test widgets) onto the map
    window -> opens MAP SCROLL (cid 246, SM + view-create + Lua creates 16
    widgets) but only the page title paints: body widgets never issue a
    single drawImage.  NEW DEFECT (below), independent of r779: update()
    runs the draw block for the page every frame (both views pass
    isViewDirty+isBlocked continuously once EA state is 2).
  * Radio presets 1-6 / card header (65,217 etc.): no visible effect, no
    message fired - the foreign svg card layer is not touch-targeted in
    this stack (device behavior likely identical; inputs there go through
    HSI/hardkeys).  Recorded as fidelity question, not a defect.
- NEW DEFECT (open, milestone-3 tail): MAP SCROLL body invisible.  Facts:
  cid 246, 16 CreateWidgetInstance after view-create, render pass active
  (vDraw/draw burst, title paints), yet no body drawImage.  Hypotheses to
  discriminate next: widgets parented to a map-surface container that is
  never sized/shown (geometry 0), or they draw into the MAP layer target
  which our window dump does not show.  EMU_ISB_LATE="start:end" moves the
  isBlocked forensic window over any scripted-tap schedule (added r780).

## r781 - MAP SCROLL "black body" was misdiagnosed; real gap = touch vs live map
- Probe suite (all EMU_VDRAW_LOG-windowed, kept): GUI_Rect::intersects entry
  (r780: callDraw's clip branch is mid-TB, unicorn only dispatches TB entries,
  so read rects from the intersects entry itself), GUI_Scripted::vDraw gate
  (+0x68/+0x6c/+0x70/+0x74), prepareCallByRef + callInts, GUI_Image/
  GUI_Button::vDraw entry (handle +0x60).  Plus EMU_DRAWTRI_LOG (drawImage
  tex/rect dump) and gpu::map_draw_calls() (post-dump `mapdraws=` field).
- Facts after MAP SCROLL (cid 246) opens at t=25 s:
  * clip stack is full-screen and EVERY visited widget rect intersects ->
    callDraw->vDraw runs for all of them (earlier "clip/geometry 0" theory
    falsified).
  * 4 widgets are GUI_Scripted: gate byte +0x68=1, Lua ref +0x6c set, no
    pre-fn (+0x70=0), post-draw fn +0x74 set; prepareCallByRef SUCCEEDS and
    callInts runs every frame - the scripted paint path is healthy, the
    Lua funcs simply draw nothing in idle (no pan state).
  * 2 GUI_Image + 2 GUI_Button (the title bar icon/strip) draw normally.
- The page's real "body" is the live map.  At t=25 s the map engine has not
  painted yet (first paint burst at t~118 s in current builds), so behind
  the title everything is black regardless of the page -> the defect I filed
  in r780 was (mostly) the engine-idle black, not a widget gap.  RETRACTED.
- After the engine burst the main page looks correct (full-screen land
  polygons + intact card/Menu/compass/scale overlay, dump at t=118 s).
- REAL NEW GAP: once the map surface is live, a tap in the map area is no
  longer dispatched at all (t=163 s run: vPressed/vReleased delivered, zero
  msg-fire, zero SM transitions, zero pixel change).  Before the first paint
  the same tap passed through to the HMI background and opened MAP SCROLL.
  On the device the map engine receives touch (pan/zoom) and informs the
  HMI, which is what opens the MAP SCROLL page; our stack never relays
  touch to procmapengine, so map interaction is dead while the map is up.
  Next defect to work: touch routing to the map engine (procmapengine
  input path via OSAL queue/registry - investigate its PortControl/
  message loop).

## r782 - MAP SCROLL composites the live map fullscreen; defect = engine paint latency
- Decisive 235 s run (6 s dumps, scripted tap 165 s): tap at 165 s -> MAP
  SCROLL opens; engine paint burst arrives at ~194 s; the following dump is
  the page fully correct: fullscreen land polygons + title bar + 50m scale.
  So the page DOES composite the live map (HMI publishes the MAP_View1 layer
  fullscreen - "map layer window published enabled=true rect=(0,0,800,480)" -
  and the CPU compositor honours it).  All earlier "black body" observations
  were made on runs whose first engine paint had not happened yet (burst
  timing is data-driven and varies: 118 s / 194 s / never within 160 s).
- Two r781 claims RETRACTED: (1) the 163 s "dead tap" - the window-dump
  thread is capped at 40 dumps, so nothing after that time was ever captured;
  scripted taps after the paint DO dispatch and open the page.  (2) the
  implied "touch must route to the engine for the page to open" - the HMI
  opens MAP SCROLL itself on the background-pane click.
- REMAINING DEFECT (renamed): first engine paint takes 2-3+ minutes (DAPI
  map-data query chain), and the engine paints only in rare bursts (9
  multi-draws then silence).  On the unit the engine repaints every frame.
  Consequences: fresh runs show a black map area for minutes; panning the
  map (touch -> engine) is still not wired, so a pan cannot force a repaint.
  Next work: shorten time-to-first-paint (investigate the DAPI tile-query
  pacing) and/or keep the map surface republished on every burst (already
  sticky - the FBO persists), then touch-to-engine routing for pan/zoom.

## r783 - time-to-first-map-paint: adaptive renfi cadence + post-copy render wake
- Latency forensics (235 s run, host-ts buckets): the whole request chain
  IS alive from t=15 s - CCA requests, DAPI answers, navdata reads and 14
  AddNewRawDataBlocks all happen within the first 60 s.  What fails to
  happen is the MOUpd state-2 capture: every early MOUpd job executes while
  the request machine churns (job-lag lottery, cf. r646/r651), so
  copy-to-active first fires only after a long quiet window (t=150 s), and
  the pure render kick then waits for the `tick % 20` coincidence at the
  engine's wait-skip sites - another 37 s wall time before the first
  glMultiDrawArraysEXT (t=188 s in that run; 118-190 s across runs).
- Fix (emulator-side pacing only, no guest logic touched):
  * procmapengine exports MAP_LAST_BLOCK_LOAD_MS (stamped at every
    AddNewRawDataBlock) and MAP_MO_COPY_DONE becomes pub.
  * procnav renfi camera driver: adaptive cadence (r783) - while blocks are
    still landing (or none ever landed) keep r651's 40 s quiet window; once
    loads have been quiet 10 s and copy-to-active has NOT happened, tighten
    to the proven 10 s nudge (r611/r612 cadence): with all blocks local the
    re-arm hits the "Rect matches -> idle" short-circuit instead of
    restarting a wire round, so extra nudges only buy more MOUpd lottery
    tickets.  After copy-done relax back to 40 s.
  * all four engine wait-skip gates now honour a shared 1-per-wall-second
    throttle (render_wake_now()) once MAP_MO_COPY_DONE is set, so the pure
    render kick loop starts within ~1 s instead of waiting for a
    `tick % 20` coincidence; the throttle keeps idle engine threads blocked
    (no hot spin on the serialized host).
- Measured: copy-active T=103.4 s, first multi-draw +3 ms (was +37 s),
  window dump 0026 (~106 s) already shows the painted fullscreen map with
  the radio card/Menu/compass/scale overlay intact.  Two runs: painted at
  ~101 s and ~128 s (vs 120-190+ s before; runs <130 s used to show nothing).
- REMAINING: a ~50 s dead zone between the first partial block load (t~50 s,
  14/34 blocks) and the flush round that delivers the rest (~110-120 s);
  it appears on every run and looks like DAPI/CCA answer pacing for the
  missing-block rect.  Next lever: why does the re-arm at t=50 s not flush
  the remaining 20 blocks for 60 s (DAPI service scan cycle vs the engine's
  wire-round serialization).

## r784 - destination-entry (keyboard) page forensics; pump multi-round; probes
- Repro (EMU_TOUCH_TEST, deterministic across c6/d5/d6/d9 runs): Menu tap
  (67,335) opens the menu (MAP OPTIONS p1, view cid 229); a tap at
  (205,165) hits Enter Destination -> cid 229 destroyed (view-destroy
  t=26.1), MAIN view cid 294 created, and the HMI render pass then blits a
  full six-button "NAVIGATION" page every frame (header 799x83 + 6x 359x104
  buttons + 6x 89x76 icons, EMU_DRAWTRI_LOG 29-30 s).  So cid 294 *renders
  fine* - Enter Destination opens a category page that reuses the menu page
  template.  A further tap (205,165) on it (= New Address) does NOT
  destroy/replace 294: instead a cid **0** main-ish view (lr 0x1364940) and
  the map companion cid 653 (waypoint circles) appear.  cid 0 = failed page
  creation: the destination-entry/keyboard page's view is created empty, so
  the screen stays on the category page forever.  This is where milestone C
  blocks - not a paint problem (pump invalidates view 294/607 every round,
  EA state is 2, swaps run, ~854 swaps after the tap).
- Keyboard plumbing audited (EMU_PROCHMI_KEYB_TRACE probes, kept):
  * clFi_KEYBRD ("FI_KeyBoard") IS instantiated at t=2.19 s via
    clFiFactory::CreateFi (id 51), and clHSI_CMKeyboard::bExecuteMessage
    (0x1812de4) executes 8 boot messages (sMsg ids 0x2b09-0x2b0b, unhandled
    -> default).  The handled id family in the decompile is 0x29db-0x29eb
    and maps to GUI_KeyInjector_vKeyPressed/vKeyReleased(keycode) - i.e.
    CMKeyboard = physical/remote KEY events routed into the GUI KeyInjector,
    not a "show keyboard" message.  clFi_KEYBRD SendEvents/ReceiveEvents
    (EvtEn path) never fire (evt-in/evt-out 0 hits).
  * After the tap DAPI srvctrl jobs churn at ~2/s forever: type=4
    (dap_map_tclManager) fct 0x101/0x103/0x104/0x401 and type=6 (rnw = road
    network, rnw_tclManager) fct 0x80/0x81 - all matched to workers and
    processed; the RNW country files (/dev/cryptnav/data/data/rnw/CCP/POL/
    NAV*.DAT) open and load fine.  This churn is a symptom, not the block.
- Tools kept this revision: per-pending-pointer SM stuck deep-dump (was a
  global 40-slot budget eaten by boot transitions - the destination churn
  pending=0x25b3198 decoded as the benign sysdlg 0x1e6->0x1e6 self-transition
  after the fix); keyb path probes; srvctrl job-dump (type/fct/payload once
  per key, log after t=15 s); view-paint slot+word dumps; pump now re-arms
  its invalidate/EA-force/kick pass up to 5 times (~2 s apart) since pages
  finish building widgets after the RNW lookups land.
- Open question (next): what data source must answer so pCreateView for the
  keyboard page yields a real cid instead of 0.  Candidates: a NAVI AMT/FI
  dynamic-list config (rodata has dynlistconf_NAVI__DESTENTRY_LIST2 /
  _row_entries; similar MEDIAPLAYER__QSEARCH_KEYBOARD_LIST exists for the
  media keyboard, fed by its CCA service), or dap_li_userif dataset/language
  validation (u16SendAndWaitWithValidLanguage).  The HMI side is healthy:
  page-open SMs run (state 0x5da, trans 0x182a), the category page paints,
  companion cid 653 animates.

## r785 - pump round knob env-gated; injected-call register restore is fragile (filed)
- r784's always-on 5-round view paint pump killed prochmi: with more injected
  vInvalidate calls per second, the TOUCH_RETURN_PENDING restore of the
  bGUIMainloop site lost a race (an unrelated site re-fire consumed the flag)
  and the real `clGUIWidgetEngine::bGUIMainloop` ran once with r0(this)=0 -
  prochmi main() then exited(1) at t~20-31 s (f1/f2 runs; the 6 this=0x0 log
  lines in every c6/c7-era run show the same race pre-r784, just lucky).
- Two "safe" replacements were A/B'd and both regress WORSE (kept the old
  path): (a) call_guest_function writes its save frame at the live r13, but
  this site sometimes fires while caller code parks a GOT/static base in r13
  (f1: frame written into the code segment at 0x133be70, guest died);
  (b) new call_guest_function_scratch (private scratch stack page + the
  literal-slot return stub) reached the stub with the app's own sp
  (stub-entry lr=0x47) - the vReleased callee does not return through the
  frame we laid, so the pop loaded garbage registers and main() exited(1).
  The helper stays #[allow(dead_code)] for the eventual fix (next idea:
  instruction-step trace of vReleased's epilogue to see what it returns
  through, then align the frame contract to it).
- Pump multi-round is now EMU_VIEW_PAINT_ROUNDS=N (default 1 = r783
  behaviour).  F3 verification (default): all 3 taps land, full view
  lifecycle intact (tap2: destroy 229 -> create 294; tap3: destroy 607 ->
  create cid0 -> destroy 0 -> create 653), no thread exits, screen stays on
  the category page while the waypoint-circle companion animates.
- Refined milestone-C statement (from f3): the tap on New Address IS accepted
  (companion re-registration 607->653 + cid-0 placeholder), but no MAIN-view
  transition is ever initiated - the app waits for the destination-entry
  input session (dynamic-list config `dynlistconf_NAVI__DESTENTRY_LIST2`
  / input-method list) that no emulator service ever answers.  That answer,
  not painting, is the milestone-C gate.

## r786 - milestone C root cause decoded: popup 0x636 requests view cid 0
- Full click->view chain decoded with new probes (msg-fire type+vtable
  fingerprint, SM-PAGE onMessage, dynstack-op + state slots, viewchg-ovl,
  ovl-cid MEM_WRITE watch on the overlay record 0x0391ab50):
  * Every content button fires a type-0x49 message; GUI_ContentPopupOper::
    s_vPerformAction_now(0x1364044) maps id&0x07ffffff into a static action
    table (ptr at GUI globals 0x02a1769c, 8-byte entries {op,a,c,b}) and
    calls GUI_SuperStateMachine::vPerformDynamicStackOperation (0x1377370)
    on the SuperSM singleton (0x39e1cb0).
  * tap1 Menu / tap2 Enter-Destination / tap3 New Address all execute
    correctly.  tap3 = REMOVE popup 0x4fa + INSERT (op 0x1e9 sysdlg family,
    popup 0x636, prio 0x12c).  The dynamic-stack insert runs and
    vShowDynamicState enters the popup's GUI_StateView.
  * GUI_StateView::vEnter (0x1375a4c) requests the view via
    GUI_MenuManager::vSetView(this+0x30, layer=[parent+0x21c]) - the u16 at
    state+0x30 is the view cid, passed through verbatim into the layer
    pending slot (record +0x18/+0x1a).  vCreateStates (0x13729c8) binds it
    at compile time from the descriptor record word +0xc (view = v>>4).
  * The popup-0x636 state object (slot table *(SuperSM+4)[0x636] =
    0x39fa2d8) has +0x30 == 0 and +0x04 (view ptr) NULL, while the working
    page states (0x5da) carry a non-zero view + bound view ptr.  So the
    keyboard/dest-entry dialog's compiled state asks for view cid 0 ->
    pCreateView(0) -> cid-0 placeholder companion -> replaced by the
    waypoint animation ~0.5 s later.  This also explains the "no vEnter
    write" observation: vSetView(0) only clears/arm slots, the observed
    0x1375c6c writes; no real cid write ever happens.
- Interpretation: the dest-entry dialog's real view id must be filled into
  that state by the DYNAMIC state machine (dynsm) path - SuperSM byte +0x18
  (bIsDynamicStateMachineActivated) gates whether vEnter even requests a
  view; SM-DEC dynsm-active last fires at t~12 s.  The dynamic descriptor
  for the NAVI destination-entry dialog is fed by CL/fi_tcl data that
  PROCNAV would deliver on the unit (msg ids 0xc0000052/5a are per-class
  message ordinals, w0=6 at tap3 = mode/session arg).
- Per the AGENTS.md fidelity rule, the fix target is therefore PROCNAV's
  precondition for emitting the dest-entry session data (its srvctrl/RNW
  service state), NOT an injected answer.
- Working reference for a healthy dynamic popup: waypoint companion 653
  (0x28d) requested via the same vEnter->vSetView path at t=33605.

## r787 - correction + deepening of r786: popup 0x636 DOES open; the wait is mediator 0x90000206

Full-run forensics (f15..f20, probes: dynstack-op dedup+child dump,
GUI_StateMachine::vEnter/vLeave, dynsm-flag MEM_WRITE,
GUI_StatePseudoMediatorSync::vStart/bCheckState, f17 map-dump screenshots):

- r786's "view cid 0 + no vEnter" reading was wrong: the cid-0 companion at
  tap3+8ms is the transient left by the vLeave pair (pending=0 armed the
  switch before the new enter came).  The dedup'd dynstack-op timeline shows
  popup 0x636 inserted at tap3 (prio 0x12c) WITH a compiled GUI_StateView
  child (record 0x39fa2d8+0x50 = 0x39fa328, id 0x40003d7a, view cid 0x28d),
  which enters ~0.5 s later (dynsm flag 1, lr 0x136e09c) and creates the
  companion cid=653.  Screenshots confirm what 607/653 actually are: the
  dotted busy spinner - it appears after tap2 (Enter Destination) and never
  clears; tap3 (New Address) re-arms it (607 -> 653).  The HMI-side state
  machine machinery is fully functional; milestone C is NOT a paint bug and
  NOT a dynsm-gate bug (dynsm flag toggles are consequences of enter/leave,
  writers 0x47dc4f8c set / 0x139e8c4 clear).
- The real wait: opening the dialog starts (mediator-sync probe, f20) a
  GUI_StatePseudoMediatorSync at 0x39fcd88, state id 0x40001b5a, vStart lr
  0x13716ec (inside GUI_StateMachine::vPerformTransition).  Its descriptor
  0x025ad8d8 is a row of a static ROM table at ~0x025ad600, 20-byte rows
  {stateId, fnptr, mediatorId, runtime-count u32, view-cid u32}: 13+ rows,
  all bound to mediator content 0x90000206 (kind 9), rows waiting on it
  target views 0x277..0x2A6 (the keyboard/dest-entry dialog page cids).
- bCheckState (0x137586c, f14/f20: called exactly once at tap3): passes only
  when `[desc+0x10] != 0` (runtime registration count, still 0) and the kind-9
  mediator handler count (registry [0x5972a30 + 9*4], vtable+4) matches the
  wait polarity.  It never passes -> the transition to the dialog page (view
  645/0x285 area) never fires -> spinner forever.  mediator-check probe keeps
  logging handler+countfn; bCheckState hits are rare = the SM polls via
  vOnDataUpdate, not a free-running timer.
- Producer identity: the HMI resolves interface/peer apps through registry
  PROCESS/* APPID values (log).  0x0107/0x0108/0x010f/... are audio/tuner/
  smartphone FIs (legitimate fabrications); svc 0x0027 @ app 0x0024 =
  FUNCTIONAL_COMPONENT/HMINAVSERVER, whose strings + HMINAVSRV CL thread +
  D605_registry/ni2k_*/hminavserver.reg all exist - the nav server interface
  the keyboard waits on (NAVIF, NAVIF_APPID 0x121 commented in reg).  The
  kind-9 mediator handler is fed from that CL interface (clHmiNavServer
  client layer inside prochmi); no CL data ever arrives because the nav-side
  endpoint (PROCNAV/srvctrl RNW session) stalls: after each nav tap PROCNAV
  opens an IOSC session (MQB_1/MQB_3 via monitor svc 0x280664 obj 100), posts
  one {1, shm_ptr} event and then its worker threads (75/78/79) poll in
  0.3 s loops without completing the session (f18/f20).
- Next fix target (AGENTS.md rule 1): complete PROCNAV's dest-entry CL
  session so prochmi's mediator gets fed and bCheckState passes by itself.
  Investigation points: monitor svc 0x280664 session semantics (who is the
  peer of MQB_1/MQB_3 - find the second opener/receiver), the wait-stack
  word 0x0032b710 shared by all PROCNAV srvctrl-thread waits, and
  hminavserver.reg NAVIF registration expectations vs our CCA directory.

## r788 - the wait bottomed out: content-function 0x206 getter slot is empty

- Corrected the mediator registry address: it is NOT the prelinked Ghidra
  constant 0x05972a30 but GOT [0x02a13300] -> 0x02cb9518 at runtime.  All ten
  kind handlers ARE installed (k9 handler 0x391a250, vtable
  0x029087f8 = GUI_ContentFunction).  The "mediator count" is
  GUI_ContentFunction::nToInteger(id): the getter table (s_pGetData, ids <
  0xeac, runtime base 0x05202510) holds {fnptr, raw, type} per content id.
- Entry 0x206 (mediator 0x90000206) = {getter=0, raw=0, type=0}: the dest-
  entry data getter is NEVER registered.  The dialog's wait is bound to a
  content function whose provider (the CL/FI proxy of the destination-entry
  session) does not exist in the emulator - so not even a data-update event
  could ever be delivered: vCheckFire runs only at SM compile (51 rows,
  t=13.4 s) and at dialog open; evtqm shows no CL event at tap time ever
  (only the 0x8100801a action event).
- Ruled out as the feed path: enavi_tcl (prochmi's only CL thread,
  enavi_tclEnaviThread = external-nav/HMINAVSRV, no navif_tclIFMediator
  instances), AIL service requests (HMI sent 423 boot requests, none ever
  to a NAVI-family app; zero inbound class-0x45 pushes to mbx_265), and the
  IOSC ringbuffer ioctl (never called; MQB_1/MQB_3 = PROCNAV monitor-queue
  session bodies with one {1,shm_ptr} post each after tap1/tap2, unrouted).
- Fix target stands: PROCNAV's dest-entry CL session must come up (srvctrl/
  RNW worker threads 75/78/79 poll in 0.3 s loops, main thread 17 blocks
  forever on a monitor queue svc 0x350564), which would register getter
  0x206 and push the data event that fires the mediator-sync state.

## r788b - GUI regression from empty-file provisioning + missing rename(2)/rmdir(2)

Symptom (user report): buttons render without label text, the start popup
is gone, console shows `not implemented syscall #38 / #40`.

Root cause chain (bisected g1..g7 runs, /home/marek/Temp/g*.log):
1. r788's `provision_dynamic_ffs` seeded PDATA_MNGR_LM(_OLD).DAT and
   MAIN.LUA as EMPTY files.  On a fresh-flash unit these files DO NOT
   EXIST - the guest creates them on its first save.  prochmi's persistence
   parser treats a zero-byte file as corrupt state instead of absent state
   and silently drops the label/icon/popup bring-up paths.
2. Once the (empty) files existed, the save path became reachable and the
   guest called rename(2)/rmdir(2) - the dispatch table mapped only 39
   (mkdir); in Linux 2.6.32 ARM EABI (arch/arm/kernel/calls.S) 38=rename,
   40=rmdir.  Both returned ENOSYS and aborted the persistence rotation.

Fix (cb9e663 + this commit):
- implement syscall 38/40 (stat::rename, stat::rmdir), add rename/rmdir to
  the FileSystem trait, real OsFileSystem rename/rmdir/unlink (unlink used
  to reject everything), MountFileSystem dispatcher enforcing EXDEV and the
  read-only flags;
- provisioning reduced to the DATAPOOL tree (harmless, g5 A/B: DATAPOOL
  seeded = textures identical); PDATA/LUA entries deleted, EMU_SKIP_FFS_
  PROVISION=1 disables the rest.
Verified: no `not implemented` lines; the 710x189 popup texture uploads at
the NORMAL state transition again; glyph atlas uploads back to 12x512 (8 in
the broken build).  Icon uploads 90x77/180x182 seen in c9/d8 belong to the
touch dialogs those runs opened - not part of the untouched baseline.

## r789 - milestone C: tap chain proven end-to-end; blocker is the enavi engine's service gate (open)

Evidence: /home/marek/Temp/f66..f78.log (golden triple-tap recipe).

- **r788b's "domain 0x80 not mounted" theory was WRONG**: the dest-entry
  elements are 0x08000000-band (elem 0x08007023/0x08007025/0x0801a0c3/
  0x0801a0c4, wait element 0x08076000) - the log's hex display dropped the
  leading zero and every probe filter was written for 0x80xxxxxx.  With the
  band fixed (commit 7b6e2c7): the writes reach cldp_datapool::u32dp_set,
  the pool FINDS the elements and notifies (hsa-event ev=0x95 fires from
  the 0x08076000 write).  No get_podp miss.  The mounted-domain vector is
  52 boot-configured pools (cldp_Mngr::bConfigure 0x1382ecc, cfg list at
  heap, lr 0x134f128), indexed by elem>>24 - domain 8 exists.
- The whole tap-time chain is alive (f67-f73): bEvent2App decision site
  0x1352930 dispatches 0x81008089 to fi=0x38f9970 (vfn+0x3c=0x14bcd74,
  clFi_Navi::ReceiveEventsFromEvtEng, guards [+8]/[+0x2c]/[+0x30] pass) ->
  slot table 0x14bcdfc[idx 0x59] b-stub -> real case body bl 0x15a7228 =
  clHmiNavServerHandler::vInitDestinationEntry(mode=6): ALL guards pass
  (trace [+0x160]=0x38ca510, locinput enavi_tclILocationInput proxy
  [+0x7e8]=0x3a24930) -> vEnableElementListRequest runs and sets the
  enable flag [li+0xc3]=1.  The request is not sent there; the enavi
  request ENGINE is supposed to run it as a process.
- **The engine was frozen by an emulator bug**: enavi_tclEnaviThread
  (OSAL thread HMINAVSRV, object heap-0x3a31ab0, vEntry 0x17bebec) waits
  OSAL event ENAVI_EVENT with mask 0x6e **mode 1 = OR-any**; our
  OSAL_s32EventWait only ever implemented AND-all, so vRunProcess's single
  bit-4 STARTUP post was never consumable and the thread slept forever
  (blocked(sleep) = the r753 infinite-wait park).  Fixed in 6826b95 (OR
  mode consumes matched bits only + writes them through the out-param).
  This fix changes every multi-bit OR wait in the guest - fidelity win.
- f77/f78 (fixed build): engine wakes t=3.0 s, STARTUP process runs, case
  4 of bRunWhenRunning (0x17c73c0) waits the registry HmiNavServerThread-
  Wait (~17 s), calls bStartupNavIF (creates enavi_tclNavIF + CCA status/
  command subscribers), case 5 then gates on
  enavi_tclCCAStatusSubscriber::bIsServiceAvailable for the seven required
  services {0x0a,0x2c,0x1f,0x1e,0x18,0x06,0x152} (defaults-table matches
  hminavserver.reg USE_CCA_SRV_* = 1; bIsCCAServiceRequired==0 also passes
  the check).  All five non-HMI services fail -> state 4 at t=20.1 s,
  retry loop to state 1 at t=35.6 s -> the server never reaches READY, so
  every LI/RC request is dropped and the keyboard page (its mediator
  WaitSyncState on 0x08076000) never completes.
- **Where the availability must come from**: bIsServiceAvailable delegates
  to the navif middleware service table (via [navif+8] vfn+0x1c); that
  table is fed by enavi_tclNavIF's CCA route: enActivate(0x17c1bf8) ->
  poRouteCCACreate(1) -> bCheckCCAInput/bMessageRead (0x17cb574) poll the
  route queue for class-'E' (0x45 ServiceData) messages -> TransferMsg-
  Handler -> service table -> vOnServiceChanged (0x17ba4a8) wakes the
  engine (vPostEvent 4).  f78 probes: navif-activate, routecca-create,
  routecca-read = ZERO hits -> enActivate is never called, no route, no
  feed.  Static analysis: bNavIFCreate/enActivate/navif API (bInitialise,
  enNavIFCreate, IFMediator/ServiceManager ctor, enServiceRegister) have
  NO static callers - only .ARM.exidx hits, i.e. invoked through
  function pointers (FI-framework glue).  bl-literal scans for slot+0x10
  (vServiceStateChanged) found no direct callers either.
- Note bStartupNavIF itself does NOT call enActivate (decompiled 0x17be644);
  the activation belongs to the FI-NAVI proxy side (clFi_Navi, whose
  vSetRegisterID/vServiceStateChanged/vNewAppState navifi probes are also
  zero-hit -> FI-8 never got a CCA registration either).
- Provider identity of the seven services: procmapengine (app 0x0400) only
  registers 0x26+0x06 toward DAPI (0x0007, answered by DAPIAPP itself,
  f77 04:34:04); svc 0x27@0x24 (hminavserver) is answered by our
  cca_server_answer_register fabrication.  GUIDANCE/POSITION/LIM/RC/NAVI
  (0x1f/0x1e/0x18/0x2c/0x0a) and 0x152 have no provider among the emulated
  processes - on the unit they come from the TE-side nav core through the
  CCA.  Per AGENTS rule 1 that makes a *logged, flag-gated* fabrication of
  their ServiceData/0x45 provider traffic legitimate - but the splice
  target must be the RouteCCA queue once it exists, so finding/creating the
  enActivate precondition comes first (rule: fix the environment, not the
  answer).
- Next investigation points: (a) FI framework glue that owns
  navif_tclServiceManager and calls bNavIFCreate - probe the clFiFactory
  bAfterConfigure/vInit path and the hmicca_tclApp register-arrived tail
  0x665720 for FI-8; (b) whether FI-8's own ServiceRegister (which service
  id? svc 0x27? 0x121?) is expected on mbx_36 and merely unanswered because
  DAPI/procmap never answer for 0x1f/0x1e/... - if the real provider is
  PROCNAV's RNW side, fix PROCNAV's precondition instead (rule 1);
  (c) page-hook (not narrow hook) the FI-8 vtable slot+0x14/+0x10 dispatch
  sites before believing navifi zero-hits (AGENTS mid-TB hook quirk).
- Side effects of the event-mode fix not yet visually verified: engine
  processes now run at boot, so the "Starting navigation" popup behaviour
  may have changed - needs one user-visible golden run.

## r790-r791 (2026-10-19): navif CL registrations now succeed - bStartupNavIF returns 1

Fixed (all emulator-side, no guest patching):

- `OSAL_s32MessagePoolOpen` returned -1 in every process except procbaselx:
  the pool "created" flag at IOSC-data+0x1ac is per-process, so only the
  creator could open (`enNavIFCreate` -> 0x80010001).  The kernel-IOSC pool
  object is *shared* state; the emulation now fabricates created-state at
  `PoolOpen` entry when the pool object is unopened (`pool-open` in
  `libs/libosal_linux/mod.rs`).  All processes reach pool-open-ok and
  `enNavIFCreate` returns 0 (f83+).
- The scd client library in every Linux process flips itself into SCD
  queue-mode (`scd_bTestLookupIoControl` 0xfbed38 probes ioctl 10 on the scd
  queue handle; our layer acked it) but nothing answered the lookup ioctl
  0xb, so every service resolved to nothing: registrations went to the
  client's own queue and `ProxyOutCCA::enConnect` died with 0x8000000b.
  Trap `OSAL_s32IOControl` (0x484f0da4): request 10 keeps its native body
  (suppressing it stalls bring-up - f95/f96), request 0xb is answered from
  the shared registry (`find_key_by_u32_value("SERVICEID")` -> the owning
  app's APPID; svc 0xa/0x1e -> 0x15 SENSORFI, else fail).  This mirrors the
  kernel SCD channel on the unit (logged per lookup).
- Generalized the cca-server register answer: RegisterConf+ServiceStatus now
  go to `mbx_<client>` (was hardcoded `mbx_265`/client 0x0109 only), with a
  HOSTED_SERVER_APPS guard (0x0001/0x0007/0x0109): registrations landing on
  an emulated process are never fabricated here (AGENTS rule 1).  Missed
  live deliveries ride a per-client `SCD_REG_SPOOL`, flushed at that
  client's next bridged Wait.
- registry_compat.reg: publish the 14 remaining FI CL service ids
  (0x2c,0x1f,0x18,0x0d,0x06,0x152,0x3c,0x07,0x37,0x32,0x300,0x56,0x7e,0x7d)
  under `FUNCTIONAL_COMPONENT\HMINAVSERVER` (app 0x24, not emulated - its
  registrations are answered by the cca-server emulation like the HMI-side
  ones since milestone 1).  The unit publishes them from the encrypted
  NAVAPP hive.  The C1 loop's required/skip decision still comes from the
  genuine USE_CCA_SRV_* values (indices 7,8,10,11 skipped in f99).

Result (f98/f99): every required registration in
`enavi_tclCCACommandSubscriber::C1` returns 0,
`bStartupNavIF` returns **1** for the first time, the enavi SM retry at
+15s now re-checks service availability instead of re-registering, and the
third golden tap fires the SM-PAGE transition cascade.

Open / next:

- The state word of the STARTUP process still shows no a1=5 transition in
  the probes (enavi-setstate only fired with a1=1) - verify where
  `bRunWhenRunning` goes after `bStartupNavIF`==1 (lr 0x1c7c554 retry loop
  does bIsServiceAvailable checks, lr 0x17c7634) and what gates state 5.
- The address-database gate (popup persistence, DAPI DAPDATAS) stands
  between the case-5 gate and the keyboard page (milestone C).
- Probe hygiene: several r790 probe budgets/timers (`allocmsg`,
  `enconnect-*`, `cmdsub-res`) are still compiled in behind
  `EMU_PROCHMI_NAV_TRACE`; trim to what milestone C still needs.

## r791 (d-s) + r792: SM-pump survival, cryptcard fabrication completion, and what the "No SD card inserted" popup really is

### r791d-r791s: SM-pump death chain fixed (f123)

The SM pump that died with `READ_UNMAPPED pc=0` was dying on *incomplete
fabrication*: the fabricated cryptcard's vtable lacked slots 0x2c/0x34/0x38
and `record+0x10` was not a real `fi_tclString`.  Fixed by extending the
fabrication with the existing guest gadget `bx lr @0xfb70f0` (NAME_MATCH,
returns its argument) and copying a real `fi_tclString` {vt, chars} pair from
the first idapi-vector action's own `+0x58` name - the guest logic then runs
organically over it.  Result in f123: zero thread deaths, 18/18 conf/status
delivered to mbx_289, `bSetState` fired for all 9 services, and the
`case5-pass` continuation at 0x17c776c executed for the first time (+1 s
later the dynstack swapped popup 0x5da "Starting navigation" for 0x5e2).

GVA audit (same commit): each process has its own Unicorn VM + host thread;
the shared namespace carries only host bytes/global tids.  Hardened with
`GuestAddrCell` (cached guest addresses tagged with owning pid, shared-futex
peek/steal owner-checked); zero live cross-VM corruption found.

### r792: the "No SD card inserted. Please use Nissan SD card only." popup (0x5e2)

User-visible switch from "Starting navigation" to "No SD card" looked like a
regression.  It is not: **no saved run ever served the card CID** (zero
`card CID ->` log lines in f90-f124), and r792 probes show
`dap_dev_tclDeviceTableWorker::bCheckSigniture` (DAPIAPP 0x8b2ee4) and the
whole libosal SDX chain (`fd_crypt_verify_signaturefile` 0x485313a0,
`fd_crypt_sdx_xml_verification` 0x485307d8, `fd_crypt_verify_xml_signature`
0x4853204c, `BPCL_EC_DSA_Verify` 0x48539c50 - traps in io.rs) are *never
called* in any run.  The SDX crypto chain has never started.  What changed is
that the availability gate now passes (r791), so the NAV state machine
advanced one stage and evaluated the data medium for the first time (f121 has
no dynstack ops after t=18 s; f123+ pushes 0x5e2 exactly ~1 s after
`case5-pass`).

Decision-path findings (f123-f135):

- The popup swap runs through dynstack `vPerformDynamicStackOperation` from a
  common GUI-framework caller (lr 0x1364094); the deciding input is the
  enavi/DAPI media state, not a GUI flag.
- DAPI's own media pipeline is ALIVE: `vPRMCallBackMediaState` runs,
  `/dev/cryptnav/MEDIUM.CFG`, `DATASET.CFG` and the map files open through the
  mounted card image, and the regionselection "is NAV medium accessable"
  check (0x8c4bb8, result probe `nav-acc-RESULT` at 0x8c4c18) returns
  **r0=1** at t=2.1 s once the media-info entry has type 6.  The
  media-info name string that looked garbled ("*devdev/ev/cv/...*") was a
  probe artifact: the fields are std::string objects / reused heap cells, and
  the name-match demonstrably succeeds.
- What is NOT alive: CL service **0x56** (`HMINAVSERVER_CL0056`), the media
  status service of the FI navigation server.  `isavail(0x56)` = false
  (`case5-56fail`), both DAPI's status-consumer (0xb3a448 r1=0x56) and
  prochmi's media action have no server to answer media state, so the media
  state stays unknown and the dynstack picks 0x5e2.  On the real unit the
  server of all HMINAVSERVER_* services is the NAVAPP application =
  **PROCNAV's CL/CCA face** (APPID 0x24; the encrypted NAVAPP hive publishes
  the service table), which is exactly the responder face still on the
  backlog (PROCNAV pump parks on mbx_1, threads parked/exited(139)).
- r792 side-quests, all negative results worth remembering:
  * narrow trap at the `clFi_Navi::ReceiveEventsFromEvtEngine` dispatch
    (0x14bcdf4, un-relocatable `ldrcc pc`) never fires even as a page-hook -
    that dispatch site is simply never fetched (the FI events route through
    `vHandleEventsForNaviHsiItself`); do not re-add probes there.
  * the fix-size mempool double-hand-out audit (GetBlock 0x4850d4e8/ret
    0x4850d63c, RelBlock 0x4850d32c/ret 0x4850d408, host live-set) saw zero
    blocks in the DAPI heap arena around the "corrupted" cells - those cells
    are plain allocator reuse, not a double-allocation bug.

Next (milestone D): give PROCNAV its genuine CL responder face (read mbx_1,
answer CL per body layout) so svc 0x56 - and with it the media state - is
produced by the guest itself.  The SDX/CID chain then needs only its
environment (ioctl 0x410 CID + HASH_FILE_LIST files are already served from
the real card image); no patch of any app binary is needed.

## r793: who hosts HMI_NAVSERVER (0x24) - and FC_LBS finally boots deterministically

The plan was to give PROCNAV a "genuine CL responder face" for the FI/CL
services the enavi client registers (svc 0x27/0x56...). A systematic search
shows **no emulated binary ever instantiates an AIL app with id 0x24**:

* PROCNAV's bring-up app set is compiled in and observed as
  `{1, 3, 0x13, 0x16, 0x17, 0x11c}` (SCD mode-0 walk + `vStartApp` gates at
  `0x34f14e`); it never reads `FUNCTIONAL_COMPONENT\HMINAVSERVER` at bring-up
  (every HMINAVSERVER registry query in f135-f141 is a *client* SCD map round
  at t>=15 s), and the string `HMINAVSERVER` does not exist in the decrypted
  PROCNAV.OUT at all.
* procmapengine hosts exactly one app (`0x400`, all its `ail_bIpcMessagePost`
  lines say `app=0x400`); DAPIAPP opens the FUNCTIONAL_COMPONENT tree at
  t=04.4 only as part of its SCD map walk (lr `0x8149a8`/`0x98f5cc` are
  registry value-read helpers); prochmi hosts the `0x109`/`0x10f..0x121`
  family; no binary's static map resolves any service to `0x24` - the
  `0x24` clients post to comes from *our* seed (which mirrors the plaintext
  `ni2k_eur/hminavserver.reg` client config; the real per-service APPIDs live
  in the encrypted `NAVAPP.ERG`, undecryptable - see
  `docs/reverse_engineering/erg_registry_crypto.md`).
* procmapengine already resolves svc `0x06`/`0x26` to **DAPI (0x07)** and the
  whole register+conf+status+data round trip runs organically (f136:
  `from-0x0400 q=mbx_7 class=0x41 svc=0x0026` -> DAPI `ServiceRegister
  after-service-check (r0=1 ok)` -> DAPI posts its conf into mbx_1024).
  DAPI answers CL natively for its own services; the media-state message
  classes (`dap_resinffi_tclMsgdap_ri_MediaStateOfDevice*`) are compiled into
  prochmi (client+server classes) and PROCNAV (`pc_tclMediaStateHandler::
  s32HandleRegistrationToMediaState`, `vGetDAPIRIMediaStateOfAllDevices`).

Experiment (reverted): re-point `FUNCTIONAL_COMPONENT\HMINAVSERVER` APPID to
`0x11c` and stop fabricating conf/status for it.  Result (f140): the navif
registrations land on `mbx_284` (nothing waits there) *and* PROCNAV's own
FC_LBS bring-up hangs - when `scd_bGetAppIdFromServiceId` resolves a service
FC_LBS consumes (0x06/0x07/0x1f) to its own pending app ("App 284" vs "App
36"), the `vStartApp` chain never reaches `FC_LBS\APP_THREAD`.  PROCNAV's map
must keep these services external, so HMI_NAVSERVER is a host process outside
the emulated set; per the fidelity rule its conf/status answers are kernel
fakable (like LPM/GPS), but their **content** must track DAPI's real media
state (next step).

Side discovery worth keeping: PROCNAV's bring-up was a scheduling lottery
purely because *every* io-object (`0x6e`) registry op parked its caller 1-5
ticks; f136 completed the SCD registry walk at t=27 s while f137-f139 were
still walking top-level `PROCESS` at t=100 s (FC_LBS present in f136 only).
`monitor::yield` now answers modeled io-object commands in place (they are
leaf requests the triton side completes in microseconds on the unit; only
`complete_osal_io`-modeled commands, none of which wait).  With this +
`ON_TENGINE=1` (seed), `GLS_284` + 8 `mbx_284` + 6 `MQB_284` pumps appeared
in 2/2 runs at t=22.6/26 s, `case5-pass` moved 25.2s -> 20.2s, and the touch
smoke test (f142, 3 scheduled taps, popup swap t=21.2s) shows the old
timeline shifted ~4 s earlier with no regression.

## r794: the media-state chain answers organically - "No SD card inserted" popup no longer surfaces

r793 established that app 0x24 (HMI_NAVSERVER), which the seed made the
provider of the FI services, is a host process outside the emulated set, so
the kernel fabricated its conf/status.  But the two services the enavi media
round actually registers (0x0006/0x0007, `dap_resinffi` media state) belong
to **DAPIAPP**: procmapengine's compiled map already registers them at
`0x0007` and DAPI answers natively.  Only the *client-side SCD resolution*
(kernel ioctl `0xb`, `find_key_by_u32_value`) steered them to the
HMINAVSERVER mirror, because the emulated registry publishes those SERVICEIDs
there (alphabetically first among duplicates).

Change: `Registry::find_key_by_u32_value_prefer` (+ shared
`HOSTED_SERVER_APPS`) lets duplicate publishers be resolved preferring
providers owned by emulated processes; the seed now publishes SERVICEID
0x0006/0x0007 under `PROCESS\PROCDAPI\DAPIAPP\SERVICES\DAP_RESINF[2]`
(key names are guesses - the real ones are in the encrypted
`ni2k_eur/DAPIAPP.ERG`; only the ids are certain from procmapengine's
traffic).  The HMINAVSERVER `CL0006`/`CL0007` keys stay (the C1 loop reads
their SERVICEID values by name) but no longer win resolution.  PROCNAV's
static map still sees the FUNCTIONAL_COMPONENT match first (alphabetical) and
keeps treating the services as external - its bring-up stays unaffected.

Result (f143, f144 2/2): during the enavi media round the registrations go to
`mbx_7`, DAPI's ServiceRegister handler accepts them (`after-service-check
(r0=1 ok) srcApp=0x121 svc=0x6/0x7`), and its own RegisterConf + ServiceStatus
arrive in the navif queue (`sm-dispatch ... class=0x43/0x44 src=0x7
svc=0x6/0x7`).  The media deciders then no longer push the SD popup: with the
genuine answer the popup `0x5e2` ("No SD card inserted") is *never* put on top
of the popup stack (`top=0x1e9/0x5e2`: 3 -> 0 operations), while before the
change it surfaced every run.  This is one less fabricated responder face,
not one more: the kernel stopped answering for these services.

Not yet done: procmapengine's 17 svc-0x26 class-0x45 map-data requests still
get no data reply from DAPI (dataset/RNW precondition), and the GUI needs a
visual confirmation that the popup really stays gone and the keyboard page
(milestone C) appears on the triple tap.

## r796: the state-6 deadlock fully unwound - missing CCA connect-confirm for the external FI providers

The startup FSM (`0x17c73c0`, state at `[obj+0x30]`, jump table at `0x17c73e4`)
advances 0->6 now (svc-availability gate passes, r795) but loops forever in
state 6 (handler `0x17c7908`), keeping popup `0x5da` on top of the GUI stack.
This round pinned the exact reason end to end:

1. Advancing 6->7 needs the out-byte set by `0x17c2760`.  With
   `bIsCCAServiceRequired(0x0a)==1` - which is genuine: the firmware's own
   `D605_registry/ni2k_eur/hminavserver.reg` ships `USE_CCA_SRV_*=1` (the
   emulated registry serves exactly those values, confirmed via the new
   `reqcheck` probes) - the gate takes the interface branch at `0x17c289c`,
   whose only byte-setting path is the loop-back `b 0x17c2788` reached when
   the inner proxy getter (`0x166c0d0` = `[obj+4]==2`, "server READY") is
   true.  `bl 0x169de68` then supplies r0=1 -> `*out=1`.
2. The gate word `[0x3a23f5c]` was observed live (dynamic write-watch
   installed from the `envigate g-vfn18` probe, heap addresses move per
   build): it starts 0, becomes 1 (CONNECTING) via the one `state=0` server
   broadcast at t=3.35 s, and is NEVER written again (watch: 0 hits over
   20+ gate cycles).
3. The setter chain is `navsrv::vSetStateAndNotify` (`0x161d3d8`, broadcasts
   to the sink vector `[navsrv+0x58..0x5c]`, sink vfn+0x8 = `0x15f48a4`
   writes `[sink+4]` and notifies the FINAVI singleton via vfn+0).  The new
   `srvsetter` trap saw it fire exactly once: `(0x38fa1dc, state=0)` at
   t=3.35 s (bt `0x17be808 0x17bf660`).
4. The producer of state=READY is the enconnect handshake:
   `enconnect-entry (svc=0x299) -> scd-lookup -> scd-openq (svc 0x0a.. app
   0x15 SENSORFI / app 0x24 HMINAVSERVER) -> post request to the provider's
   queue -> wait on OSAL semaphore (e.g. semh=0x4001b174)`.  The kernel
   answers the SCD lookup (r791) and ServiceData (r795) but nothing answers
   the connect request, so the semaphore never posts, the proxy stays at
   CONNECTING(1), and the gate can never pass.
5. The state-6 timeout branch (`0x17c7b64`, reached from `0x17c78e0` via
   `[sl+0x4c]` byte) calls `[iface+0x78].vfn+0x24` (`0x1719458`) and jumps
   to `0x17c7c18` (state:=9, popup release) when it returns != 0; for a
   state-1 proxy that path routes through vfn+0x44 (no-op `bx lr` on this
   class) and returns 0 -> no escape.  `s9-vfn24res` (probe at `0x17c7ba4`)
   confirms it never even runs while the FSM polls state 6.

Fix belongs in the CCA directory emulation (`libosal_linux/message.rs`):
apps 0x15/0x24 are not among the emulated processes, so their connect-confirm
is fabricated per the fidelity rule (logged, env-flag gated), which lets the
guest's own post handler + `vSetStateAndNotify` + broadcast + gate run
genuinely.  New probes this round: `s9gate` (state-9-skip caller, `0x1719458`
chain, svc-post events 0x8070000/0x80b9000, `cl-*` CL-client consumer at
`0x1382cf0`, sink-vector scan, bcast sites), `envigate` (LIM availability
verdict `0x161fb14`->`0x162f570`->`0x162d3b0`/`0x162d3a0`, dynamic gate-watch
+-0x100), `reqcheck` (`0x169e308` registry reads -> `USE_CCA_SRV_*`=1),
`ready-setter`/`srvsetter` traps, finavi-global slot watches (`0x2a1be8c`,
`0x2a1ce78`, stored by a static ctor at lr=0x47371944), dynamic
`navsrv-state` watch on `[proxy4c+0xe0]`.

## r796b: the gate word is the CL connection entry's state; writer located, event pipeline pinned

Extending r796, the gate object `0x3a23f58` is the *command-subscriber entry*
for service 0x000a in the navsrv CL client, and the whole state-6 gate reads
`[entry+4]`:

- `0x17c2760`'s inner getter `0x166c0d0` = `([entry+4]==2)`; the entry's
  runtime vtable is at `0x291b698` (**bss-resident, file reads useless** -
  probe `[vt+0x18]=0x166c0d0` live, offline read of that address is garbage).
- The state-1 writer is the transaction step `0x1719c8c` at `0x1719cf0`
  (`streq r0,[r5,#4]` after the send/reply helper `0x17b94b8` returns 1 =
  RegisterConf accepted). Confirmed live (earlygate write-watch, pc=0x1719cf0,
  value 1) - the fake's RegisterConf *is* consumed.
- The state-2 stores in the same page are `0x1719d74` (svc-not-required leg,
  dead for NAVI since `USE_CCA_SRV_NAVI=1` is genuine) and `0x1719ea4` on the
  `0x1719e58` branch of the SERVICE_STATUS event handler `0x1719b74`
  (entry vfn+0x1c): `0x17192ac` re-drives the entry when `[entry+4]==1`.
- f185/f186: that handler runs - but only for the svc 0x0006 entries (event
  pump `HMINavServerLib_1`, posts from `0x17c3698`, 3 total). The svc-0x000a
  entry never receives its SERVICE_STATUS event even though its 0x43/0x44
  answers are posted to mbx_289 (`cca answer posted ... live 2`).
- DAPI's genuine 0x44 (svc 0x06 -> mbx_289) carries `regid 03 00` at +0x16
  and `fe ff` at +0x14/15, ours (r755 shape) has stamped regid 0xe0.. and
  zeros there; the f187 experiment (state byte 1 for svc 0x000a) was a
  REGRESSION (state 1 = UNAVAILABLE drops the state-5 gate, `c6-svc0a` 1->0)
  and was reverted - state byte must stay 0.

Remaining suspects for the missing svc-0x000a entry event: the +0x14/0x15
field or the regid pairing consumed by `bIsDataSet`-style reference matching,
or a class-0x45 follow-up the real SENSORFI sends that our responder covers
only for GpsDataStatus requests. Next: delivery-side content dump of the
fake's mbx_289 answers + cl-entry event-id comparison.

## r796c: provider-attribution audit + event-dispatch located (session wrap)

**Audit („czy PROCNAV/procmwlx ma hostować FI?”) - answer: NO, the libosal
fabrication point is correct:**
- `procmwlx_out.out` hosts IPOD/audio-route/UPS/device-manager/demo
  (apps 0x33/0x89/0x3b/0x1b/0x800; services 0x43/0x4c/0x5e/0x77/0x93/0x888).
  No sensorfi/posfi/navifi strings in the binary; none of our FI services.
- SERVICEID 0x0a/0x1e exist in NO firmware .reg; the real unit resolves them
  through the kernel SCD runtime table. `procmw.out` (apps incl. SENSOR
  0x2739) is not in the Linux firmware at all - RTOS core.
- PROCNAV is a *client* of 0x0a (r388/r215 ground truth in
  `data/registry_compat.reg`): `STAT_tclThread` consumes GpsDataStatus;
  APPID 0x15 in the compat seed is deliberately an *external provider id*
  so replies route to mbx_21. So SENSORFI/POSFI = TE/RTOS-side provider -
  fabrication in libosal is AGENTS-rule-1 legitimate and must stay.

**Session fixes (kept):** fake answers now carry `AIL_NO_UNIQUE_ID (0xfffe)`
at +0x0e (claim-walk `bIsDataSet` parity with DAPI, f190 byte diff) and the
NAVI (0x000a) register answer re-announces the ServiceStatus (genuine
providers send status >= 2 per register; the enavi transaction consumes one
and the event walk needs the next) + overflow fix in its log line.

**Dispatch chain located:** the SERVICE_STATUS handler `0x1719b74` is called
from the keyed dispatcher `0x17b93fc` (`bl` at `0x17b9460`, f199 lr):
`obj` = navsrv-side service container `0x392bb08`, event ids seen
`0x80000218`/`0x80000215` (state-change family), key = vector index,
arg5 = 0x22, caller `lr=0x1c80c3c`. The svc-0x06 entries are members of the
container's PRIMARY vector (`obj+0x14`, filled outside `0x17ccb88` - that
only saw the connector-list registrations at `0x3b223ec`); the gate entry
(0x000a) is in the connector list but NOT in the dispatcher container.
**Next session:** find who registers service objects into the `0x392bb08`
container (probe writes to `[0x392bb1c+0xc]` count field / callers of its
add helper near `0x17ccc04`), and what makes the guest add the 0x0a service
there (compare the 0x06 chain: it completed via DAPI's genuine
RegisterConf+2xStatus+0x45 data on mbx_289); then expect
ev-entry(0x0a) -> `0x17192ac` state1 -> `0x17193b0` -> vfn+0x4c -> store 2
(`0x171ae04`) -> gate opens -> FSM 6->7->8->9, popup 0x5da closes.

## r796d (session wrap): the dispatcher container is the transaction object itself

`vecwalk` (dynamic MEM_READ watch on the first `list-add` anchor, installed
from the `list-add` trap) showed the watched vector header (data @+0x18,
count @+0x20 of container `0x392bb08`) is touched by:
- `0x17cce38` (lr=`0x17b9554`) - which decoded is NOT a walker but the
  vector **push_back** (`0x17b94b8`'s site at `0x17b9550`: appends
  `{u16 [sp+0x16], r8}` to `this+0x14` for every transaction type != 5);
- `0x17ccc04` (lr=`0x17b944c`) - the keyed dispatcher find (`0x17b93fc`).
So container `0x392bb08` == the transaction/connector object `this` used by
`0x17b94b8`; its vector is the transaction response log keyed by a u16
(the `key=1/2` in the dispatch lines), NOT the CL-entry registry. The
`0x17ccb88` registrations (anchor seen `0x3b223ec` in f196) are the real
CL-entry vector. NEXT: dynamic-read-watch the `0x17ccb88` anchor captured at
the list-add whose anchor != the transaction object (skip anchors in the
first hit's range), i.e. capture the THIRD argument pattern: anchor is the
connector list at helper-object `0x3b223d8+0x14`; find who reads THAT list
- that reader is the SERVICE_STATUS event walk that must reach the 0x0a
entry. Then: handler `0x1719b74` -> `0x17192ac` state1 -> `0x17193b0` ->
vfn+0x4c -> store 2 at `0x171ae04` -> gate opens -> FSM 6->7->8->9 ->
popup 0x5da closes -> golden triple-tap (45-50s, taps >=6s apart) with
EMU_GPU_HMI_CAPTURE_EVERY=1 -> keyboard/Enter Destination; then position
page (Krakow coords via fc-stat M8/M7 encodings, pin empirically).

## r797: late-Status injection + per-pair app-info + full gate-entry live picture

Fixes (all committed, all still default-on):
- Fake conf/status now mirror DAPI's wire shape: reg-id stamped at BOTH
  +0x16 and +0x1a; the NAVI pair (mbx_289, svc 0x000a) is handed reg-id 3
  (DAPI's own id style on that queue; `bDispatchCCAMessages` routes by
  reg-id).
- `ApplicationInfoStatus(AVAILABLE)` for EVERY unhosted server is now
  synthesized per (server, client) pair when its ServiceRegister lands
  (previously once-globally, so only the first pair got it). Without it the
  client never marks the server known. (f209: 0x0015 got it; still no gate
  event - the client-side answer routing needs more than directory state.)
- `cca_post_deferred_status`: the gate conf-store probe injects one exact
  DAPI-shaped 0x44 (len 0x1a, blob 34) for svc 0x000a onto mbx_289 AFTER the
  entry registered (the burst was always consumed earlier by the startup
  connector's synchronous Wait on the same queue). EMU_LATE_NAVI_STATUS=0
  disables. Empirical result: consumed by tid 84's enConnect Wait again,
  routed to the FI-event layer (event2app 0x81008114 x14, same in all runs)
  - never reached the keyed dispatcher.
- stepprobe: `conf-tail` site (0x1719cf4) + live per-entry dump of the
  runtime vtable slots and timeout fields. f212 ground truth, all three
  entries (two svc 0x6 + the gate svc 0x0a):
  vfn+0x1c=0x166c0e4, vfn+0x30=0x1718d3c (state 0 -> vfn+0x58),
  vfn+0x34=0x1670340, timeout armed (f18=now f1c=0x01000000); gate
  vfn+0x38=0x17b1adc returns the static singleton NON-NULL (its store-2 leg
  at 0x1719d70/0x1719ea4 is only evaluated when 0x17b94b8's transaction
  result != 1, and 0x17b94b8 = bRegisterAsync always returns 1 on push_back
  success - so state 1 is terminal there BY DESIGN: the async table waits
  for the provider's ServiceStatus EVENT).
- sink/container walk decoded: 0x17ada8 (vfn+0x4c) stores 2 into the entry
  once every [this+0x24..0x28) member's vfn+0x30 returns != 0; it is only
  invoked from the keyed-dispatch handler path (0x1719b74 -> 0x17192ac
  state1 -> 0x17193b0). The keyed dispatcher `0x17b93fc(obj, event, key)`
  is called from ONE site (`ldr pc,[ip,#0x10]` at 0x1c80c38) inside a
  per-message handler 0x1c80b40 (parses a 0x300-byte CCA message, event
  0x80000215/0x80000218, key = transaction id, arg5=0x22) that runs on the
  navsrv PUMP thread (tid 62) - the thread whose receive on the answer path
  returned EMPTY (rx-pump content=0x0) in every run: the startup connector's
  blocking Wait (tid 84) on mbx_289 steals every late answer first.
  NEXT SESSION: deliver the late 0x44 so the PUMP receives it - i.e. make
  the navsrv pump's own receive see the message (inspect what API feeds
  0x17cb574's [obj+8]: msgrcv on mbx_289 vs our Wait-only delivery), e.g.
  deliver the deferred status while the connector's Wait is NOT pending, or
  queue-deliver it as a priority message the pump's recv picks up. When the
  keyed dispatch fires with the gate's reg-id 3, the rest of the chain is
  proven: handler 0x1719b74 -> state1 leg -> vfn+0x4c -> store 2 ->
  0x17c2760 gate passes -> FSM 6->7->8->9 -> popup 0x5da closes -> golden
  triple-tap -> keyboard/Enter Destination; then position page.

## r798-r799h: state 8 -> 9 (f74 cryptcard readiness) fully decoded and closed

The state-8 hang (startup step cycling consume/register/getter forever while
waiting for the f74 "/dev/cryptcard" readiness gate) is SOLVED; f331/f333
reach `startup-step state=9` and the HMI shows the numeric keypad, then the
MAP OPTIONS menu under scripted touch. Chain, all proven with page-range
hooks (AGENTS.md: the narrow traps at 0x1718xxx/0x17b9xxx and the static
seg2 dump at -0x10000 were BOTH misleading - seg2 maps at -0x8000):

1. Notification routing (f322): the receiver 0x394cef8 keeps a {u16 ord, ptr}
   vector at +0x14; the register leg 0x17b94b8 inserts the gate under the
   connector-ASSIGNED ordinal (`ldrh [sp,#0x16]` after the conn vtbl+0x1c
   call, visible at the 0x17b9554 site), NOT under the service id.  The pump
   completer dispatches answers with key = answer halfword +0x18, and the
   dispatcher 0x17b9400 looks THAT up in the vector.  Echoing the request's
   own ordinal (6/7) hit leftover member entries; the gate re-registered
   under 0xb.. every tick -> gnf never reached it.
   Fix: regleg page-hook publishes [sp,#0x16] as `message::F74_REG_ORD`;
   the f74 (svc 6 regid 3) answer stamps it at +0x18 (completion key) and
   +0x1c (intentionally missing the pump CL lookup - the golden-era answers
   completed through the NOT-found leg 0x1c80c3c, whose gnf args a4=buffer
   ptr, a5=lib ptr are the ones that stored +4=2).  gnf count 10 -> 83.
2. Delivery timing (f323): gnf+vp-ev now stores gate+4=2 reliably, but the
   store lands AFTER the tick's getter (~1 ms after st1) and the next tick's
   consume (0x1718db8, vtbl+0x54) zeroes it before the getter can see it.
   In-window delivery is impossible: between st1 and the getter tid 62 runs
   pure guest code (no syscall => no VM lock release), and anything drained
   during the register's ack-wait is clobbered by st1 afterwards.  The getter
   leg is NOT the real exit.
3. The real exit (f329/f330): the leg vfn+0x24 at [gate+0x78] is 0x1feb794
   (runtime bytes; preceded by an `andeq` NOP-sled - the vtable entry points
   INTO it): `1 - strcmp([gate+0x7c], [sl+0x4c])`, NULL-gate+NULL-step -> 1,
   NULL-gate + step byte<=1 -> 0.  Returning 1 takes `bne 0x17c7c18` ->
   store-9.  The f74 gate's pending name [gate+0x7c] is NEVER written in the
   emulator: the genuine connector attaches it during the register round-trip
   (the real TE's ack carries the name); our fabricated ack skips it, so the
   gate stays nameless and the leg compares NULL-vs-step-name forever.
   Fix (EMU_F74_ATTACHNAME=0 disables, logged): on every state-8 tick, if
   [gate+0x7c] == 0 write the step name pointer [sl+0x4c] into it - identity
   equality -> strcmp 0 -> leg returns 1 -> state 9 at ~28-32 s, run alive
   past 50 s (GPS provider posting GpsDataStatus, PROCNAV parks normally).
4. GUI verified headless: keypad screen renders (keys 1-6 + Menu), scripted
   taps on the SAME queue as SDL open MAP OPTIONS ("Enter Destination",
   Quick Stop POIs, Route List, Avoid Road, Cancel Route, Next Page) and page
   2 "MAP SCROLL".  The second "Next Page" tap stayed on MAP SCROLL - the
   page body renders black in the HMI-only capture; check whether its widgets
   live on another layer or need the map surface.

Cleanup still owed: idq-*/st*/regleg/vpoll/gnf/lookup/clook/compres/hres/
v24body/s9path/rmv probes, f74-watch, `f74 REQUEST raw`, responder-skip logs
can all be trimmed now that the chain is closed; decide whether the mbx ack
leg (+0x10=0 stamp, EMU_F74_MBX) stays.  NEXT: the GPS position page
(Krakow coordinates; lat/lon M8 + altitude M7 encodings are empirical).

## r799i/r799i2: post-state-9 GUI forensics - keypad OK, touch OK, hardkeys dead, GPS POSITION gated by SETUP

- Keypad + touch verified (f332/f335): the state-9 screen renders the
  numeric keypad (1-6 + Menu) and scripted taps on the SDL-queue path open
  MAP OPTIONS and page 2 (title MAP SCROLL).  MAP SCROLL is a dead end:
  body fully transparent, grid taps do nothing.
- GPS POSITION page: title string va 0x2522390, table ref 0x2a58180 - it
  is a SETUP/system-settings tree page (same table neighbourhood as SETUP,
  SYSTEM SETTINGS), NOT a map-options page.
- Hardkeys (r799i): row 18 is lp=1500 DUAL: tap -> HK_BACK_LONG, 1500 ms
  hold -> HK_SETTINGS_PRESS (one physical key; setup/back sharing the row
  is by design).  EMU_KEY_TEST gained the +<secs> hold suffix.  BUT both
  the tap (back) and the hold (setup) injections deliver=true via
  GUI_KeyInjector_vKeyPressed/vKeyReleased with ZERO visible effect
  (f338/f340) - suspect the injector API is not what emits tap content
  for lp rows; next candidate is synthesizing the KBD FI sample into
  clHSI_CMKeyboard::bExecuteMessage (scan -> row jump table 0x1813620),
  which is the path real keys take.  This gates the GPS POSITION page.
- f341/f343 trap: scripted taps scheduled by wall-clock can land in the
  member era (drifts 28..35 s) and wedge the startup FSM at state 6
  forever.  r799i2: scripts rebase from the s9path B9 fetch
  (STARTUP_PAST_9), relative spacing kept.  Do not gate on
  hmi_fsm_state() - it is the currently-stepping process's state (f344).

## r799j-open: user GUI session feedback (2026-10-21) - three open bugs, evidence captured

User interactive session findings:
1. THE DESTINATION KEYBOARD NEVER OPENS: tapping Enter Destination /
   New Address shows a spinner that never resolves. That screen's
   address-entry backend request is unanswered (candidate: navsrv
   address-entry / POI-database service on PROCNAV, which sits PARKED
   with svc 0x350564 yields). NEXT: scripted repro (EMU_TOUCH_TEST
   2:70:330,6:180:160 - the r799i2 rebase makes it land after state 9),
   then trace the FIRST unanswered CCA/mbx request after the tap
   (responder logs + mbx_289/mbx_7 traffic + which CL transactions stay
   open). This is the real "pojawienie się klawiatury" the user means.
2. INTERMITTENT state-6 stall (new variant, g1.log): startup reached
   state 6 at 34.5 s (era drift), fabricated FI-ready txns 1,2 landed,
   but member transactions 3..0xa NEVER issued their requests (0 pushes
   after 35 s vs 13 pushes inside the 27-28 s era in f342 which then
   closed 6->7->8->9 in <100 ms). So SOME precondition of the member
   request loop is missing in slow-era runs - and it is NOT the responder
   (it never even saw the requests). Probes burned their budgets early
   (conn-b 88/90 in BOTH runs) - raise budgets / filter to era window
   before next comparison. g1.log vs f342.log are the diff pair.
   CORRECTED (g4 vs g3): the stalled runs had EMU_TOUCH_TEST set but its
   taps were NEVER injected (STARTUP_PAST_9 gate held them - grep shows
   zero 'touch-test tap' lines in g1/g2/g4), so this stall is NOT
   tap-induced.  Perfect correlation found: the stall happens iff the
   startup era starts LATE (g1/g2/g4: era at 34-36 s -> stuck; g3/f342:
   era at 27.7-28.2 s -> 6->7->8->9 in <100 ms).  f345 (era ~36 s) did
   close, so late eras CAN close - it's a race inside the late-era path,
   roughly 50/50.  Host load (late era itself correlates with load) is
   the suspected driver.  Repro recipe: run under CPU load until era >33
   s; diff member request issuance at era time (push histogram via ISO
   timestamps, see the python snippet pattern in this file's history).
3. HARDKEYS (interactive SDL path): Setup opens the Setup screen on EVERY
   press (short!), but Esc (Back) does NOTHING now - yet it DID work for
   the user after ceac5f1 (verified later, before the state-9 work).
   Row 18 is lp=1500 press=HK_SETTINGS_PRESS short(tap)=HK_BACK_LONG.
   Both keys pin row 18; the user's observed asymmetry means the bridge
   likely emits the press-slot content (SETTINGS) immediately on
   vKeyPressed, not on the lp timeout (contradicts the r796 model), and
   Back's tap content HK_BACK_LONG is a no-op on the current screen (it
   worked on the old state-8 popup screen - context-dependent, so maybe
   NOT a regression!). Verify: (a) probe GUI_KeyInjector_vKeyPressed
   0x1368138 leg taken on tap, (b) scripted back from the MAP OPTIONS
   popup (rebase makes this scriptable now), (c) whether HK_BACK_PRESS
   exists in ms_apcStrings and on which row (it is absent from the
   keyinj row dump - so 'HK_BACK_LONG' as the tap content may be right
   after all and Esc-on-main-screen legitimately does nothing).
GPS POSITION page: still gated behind Setup-tree navigation - the user
sees the Setup screen, so once (3)/(1) are sorted, walk it with scripted
taps (capture the Setup screen first - its layout is unknown).

## r799k/r799k2: state-6 late-era stall FIXED (3d1af7e, d21fa60) - enConnect eats our fabricated mbx_289 answers

Full root-cause chain (g4-vs-g3 era diff, confirmed by forced reproduction):
1. The era's member/connector completions (svc 0x0006 txn 1/2 via
   `fabricate_fi_ready_push`, gate svc 0x000a txn 3 via `push_gps_answer`)
   ride mbx_289, which has TWO legitimate consumers.  tid-84's enConnect
   Wait pops nearly everything while idle and silently drops class-0x45
   messages that are not its own transactions (r799d note).
2. Late-era runs (era 34-38 s, correlates with host load) push the answers
   up to 1.4 s BEFORE the startup connector's pump tick reads the queue ->
   enConnect eats them -> `f-accept` never pops txn=2 (h7) or the gate
   txn=3 (h9) -> the step loop aborts at iteration 3 (conf-store 3/13,
   members 3..0xa never register) -> `g7-vfn18res` getter stuck 0 (h7/h8)
   or flips to 1 while the gate entry still waits (h9) -> FSM state 6
   forever.  Healthy eras win the same race by ~100 ms of margin; the
   earlier g-m18 re-fire hammer could not help because it lives INSIDE
   readygate (0x17c2880), which is never reached in this scenario.
3. Fix: FI_ERA_PENDING map in message.rs tracks every fabricated answer
   for the navsrv-consumer services {0x0006,0x000a}; `f-accept` (0x17c_b5ec
   probe) removes (svc,txn) when the pump consumer actually pops it; the
   startup connector's readiness-getter leg 0x17c7938 (`g7-vfn18res`, fires
   on every pump tick while the FSM gates on it) re-pushes all still-
   pending answers while hmi_fsm_state() is 5 or 6 (bounded 40/pair,
   EMU_FI_ERA_REFIRE=0 disables, every re-push logged).
   Even healthy eras needed it: i1 re-fired the gate txn-3 3x before the
   accept - the race is won "by luck" every normal run.
Verification: i2 = exact h7 stall recipe (late era 37.7 s, taps): state
7/8/9 close at 37.7 s, 53 re-fires, all 3 taps fired.  i3 late era closed
at 38.4 s.  Healthy runs (h1,h4-h6,i1): re-fires bounded, state 9 @28 s.

Remaining from r799j-open:
- Bug 1 (Enter Destination spinner): headless blind-tap repro did NOT find
  the button - Menu(70,330) opens the MAP OPTIONS popup, but (180,160) AND
  (180,280) both open page id=0x400024ca titled "MAP SCROLL", and
  (530,160)/(530,280) do nothing (the popup body is transparent in
  hmi_raw, so captures cannot locate the button).  NEXT: capture a run
  WHILE THE USER taps Enter Destination interactively (their GUI session,
  RUST_LOG=...=info to a file) - then diff the first unanswered
  transaction right after the tap (responder + f-accept + pumptrace
  probes are already live).  Suspect stays: address-entry service in the
  parked PROCNAV.
- Bug 3 (hardkeys): user says Setup opens on EVERY press (even short) -
  consistent with `GUI_KeyInjector_vKeyPressed(row)` emitting the +0x0c
  press content immediately (bypassing the clHSI lp timer, which lives in
  the real-KBD path we skip).  Back/Esc = same row 18 -> emits SETTINGS
  then BACK_LONG on release; net "nothing" on the main screen (HK_BACK_LONG
  is a no-op there; it worked historically on the SD popup which it closes).
  NEXT: probe 0x1368138 (vKeyPressed) logging the emitted content id per
  press, then decide: keep (row18 press=SETTINGS IS the real tap content?)
  vs move Back to a dedicated row / synthesize at KBD-FI scan level
  (clHSI_CMKeyboard::bExecuteMessage, scan->row table 0x1813620) so the
  lp timer runs and tap/hold split works for real.

## r799l: ENTER DESTINATION found headless + spinner's gate identified (j1/j2)

The blind-tap "missing button" was a capture artifact: `EMU_MAP_DUMP_DIR`
(window default-framebuffer dumps, r760/r768) shows the same hidden body -
the popup page really renders no button art (its GUI_Image textures bind
handles 0xe0000000/0x10000000, the r780 silent no-op pattern), but the
HIT-BOXES are live.  Tap (150,120) or (180,160) on the page opened after
Menu(70,330) IS "Enter Destination":
- stateview enter id=0x400024ca view=0xf6 ("MAP SCROLL" titlebar is just
  the container's last-set title),
- `GUI_MessageBoxSystem::parseMessage opcode=1001` then 1002 (the busy
  dialog the user sees as the spinner),
- the spinner really animates: repeated GUI_Button vDraw on obj 0x3b1ecf8
  / 0x3b1ea48 (j2 EMU_VDRAW_LOG 28000:42000, 12+ redrives per tick).

Gate: after the tap NOTHING leaves the HMI - zero routecca-create, zero
enavi-req-check, zero ail-svc/IPC, zero mbx writes beyond the known FI
flood.  What it waits on is in-process: `hsa-waitstate hsa=0x3908d98
waitmask=0x4000000 reqmask=0x0 proxy=0x38eee88` (j1 t=34494) with
`hsa-set-syncmap a1=0x291ef50` one tick before - the HSA sync-map BIT 26
(0x4000000) is never set.  NEXT: find which service/entity owns bit 26 of
the sync-map (walk 0x291ef50 descriptor table -> the bit's service id),
then supply its ServiceStatus the same way the era gate is fed
(cca_post_deferred_status / fabricate_fi_data_request) or fix the real
precondition if the owner is PROCNAV (still parked).  The descriptor
words at file 0x2916f50 are pointers into 0x194xxxx-0x196xxxx (strings?
vtables?) - decode in the next session.

## r799m: spinner wait decoded to the last instruction; break is ONE level earlier than r788 thought

j1/j2 (state-9 world, tap (150,120) = the Enter Destination hit-box):

- `hsa-set-syncmap` (0x1879b88) decoded: 4 instructions, NO arguments read -
  it hard-codes `waitmask([hsa+0x80]) = 0x4000000`, `waiting([+0x68]) = 0`.
  The getter `clHSA_Navigation::ulwWaitSyncState` (0x18928e0) returns "not
  ready" while the mask is set.  So 0x4000000 is THE dest-entry-prepare wait
  bit by construction; the dialog = the busy spinner (`GUI_MessageBoxSystem
  opcode 1001->1002`).
- BUT the request leg that the real unit issues for this dialog NEVER RUNS
  here: `hsa-setevent` (0x18a3394) and `hsa-set-destenv`
  (0x1879af8/vSetWaitEv_ReqPrepareDestEnvMap) have ZERO hits after boot
  (r788-era had waiting=1 set - today it is 0, the setter never ran);
  the DAPI `srvctrl GetWorker` probe (still installed, budget 4096) sees
  ZERO jobs (r787-era: post-tap type-4 dap_map_tclManager/type-6
  rnw_tclManager churn + PROCNAV IOSC MQB_1/MQB_3 sessions - today PROCNAV
  is fully parked before the tap and nothing wakes it).
- Event layer: the FI event 0x81008089 (r789's route to
  `clHmiNavServerHandler::vInitDestinationEntry(mode=6)`, slot idx 0x59) is
  absent from the WHOLE run; the tap window shows no `event2app` at all
  (bEvent2App 0x1352930 never consulted for the action), only the periodic
  0x810080a8 (44x, unrelated) and the dialog's hsa-event 0x87/0x8e +
  obs-notify pair.  The opened popup's companion view is cid=0xf6 (246),
  NOT r787's spinner companion 0x28d (653) nor the keyboard page
  0x285-area - today's tap opens a *smaller local* busy dialog, not the
  dest-entry session bootstrap.
- Interpretation: in the state-9 world the softkey/action behind the hit-box
  no longer resolves to the dest-entry LIST-action (r786 lead:
  `dynlistconf_NAVI__DESTENTRY_LIST2/_row_entries` dynamic-list config feeds
  those buttons, the media analogue is fed by its CCA service).  The
  request-engine and PROCNAV paths from r787/r788 are NOT the first break
  anymore - the GUI action binding is.

NEXT (in order):
1. Ground truth cheap: capture the USER's interactive session log
   (they see the real button labels in the visible window) - tap Enter
   Destination + New Address, keep the log; compare the tap-window event/
   action ids against j1's.  If their flow DOES fire 0x81008089, our
   scripted coordinates simply hit a different widget and only the
   visible-window ground truth pins it.
2. Static: find the action id -> FI event table (GUI_ContentString ids in
   the action messages, `[1, slot, w2, w3]` pump posts), see which action
   id posts event 0x81008089 and which content string "ENTER DESTINATION"
   binds to on the MAP OPTIONS popup page - dynlistconf rows likely come
   from the missing dest-entry dynamic list (r786 open question), fed by
   CL data = the same unregistered getter 0x206.
3. If the action table binds 0x81008089 only when the dynlist is populated,
   the fix target reverts to r788's: register/feed the dest-entry data
   provider (getter 0x206 / bdp element 0x08076000) - via PROCNAV's
   session per fidelity rules, NOT a synthetic event.

## r799n: GPS CURRENT POSITION page REACHED in emulator; setup-tree recipe decoded; spinner flakiness pattern

Self-generated ground truth (scripted keys+taps + 1 s window dumps, k1..k20
in /home/marek/Temp, PNGs /tmp/opencode/sp1[0-9]/,sp2[0-7]/):

- Hard-key model refinement (completes r799i): a SINGLE EMU_KEY_TEST entry =
  one Press; row-18 tap content (HK_BACK_LONG) only navigates back - from the
  nav/map function pages that lands ON the SETUP page (0x40002ed1).  The
  HK_SETTINGS_PRESS content needs the hold suffix, and the page CLOSES on
  Release - so the working recipe holds the key long:
  `EMU_KEY_TEST="1.2:nav,1.4:nav,2.2:map,2.4:map,3.2:setup+15"`
  (nav+map pairs first so 0x40003c2c exists for BACK_LONG to bounce from).
- SETUP page (Audio/System/Navigation/Telephone&Bluetooth) then renders
  (ac53ffea); tap Navigation (180,252) -> NAVIGATION SETUP content view
  0x40003017 renders FULLY with live CL data: Route Options / Guidance & Map
  Settings / MyPOI Settings / Distance Units / Show Position Information.
  Deeper: (180,112)->OPTIONS (Route Mode Fastest, Minimis...), (180,240)->
  POI SETTINGS (Download MyPOIs Now...), Distance Units row toggles km->Miles
  (write path proven!).  => the CL pump + navsrv data path WORKS for the
  setup tree; the dest-entry spinner (r799m) is NOT a global data outage.
- CURRENT POSITION page 0x400030aa OPENS from Show Position Information
  (180,425): titlebar CURRENT POSITION + Lc/La/Alt rows + Save button render
  (window_0035 sp26/sp27) - BUT a grey "Starting navigation." system dialog
  covers the values and survives plain taps; dismissing it is the open item.
- Spinner-vs-content flakiness pattern (k10-20, same recipe 3x pass 5x fail):
  the 0x40002ed1 busy container resolves to content ONLY if a tap lands on
  the list area (y~240-252) WHILE the spinner shows (~B9+6 s).  A tap at
  y=425 or (2,2) there never resolves it.  Strong hint for r799m: the
  content-view fetch is driven by the touch/pump tick, not by the CL push
  alone -> the dest-entry dialog likely needs the same missing tick, not
  missing data.  (Touch-tap #0 in every EMU_TOUCH_TEST run fires clamped at
  B9-arrival, later taps keep their 6 s spacing.)
- Tooling: EMU_MAP_DUMP_BUDGET knob added (gpu/mod.rs); 1 s dumps + stateview
  id timeline + per-tap now=33;ms logs are enough to drive the whole GUI
  headless.  Guest-ms ~ dump-seconds + ~5 s.

NEXT:
1. Find what dismisses the "Starting navigation." sysdlg (both string cases
   have read-hooks installed but the 40.5 s hit was not logged - check hook
   budget/branch) and whether Lc/La/Alt then show the Krakow fix values.
2. r799m spinner bug: try reproducing the "tap-on-spinner resolves content"
   effect for the dest-entry dialog - identify the tick the tap delivers
   (vDraw re-eval? timer kick?) and feed it flag-gated the same way for the
   Enter Destination dialog.

## r799n2: BOTH spinners share one root cause candidate: mediator timer slot [m+0x24] == NULL

- k19 tap-window decode (t=34399-34411): touch vReleased -> msg-fire x3 ->
  viewpump n=545 -> SM-PAGE onMessage(sm=0x39e2468 msgid=0x0 type=0x25a
  sid=0x415) -> performTransition -> stateview leave/enter 0x40003017.  The
  page SM is MESSAGE-DRIVEN: content was ready but nothing posts it a message
  until the touch does.  On the real unit the periodic re-check comes from
  the GUI_ContentFunction mediator timer.
- Probe evidence (k19): `mediator-timerarm` 23 hits ALL with `tmr=0x0`
  ([mediator+0x24]=NULL, flags=0x3a); `mediator-timerfire` ZERO hits ever.
  timerarm (0x1375670) sets flag 0x40 and asks the timer OBJECT
  [[m+0x20]] vtable+0x10 to start; timerfire (0x13755d8, clears 0x40 and
  jumps to the bCheckState tail 0x13754b0) can therefore never run ->
  busy container + dest-entry dialog never re-evaluate.  Same for the
  r788-era keyboard wait.  THIS is the missing kick for both spinners.
- Creation chain decoded (all static, prochmi): constructor 0x13746c0
  (GUI_ContentFunction-sync variant) stores +0x24=0; setter site 0x13751cc
  `bl 0x136e0d0; str r0,[r5,#0x24]`.  0x136e0d0: new(0x18) + vtable GOT
  0x2a0abac(&OSAL_s32TimerCreate) + `bl 0x1357218` - and 0x1357218 returns 0
  iff `bl 0x1357ed8` (timer-manager singleton getter) returns 0.
  0x1357ed8: instance=[[GOT]]; if NULL -> `bl 0xfb69b0(r0=rel data,
  r1=rel data, r2=0x4a)` create-attempt then reload; empty instance =>
  getter NULL forever => [m+0x24]=0 (matches probe) and OSAL_s32TimerCreate
  is NEVER called (k19 log: zero `sTimerCreate` trace hits - consistent).
- NEXT (runtime-first, per AGENTS hook rules): page watches on 0x1357000 +
  0x136e000 + 0x1375000 (dispatch on exact fetch addrs 0x1357ed8/0x1357f2c/
  0x1357f04-r0/0x136e0d0-r0/0x13751cc-r0) to see WHICH leg dies; decode
  bl 0xfb69b0 args (two GOT-relative structs, id 0x4a=74) - likely an scd/
  class-factory lookup for the timer-mngr service that fails in the
  emulator; fix = make the genuine in-process registration succeed (fidelity
  OK: this is HMI-internal, not a fabricated IPC answer).
- Side notes: MAP hard-key (row 17 tap) ALONE opens 0x40002ed1 + SETUP page
  ac53ffea (no setup key needed); the busy-container content getter fetch
  (s9gate cl-entry) runs DURING content-view build (34419ms) - data is
  pulled on demand, not pushed.

## r799n4: F6 "Starting navigation." popup wait FULLY decoded - it waits on a 0x1-domain datapool element that nobody writes

User-visible bug: pressing F6 (NAV) opens the Navigation screen and the
grey "Starting navigation." sysdlg; it never closes.  Root cause chase
(n1..n17, nav-only key flow, stuck objects stable at med=0x3a03128 /
desc=0x3970005 / content 0x25b0114 id 0x40003630):

- Popup identity: dynstack-op `sm=0x39e1fc0 op=0x42b popup=0x5da kind=0
  prio=0xca slot[0x5da]=0x39f8d78 child{id=0x40003c2c view=0x25f}`.
- Close path: msg type 0x49 (`ldr r0,[msg+4]; bl 0x1364044` at 0x1376190,
  handler 0x1376088) closes popup [msg+4]&~0xf8000000.  sysdlg-49 probe:
  type-0x49 flows freely (129x content 0x5a) but 0xc00005da NEVER appears.
- Wait machine: GUI_StatePseudoMediatorSync for content 0x40003630,
  constructor 0x1374f30 (medsctor probe) seeds [m+0x28] from ([content+4]&0xf):
  kind0->0(+err 0xb7), 1->1, 2->7, 3->2.  The F6 popup row is kind 3
  (write-watch caught `0x1374fd4` storing 2).  Heartbeat (SM tick, caller
  0x13716ec) runs every ~25-40ms: wrapper 0x13757cc (or 0x13756ec after a
  data-update via 0x13706f4) sets 0x20/0x10, calls Start 0x1375150, vStop
  0x1375140, tail-jumps vCheckFire 0x13754b0.  Write-watch (medflags) shows
  the flags cycle 0x2a->0x3a->0x1a->0x2a forever; bit 0 (timer mode) is
  NEVER set for kind 3 => Start (0x1375174 tst #1) never creates the timer -
  BY DESIGN.  The 0x10/0x20 re-arms are no-ops (0x137567c tst #1 pop-eq).
- THE decision: vCheckFire with (flags&0x1d0)==0x10 and bit1 set takes
  0x137559c: `bl 0x1374e4c([content+8])`; `cmp r0,#1` == 1 => 0x13754f0 leg
  (count dispatch, wait satisfied, popup pops).  Probe at 0x13755a8:
  `ANSWER=0x0 content=0x25b0114 c8=0x3970005` on EVERY tick - never 1.
- 0x1374e4c decoded: `r1=[[data]+0]`; only ids with domain nibble 0x1
  (0x1xxxxxxx datapool elements) are dispatched to the element-handler
  (vfn+4 exists, vfn+8 count).  ANY other value => `mov r0,#0` straight
  away.  [0x3970005+0] = 0x3970005 (self-pointer, never written after
  construction - medflags watch) => the wait's element binding is EMPTY.
- Datapool evidence (dpset-filter widened to the 0x1 domain): 0x1xxxxxxx
  element writes happen ONLY during boot (0x100c000/0x101d000/... at 3.1s,
  v0=0, writer bt 0x13cb834); ZERO 0x1-domain writes around or after F6
  (28.4s+).  The "navigation initialization finished" element transition
  that a real unit gets (from the navsrv/CL feed) never happens here -
  exactly the same missing CL pump as the dest-entry spinner (r788:
  bdp_Register 0x08076000 zero) and the busy container (r799n2).
- r799n3 contradiction RESOLVED: Start IS reached for this mediator via an
  indirect `mov lr,pc; ldr pc,[r3]` thunk - the narrow/page probe on
  0x1375150 missed it (unicorn HOOK_EXISTS_BOUNDED quirk, AGENTS design.md);
  the write-watch on [m+0x24..2c] is the trustworthy observer.  Factory +
  timer-mngr singleton are healthy (tmfac: returns non-zero, tmngr inst
  0x2cb94cc) - timers ARE created for kind-1/2 mediators (0x13751cc hits).

NEXT:
1. Identify the element id the wait row expects: dump rows around
   0x25b0114 and the sibling desc 0x397003d for a 0x1xxxxxxx word (the boot
   writes prove the pump exists at 0x13cb834; the popup's element was bound
   to NOTHING - [0x3970005]=self).
2. Find the binder that should store the element id into [desc+0] (writes
   to 0x3970005 after t=3507ms: medflags watch shows NONE) - likely the
   same LI/IOSC session bootstrap that starves the dest-entry CL pump:
   fix the PROCNAV session precondition ONCE and all three waits
   (F6 popup, dest-entry spinner, busy container) should resolve.
3. Re-check: sysdlg-49 must then see pid=0xc00005da and the popup frame
   must vanish from the dump within a tick.

## r799n5: popup close-cascade decoded; probe VA-bug fixed; FI 0x81008089 injection built (flag) but the HSI event route is still closed

Corrections to r799n4 (probe bugs, not guest bugs):
- The tmset/ANSWER probes' `rd()` shifted every static read by 0x8000
  (runtime VA == file VA for this image, base_address == ORIGINAL_BASE):
  `c8=0x3970005` was garbage read from 0x25a811c.  The wait desc IS the ROM
  row 0x25b0114 itself and `[desc+8]` is the STATIC 0x900006b7 (raw reads in
  the old mediator-sync probe show it all along).  No "empty binding".
- The shared probe budget (4000) was exhausted at ~13.7 s, silently
  discarding all late-era hits; now 60000 with a per-(pc,r0,r1) throttle
  (F6 ids 0x900006b7 / mediators 0x3a03128 / pid 0xc00005da get cap 12).

Close cascade (nav-keys flow, n28..n34):
- Popup 0x5da DOES get removed twice per run: startup era ~13.7 s and
  post-F6 era ~29.8-30.0 s (`dynstack-op kind=1`), and is even re-pushed
  under op 0x208.  Close driver (MEM_WRITE watch on sm+0x2c..0x3c):
  pc=0x1376e88 inside the SM purge loop 0x1376e00 - for each stack record
  it calls `bl 0x13a6740(smmgr, recOp, recKey)`, which returns true iff a
  wait-list entry {op,key} exists with `[entry+0x15]==0` AND the domain
  registry has an object for the key with `[obj+0x46]!=0` (GUI-content
  ready flag); the entry flag is then set.  So the REMOVE is a GUI-content
  readiness gate, NOT the data wait: the heartbeat wait on content
  0x40003630 (mediator 0x3a03128, wait 0x900006b7) keeps ticking after the
  popup visually pops.  Cosmetic close != the user's goal.
- Wait 0x900006b7 is never evaluated at all: `tmq` watch shows 0x1374e4c
  runs 1128x for other ids (0x900000xx, 0x1xxxxxxx, 0x5xxxxxxx), QHND
  dispatches domains 5/7/8 via handler objects 0x391a210/0x391a230/
  0x391a250 (domain table slot 9 = 0x391a250, vfn+4=0x13a6288), but for
  0x6b7 neither the 0x1374e4c leg nor the wait-table lookup 0x13a5040
  (tmwt watch, filter id&0x7ffffff==0x6b7) is EVER reached.  Satisfaction
  is push-based (data write -> pool notify -> vOnDataUpdate wakes the
  wait); nobody writes the nav-init element, so the wait just sleeps.
- 0x49 close-handler (`sysdlg-49`) closes ids < 0x300 only (wrapper
  0x1364044 `bic/cmp #0x300/bhs`), 0x5da is out of range by design; lr
  0x1364094 on its dynstack ops is stale (cascade from inside 0x1377370).
- The kind-3 satisfaction leg 0x137559c/0x13755a8 (the old ANSWER probe)
  never executes in n24+ runs while it DID in n13-n22 => mid-TB fetch-hook
  dispatch depends on TB-translation history (AGENTS unicorn quirk); stop
  trusting fetch probes for 0x13754xx/0x13755xx, use the write-watch.

FI 0x81008089 injection (EMU_FAKE_DESTENTRY=1, default off, logged):
- `fabricate_fi_destentry_event` posts a gps_data_status_answer_body-shaped
  class-0x45 message carrying msgid 0x81008089 on NAV key press: routes
  tried = mbx_265, mbx_289, pump_push_completion (native guest post).  All
  post succeed but event2app (0x1352930, per-event throttled now) never
  sees 0x81008089: the pump completer forwards only msgids completing a
  live transaction; 0x81008114 arrives this way via the gate's own
  txn-ordinal-3 request.  Event producers are in-process (bt at the
  event2app trap: frames 0x180a984/0x18641a0/0x18b0b38/0x1827b64 +
  consumer adapters 0x135a70x/0x135a75x/0x135a7ax reading through the
  0x1827810 singleton's vfn+0x18); no static bl-callers exist for any of
  these (function-pointer glue), so the genuine post site must be found
  from the CONSUMER side at runtime.
- NEXT: probe the consumer adapters (page 0x135a000 watch, targets
  0x135a728/0x135a74c: after `bl 0x1827810` dump r0 and the message body
  at r1) to learn the in-process FIFO record shape, then either (a) call
  the manager's post vfn via guest_call_via_lr on NAV press (bridge class,
  same as key injection), or (b) fix the real producer: the r787-era tap
  chain shows the GUI action table posts the event, and today's action is
  unbound because the dest-entry dynamic list config (getter 0x206) is
  still empty - i.e. keep chasing the CL provider registration.

r799n5b (same day): wait-entry 0x6b7 CONTENTS decoded (runtime WTBASE probe):
- Wait table base 0x28f1718 (file), 0x14d2+ entries, stride 12:
  entry = {type, id1, id2}.  [0x6b7] = {0xa, 0x50001161, 0x400000b2} - a
  type-0xa two-element wait: domain-5 element 0x50001161 AND domain-4
  0x400000b2 (content-class membership pair; compare [0x1a2]={0xc,
  0x700000f5, 0x10000001}).
- Neither 0x50001161 nor 0x400000b2 is EVER queried (tmq QID=0 hits) and
  domain-5 queries stop at 0x5000006d.  The whole 0x6b7 subtree is simply
  never re-evaluated: satisfaction is push-based (pool write -> notify ->
  mediator wake), and the writer is the navsrv/CL data path.  Closing the
  popup genuinely therefore REQUIRES the data (matches the user's goal:
  the city list needs the same feed); no framework kick exists.
- Domain-4 ids that ARE queried: [0x6b6]={0xa,0x50001160,0x90000726} (the
  sibling wait) shows the encoding pattern; 0x50001160/0x50001161 look
  like consecutive content-CLASS ids allocated by the dest-entry/nav
  session bootstrap - they only exist once the session runs.

## r799n6: FI 0x81008089 now reaches vInitDestinationEntry(mode 6) - bridge through the guest's own receiver; blocker moved to the empty dest-entry element list

- PROCNAV status clarified: tid 16's NAVEND park (`monitor-yield(svc 0x350564)`)
  IS the real unit's idle main loop (docs in monitor.rs); tids 67-83 are live
  worker waits and tid 72/77 keep ticking - PROCNAV is healthy, it is simply
  never asked anything.  tid 19 is a one-shot bootstrap that self-deletes
  (IOSC cmd 0xd000a, then OSAL thread delete(0)=self) - benign; tid 66 died
  against a null teardown continuation (139) - investigate later, it matches
  an exited helper on the real unit? (open question).
- FI receiver is ALIVE: fi-recv per-event throttle (boot 0x810080a4 GUARD-FAIL
  flood used to eat the flat probe budget) shows the clFi_Navi object
  0x38f9970 passing guards from ~12 s on (trace=0x38ca510, f2c/f30 set); only
  the 0x80a4 boot-time calls failed.
- Injection final form (EMU_FAKE_DESTENTRY=1): host calls the guest's own
  `clFi_Navi::ReceiveEventsFromEvtEng` 0x14bcd74 via guest_call_via_lr with
  (FI_NAV_OBJ, 0x81008089); FI_NAV_OBJ is stashed by the event2app-target
  probe from live 0x81008114 dispatches.  mbx/pump routes stay as fallback.
  Result (n43): fi-recv PASS -> fidispatch idx 0x59 -> slot b->0x14bfb2c ->
  `vInitDestinationEntry` 0x15a7228 mode=6 PASS -> 0x1578d88(locinput,1) ->
  case-6 tail 0x15a73a8.
- Remaining break (n43/n44): the case-6 tail iterates the handler's
  dest-entry element list ([hnd 0x38f99b0 +0x88..+0x8c], stride 0x30) and
  skips issuing the wait event when it is empty; de-list write-watch shows
  ONLY zeroing writes (clear 0x166681c, pc 0x200fae8/0x200fb60) every ~50 ms
  in the F6 era - no push/populate ever.  hsa-mask [hsa+0x80] is written only
  twice, at 2831 ms by 0x1884620 (ctor zero); hsa-setwait (0x18a3394/
  0x1879af8/0x1879b88) stays at zero hits => no ReqPrepareDestEnv request is
  ever issued => no srvctrl job => no PROCNAV session => no city list.
- NEXT: find the list POPULATE site: watch is in; the pusher must write
  [list+0x1c] (reserve/push at 0x16668xx family does the reverse).  Likely
  feeders: the CL entries registered via li-elemlist4 (locinput 0x3a24c40,
  a1=1 ran at 32792!) or hminavserver's element registration leg (r787-era
  dap/rnw tclManager jobs).  Trace what 0x1578d88(locinput,1) should do with
  the locinput's own element list and why it adds nothing to hnd+0x88.

r799n6b: hnd+0x80..0xa0 stays untouched after the ctor (n45: de-list writes
only until t=2678ms; F6-era = zero); hsa-waitstate getter (0x18928e0) no
longer runs at all in this era (the r788 busy-dialog evaluation is gone),
so the HSA base 0x3908d98 could not be re-confirmed live - hsa-list watch
(0x3908e18..0x38) added; it fired zero writes in n46, suspect the ctor site
0x1884620 family builds the HSA (writes [r7+0x80]=0,[r7+0x8c]=8) but the
base identity needs a live capture (NEXT: stash r7 at 0x1884620 once into a
static and re-point both watches dynamically at runtime).

## r799n7: CMNAVI 0x2950 poll decoded - it DOES ask the CL adapter; the answer stays "not ready" because nobody writes 0x8076000

- de-list/hsa-list watches (post-ctor): both element vectors are dead from
  their constructor on (writes only 2.6-2.8 s, ctor + vector-init 0x16640dc
  lr 0x15e87c8 + periodic clears); hsa ctor captured live at 0x18844dc
  (a0=0x3908d98 - static base was right after all).
- clHSI_CMNAVI::bExecuteMessage(msg type 0x2950, polled ~30 ms): consumer
  0x1881644, this 0x390ff48; [this+0xc] = CL adapter 0x38eee88 IS installed
  (cmnavi-obj watch: written 2836 ms by 0x180a9e8 lr 0x180a9e8 - earlier
  "null sub-object" reading was WRONG).  Path: adapter vfn+0x14(1,&byte) x2
  readiness polls (byte results 0/0), then 0x187a230 -> vfn+4 with the
  0x8076000 entry (s9gate cl-entry r0=0x38eee88 r1=0x8076000) -> outputs all
  zero (obj+4=0 = value not set) -> message answered d0=0 - the poll loop
  IS the genuine re-evaluation machinery, starving only on the value.
- So the wait chain from the HMI side is FULLY ALIVE end-to-end (event ->
  vInitDestinationEntry -> CMNAVI poll -> CL entry lookup).  What no
  emulator run ever produced: the WRITE of 0x8076000 (dest-env prepare
  result) by the RNW/DAPI->PROCNAV leg (r787-era type-4 dap_map_tclManager
  / type-6 rnw_tclManager srvctrl jobs + MQB_1/MQB_3 IOSC sessions).  That
  write is also what fills wait 0x6b7's pair (0x50001161/0x400000b2) and
  feeds the city list (getter 0x206) - one fix, three symptoms.
- NEXT: on the DAPI side, dap_map/rnw_tclManager workers ARE registered
  (srvctrl types [0x2 0x18 0xc 0x6 0x16 0x4 0x17 0xe]); find what client
  must POST a type-4 job for ReqPrepareDestEnv (r787: the tap chain did).
  The poster lives HMI-side behind the 0x187a230 vfn+4 miss or the RNW
  client's CCA subscription - trace the DAPI worker wake (GetWorker zero
  hits since boot in F6 era = nothing posts, not a worker-lookup failure).

## r799n8: open(2) directory semantics (EISDIR/ENOTDIR) + r788b "domain 0x80 cannot mount" theory DEAD

- procbaselx SPM-slave pool loader opens existing directories
  (.../datapool/fff0, /001c) with O_RDWR|O_CLOEXEC; our FS returned ENOENT
  where a real kernel answers EISDIR.  open(2) now mirrors Linux: write
  access on a dir -> -EISDIR, read-only dir open succeeds (fd for
  stat/readdir), O_DIRECTORY on an existing regular file -> -ENOTDIR
  (OpenFileError::{IsDirectory,NotDirectory}, fcntl.rs ARM flags verified
  against arch/arm/include/asm/fcntl.h: O_DIRECTORY=0o40000=0x4000 was
  already correct).  Loader proceeds through all pool subdirs now;
  vSetError(0x72008) count 15->9.  .tmp probe ENOENTs are genuine
  (fresh-flash units lack them too; pool persistence is not the blocker -
  DP* files exist nowhere in firmware/SD, and PDATA_MNGR_LM.DAT was
  self-bootstrapped by prochmi to 122 KB).
- dp-configure re-read (n50): mounted cfg is built at runtime (0x2a1ed1c
  differs from file bytes), 52 domains incl. 0xdf and 0x105 - domain ids
  are NOT capped by count (r788b conclusion was wrong).  Decisive evidence
  against the whole "domain missing" theory: sibling element 0x8070000,
  SAME domain 0x80, resolves to a live pointer object (obj=0x38f99b0
  obj+4=ptr) through the same adapter 0x38eee88 while 0x8076000 returns
  obj=0x2950 obj+4=0.  Mounting works; the VALUE was simply never written.
- clFi_Navi instantiates in-run (13.9 s, lr 0x15f7db0) - late, but the
  pool config predates it at 2.7 s and sibling elements still work, so
  late FI creation is not the data blocker either.
- Remaining single root cause (unchanged from r799n7): nobody writes
  0x8076000 (dest-env prepare result).  Writer chain = hnd+0x80 descriptor
  map (empty since ctor -> case-6 tail never issues
  vSetWaitEv_ReqPrepareDestEnvMap) -> HSA mask 0x100000 -> srvctrl type-4/6
  jobs -> PROCNAV MQB sessions -> CL write.  The map INSERTS never execute:
  de-list watch (now filtered to guest-code pc, cap 200) shows zero
  post-ctor writes.  NEXT: find the insert-caller for the hnd+0x88/+0x8c
  vector - static scan of stores to [r,#0x8c] with stride-0x20 growth in
  the hminavserver init range (0x15e8xxx..0x15f0xxx), and check whether it
  is gated on a CCA/registry flag the emulator's registry_compat.reg
  already fakes (pattern of the r790e USE_CCA_SRV_* publication).

## r799n8b: prochmi_out.out is THUMB-2 - all prior static ARM-word scans (bl scans, store patterns) were void

- Full linear Thumb disassembly recipe (capstone, resume +2 on invalid):
  saved listing /tmp/opencode/full_t.txt (9.45M insns, code range
  0xfb7080..0x23beb1b).  Re-scan for stride-0x20 push (store [r,#0xc] near
  lsl #5): only 6 hits, all jump-table garbage -> the container's push is
  a generic helper not pattern-matchable this way.
- Runtime facts unchanged and STRONG (MEM_WRITE watches are trustworthy):
  container at hnd+0x80 {begin +0x88, end +0x8c, stride 0x20} receives zero
  post-ctor pushes; class API located (0x1661dc4 set-entry-key-by-idx,
  0x1661df4/0x1661e5c/0x1661e8c get-by-idx field +8/+0x10, 0x1661e90 find,
  0x16620fc keyed find, 0x1661da4 reset(+0x90.. sentinel mvn)).
- The r788 inst-probe anchors for keybrd/spm (0x13e7000/0x1469a08) are
  garbage mid-table sites (never fire); inst-navi/inst-hsa-nav-constr do
  fire.  clFi instances actually materialize via clFiFactory::CreateFi
  (r784: KEYBRD id 51), not via those pclGetInstance sites - the KEYBRD
  sibling-container watch (sib-list, kept) therefore stays silent; note
  the keyboard page does not use the CL element-container machinery at
  all, so it is not a valid healthy-sibling control anyway.
- NEXT: find the CL content PROVIDER registration behind getter 0x206
  (r788) inside the hminavserver code range in prochmi - grep full_t.txt
  for stores of 0x206 / for 0x40003630 (dest-entry content id) references
  to reach the registration function, then check which precondition
  (CCA service start / registry_compat flag) gates it.

## r799n8c: dynlistconf string consumers = ZERO; decoded registry hives contain no dynlist keys

- MEM_READ watches on the three rodata strings (dlc-list/_row_entries,
  dlc-row dynrowconf_, dlc-conf dynlistconf_): the ONLY touch in a full
  run is ONE byte at t=985 ms of "dynlistconf_NAVI__DESTENTRY_LIST2" from
  libc code (pc 0x4737e498 lr 0x47371638), immediately after DAPI's
  FC_LBS registry read burst and just before the finavi-global list-sel
  write t=990 - the comparison aborts at the first byte and the row-config
  strings are never read at all: the dest-entry dynamic-list registration
  dies before consuming its own config name.
- Firmware registry source hives are DECODED at
  Firmware/D605_registry/ (ni2k_eur/*, NAVAPP.REG, DAPIAPP.REG, ...) - and
  NONE contain dynlist* keys, so the rodata defaults-table IS the config
  source; the registry is not the missing feed (registry_compat.reg needs
  no dynlist publication).
- NEXT (entry point is now pinned): trace the consumer context around the
  t=985 ms libc comparison - stack-scan backtrace from a widened read-watch
  (read-watch lr is inside libc, so capture sp words like event2app does)
  to name the guest function performing the default-table lookup, then
  read its miss-branch: why does the NAVI__DESTENTRY_LIST2 entry not get
  its dynlist registered (likely the same first-break as the empty hnd
  container - both are fed by that registration).

## r799n8d: the 985 ms dynlistconf read was a FALSE positive (malloc consolidation)

- Stack dump (sp=0x7ff3c728: words 0x183d70, malloc ptrs, libc bases) shows
  the read happened inside libc malloc/_int_free housekeeping (lr 0x47371944
  is libc-internal, pc 0x4737e498 = memcpy/consolidation loop) touching a
  stale heap pointer that pointed INTO the rodata string - not a consumer.
- Conclusion stands harder: NO code ever reads the dynlistconf NAVI
  DESTENTRY strings in a full run -> the dynamic-list registration path is
  never entered at all.  dlc read-watch stays installed (with stack dump)
  for future capture.

## r799n9: container census - ALL 38 element-containers empty; DAPI receives svc-0x1e ServiceData requests but never acts

- de-ctor trap (0x16640dc): the hminavserver init (lr chain 0x15e8a40..
  0x15e8c2c, stride 0x78 objects 0x38fa470+) builds 38 containers at boot -
  ALL get end=0 and NONE is ever pushed into (de-end watches, pc only the
  ctor itself).  So the fill is NOT boot-static - it's demand-driven per
  content, and NO content in the unit-emulator ever gets data.  The empty
  dest-entry list is one symptom of a GLOBAL missing data feed.
- The wire into DAPI exists: procmapengine-side client (app 0x16) posts 6x
  ServiceData class=0x45 svc=0x1e regid=0x17 func=0x60010/0x60013 (msgid
  0x15f90) to dst=0x400 and the DAPI queue WAKES ("queue_id=1046 woke with
  materialized snapshot len=744") - but DAPI produces NO srvctrl worker
  lookup (zero hits again) and only 2 outgoing answers (svc=0x300 func=0x200).
  The requests die INSIDE DAPI's ServiceData dispatcher.
- Handler link-gate theory DEAD: all four vSvcReady guards now read live
  (+15c=0x38f9970 fsvc=0x29f75b0 +7dc=0x3a241c8 +160=0x38ca510; only the
  finavi *value* deref is 0) - 0x1576db4's [r6+0x7dc] precondition is met.
- 0x187a230 is NOT a function (jump-table region, garbage disasm) - the
  r799n7 note about its vfn+4 was based on the s9gate probe's register
  dump, keep that data but not the code claim.
- NEXT: inside DAPI, trace what its ServiceData(0x45) consumer does with
  the woken message: it reads the job, then must decide to post type-4/6
  worker jobs.  Find the abort: instrument DAPI's mbx_7/DAPDATASMBX read
  side after the wake (thread [61]), especially the svc-status/registration
  check on svc=0x1e (the CCA registration-id directory is emulator-side
  fabricable and per AGENTS is a legitimate fabrication surface: regid 0x17
  must map to a serving app).  Compare against the 2 messages that DID get
  answered (svc=0x300) to find the discriminator.

## r799n9b: the ServiceData death point - DAPI's answer leg fails in ail_bIpcMessagePost with queue=0xffffffff

- tid[61] (DAPI mbx/ServiceData consumer, waits mbx_1024 handle 0x400939f8)
  wakes on every svc-0x1e request, then posts the ANSWER back
  (message content word0=0x00160400 = src 0x400 -> dst 0x16): 8x
  OSAL_vSetError(0xffffffff)+queue=0xffffffff (lr 0x660eec,
  OSAL_IOWrite h=0x40034dcc) at t=16617 - exactly the 6 unanswered
  svc-0x1e requests + retries.  A different post to the SAME app 0x400
  succeeds minutes earlier (queue=0x40093a20), and the 2 answered svc=0x300
  requests do get replies - the discriminator is per-message, not global.
- The failure surfaces via the libosal IO-write driver method (r799e probe
  site 0x484f0a50), i.e. the IOSC/AIL app-directory -> queue-handle
  resolution for this message shape returns -1 host-side.
- NEXT (entry pinned, cheap): decode what ail_bIpcMessagePost looks up for
  the failing vs succeeding post (same lr 0x660eec): dump both message
  contents fully + trace the monitor-side queue lookup (monitor.rs
  pump-post only maps dst 1..=0xff -> mbx_{dst}; check whether the answer
  for dst=0x16 needs a REGISTRATION id (regid=0x17) -> queue mapping the
  CCA directory fabricates - and whether the 2 answered svc=0x300 requests
  differ in regid).  Fix = fabricate/complete the registration-id
  directory entry (legitimate fabrication surface per AGENTS), logged.

## r799n9c: app 0x400 = procmapengine itself (mbx_1024!); failing posts are class-0x42 self-targeted, successes class-0x45/0x49

- The "PROCMAPENGINE" probes fire in tid 60/61 - the SERVICEData consumer
  IS procmapengine (opens mbx_1024=app 1024=0x400 AND mbx_22 for its
  client side).  Wire src=0x16 requests ask procmapengine for data;
  procmapengine answers succeed for class 0x49 svc 0x16/0x20 (queue
  0x40093a20, 8 posts, tid 60) and class 0x45 svc=0x22 regid-dir posts
  succeed (queue 0x40094718/0x400943f8), but ALL 14 posts of shape
  `type=1442816(0x160400=1024 dec) content=[00160400 00000018 42000002
  0000ffff msgid 0017001e|ffff0300 ...]` (class 0x42, svc=0x1e, regid=0x17,
  subid 0xffff, dst-app 0x400) resolve queue=-1.
- 0x160400 decimal hint: ail_bIpcMessagePost resolves dst APP 1024 to
  "mbx_1024" by DECIMAL name while the emulator registers queues per
  opening process; the two shapes that resolve use non-1024 targets.  The
  resolution goes through the IOSC/OSAL service channel (the
  0xa0064-cmd-0xa family answers all-zero words around these posts).
- NEXT (concrete): find where the guest asks the monitor for "queue for
  app id" (log all OSAL-service cmds with their answers during one failure
  burst, t~16617 tid 60/61) and make the answer return the real queue id
  for mbx_1024 (legitimate emulation-side fix - it is OUR queue table).
  If the lookup is name-based decimal-string formatting, verify our
  ensure_queue("mbx_1024") table name matches byte-for-byte.

## r799n9d: answer-leg deep-dive - queue=-1 is read from the caller's own memory (not a runtime lookup), and the app-directory does not know app 0x400

- Timeline (n55): valid-handle posts (queue=0x40093a20) run at t~2-3 s
  (boot churn) and once each at t=14.3/54.3 s (svc-0x300 answers, which
  DO go out: wire class=0x45 src=0x400 svc=0x300 func=0x200x); the 14
  queue=-1 failures span t=11..55 s for the class-0x42 svc-0x1e shape -
  a never-bound, never-late-binding slot, not a startup race.
- The `-1` is the r1 ARGUMENT of ail_bIpcMessagePost (probe at 0x660ea0
  logs r1 before the call) -> the guest read it from its own port/adapter
  object; fixing this is NOT an emulator-side post-routing fix, it is
  making the guest's own bind step write a real handle there.
- Registry cross-check: no APPID 0x400 exists in the decoded hives; the
  cca directory emulation answers ApplicationInfoStatus only to HMI-state
  queries; the scd lookup answers service->app (0x1e -> POSFI 0x15) but
  the requester clearly routes the request to 0x400 (PROCMAP per
  StartConfigDefault CONFIG/PROCMAP). procmapengine registered svc 0x1e
  WITH server 0x15 and got conf+status answered (live 2) - yet the answer
  leg needs a queue handle for 0x400 nobody ever registers.
- NEXT (one cheap round): at bPostIpcMessage entry, for the class-0x42
  failing shape dump the adapter registers (r0..r4) and the candidate
  queue-handle slots [r0+0x10..+0x30]; compare with the svc-0x300 success
  (lr 0x669c58 wrapper) to find WHICH guest step must write the handle
  into the port (bind on connect-conf? self-register?) and which message
  that step waits for. That message is the emulator-side hole to plug -
  not the post itself.

## r799n9e: adapter dump pins the -1 to a per-port reply-queue slot; static disasm of procmapengine is dead (stripped phdrs)

- bPostIpcMessage failure dump (r2==0xffffffff): caller object r0=0xa07970
  {+0x00 vfn 0x685450, +0x04=0x40093a20 (valid mbx_1024 handle!), +0x0c=
  0x400939f8 (valid mbx_22-handle twin), +0x10=0x400 (own app id), +0x20=
  0xffffffff <- the slot the failing leg posts to}.  The SUCCESS legs pass
  r2=[adapter+0x04] (=0x40093a20); the class-0x42 answer leg passes
  r2=[adapter+0x20].  +0x20 = per-port PEER reply-queue, written from the
  REQUEST's reply-queue header field when the request was claimed.
- So the -1 is a pass-through: the STAT sender (same process) wrote -1 as
  its reply queue into the svc-0x1e request, the server stored it, the
  answer leg faithfully posts to -1 and dies.  Root = the STAT-side sender
  never resolves its own reply queue (no QueueCreate logged for tids
  52/60/61, opens only via fallback).
- procmapengine.out ELF program headers are zeroed (PT_NULL x8) -> static
  file disasm impossible; use runtime mem-dump (r799n6-dump pattern) for
  any code reading there.
- NEXT: dump procmapengine's GUEST OSAL queue table (libosal .bss table at
  base+0x25220, stride 0x5c, in-use flag +0x08, name +0x38 - constants in
  fallback_open_to_create) at t~16 s: confirm which queues the process
  believes it owns + their handles.  If mbx_22/mbx_1024 entries carry the
  right handles, the missing link is the sender-side reply-queue field
  write in the STAT request builder - locate via the request's 744-byte
  body (find the field's offset by diffing a request that got answered,
  svc-0x300, in the same table) and find why the STAT side leaves it -1
  (likely the same CCA admin-state gate r796 unwound for prochmi).

## r799n10-n13: guest OSAL queue-table completion + kernel reply-queue stamp; remaining gap = AIL route-record q-field frozen at creation

- procmapengine's libosal queue table (core = [[0x485679e0]], block at
  core+0x25220, NOT a flat stride-0x5c array - word-scan it) had two holes:
  entries created via the monitor's queue-create(ptr) path carry 0xffffffff
  in the kernel-handle slot (name-0x24) and `mbx_22` (created by tid 71
  CCA bring-up before procmapengine booted) was ABSENT from procmapengine's
  table entirely.  `fixup_mbx_entry_handles` now patches the -1 slots and
  inserts the opened-but-missing mbx_ queue, using a SELF-POINTER recipe:
  entry+0x0c = entry makes the entry its own info block for
  decode_queue_handle (name at +0x38), and entry+0x14 carries the handle.
- Kernel stamp: the materializer (os/osal_queue.rs) now fills wire word 3
  (blob+0x14 = body+0x0c, the reply-queue field senders build as -1) with
  the source app's queue handle read from the receiver's own guest table,
  so the value is always one the receiver's OSAL can decode.  Verified:
  10 svc-0x1e requests arrive with word3=0x4002b9e0 (mbx_22 self-handle).
- STILL broken: the answer leg does NOT take the queue from the message.
  Resolver chain fully mapped: wrapper 0x669c38 bl 0x66df4c (app, msgtype,
  msg) -> channel-record walk (16-byte records, key {u16 lo, u16 hi @+0 =
  queue halves, u16 @+4 msgtype}, getters 0x66e820/0x66e83c, creator
  0x664260 stores R1 = LITERAL -1 from its pool) -> the only record for
  the answer shape (created t=4061, lr 0x664300) keeps q=0xffff forever.
  On the unit some step must re-bind that record's q halves (u16 stores,
  not yet located); the stamped word3 is evidently NOT that source.
- NEXT: find the record re-binder - scan procmapengine file (PLAIN text,
  ARM mode, VA-0x8000 = file offset works even though phdrs are PT_NULL)
  for `strh rX,[rY]` / `strh rX,[rY,#2]` sites whose context loads a queue
  handle (0x4009-style or [obj+4]); correlate with reception of a stamped
  request (the rebind may read a DIFFERENT field of the 744-B body, e.g.
  the amt envelope header, not word 3 - diff an answered svc-0x300 request
  body fully, both halves of word0-vs-word3 usage).
- Note: procmapengine code is ARM (not Thumb); earlier Thumb sweeps on it
  were void.  The runtime codedump technique (mem_read around pc/lr from a
  trap) works for any stripped binary.

## r799n14: answer leg CLOSED (13/14 svc-0x1e answers delivered) - two root causes, both emulator-side

- The resolver fallback 0x383adc does NOT read the stamped word3: it calls
  IOSC route-open-by-app, which lands in the guest's own
  `OSAL_s32MessageQueueOpen("mbx_22")`.  Our QUEUE_OPEN trap found the entry
  but left r2's out slot untouched, so the route open returned -1 and the
  frozen `queue=0xffffffff` posts followed.  Fix in fallback_open_to_create
  (gated to procmapengine + mbx_22): complete the open from the process's
  own guest table (guest_table_handle word-scan) and return to the CALLER's
  lr - the trap sits on the open's `push {...lr}` (first instruction), so
  skip-to-addr+4 runs the body and pops a never-pushed frame (n88: tid 60
  resumed at pc=0x0).  Broad skips regress boot (n85 procbaselx/DAPI stall,
  n86 map medium never active: the stub's retry open path is what later
  delivers the adapter chan-head route-open).
- With the route open fixed, the resolver returns 0x4002b9e0 and all 14
  answer posts carry it - but none reached tid 72 (AE_16, the PROCNAV
  STAT/app-22 entity): the queue backlog histogram showed `42:N` frozen
  while 0x45s drained.  Priority inversion: OSAL 0 is the HIGHEST rank
  (mq = 7 - osal) and the fabricated sensorfi GPS/posfi providers posted at
  0, while real AIL traffic posts at 2 (bPostIpcMessage wrapper r3=2).  The
  synthetic feed outranked every answer sharing mbx_22.  Providers now post
  at 2 (procnav.rs).
- n93: 12 STAT requests, 14 answers resolved+posted, 13 delivered to AE_16
  (8-byte refs `00 04 16 00 18 00 00 00 02 00 00 42 ...`); the
  queue=0xffffffff failures are gone.  The STAT<->PROCMAP loop is genuinely
  live end-to-end with NO reply fabrication.
- Still open downstream: `s9gate cl-entry` for the city-list entry 0x8076000
  (obj=0x2950 CMNAVI) still reports obj+4=0 - other entries DO have data
  (0x80b9000 obj+4=0x038f_9b0-ish live value at t=37968), so the CL feed
  works generally but the 0x8076000 content never gets written.  Answers
  are header-only (0x18 body, class 0x42, no data blob) - consistent with a
  status ack; the DATA for 0x8076000 evidently comes from another leg.
- NEXT: identify the genuine writer of the 0x8076000 entry content: MEM_WRITE
  watch on 0x8076000..+0x100 in the PROCNAV VM (entry objects live beside the
  CL datapool), and compare against 0x80b9000 - find which code fills THAT
  one (pc/lr tells the supplier path) and why the same path skips 0x8076000
  (likely the dest-entry/city-list content request leg, svc/regid different
  from the STAT 0x1e loop).

## r799n14b: post-delivery census - answers arrive but are header-only (data-content gap is next)

- tid 72 (AE_16, the STAT/`app 22` pump living in the PROCNAV image) really
  consumes the 13 delivered class-0x42 svc-0x1e answers (monitor pop logs
  `pop class=0x42`, backlog histogram 42:N now drains).
- The STAT client (probe-sd-refres, refs `0a 00 0a 00 01 80 01 00 ff ff 01 00`
  = svc 10 / prop 1 / op 1) keeps re-requesting: the answer is a 0x18-byte
  header with word6=3 (an ack/status, NOT a data payload).  The responder
  (procmapengine server half) answers from its own containers - and those
  are the r799n9 empty set: the DATA behind svc-0x1e is still not produced.
- Also answered class-0x42 bodies on other legs carry `fe ff` (regid 0xfffe
  livelock family) - shape differs from the svc-0x300 ack (sub12=0x66f8).
- The HMI datapool mediator element 0x8076000 still gets only the byte
  dp_set v0=7 (t=12041); no pod -> get_podp()==0 -> dest-entry writes die.
  dp-add census: exactly ONE element ever added (0xc0fa000, single virtual
  call site 0x138173c vfn+0x18 -> add_element).  On the unit the CL
  provisioning adds many elements; ours never starts that loop.
- Static notes: `cldp_datapool::add_element` (0x1388180) has exactly one
  branch-to from 0x13883c0 (search of BL/BLX immediates over the whole RX
  segment); the runtime lr 0x1381740 = vtable+0x18 caller.  Element id
  0x8076000 occurs in prochmi rodata at 8 sites (0x182c840, 0x182e890,
  0x1831f70, 0x18320c0, 0x18374d4, 0x1837adc, 0x1837c28, 0x18937c8) -
  tables, no direct xrefs (aggregate bases).
- NEXT: find the provisioning pass that walks those element-id tables and
  calls the vfn+0x18 add per entry (likely a mediation-list init keyed off
  a service-connected event); watch vfn+0x18 (the add wrapper entry near
  0x1388240) for attempted elems, and diff against the svc-0x1e responder:
  identify which container the server reads when building the answer and
  why word6=3 (data-not-ready) instead of a payload round.

## r799n15-n16: svc-0x1e dataset census + CCA directory answers every client

- `ExamineInfluence` probe: the procmapengine server half answers svc-0x1e
  from admin-ref 0xa0fc70 whose dataset vector is empty (`+0x14 == +0x18`,
  storage 0xd9caf0, capacity 1) from t=12539 on - the DATA producer, not a
  lost insert, is missing.  `add_svc_dataset_fill_watch` names every writer
  of head/storage (init/clear only -> zero pushes all run).
- `observe_cca_directory` extended (kill-switch `EMU_NO_APP_INFO_SYNTH`):
  the LPM stand-in now answers every client's ApplicationInfoRegister on
  mbx_0, not just the HMI - the engine's svc-0x26 (DAPI map service)
  registration starts exactly like the HMI's, and without the
  ApplicationInfoStatus its deferred REGISTER never executed.  n109 proved
  the genuine chain: LPM class-49 -> engine posts real ServiceRegister ->
  DAPI RegisterConf -> directory stamps `MAP_DATA_REGISTER_ID` ->
  `OnUpdateServiceStateDAPIFastmap` runs (variance later, see n19).

## r799n17/n17b: STAT claim path = admin-5 leg; list layout; claim-repair A/B

- STAT svc-1e requests are served by dispatcher 0x66963c with r1=5 (wire
  class-0x45 -> leg 0x669ba0): opcode gate 0x386c60, answer built by
  0x388860 (class-0x42, fills only through +0x16), port over `[app+0x54]`
  via 0x66df4c, posted by 0x660ea0 mode 2.  `r0=1` from 0x660ea0 is
  SUCCESS (0x72000 = OSAL success code, not a failure!).
- Client-reference list layout (guest vAdd 0x65e830):
  `{vtable, head@+4, tail@+8, size@+0xc}`; posfi admin registration item
  is 36B, comparator legs 0x66e5e0/0x66e6b0 read `{+4 type=1, +0xc server,
  +0xe svc, +0xe unique? +0x10, +0x12 regid, +0x14 msg+0x1c, +0x20 msg+0x24,
  +0x22 msg+0x2c-byte}` in `[app+0x5c]`.  n122 proved the engine's genuine
  class-0x42 flows to mbx_22 and STAT rtdisp (target 0x46) consumes it.
- A/B (`EMU_NO_CLAIM_REPAIR=1`): the claim-repair bookkeeping silences the
  DAPI/echo storm that kept STAT threads spinning; with it OFF, STAT stays
  alive but the repost storms return.  Dynamic stamping of admin-item
  fields from live requests is over-matching (masking) - replace with real
  registration preconditions when the producer is found.

## r799n18: TSE flag wake made FIFO-fair (f01a98e)

`TSE_FLG_WAIT` now queues `(tid, pattern)` per flag and wakes the EARLIEST
waiter whose pattern is satisfied (an earlier waiter with a satisfied
pattern would steal the event anyway).  Before, the first thread polled by
the scheduler consumed every set and STAT thread [72] survived only by poll
order luck.  (The queue introduced two host panics - see n19.)

## r799n19 (4e10784): two host-thread panics were the entire "run variance"

- monitor.rs TSE serve path sliced `queue[..usize::MAX]` when a thread
  re-entered WAIT without a queue entry -> panic at t~7s -> the guest
  thread's host execution died -> PROCNAV boot legs, STAT workers 70/72
  and HMI app 0x26 silently never spawned.  ALL of n126-n132 hit it;
  n122/n123/n125 "good runs" predate f01a98e.  Fix: fall back to
  `queue.len()`.
- procmapengine.rs:10113 `copy_from_slice` u32-reader into 2 bytes (r799n17
  filler) panicked the engine host thread at t~13s.  Fix: cast u16.
- Lesson: a missing guest thread after ~t=12s was host death, not guest
  logic.  Check `panicked` in EVERY log before believing variance.
- n134 first clean run: CCA fanout flush 40 replies, 55 answered
  ServiceRegisters, HMI opens mbx_38 (app 0x0026),
  `OnUpdateServiceStateDAPIFastmap` RUNS (t=12645ms state=1) and opens
  fastmap channels 4/5/6; class-0x45 svc-0x26 requests reach DAPI mbx_7,
  DAPI spawns AE threads (apps 1, 22).
- Static facts: only caller of 0x512400 OnUpdateServiceStateDAPIFastmap is
  vtable slot 0x4b9fdc (gates `*0x7907ac in {1,2,3}`, arg =
  MDM-object([0x7907c8]=0xaa4950) byte +0x21 - the 0x48 "phantom" was that
  byte, a pointer's second byte at the old wrong watch 0xaa4970).  App
  interface 0xa07970 state word +0x30: 0->1->3 (vtable+0x30 leg 0x667d04)
  ->2 demotion (vtable+0xe8 = 0x66163c, entry thunk 0x65d76c lr=0x65d77c)
  at t~4.07s, same window as fmstate 1->2 (predicate leg 0x51db60).
- NEXT: (1) why does STAT thread [72] still not activate (n122 had it);
  (2) the engine's class-0x42 answer status word content+0x18 stays 3
  (data-not-ready) - who must write the real status/dataset once DAPI
  answers the fastmap channel loads; (3) then popup 0x5da close + city
  list WITHOUT EMU_FAKE_DESTENTRY.

## r799n20 (8407263): trace-device write contract + TSE waiter-id - STAT worker alive

- libosal `TRACE_s32IOWrite` (secondary entry 0x484eff30 of the device stub)
  crushed `TR_core_uwTraceOut`'s result to 0/-1, so every guest trace write
  failed the `IOWrite` success contract (`result in [sl, 0x72000]`) with
  0x72014 - 962k failures/run, engine re-posted in-flight messages.  Fixed by
  stubbing the device entry (library-level, allowed) to return 0x72000.
- TSE FIFO poll (`complete_tse` from scheduler poll/deadline context) used
  `get_data().thread_id()` = wrong thread, parked flag-waiters never woke.
  `poll_tse_wait`/`complete_for` now pass the blocked tid; parking purges a
  thread's stale entries from other flags' queues.  n141+: STAT worker [72]
  activates, opens mbx_22 via monitor service 0x290364, pops its messages.

## r799n20b (ee687b6) + r799n21 (69b2725): the claim tuple was live, not static

- Comparator 0x66e650 at the claim5c leg (0x66a2f0, `add r0,#8` runs one
  instruction later) consumes {r1=req msg+0x00 SRC, r2=fp=req msg+0x16
  register-id, r3=[sp+0x24] dispatcher case (= msg+0x1c, observed 2..4),
  r4=[sp+0x28]} vs item {+0xc,+0x12,+0x0e,+0x10}; the `bl 0x66e650` at
  0x66a2c4 (case-2 leg) pre-adds at 0x66a2b0 (probe item = r0 there).
- Static stamps (svc at item+0x0e, reg-id 0x17) matched nothing: the case
  code and STAT's register-id change per request (0x17 -> 0x010c).  The
  repair traps at 0x669c9c/0x669f3c now restamp item{+0x0e,+0x10,+0x12}
  from every request's frame before the walk.  n158: 12/12 MATCH, engine
  builds real class-0x42 svc-0x1e acks (status byte 3 AND 0 variants seen)
  and STAT pops them.
- Storm accounting fixed: repost-caller page-watch BUDGET (not dead-TB) hid
  the 80k wrapper passes; caller 0x66869c in the Post census is FILE-based
  (module+off, VA = +0x8000) = wrapper 0x670634's own libosal bl 0x670698.
- Remaining debt (300 family, ~260/s, was 35k/s): engine self-mints
  class-0x45 svc-0x300 op-1 requests (src=0x400 dst=0x16) into its own
  mbx_1024; with [sp+0x40]=[sp+0x44]=0 the dispatcher bails at 0x669df0 to
  the unclaimed exit 0x669f3c without consulting either comparator leg, and
  the sync-request helper 0x6612b8 (libosal 0x388e40 return==1 "no waiting
  requester") re-posts via 0x660ea0.  Consumer page-watch: obj 0xa07970,
  recvq 0x400939f8 (mbx_1024), handler [obj+0x60]=0xa0fc70, state word
  ping-pongs 2<->3.  Candidates left: who should park a waiting requester
  (sync-request timeout leg?) or who fills [sp+0x40/44] on the unit.
- n158 state: OnUpdateServiceStateDAPIFastmap state=1 t=12.6s (golden-era
  marker back), HandleMapDataRequest x2 (t=4.8/33.8s, tracks the posfi tick
  cadence), channels 4/5 open with shm, DAPI cryptnav opens succeed
  (MEDIUM.CFG, DATASET.CFG, tp_meta.dat, POI_MAPPING.DAT fd=14), STAT
  still re-asks svc-0x1e at ~4/s (dataset vector of admin-ref 0xa0fc70
  never pushed).
- NEXT: find the producer that pushes dataset blocks into the admin-ref
  0xa0fc70 vector (storage 0xd9caf0, watch `svc-dataset-store`) once the
  engine's fastmap channel loads complete - i.e. what must answer the
  engine's GetBlockIDs/LoadBlocks so promotion 1->2 runs; then popup 0x5da
  close + city list WITHOUT EMU_FAKE_DESTENTRY.

## r799n22: n158/n159 census - claims MATCH end-to-end, popup's datapool element identified, DAPI internal errors open

- n158/n159 (60 s): panics 0, OnUpdate state=1 @12.6 s (golden marker),
  channels 4/5 open with shm, STAT 25 svc-1e requests, 12/12 comparator
  MATCH, engine class-42 acks with status byte 0 AND 3 reach mbx_22, ack to
  the 300-family seen, STAT pops 15.  svc-300 self-post drip: 15575 (n158)
  / 6510 (n159) per run - run-variance 260/s..2.6k/s, was 35k/s.
- DAPI answers svc-26 after all: the class-0x45 `07 00 00 04 ... FM_T_4/5/6`
  events on mbx_1024 are its fastmap-channel notifications (earlier
  ack26toEngine=0 was a census-filter artifact).
- `DAPI ServiceDataError 0x215/0x218` (x5/run) belong to DAPI-internal
  request jobs (type 0xe / svc 3): the `request_svc` word is DAPI's internal
  service number (no AIL svc-3 messages exist); request-hdr now dumped by
  poCreateErrorResult.  DAPIAPP.OUT on the SD (`CRYPTNAV/DNL/BIN/NAV/COMMON`)
  is a cryptcard container, static disasm needs the container reader ->
  Ghidra-side follow-up.  Present in n134..n159 identically.
- `procbaselx` opens `/CRYPTNAV/` (volume root) 3x/boot: mounted read-only
  (main.rs), n159: no more failed.
- App 0x24 (registry THREADINFO: `AB_24,DIM,CSMA`) has queue mbx_36 with NO
  consumer (Wait(mbx_36)=0); PROCNAV (0x121) sends it class-41/45 svc-0x1f
  requests (22 msgs).  Same for prochmi's svc-0x27 posts.  Not yet known to
  matter for the popup, but it is an unhosted partner (candidates: run
  procbaselx as host of AB_24, or answer like the CCA directory).
- Popup 0x5da wait binding DECODED from ROM (prochmi file-VA = file-off +
  0x8000, .rodata): the 20-byte dynstack row for content 0x40003630 is
  {0x484, 0x40003630, elem=0x12649081, wait=0x900006b7, 0}; sibling rows use
  elems 0x12449051/0x12449060/0x12649091/0x127090a1 - all domain-1 pools.
  Word 0x12649081 exists ONLY in that ROM row (no PROCNAV/registry ref):
  element ids are computed, not literal.  Boot initializes nine 0x1-domain
  pools (0x1001000..0x101d000, writer bt 0x13cb834 = datapool wait-table
  ctor 0x13cb82c -> 0x13a5040/0x13a5060); the 0x12649x pool is NEVER
  instantiated -> the push-based satisfaction can't fire.  Pool
  instantiation = the LI/IOSC session bootstrap (r799n4 NEXT-2 holds).
- envigate `f-accept` does consume our class-45 svc-0x000a FI messages
  (rdy=1 holder=0x2a12818) but the per-message target object arrives
  vt=0/v18=0/v1c=0 (holder fields hmi+0xc/+0x24 = 0) and srvsetter stays
  state=0 twice - FI-NAVI never reaches READY on the HMI side.
- NEXT: (1) find the LI/IOSC session leg that instantiates CL pools and why
  the NAVI pool is absent (compare boot pool writer's caller table);
  (2) identify the writer that must fill the envigate holder for svc 0xa;
  (3) DAPI svc-3/type-E errors via Ghidra (needs cryptcard container load).

## r799n24: datapool-master located, ARM pool-setter decoded, connect-confirm fabricated (consumed, gate unmoved)

- procbaselx IS the Datapool Master: opens `DpMaster` queue, scans
  `/var/opt/bosch/dynamic/ffs/datapool/` (`Dp00000001.dat`, `fff0/
  DpInternData.dat`, `001c/SpmSlvDpConf.dat`) - all zero-length in the
  emulator's FFS overlay (never persisted), so DPM starts with an empty
  persisted pool registry.  Not fatal: the runtime default-init sweep covers
  the element set anyway (below).
- prochmi contains an ARM-compiled data-adapter section (file delta also
  0x8000).  `0x13cb5a8` builds a {elem,val,obj} message and calls the
  object's vfn+0x10; `0x13cb80c` (the `0x13cb834` under every boot dpset
  bt) is that vfn leg: vfn+0x0c(obj, elem, valptr) - a boolean set.
- The dp-initwatch probe (budget fixed to log domains {0x01,0x08,0x12,0x90}
  only, n162): the entire observed set-activity is ONE sweep at t=3.4-4.1 s
  (232 in-band + ~400 earlier silent) writing defaults across domains
  0x80/0x21/0x2b/0x18/..., domain 0x01 gets its nine 0x1XXY000 ids.
  Domain 0x12 (the popup's 0x12649081 family) NEVER appears, boot or late.
  => the pool is instantiated by a leg that never runs, not merely unwritten.
- EMU_FAKE_CONNECTCONF (r799n23) works as designed (n160): 8 pairs, svc
  0x000a pair delivered live x2 to mbx_289, popped by tid 81 - but no
  downstream movement: srvsetter stays state=0, no 0x1-pool init, popup
  stays.  Consistent with r796c: the svc-0x000a CL entry never joined the
  keyed-dispatcher container, so SERVICE_STATUS cannot route to it; the
  container push leg is the same session-bootstrap we are hunting.
- SCD service->app attribution is registry-driven
  (`find_key_by_u32_value_prefer("SERVICEID")`); svc 0xa->0x15 and
  0x1e->0x15 come from OUR registry_compat.reg seeds (real NAVAPP hive is
  encrypted ERG), while 0x300/0x1f/0x18/0xd/0x2c -> 0x24 (AB_24,DIM,CSMA)
  mirror base.reg THREADINFO.  mbx_36 has no consumer: PROCNAV's svc-0x1f
  requests and part of the engine's unclaimed svc-300 drip target this
  unhosted app.  Candidate future work: host AB_24 inside procbaselx or
  answer its service set like the CCA directory.
- n162 healthy: panics 0, claim-walk MATCH x12, svc-300 census 26k
  (run variance), fastmap state markers intact.
- NEXT: (1) statically map ProxyOutCCA::enConnect 0x1c826xx post+sem-wait
  legs (who posts the sem - a reply-port callback inside libosal's request
  matcher?); (2) if the matcher keys on the reply-port word (0 in our
  requests), the provider answer must flow through libosal's own
  request-response path, not a raw queue post - then the right fix is a
  host worker popping mbx_21/36 calling the responder with the request's
  full header; (3) svc-3/type-E DAPI job errors (client 0x0121 visible in
  job hdr word +0x0c) via Ghidra once the cryptcard container loads.

## r799n25: enConnect body decoded (post-only), finavi slots are linker-initialized, popup wait needs a page-era transaction

- `ProxyOutCCA::enConnect` (0x1c83520) is ARM code (prochmi mixes ARM and
  Thumb in .text!): cs-guard -> queue resolve via `scd_bGetAppIdFromServiceId`
  0xfc1378 (app at proxy+0xa, svc at +0xc) -> build request -> helper
  0x1c83178 = plain `OSAL queue post` to [proxy+4] + success trace.  There
  is NO semaphore in the proxy leg; the earlier `semh` lines belong to a
  different waiter.  Post succeeds now (n160+), so the client-side connect
  is complete - completion lives ONLY in the keyed completer (0x1c80adc)
  fed by class-0x45 ServiceData whose txn ordinal matches entry+0x1c.
- The three `finavi-global WRITE` hits (r799n22) are ld-linux RELA/GOT
  initialization (lr 0x47371944 inside /lib/ld-linux.so.3) - the singletons
  are statically constructed objects, not runtime creations.  The list-sel
  global 0x2a139f8 is non-null since t=995 ms, so entries registered later
  (svc-0x0a at t=33.6 s) should land in the re-ticked +0x28 list.
- envigate `f-accept` accepts our fabricated 0x45 svc-0x0a answers
  (txn 5, 23205) and `note_fi_accepted` marks them complete - but the
  popup-era wait (mediator-sync row {0x484,0x40003630,elem 0x12649081,
  wait 0x900006b7}) never fires: no matching transaction with the popup's
  ordinal.  The dynstack page's own CL data-open (which should mint that
  transaction and route the answer to element 0x12649081) never runs; its
  binding leg is the remaining unknown.
- Static hunt result: 0x12-domain element ids occur ONLY in the dynstack
  ROM rows (0x25b00dc..0x25b0154 family {0x12,tab,0x90,idx}); all other
  code-sightings were f64 constants.  The ids are composed at runtime by
  the (not-yet-run) CL-table instantiation leg.
- NEXT: instrument the dynstack open path for content 0x40003630: when the
  page opens, dump who the CL-bind callback is (constructor callers of the
  mediator-sync 0x1374f30 with content=0x40003630) and what request that
  callback would post - then make that request flow genuine-shaped through
  the SENSORFI responder with the page's ordinal.

## r799n26: application libtrace texts decoded - the emulator's best diagnostic window opened

- The trace device write stub (r799n20) now surfaces record text: on the
  0x484eff30 entry the transfer struct is the THIRD arg (r2, caller stack /
  heap): `{length, sub, tag(0x8NNN-ish), fmt ptr, [arg ptrs], len, lr}`;
  the string at +0x0c is already formatted. Per-(tid,text) dedup (3x +
  every 2000th) keeps the 849k writes loggable. EMU_TRACE_TEXT=0 disables.
- n164 findings (2401 lines first pass):
  * tid 81 CONSUMES our EMU_FAKE_CONNECTCONF pair: 'vHandleSVCRegConf:
    Service 0xa registration successfull' + 'vHandleSVCStatus: Service 0xa
    gets available' (SSTATE=0x0, RID=0x3) x2, same for svc 0x2c - the CL
    client library DOES reach service-available state now.
  * 'Application 0x0109: vOnServerApplicationStatus() => ServerAppState
    changed to UNAVAILABLE' (x1) - a server the HMI (0x109) watches is
    unavailable; candidate blocker for the navsrv pump.
  * CtrlDisp decodes the DAPI errors: [FID=0xd] 0x218, [FID=0x8000] 0x215,
    [FID=0x1] 0x218 - the FIDs name which FI consumer received each error.
  * 'Application 0x001c: Wait on Queue failed 0x72010/0x72015' - app 0x1c
    (Mmgr per base.reg THREADINFO) polling with timeouts.
  * engine AIL traces 'Application 0x0400: An asynchronous answer is
    awaited' + per-message poolspace lines (MsgPool telemetry -12 odd).
- NEXT: (1) grep the full trace corpus for the navsrv/datapool producers
  around the F6 page open (t=34s); (2) chase the UNAVAILABLE 0x0109 server;
  (3) FID->request mapping for the 0x215/0x218 errors.

## r799n27: navsrv dispatch-table decoded end-to-end - the svc-6 round trip is HEALTHY

- `bHandleResponseSVCData`(0x1c7d110) -> `pRetrieveRequest`(0x1c7ddac) keyed by
  the u16 the rx-pump reads from the answer; sender `enExecuteCommand`
  (0x1c7d248) stores pending entries via `enStoreRequest`(0x1c7de88), which
  mints an IDFactory id (bCreateID 0x1c813ac) whenever the message's type byte
  (+0x1a) is not 5, and maps it to {key, svc, func, t4, ord=msg+0x18, type,
  t5=svc}.  Completion removes the entry (0x1c7d1d8, lr 0x1c7d1e4).
- n171/n172 probe series (execcmd/store/storekey/remove/lookup+answer-blob):
  every FIRST answer to a stored request HITS and completes (removes for keys
  1,2,3 follow their storekey ms later); the 9 misses of 12 lookups are all
  DUPLICATE deliveries of already-completed keys (2nd-5th copy of the same
  answer).  'bHandleResp: Request not found in Table -> message lost!' is
  therefore benign duplicate noise, NOT the popup blocker.  The whole
  property-registration plane (svc 6/0xa/0x1f, func 1) completes.
- AAPCS note re-confirmed: stack args start at entry_sp+0 (BL keeps the link
  in a register) - earlier probes that read [sp] as a stack arg were right.
- The pumptbl write-watch on 0x3920b00 watched the WRONG object (mediator was
  0x392e148/0x3941690 across runs, the table follows the mediator); the probe
  stays but its early-n16x interpretation was void.
- NEXT (popup chase, unchanged goal): domain-0x12 pool instantiation for
  element 0x12649081.  The CL completer being healthy removes one candidate;
  look for what leg is supposed to MOUNT domain 0x12 (cldp_Mngr mount vector
  watch 0x38eee88+0x20 is already live - check for late mounts) and whether
  any answer-completion handler reaches the data adapter 0x13cb5a8/0x13cb80c
  with a 0x12-domain element after t=34 s.

## r799n28: popup-close chain fully traced - blocker = nobody instantiates domain-0x12

New probes (prochmi.rs): `obsreg` (GUI_ObserverList::vRegister 0x13668a4,
dumps raw content-id words), `dpobserver` (clDPElemObserver::bInit 0x13cb5a8,
elem+proxy+mngr), storekey (post-IDFactory key), lookup answer-blob dump.

Decoded ROM family (prochmi .rodata 0x25b0080.., stride 0x14):
`{flags/period, state, content, elem(0x12-domain), waitQID}` - state 0x484
(the 'Starting navigation.' popup) = {0x10000096, 0x484, 0x40003630,
0x12649081, 0x900006b7}; sibling 0x485 = {0, 0x488, 0x400035fa, 0x12449060,
0} (wait 0 = timer variant, period from the flags word).

GUI_StatePseudoMediatorSync semantics (0x1375028 ctor, 0x1375150 vInitialize):
ROM byte+0 low nibble -> flags {0->1, 1->2, 2->5, 3->7}; vInitialize creates a
GUI timer iff bit0, and registers an ObserverList data-event receiver iff
bit1 - i.e. exactly the kind=1 (wait-driven) rows.  The callback is
vOnDataUpdate(int contentId) (0x13755d4, vtable-internal callers only).

n173 facts:
- obsreg: 268 registrations, NONE with content 0x900006b7; dpobserver: 422
  bridges, all from one caller lr=0x138dd04, ZERO domain-0x12 elements.
  => the popup's data-exists wait was never armed and nothing ever bridges
  0x12649081 - in ANY era (n17 also had zero 0x12-domain element writes; its
  med ticks came from the 0x2950 re-post storm, flags only cycled
  0x1a/0x2a/0x3a = timer/fire-attempt, data never became present).
- SM never enters state 0x484: n173 SM-DEC to-paths are [483,485]; the n17
  era re-decisions that DID include 0x484 rode trigger events r9=0xb0001c39/
  0xb000241d (174 each) generated ONLY by the msg-0x2950 re-post storm
  (trace-write bug era) - legitimate successor needed, but even n17's
  activations never satisfied the wait (see above), so the activation leg is
  NOT the blocker either.
- Real blocker unchanged: domain-0x12 CL pool instantiation.  The dp
  configure table (clHMIMngr::bConfigure, sHMIConfig->[0], 52 rows) does NOT
  contain domain 0x12; 0x12 ids exist ONLY in the dynstack ROM rows above.
  => domain 0x12 must be mounted at RUNTIME by the navsrv SERVER side.  The
  HMI opens its queues as app 0x121 (navsrv identity), and PROCNAV - the real
  unit's navsrv server - is stalled: tid 16 spins in monitor-yield(svc
  0x5c022a/...) and tid 19 exited(1), every park report since early boot.

NEXT: make PROCNAV's navsrv-server leg live (audit its monitor-yield service
requests - what svc is 0x5c022a, is the monitor's answer wrong/absent?), then
watch dpmount/dpobserver for a domain-0x12 mount and the 0x12649081 write.

## r799n28b: wait table is static ROM; nobody evaluates or mounts for the popup

- Wait-table (tmwt probe, 0x13a5040) base WTBASE=0x28f1718, stride 8,
  lim 0x14d2: entry[0x6b7] = {0x9000000f, 0x10000002, 2} is STATIC ROM (the
  dynstack family rows) - 'data exists' is evaluated against live pools at
  query time.  n174: ZERO queries for id 0x6b7 all run; the sweep only walks
  ids of ACTIVE waits (t=12010 sweep walked 0x1a2..), so nothing ever even
  asks whether 0x12649081 exists - because state 0x484 was never entered.
- cldp_Mngr mounts (dpmount watch now prints pc): ONLY ever written from
  0x1382fc4 (inside cldp_Mngr::bConfigure - the 52-domain loop) and
  0x1383938 (vector insert aux) at t~2.8s.  There is NO runtime mount leg in
  this binary at all => domain 0x12 can never be mounted via cldp_Mngr; the
  0x12-domain ids must live in a DIFFERENT pool instance (not the 52-domain
  HMI one).  cldp_Mngr::get_podp THROWS for unmounted domains (0x1382b00
  __cxa_allocate_exception leg) - and no caller ever passes a 0x12 id.
- 'Starting navigation.' sysdlg popup id = 0xc00005da (tmwrap probe notes);
  its startup-era REMOVE ran without a 0x49 close.  The golden close must
  come from the dynsm decision cascade, entered via a posted SM event
  (n17 rode trigger r9=0xb0001c39, storm-artifact).  Transition ids like
  0x1ab3 are runtime indices, not ROM literals.
- NEXT: instrument the dynsm event-post leg feeding SM-DEC (find what posts
  r9 triggers and which trigger ID transition-to-0x484 listens for); the
  SM-PAGE doTransition probe already names trans IDs per event.

## r799n29: the 30 s startup-FSM sleep decoded (state 4); blocker moved to the state-9 property gate

- The "navsrv era starts at t=34 s" mystery (n172/173/174 all): the startup
  FSM (`enavi_tclProcEnaviStartup::bRunWhenRunning` 0x17c73c0, jump table one
  slot off - `addls pc,pc,r3 lsl 2` at 0x17c73e0 makes state k land on the
  entry at 0x17c73e8+4k, confirming the documented state-6 handler 0x17c7908)
  stalls in STATE 4 for ~30.0 s (n174: state=4 @4329 ms -> state=5 @34309 ms;
  n172/173 identical within 100 ms) and then runs 5->6->7->8->9 in 0.7 s.
- State 4 handler 0x17c755c: `r4 = enavi_tclRegistry::u32GetHmiNavServerThreadWait()`
  (0x169e558; opens a registry key, default literal 0x2710 = 10000 on the
  key-open-fail path, queried U32 returned on success; no matching value
  exists in the shipped D605_registry .reg files) then `OSAL_s32ThreadWait(r4)`
  at 0x17c7590 - the thread parks as `blocked(sleep)` (NOT a futex: there is
  no SHM-WAIT for tid 65; the t=34180 SHM-WAKE line lists tid 65 as the
  CALLER, and the same-millisecond `forced HMI framework app state` line is
  the standing per-tick force, not the wakeup cause).  bStartupNavIF
  (lr=0x17c75a8) then runs and the FSM advances.
- Interpretation: this is the GUEST's own startup delay (~30 s observed,
  ~3x the getter default - tick scaling or queried value unresolved), and
  the real unit plausibly boots just as slowly ("Starting navigation." for
  tens of seconds is the product behaviour).  It is NOT a missing-precondition
  stall: do not try to shrink it.  Test flows that want a READY navsrv must
  press the nav key AFTER ~35 s, not at 2 s.
- Consequence for the popup chase: with NAVIF up at 34.4 s the whole CL
  property plane DOES run (pumptrace: 13 registrations STOREd, keys 1-0xd);
  the state-9 leg `bRegisterPropertiesAfterDatasetIsAvailable` (0x17c6330,
  per-svc cascade gated by `bIsCCAServiceRequired`) still never sets its
  out-byte, and the g7-gate aggregate probe (holder 0x3a25270, obj 0x3a254b8,
  vfn+0x24 = `enavi_tclAction::bRun` 0x1719458) stays rdy=0.  bRun on
  state==1 re-runs vfn+0x44 and on state==2 re-runs vfn+0x20 (0x17194f0):
  state==2 -> bIsDone true; state 1/0 keep the timer re-arming.
- Key-collision suspect for the gate (from the n174 STORE/lookup/REMOVE
  chronology): the request table (mediator 0x392dfb8, map 0x392e6b0) is
  keyed by ONE IDFactory, and the STOREd ids carry the minted key in their
  low byte (id 0x3a00004 -> key 4, id 0xa -> key 0xa).  The gate registration
  (gate 0x3a254b8, svc 0x000a func 0x0f02, reg-leg 0x1719ccc at t=34446) was
  key 3; at t=34467 a svc-0x0006 func-0 optype-9 ANSWER stamped key=3 HIT and
  REMOVEd that entry (pRetrieveRequest matches key only), so every later
  GpsDataStatus answer for the gate (t=34494/34563/34730 key=3, then
  34769/34874/35019 key=0x5aa5 after bRun re-minted) missed.  Our fabricated
  svc-6 pushes (`fabricated FI ServiceData(svc 0x0006 txn N)` + materialized
  mbx_289 snapshots + fi-era re-fire) enumerate txns independently of the
  minted keys - the duplicate/stale-key flood is hack-generated and is the
  working hypothesis for the stuck state-9.
- NEXT (subtract hacks): capture the genuine client request frames the HMI
  posts (reg-leg 0x1719cc0 / IOWrite on the navsrv queues) and answer each
  EXACTLY ONCE echoing its own txn@+0x1c/func@+0x18/svc/regid; drop the
  blind-txn FI pushes and the snapshot re-materialization once the single-shot
  echo path proves out.  Expect the gate entry to reach state 2, the g7
  aggregate rdy=1, state-9 byte=1 -> state 0 -> 1 (DAPI SetDefaultDevice) and
  the golden popup-close cascade to start on its own.

## r799n30: state 9 REACHED (n176); the eraser was our own responder; the state-9 cascade decoded

- Milestone: n176 was the first run to walk 7->8->9 (34.5/34.5/34.5 s).  The
  gate key-3 REMOVAL at 34025 ms was the genuine completion this time.
- THE ERASER DECODED: it was never g-m18.  `cca_server_answer_sensorfi_data_
  request` stamped the answer's +0x18 as "result 0", so its answer to the
  gate's OWN fabricated f02 request was a func-0 optype-9 frame; the CL
  completer (lr 0x1c7eda4) matches the key only and REMOVES on optype-9 hits,
  so the responder deleted the pending gate entry ~13 ms before the real
  func-f02 data answer arrived (n175 33612 key=3 func=0 msgid 0x5000 REMOVE
  @33623, f02 MISS @33639).  Fixed: the responder now echoes the request's
  function at +0x18.  The 0x5aa5 "stale stamps" of n174 were the deliberate
  dummy ordinals of the gate-kick leg (prochmi.rs, KICK<3).
- g-m18 restore: removing its member-era re-flood (n29) stalled n175 at
  state 6 - member completions lose the mbx_289 two-consumer race and
  nothing else completes them.  Restored, era-safe: SVC6_TXN/FI_READY_ANSWERED/
  FI_ERA_PENDING are cleared at `navif-create` (the request table's IDFactory
  re-mints keys from 1 on rebuild) and implausible r1 captures (>0x100) are
  neither recorded nor answered.
- mbx_289 is a genuine two-consumer race (tid-84 enConnect pops+drops class-
  0x45 messages that are not its own transactions; the navsrv consumer pops
  the rest).  `f-accept` (0x17cb5ec) had ZERO pops in n175-n181 - the tid-65
  bCmdWait round-trips during the state legs are what actually pop answers
  (n176 removed keys 1-5,7,9,a; DAPI's GENUINE optype-8 confs answered the
  mbx_7-plane members 4,5,a themselves - fidelity holds).
- Completion shapes pinned (n176/n178): optype-9 func-0 pushes HIT entries
  (found leg + callback) but do NOT remove them; only optype-8 CONF answers
  remove (fabricated `fabricate_fi_conf_complete` now mirrors DAPI's genuine
  conf shape, echoes the entry +0x18 func word, gated to the navsrv era
  t>=32 s: first-era conf pushes poisoned the pre-navsrv registration era -
  n179 state-9 legs went silent, n180 state-9 never ran its cascade).
  The svc-0x000a gate is excluded - it completes through its func-f02 DATA
  answer.  n178 then removed keys 7 and 9 with the conf shape.
- State-9 cascade fully decoded (`bRegisterPropertiesAfterDatasetIsAvailable`
  0x17c6330): 19 `bIsCCAServiceRequired(svc)` checks in order {0x1f,0x1e,
  0x1e,0x1e,0x2c,0x1f,0x0a,0x152(pc-pool @0x17c7158),0x56,0x300,0x37,0x32,
  0x32,0x18,0x0d,0x1f,0x2c,0x06,0x7d}; a required (==1) service enters a leg
  that checks `poGetIEnaviServer()` connector slots (+0x08..+0x40) and runs
  the ActionReq property registrations (SpeechLanguage/MetricSystem/Feature,
  PosPosition... via requestCommand 0x17b94b8 -> CCA round trip); any NULL
  slot or failed vfn+0x18 returns through 0x17c64bc WITHOUT storing the out
  byte (r5 != 1 path 0x17c6494/0x17c64b4).  Probe: page hook 0x17c6000 on all
  19 cmp sites + the final (`s9cascade`, prochmi.rs).
- n178/n181: the cascade enters the FIRST leg (svc 0x1f, r0=1, r5=1) and never
  comes back - only 2 iterations per run.  tid 64/65 is NOT blocked, just
  `blocked(sleep)`: the state-9 loop is EVENT-DRIVEN (the gate-kick budget of
  3 provided the only 2 ticks; n178 went silent 24 s after 35946 ms).  n181:
  the 1 Hz GpsDataStatus ticker (prochmi-process thread, FSM-9 gated, mbx_289
  push, 6 fires) did NOT re-tick - the event only re-ticks when the NAVSRV
  PUMP pops it (msgid -> FI-event conversion lives in the pump's rx leg), and
  tid-84's enConnect ate the ticker pushes again.
- NEXT: make the ticker's pushes reach the pump consumer despite tid 84.
  Options (all host-side, libosal layer, allowed): (a) deliver class-0x45
  answers whose (svc,txn) matches NO live table entry only when the navsrv
  consumer is a parked waiter (selective hand-off in finish_guest_wait,
  the host already arbitrates); (b) reuse `push_gps_answer`'s route
  ("pump queue first" variant) from a prochmi guest-context probe that runs
  every second instead of a host thread; (c) have the era-refire leg re-push
  through the pump-event route for the ticker keys.  Then watch the 0x1f leg
  deep path (requestCommand CCA round trips - what conf/status the Speech
  property registrations need, EMU_FAKE_CONNECTCONF coverage for the property
  plane) and the remaining keys 6/8 removals.

## r799n31: the state-9 deadlock is real - `bRunWhenRunning` decoded end to end

- Ghidra decompile of `enavi_tclProcEnaviStartup::bRunWhenRunning`
  (0x17c73c0) resolves every earlier guess:
  - the FSM state lives at `[this+0x30]`; every SUCCESSFUL transition ends
    with `vtable+0x60` = `enavi_tclAction::vSignalSelf` (0x171915c): posts
    OSAL event bit 4 to the event object at `[poGetIEnaviServer()+0x4c]+0x1c`
    - that post is the re-tick of the enavi driver thread.
  - **case 9 failure path (`out-byte != 1`) returns SILENTLY - no signal, no
    timer, no state change**: a hard deadlock unless a property completion
    itself re-signals.  case 9 success sets state back to 0 (full re-walk
    incl. DAPI SetDefaultDevice).
  - case 4 shows the 30 s sleep is `OSAL_s32ThreadWait(registry wait)`;
    case 1 arms the 2000-tick sleep; case 8 completes only when
    `bIsNavigationDataAvailable` (device-name string at [this+0x4c]).
- `bwrun-caller` entry probe (page 0x017c7000, fetch == 0x17c73c0): the ONLY
  caller in a whole run is lr=0x17195d4 (the `enavi_tclAction::bRun` runner:
  vfn+0x38 -> state set -> `bl 0xfb701c` (ThreadWait) -> vfn+0x48
  (bRunWhenRunning) -> vfn+0x18 gate) and it stops calling after the state-6
  era (n191: hits only at t=4.4 s and 34.2 s).  tid 65 parks as
  `blocked(sleep)` from then on.
- `cllookup` probe (pump-event completer leg 0x1c7ed50 -> 0x1c7d110 on the
  RECEIVER table [r0+0x38]): fires 44x during states 5-7 (all tid 65, all
  lr=0x17c1cac, pump table 0x3b24408, msgobj 0x907acd20, msgref 0x11c) and
  then NOTHING in state 9 - the mbx_289 consumer loop that drives that
  dispatcher is exactly what dies with tid 65's sleep.
- mbx_36 property-echo attempts (all logged, `EMU_FAKE_CONNECTCONF`):
  - n185/186: minimal 0x20-byte data+conf pair, msgid 0x9000-family, onto
    mbx_289 - posted OK (no rejects) but never consumed in state 9.
  - n188: conf re-fires routed through `pump_push_completion` (271 posts,
    src stamped 0x0007) - zero completer lookups.
  - n190: msgid switched to the proven 0x81008114 FI family - still zero.
  - n193: body padded to the working GPS-status frame size 0x98 - still zero
    lookups for keys 0xb/0xc/0xd, cascade stays at 2 iterations.
  - the 0x5aa5 GpsDataStatus pump kicks DO get looked up in state 9 (n188
    36065 ms) - so the pump lane reaches tid 65; the filter that drops OUR
    frames lives in the rx leg around 0x17cb574.  The old narrow `f-accept`
    hook (0x17cb5ec) has zero hits - known dead-probe quirk, so n194 adds
    the `rx45walk` page probe (page 0x017cb000, window 0x17cb400-0x17cb700,
    t>=35 s) to compare the live fetch walk of a delivered 0x5aa5 frame vs
    our echoes and pinpoint the bail-out address.
- Wake driver (state-9 ticker -> guest-call `vSignalSelf` 0x0171915d,
  30 signals while 0x1f confs pending, logged, EMU_FAKE_SENSORFI=0
  disables): fired 30x, no `OSAL_s32EventPost` trap hit - vSignalSelf
  bailed on its `poGetIEnaviServer()==0 || [server+0x4c]==0` guard (the
  enavi server object stores its event at a different slot than the
  decompile's [server+0x4c], or poGetIEnaviServer() really is NULL in our
  era).  The ENAVI_EVENT waits in the log (mask 0x6e mode 1, 72 hits, last
  at t=40.3 s) belong to `enavi_tclEnaviThread::vEntry` - a DIFFERENT
  consumer than the FSM runner.  Next iteration of the wake driver should
  post the runner's own wake object once the rx45walk identifies the real
  completer delivery; do NOT fabricate the completion dispatch itself.

## r799n32: the state-9 dispatcher anatomy is fully decoded (n203/n204 disasm + Ghidra)

- `0x17c1c50` (tid 65, HMINavServerLib_1 pump): per queue object - drains
  via `0x17cb574` (OSAL receive 8B from `[obj+8]`, class-0x45 gate at
  content+0xb), decodes with `0x16fbb54/0x16fb63c`, then
  `bProcessMessage 0x1c80298` -> `bHandleSVCData 0x1c7ed50` ->
  `bHandleResponseSVCData 0x1c7d110(holder=[mediator+0x38], r1=cookie,
  r2=reqid, r3=func, [sp]=optype, [sp+4]=svcdata)`:
  `pRetrieveRequest([holder+8], reqid)` -> entry; `pGetDispatcher([entry+0xc])`
  -> `ControlDispatch::bHandleResponseSVCData` (vfn+0x10 / per-subscriber
  vfn+0x1c / vfn+0x14) -> `enRemoveRequest(reqid)` (the REMOVE we probe).
  Cookie matching is SKIPPED for entry type 3 (all subscribers notified).
- `0x17c1c30` (tid 81, mbx_289 consumer): ONLY calls `0x17cb6b0` - a drain
  that parses each message (the `NAVIF <<<<< CCASERVER SVCDATA=` traces
  prove our frames ARE parsed there) and posts an 8-byte internal event
  with data = 0 (`0x17cb668`, r2=0) - which tid 65's `0x17cb574` receive
  reads back as content=[sp+4]=0 and drops (n203 rx45body: content=0).
  There is NO path from tid 81's drain to the completer leg: the genuine
  in-era completions ran through the synchronous bCmdWait legs of tid 65
  while it was still awake (states 6-7).  All 20 echo shapes of n185-n204
  die exactly here.
- Fix (n205): `monitor::guest_call_bhandle_svcdone` - guest-calls the
  genuine `0x1c7d110` leg (unpatched guest code, ARM trampoline re-loads
  SP so the AAPCS stack args `[sp]=8/[sp+4]=svcdata` land) once per fresh
  0x1f conf at the conf-store site.  Holder captured from the last genuine
  connector lookup (CL_TABLE_HOLDER, pump-kick legs svc 0x000a excluded),
  key = the entry's IDFactory ordinal, func = entry+0x18.  Logged as
  `MONITOR svcdone: ...`, `EMU_FAKE_SVCDONE=0` disables.  Rationale: the
  CSMA property server is not an emulated process, we already fabricate
  its ANSWER; in state 9 no emulated thread can deliver it (the delivery
  wake on the real unit comes FROM that server's side), so the answer is
  handed to prochmi's own handler verbatim instead of being traced 20 more
  times.  Replaces the earlier note "do NOT fabricate the completion
  dispatch itself" - that held while the real lane was assumed reachable.

## r799n32: svcdone guest-call VERIFIED (n210) - the property plane completes

- n207/n208 first takes died instantly: the ARM trampoline encoded the
  literal pools with `Rn=SP` (0xE59D_xxxx) instead of `Rn=PC`
  (0xE59F_xxxx), so `bx lr` jumped to a stack pointer (PC=child+0x10,
  READ_UNMAPPED).  Also two `copy_from_slice` panics (prochmi's `read_u16`
  returns u32).  Fixed in n210.
- n210 (full 75 s, no panics): every `svcdone` guest-call for a live key
  runs the genuine leg end-to-end - `pumptrace remove` fires on the
  GUEST-CALL thread: key 0xb (tid 86), 0xd (tid 88), 0xf (tid 100) with
  the same lr 0x1c7d1e4 / table as the genuine in-era completions.  Each
  completion makes the cascade re-register: fresh mints 0xe/0xf/0x10
  appear 1-6 ms after each remove; s9cascade probe count 2 -> 5.
- Remaining gap (fixed this commit): the `lr=0x47dc4ec0` store leg arrives
  with r1=0, so the mint pairing did not apply (condition was o1>0x100
  only) and those stores answered ordinal 0 (miss).  Now o1==0 pairs too,
  gated to >=500 ms after navif-create so an era-1 mint can never pre-empt
  a member completion.  Expect 0xc/0xe/0x10 removes and the cascade to
  walk the remaining checks {0x1e,0x2c,0x152,0x56,0x300,...}.

## r799n32d: cascade decoded further (n211) - 13 property actions, 11 completed

- `bRegisterPropertiesAfterDatasetIsAvailable` (0x17c6330) structure now
  fully clear: per service group it walks `bIsCCAServiceRequired(svc)`,
  then for the enavi-server interface slot (0x1f->[server+0x1c] ISpeech x3
  actions, 0x1e->[server+0x18] IPresentation x1, 0x2c->[server+0x10]
  IRouteCalculation x3, 0x0a->[server+0x28] IEDM x6) calls each action's
  vfn+0x24 (issue registration - deduped while pending, one fresh mint per
  walk) and vfn+0x18 (poll: 0 = not answered -> early return, retry
  later).  13 walk iterations in n211 = exactly the 13 actions of the
  first four groups; keys 0xb..0x17.
- 11 of 13 removes hit; keys 0x11/0x12 answered against holder=0x1: the
  CL_TABLE_HOLDER stash was poisoned by a pump-lane leg on the SAME
  0x1c7d110 probe shape (r0=1, r2=1 - bHandleSVCData's null-content early
  leg over the raw svcdone body).  Fixed: stash requires r0>0x1000.
- After the 13th completion no re-walk re-entered (runner asleep, last
  tid-65 walk 35860 still saw a pending poll because of the 0x11/0x12
  misses).  With those completing now, expect the walk to reach cmp152 /
  cmp56 / cmp300 / cmp37 / cmp32 / cmp18 / cmp0d / cmp1f-c / cmp2c-b /
  cmp06 / cmp7d and finally the out-byte store (`s9cascade final`).

## r799n33: svcdone becomes a re-firing data(9)+conf(8) pair (n213/n214)

- n212 regression taught: the walk may issue THREE actions at once (the
  30 ms mint-store pairing races the serialized workers) -> added
  SVCDONE_PENDING + re-fire on the 1 Hz state-9 tick until the `remove`
  probe confirms the key (a duplicate against a completed key is a
  harmless miss).  Also hooked enRemovePropertyRegUpReg (0x1c7e284,
  type-4 removals, `upreg` probe).
- n213/n214: pairing+svcdone gates extended to every property connector
  service (Presentation stores with svc 0x1e, Route with 0x2c, ...),
  gated to hmi_fsm_state()==9 so the boot-plane member stores (states
  5-7) and the gate (0x000a) stay untouched.  Removes climbed 11 -> 33 ->
  136, walks reached cmp1e-a/b/c and cmp2c-a.
- n214 stalled at cmp2c-a with all Route keys removed: those actions poll
  the property VALUE (subscriber data leg), not just removal.  Fix: every
  svcdone now fires optype-9 (value push through vfn+0x1c) + optype-8
  (conf/remove) - the same pair DAPI's genuine completions form.

## r799n33d: the svcdone cookie is a SUBSCRIBER ID (n217/n218 cmp2c-a stall)

- n217/n218: all Route keys get removed (conf leg, optype-8) but the
  walk still returns 0 at cmp2c-a, and the guest logs
  `HResSVCD: no subscriber found -> message lost.` - the optype-9 VALUE
  leg dies before the subscriber handler, so the Route action's poll
  never sees a value.
- Anatomy (Ghidra, prochmi): `navif_tclControlDispatch::
  bHandleResponseSVCData` 0x1c7c6d4:
  - `uVar2 = enCheckErrorInformation(disp, func, optype, svcdata)` -> out
  - `vfn+0x10(disp, out, cookie, reqid, func, svcdata)` (pre-hook)
  - type-3 entries (bVar4): iterate ALL subscribers
    ([disp+0xc] list) and call `vfn+0x1c` per subscriber -> that's why
    the Speech/Presentation trios completed with the 0xfffe stand-in.
  - non-type-3: `pFindSubscriber(disp[3] = subs-list, cookie)`
    (impl 0x1c7c070): nodes {item,next}, match `u16 [subscriber+4] ==
    cookie`.  Miss -> the warn above -> the value is LOST (the entry is
    still removed afterwards by IFMediator, which is why removals and
    completion diverge).
  - `vfn+0x14(disp, out, ...)` (post-hook) runs in both cases.
- `pGetDispatcher` (0x1c7cf28): direct index,
  `disp = [[mediator+0x14] + svc*4]` (array of ~0x301 slots, lazy via
  [mediator]-factory; a NULL slot = the service was never opened -> no
  subscribers possible).
- Fix (n219): `resolve_done_cookie(uc, holder, svc, key)`:
  1. mint-bound cookie from the `execcmd` capture (`[msgptr+0x0e]`, now
     logged with a 32-byte request-frame dump `execcmd-capture`),
  2. `found`-probe binding from the pending entry itself
     (cookie = entry+0x12 = wire body+0x0e, svc = entry+0xc),
  3. fallback: walk dispatcher(svc)'s LIVE subscriber list and take the
     first non-zero id (logged `cookie -> live sub`, EMU_FAKE_SVCDONE
     territory),
  4. 0xfffe stand-in.
  0 and 0xfffe are treated as unknown.  New `nosub` probe at 0x1c7c8b4
  dumps the dispatcher + live subscriber ids at every future warn.

## r799n33e: the availability chain ALREADY runs; the subs exist (n221)

- n221 probes (`svcchg-in/svcchg-med/propinit/addsub`): the FI status
  answers (class 0x44, state byte +0x18=0 -> flag 0x1, 0x1=state->flag 0x2)
  for EVERY connector service (0x0a,0x2c,0x1f,0x1e,0x18,0x0d,0x300,...)
  are consumed by tid-81's receiver 0x3b24478 and reach
  `IFMediator::vOnServiceChange` (med 0x3920b00) natively.  No fabrication
  is needed on that plane.
- `addsub`: exactly TWO live subscribers exist - {disp 0x392e6b0, sub
  0x394cef8, id 0x1} and {disp 0x39394a8, sub 0x392c338, id 0x2} (added
  tid-65, states 7-8).  `pGetDispatcher` is a 2-slot direct array
  ([mediator+0x14], svc<=1 - the CL entries' dispatcher key entry+0xc is
  a SLOT index, not a service id); nosub showed the Route legs dispatch
  through slot 0 (disp 0x392e6b0) with the cookie still 0xfffe.
- The earlier `subs=[]` readings were MY walker bug: fw_List (vAdd
  0x1c7ce28) is list {head@+4, tail@+8, size@+0xc}, node {prev@0,next@4,
  ITEM@8}; the item pointer is at node+8, and the walk starts at the
  node list-head points to.  Fixed in `sub_walk` -> the resolver now
  hands svcdone the real cookie id 1/2, so the optype-9 VALUE leg
  reaches subscriber::vfn+0x1c instead of dying with 'no subscriber'.
- Also fixed: the `found` probe bound MINT_INFO/MINT_COOKIES on
  `[sp+0x14]` - which inside bHandleResponseSVCData is the STORED FUNC
  (`str r3,[sp,#0x14]`), not the request id; the key is r7.

## r799n33f: the cookie plumbing WORKS; cmp1f-b is a NULL-owner exit (n224-n228)

- With `sub_walk` on the exact fw_List layout and `pGetDispatcher` as
  `disp = [[med+0xc+slot*4]+4]`, the resolver hands every svcdone leg the
  live subscriber (cookie id 1/2, 'cookie -> live sub' log,
  EMU_FAKE_SVCDONE-flagged): n224 removed the 'no subscriber' warnings
  (0), the cascade opened cmp1f-b and mints climbed 0x19 -> 0x2c.
- The subhand probe (0x1c7c878, r4 = handler result): data legs
  (optype-9) return res=0x0, conf legs (optype-8) carry
  0x80000000|decoded-error (0x800002a1 for the property funcs) - the
  handler is reached and answers; the actions still do not flip.
- Disassembling 0x17c6330 shows the `cmp*` SITES are
  `bIsCCAServiceRequired(svc)` checks; the work happens in the branch
  blocks.  cmp1f-b's block (0x17c6748) is the SECOND ISpeech trio:
  iface=poGetIEnaviServer(), owner=[iface+0x14], getters
  0x1620a04/a14/a34, issues vfn+0x24 x3 then polls vfn+0x18 x3 - but in
  n228 NOT ONE poll probe fired: the block exits through the guard
  (iface==0 or owner==0, r5 cleared at 0x17c680c) before issuing.
  Probes sp2i (0x17c674c) / sp2o (0x17c6758) pinpoint which pointer dies.
- resolve_done_cookie now derives the dispatcher slot from the LIVE
  pending element (`cl_retrieve`, host clone of pRetrieveRequest:
  node{key@+8, element@+0xc}) instead of defaulting to slot 0, so trio-2
  style requests hitting dispatcher slot 1 get the id-2 subscriber.
- n229/n230: the trap_site-based sp2 probes fired ONLY on svcdone worker
  threads (our guest-call re-enters bRegisterPropertiesAfterDataset
  IsAvailable through the notify chain!) - the tid-65 walk reaches
  cmp1f-b's `beq` but no sp2 trap logged, the classic dead-narrow-probe
  quirk; all trio-2 walk probes moved into the s9cascade PAGE hook
  (SITES += sp2o/g1/i1/g2/i2/g3/res/p1/a/p2/b/p3/c) - n231 decides
  whether the owner [IEnavi+0x14] is live and where tid 65 departs.
- n231/n232/n233: trio-2 is NOT dead-guarded - iface/IEnavi(+0x14) owner
  0x...9828/0x...80f0 are live, all three vfn+0x24 bRun issues return 1,
  the FIRST poll (0x17c67c8 `cmp r0,#0`) is what exits the block.  The
  state probes show WHY: trio-1 actions (ActionReqSpeechLanguage, vt
  0x291b250+8, svc 0x0002 func 0x0d62) flip [action+4] 1->2 within 40 ms
  of our svcdone pair; the trio-2 owner object (runtime vt 0x2912158 =
  _ZTV enavi_tclProcGuidanceProperties +8 - proc-object pattern, req
  type @+0x28) stays state=1 forever although its keys are svcdone'd and
  removed.  The runtime vtable slot values differ from the file bytes
  (this binary relocates its vtables), so static slot reading is
  useless - stspy MEM_WRITE hooks on [obj,r0+8) capture the PC that
  writes trio-1's state=2; that function's preconditions are what
  trio-2 fails.
- n234: flaky-boot regression (NOT the probes): the state-4 sleep ended
  at v14.7 s instead of v36 s, and state 6 never advanced (dataset
  event missed).  The tid-60 IOWrite garbage-len spin (len=stack ptr,
  h=0x40034dcc, ~800k lines/run, chronic since n2xx, started v15.0 s in
  n233 too) is unrelated to the stall - n233 passed 6->7 while spinning.
  Re-run (n235) to confirm flakiness; if it reproduces, bisect the
  wake-up that carries 4->5 early and starves the 6->7 event.

## r799n33g: the leg plane is decoded - delivery dies at the action maps (n235-n237)

- stspy (MEM_WRITE on the polled state words): trio-1's flip writer is
  pc=0x171aa54 = enavi_tclActionCCAReqProperty::bNotifyWhenRunning
  LAB_0171aa50 (vSetErrorState-then-state=2, the ERROR-complete path -
  poll bIsComplete passes on it too).  trio-2's sub-action state word
  (proc+0x34) receives ZERO writes in the whole run: nothing ever calls
  its notify.  The state-2 flip is NOT err-value dependent (any stored
  *piVar4 not e/f -> error-complete), so the stamp-0x0f idea was dead
  on arrival: vExtractCookieRequestID (0x1c80304) proves REQUEST-ID =
  body+0x1c and optype byte = body+0x1b (NOT +0x1a!), cookie = body+0x0e.
- The subscriber the legs reach (sub id 1, runtime vt 0x291be08) is
  enavi_tclCCACommandSubscriber: runtime slots [vfn+0x1c]=0x17b95cc
  onRequestServiceRegister (registration plane, ignores the key) and
  [vfn+0x10]=0x17b93fc onRequestCommand = the REQUEST-DONE leg: pops
  action = pPop(map1=sub+0x14, key) else pFind(map2=sub+0x28, key),
  then bOnRequest/action-vfn+0x70 -> that is what eventually runs
  bNotifyWhenRunning.  No map entry for the key = silent no-op while
  the table-wide enRemoveRequest still removes the CL entry (explains
  removed-keys-with-no-flip across n224-n236).
- Fan-out (r799n33g) to every live subscriber changed nothing -> the
  key is simply absent from BOTH maps of BOTH subscribers.  orca/orca1
  probes (0x17b93fc entry, 0x17b944c post-pPop) in n238 show whether
  onRequestCommand even runs per leg and which side (subscriber map vs
  our key) loses the registration.  Suspect: the enavi action registers
  its mint into the map via requestCommand on the ENAVI thread only
  when its bRun vfn+0x48 vIssue runs - trio-1 got in (flip), trio-2's
  re-issue loop (bNotifyWhenRunning 0x8000000e re-arm path mints new
  ids every pass) re-registers under the NEW key while our legs answer
  the CL-table key from the found-probe - key pairs diverge.

## r799n33i/j: the command-plane keys and the 0xfffe cookie killer

- `rqadd` probes (`vAdd(map1/map2,key,action)` inside
  `CCACommandSubscriber::requestCommand` 0x17b94b8, sites 0x17b9550 /
  0x17b95c0) gave the missing ground truth (n245_1): every state-9 action
  registers under a sequential era key - trio-1 = 0xb..0xd (svc 0x1f,
  func 0xd62/0xd65/0xd70), the guidance proc = 0x1a.. (func 0x106
  DataCarrierRequest first, then 0x121/0x103/0x141/0x105/0x143/0x122/
  0x1a1/0x181/0x152/0x150/0x151/0x3/0x4/0x198/0x191/0x5/0x104/0x8000).
  Those keys DO reach our legs (SVCDONE_PENDING inserts) - the bug was
  the cookie, not the routing.
- `resolve_done_cookie` conflated two planes: MINT_INFO stores the FI
  SERVICE id (0x1f), while the dispatcher SLOT is entry+0xc (0 here).
  The `svc<=1` gate saw 0x1f, skipped the live-entry walk and returned
  the 0xfffe stand-in.  A 0xfffe leg still hits `pRetrieveRequest` (any
  cookie passes it), runs the dispatcher with NO subscriber match, and
  `enRemoveRequest` deletes the entry table-wide - so by the time the
  owning subscriber (id 0x1, enavi_tclCCACommandSubscriber) could be
  addressed, the entry was gone, `onRequestCommand`'s pPop found nothing
  and the action stayed RUNNING forever.  That is exactly trio-2
  (0x3a29868/func 0x106): legs every second, zero `bonr`, zero state
  writes.
- Fix: (a) resolve now ALWAYS reads the slot from the live entry
  (cl_retrieve) - authoritative, mirroring pGetDispatcher(entry+0xc);
  (b) `rqadd` binds MINT_COOKIES[key] = subscriber id (u16 at sub+4)
  the moment the map binding is made, so the very first leg carries the
  owning cookie.

## r799n33l: the cascade CLOSES (9 -> 0 re-walk) - final-site semantics fixed

- n247_4 (GOOD boot): with the trio-2 command keys answered under the
  owning cookie+holder (r799n33j/k), every property block passes
  (sp1a-c, cmp1e, cmp2c, sp2* all r0=1) and the startup proc moves
  9 -> 0 at t=40143ms - the FIRST full completion of
  bRegisterPropertiesAfterDatasetIsAvailable ever observed.
- CAVEAT decoded: the `final` probe sits on `cmp r5,#1` at 0x17c6494,
  BEFORE the store sequence 0x17c649c..0x17c64b4:
  `r3=(r5==1); if(r9==0) r3=0; if(r3) strb 1,[r9]; r0=r5`.
  So the logged `r0` is the LAST bIsCCAServiceRequired result (0 for
  svc 0x7d, the tail check), NOT the stored byte.  r5==1 && outp!=0 in
  the log means the out-byte WAS stored as 1 - do not read r0=0 there
  as a failure.
- Next problem (r799n33l fix): the re-walk parked in state 0 - the FSM
  is only re-ticked by our 1 Hz pump-first SENSORFI kick in states 6/9,
  but on the unit that broadcast runs for the FSM's whole life.  The
  kick + era re-fire now run in states {0, 6, 9} (state 4 stays clean,
  n179).  States 1-5 keep their real-event transitions; add them to the
  ticker only if the re-walk demonstrably starves there.

## r799n33q: bRunWhenRunning fully decoded + BAD-member cookie forensics

- Ghidra decompile of `enavi_tclProcEnaviStartup::bRunWhenRunning`
  (0x17c73c0) - the whole FSM switch:
  - case 0: issue DAPISetDefaultDevice `(**(*a+0x78)+0x24)(a+0x78, name)`;
    result 0 -> `OSAL_tclSemaphore::s32Wait(proc+0x1a, 0xffffffff)` (the
    action-completion semaphore, posted by the DONE-notification side,
    which then runs vSetDeviceName/bLeave/vfn+0x2c/+0x24/+0x18).
    vfn+0x18 == 0 -> return (stays); else `param_1[1]=2` (proc state
    DONE) + optional server-state set.
  - case 5 gate (5->6): bIsServiceAvailable x bIsCCAServiceRequired for
    services 0xa, 0x2c, 0x1f, 0x1e, 6, 0x152, 0x3c, 0x37, 0x56.
  - case 6: DAPISetDefaultDevice vfn+0x1c==1 (ready) -> empty name:
    vfn+0x30 re-issue; non-empty name: sem-wait + poll vfn+0x18 != 0 ->
    bRegisterPropertiesBeforeDatasetIsAvailable, byte@sp -> state 7.
  - case 7: loop u32GetHmiNavServerThreadWait/bGetDapiSupportedDeviceName
    + enCheckDataMedium == 2 (ACTIVE) per device; fall-through picks 8
    (flag proc+0x14) or 9.
  - case 8: sem-wait, bIsNavigationDataAvailable(name) != 0 -> state 9
    (+ server state 1); == 0 -> sem post + server state 5 + return.
    -> a GOOD run literally PROVED navi data available == true here.
  - case 9: bRegisterPropertiesAfterDatasetIsAvailable (the cascade);
    out-byte 1 -> state 0 re-walk (matches n247_4/n252_2/n254_1).
- BAD forensics (r799n33p follow-up): legs DO fire in state 6 now
  (n254_2: 16/leg) but every BAD leg carried cookie=0x1 only; GOOD's
  removing legs (tids 182-214, 37353+) hit with cookie=0x2 from
  alt_done_cookies' live-subscriber fan-out - the completer slot lives
  in a SECOND dispatcher whose subscriber BAD's resolve never sees.
  Fix: the re-fire loop also throws cookies {1,2,3} blind (misses =
  harmless pRetrieveRequest warns).

## r799n33t/u: the popup IS a dyn-popup on op 489; the 0x8076000 write leg RUNS but the element was never added

Static breakthroughs (Ghidra + capstone, ARM islands - prochmi mixes ARM
(0x137xxxx, 0x138xxxx, 0x13a7xxx, 0x182xxxx, 0x184xxxx code) with Thumb; the
old "everything is Thumb" belief made prior scans blind):

- medsctor bt (r799n33t): ALL 596 sync-mediator + 6 dataupdate-mediator rows
  are built by GUI_StateMachineFactory::vCreateSyncMediators (0x13727b8, ARM)
  / vCreateDataUpdateMediators (0x13726ac) called from s_vCreateStateMachines
  (hit-probe) with a ROM-template SM object {size=[r0], states=[r0+8]}:
  mediator slot idx = ([row+4] & 0x3ffff) >> 4, kind = row+4 & 0xf.
  ROM state count = 1594, so mediator slots (1595..2364) live ABOVE the real
  states in the instance table - they are pseudo-states, never named by any
  transition path.  Popup row (content 0x40003630, elem 0x12649081, kind 1)
  -> slot 2312, med built t=3894 lr=0x137286c, stored slot=0x39e4dc0 of the
  states table 0x39e29a0 of SM instance 0x39e2778, and BOUND t=3927 by
  tmbind lr=0x1372c84.  So the popup mediator exists and is bound every run;
  only its vInitialize (0x1375150) never runs.
- vInitialize runtime callers (INIT bt, n259_4): ALL 36 Starts in a run come
  from GUI_StatePseudoMediatorSync::vActivate's tail (0x137583c), driven by
  the propagator 0x13a77d0 (ARM): per activated op it calls the domain
  handler vfn+0x1c for [op+0x18] then walks the med list [op+0x14] calling
  vfn+0x24 (activate) / vfn+0x28 (deactivate).  The activated med set ==
  meds linked on the ops the SM cascade enters; the popup mediator's op was
  never activated this way (its slot-2312 table position is NOT what drives
  activation).
- The "Starting navigation." popup identity is now concrete: dyn popup
  0x5da (state objects 0x39f8d78=1499 hide, 0x39f8dc8=1498 show, content
  0x40003c2c, view 0x25f) on dynsm 0x39e1fc0, ASSOCIATED TO op 0x1e9=489:
  every dynstack-op line carries op=0x1e9.  GOOD n258_1: push kind=0 at
  12122, whole popup family kind=1 sweep at 14688-14693 (top=empty), then
  0x5da RE-PUSHED at 14698 prio=0xca - because state 489 is still in the
  decision path at 14718.  The popup therefore closes exactly when the nav
  cascade LEAVES 489 (nav-init-done) - one fix, three symptoms confirmed.
  (484 IS entered in GOOD n258_1 from 12028 on; old "never enters 0x484"
  belief was from an early-era log.)
- The 0x8076000 write leg is ALIVE (elem-read + cmleaf probes, n259_7):
  clHSI_CMNAVI::bExecuteMessage = FUN_0182b7e0 (ARM, msgid = [msg+4]).
  msg id 0x2950 (the readiness poll, ~19 leaf hits/run at t=12.03-14.74,
  tid 50) reaches the leaf 0x182cef8: arg([msg+8]) != 1 -> vProcessNextRequest
  + u32dp_set via vfn+0x1c(dp=0x38eee88, elem=0x8076000, &byte=7, 1, 0) at
  0x182cf34.  The call RETURNS res=1 = FAILED (element not in pool), so the
  poll answers d0=0 forever.  set-ret probe witnesses this every run.
- Who must add 0x8076000: NOT the cldp add-wrapper 0x1381700 (addwrap probe:
  3 calls/run, ALL elem=0xc0fa000 - the HMI creates only its own 0xc0-domain
  wait-event elements via hardcoded SM-condition checkers, e.g. 0x1840168
  reads [sp+0xf]==2 && [sp+0xe]==6 then returns 1 - bl scans over the ARM
  islands found 138 wrapper call sites, all literals 0xc0fa000/0xc101000/
  0xc0f8000.../0x2e01e000-class ids, ZERO for 0x8076000).  There is also NO
  direct bl/blx to cldp_datapool::add_element anywhere - every add goes
  through pool-vtable +0x18.  The 0x80-domain element creator must be the
  CL/mediation provisioning keyed off the service-connected event (r799n14
  hypothesis intact), i.e. the mediation method-registry pass whose handler
  tables ({elem u32, i32 handler-delta} rows, 0x8076000 first at 0x182c840,
  0x182e890, 0x1831f70, 0x18320c0, 0x18374d4, 0x1837adc, 0x1837c28,
  0x18937c8) are only referenced from 16-byte method-descriptor entries
  {table, count, 0xc0012, id} (e.g. 0x1ca734={0x1831f80,0x150,...}).
  elem-read watches prove those pages ARE read at t=12.06-14.7 by the
  bExecuteMessage dispatch itself (pool literals), but the METHOD tables
  are never walked (no add for any 0x80/0x81 elem, dp-add census stays 1).
- The walk of those mediation tables is (per r799n9 lineage) gated behind
  the hminavserver answer plane that dies with the EMPTY pump pending table
  ("bHandleResp: Request not found in Table -> message lost!", r799n27).
  pumptbl watch (t>=32s) DOES see writers (pc=0x191c44c/0x191e78c write
  state=5 + node ptrs at 35.3s) - so the insert leg exists but inserts only
  late/different keys; the 12s-era requests never land.  NEXT: lift the
  t<32s filter, correlate request-leg inserts vs completer lookups by
  (svc,key,func) and find the gate that skips insertion in the startup era.
- Side facts: aGetStateROMData=0x25a13cc (1594 rows), sync ROM rows
  0x25ad4a0 (596x0x14), change 0x25ac224 (168), dataupd 0x25b0344 (6x0x1c);
  state rows contain content+wait-id words, no name ptrs.  Mediator ctor
  stores row at [med+0x20], flags [med+0x28] (kind0->5,1->2,2->7,3->1).
  MEM_READ hooks report value=0 always (READ_AFTER carries the value) - do
  not believe value fields there.

Probes added: mbuild (page 0x1372000: builder entries + store sites),
INIT bt on tmset 0x1375150, elem-read MEM_READ watches (6 table pages),
cmleaf (page 0x182c000 code hook: leaf/set-call/set-ret of the CMNAVI
0x2950 write leg), addwrap (page 0x1381000: wrapper entry + singleton-null
/ arg5-zero / add-OK legs).

## r799n33v/w: the GOOD tail is a QUEUE deadlock, not data - mbx_289 saturates and eats its own dispatcher

Two long-held hypotheses DIED this round, one new root cause was caught red-handed:

- media-state payload hypothesis DEAD: `enavi_tclActionReqDAPIMediaStateOfDevice::
  enCheckDataMedium` (thunk 0x1614d8c tail-calls the method 0x171fd7c, ARM) was
  probed with a `mdm` trap_site + dynamic MEM_WRITE watch.  The HMI-side action
  0x907bf020 already holds dev=2, state=3, list n=1 from t=8.0 s -> the checker
  returns 3 and `bIsNavigationDataAvailable` is TRUE.  In GOOD, tid-65 itself
  evaluates it TRUE at t=40.2 s (mdm check t=40200/40205 tid=[65]).  The zero
  payload of the mbx_36 optype-9 echo never blocked anything.  The second
  object 0x3a31fe0 (state=0, zero writes all run) is never re-checked by
  anybody - inert.
- "u32dp_set(0x8076000) FAILS" hypothesis DEAD: `cldp_Mngr::u32dp_set`
  (0x1382cf0, vtable 0x28f0e18 slot +0x1c confirmed by Ghidra read_memory -
  VA != file offset for the vtable segment!) returns the WRITTEN LENGTH, not
  an error code.  cmleaf set-ret res=0x1 == one byte stored == SUCCESS.  Also
  `get_u8dID` clamps out-of-range domains to 0 (not reject), so the mounted
  52-domain vector does not exclude domain-0x80 elements the way r788b feared.
  The CL write leg is healthy; the popup is NOT gated behind it.

The REAL GOOD-tail deadlock (n262_3 + caught with new logs in n264z_7):

1. t~22 s: our zero-latency fabricated conf bursts (EMU_FAKE_CONNECTCONF data+
   conf echoes + svcdone lanes) start filling mbx_289.  The queue is
   guest-created with attr maxmsg=120, msgsize=8, blocking (new `MQ create:`
   info-log).  The guest-side libosal trace screams `MQ-FULL : Actual count
   reaches Max msgs` x127 with err 0xd02/0x1800 (its own nonblock posters
   drop).  On the real unit the CSMA peer physically paces the same confs over
   seconds, so 120 slots never fill - our burst timing is the artifact.
2. t~45.5-46.5 s: tid-81 (prochmi enConnect/mbx_36 consumer, cloned 35.6 s)
   posts its 122nd frame and the QUEUE IS FULL.  DAPI's worker tid-42 posts
   the svc-0x6 answer onto mbx_289 and parks inside blocking mq_send - new
   log caught it: `MQ send-block: [42] name=mbx_289 id=0x44c fill=120/120`.
3. tid-65 (HmiNavServer FSM) had just issued svc 0x6 func 0xb as a CCA request
   (`[CCASERVER]: TGT=0x7, msg=[MTY=0x45, FID=0xb, OP=0x3, C=1, R=56]`) and
   waits for its completion.  DAPI DID process it (tid-31 waits on mbx_7 =
   queue id 0x40d, the tid-42 answer post is the one that parks) - but the
   answer sits behind 120 stale frames.
4. mbx_289's consumers are per-message dispatcher clones spawned BY tid-65's
   pump (clone census + pumptbl list-add lr=0x17b9338).  tid-65 waits -> no
   new clones -> queue never drains -> tid-81/tid-42 stay parked -> the answer
   never reaches the completer -> tid-65 never wakes.  Circular wait
   tid-65 <-> {tid-81, DAPI tid-42} over one full queue.  Nothing else runs
   after 46.5 s; SM idles with popup 0x5da re-pushed at 14.7 s; the 489->493
   completion (ROM row 438, kind 1, evt 0 - unconditional once the work done
   signal arrives) is simply never signalled because op 0x1e9's work chain
   ends in this deadlock.

Fix (environment class, not an answer fabrication): `EMU_MQ_MAXMSG_FLOOR` in
`MqState::guest_open_or_create` floors guest-created `mbx_*` queues to 1024
slots (default; settable; `=0` restores guest-requested geometry) - the
emulator's stand-in for the vendor kernel `fs.mqueue.*` tunables.  Rationale +
kill-switch documented at the site.  New diagnostics: `MQ create:` (geometry
per mbx_* queue), `MQ send-block:` once per (tid,queue) whenever a poster
parks on a full mbx lane.

Static facts banked this round:
- Transition ROM 0x25b0408 row layout CONFIRMED by ground-truth calibration
  (evt 0x9000052a row 423: from=w2>>16=306, to=w3&0xffff=307, kind=w3>>16=7):
  kind-1 completion rows exist for 307->312, 494->502, 502->508, 510->799,
  278->279, 280->295, 290->306, 311->313 and 489->493 - the GOOD tail path
  [...,489,494,500,502,510,800,278,280,290,307] waits on exactly these.
- GUI_ContentId::NToInteger (0x1374e4c): domain-1 ids are plain 5-bit
  integers; their QRES=0 answers are semantics, not blockers.  Only domains
  5/9/b have real handler objects (0x5972a30 runtime handler table).
- SM ROM tables (0x25b0408/0x25ad4a0/0x25a13cc) have NO static xrefs
  (no pointer, no delta, no RELATIVE reloc with that addend) - only reachable
  via the runtime-scanned SM template fields; stop hunting for the consumer.
- mbx_289 = queue id 0x44c (decimal 1100!), mbx_7 = 0x40d (1037),
  NOIOSC_CB_HDR_LI_1 = 0x3ef (1007).  BlockReason queue_id prints DECIMAL in
  SCHED park lines - always convert before mapping.

Open: GOOD-era lottery went ~1/10 through n263/n264 (floor is innocent -
floor=0 streaks equally BAD; root cause still unknown).  Verify in a GOOD run
with the floor that tid-42's svc-0x6 answer is consumed, tid-65 walks past
the svc-0x6 completion, and watch whether op 0x1e9 then reports work done
(kind-1 wave 489->493, popup 0x5da stays closed).

## r799n33x: the GOOD-tail death wave and the era lottery have ONE root cause - prochmi guest-calls fabricated in FOREIGN process VMs

n264_11 (GOOD + MQ floor) proved the queue fix works (zero send-blocks,
tid-65 walked five CCA requests further: consumed svc-0x6's DAPI answer,
svc-0x18 conf-store at 44.5 s, requests up to R=61 all echoed) and then
revealed the NEXT wall: a mass death wave at t=43.4-50 s - ~930 threads
exit(1), including tid-50 (FI router) and tid-65 (HmiNavServer FSM), after
which nothing can ever advance the SM.  The wave's signature: 508 x
`Execution error: FETCH_UNMAPPED` at 0x1c7d110 (bHandleResponseSVCData) and
23 x at 0x171915c (vSignalSelf), with the dead tids attributed to DAPIAPP's
thread list.

Forensics -> our own fault, not guest logic:

- `prochmi::tick()` is called from `scheduler::run_process_loop` for EVERY
  process with THAT process' unicorn.  Its fabrication legs
  (`guest_call_bhandle_svcdone`, the s9-wake vSignalSelf spawn,
  `pump_push_completion`, `post_blob_to_guest_handle`) all target PROCHMI
  VAs and heap-allocate tramp/stack/pool on the CURRENT unicorn - so when a
  foreign process' scheduler pass happened to run the tick (or the mbx
  intercept re-fired them), the worker thread was created inside e.g.
  DAPIAPP: first fetch of prochmi's VA -> FETCH_UNMAPPED -> exit(1).  The
  pump-completion POOL `OnceLock` could even pin to a foreign heap forever.
- That also kills the completion the wave was waiting for (the svcdone
  guest-call silently fails: `unicorn.mem_write(tramp)` can succeed in the
  foreign VM while the fetch dies), explaining both the GOOD-tail stall
  (tid-65 never wakes -> exits/never asked again) and the BAD eras.
- ERA LOTTERY root cause: survival hinged on which process' thread executed
  the tick/intercept that fired each dispatch - prochmi-thread trigger =
  worker lives, DAPI/procmap trigger = worker dies + completion lost.

Fix (fidelity-neutral - these are emulator-side fabrications, allowed to be
fixed): process gates `elf_path.contains("prochmi")` on all four legs
(monitor.rs `guest_call_bhandle_svcdone`, prochmi.rs s9-wake vsig spawn,
message.rs `pump_push_completion` + `post_blob_to_guest_handle`).  Foreign
loops return false; the pending state survives and prochmi's OWN scheduler
loop re-fires the leg on its next tick (doorbell-rung by queue activity).
Verification: n265 series must show zero FETCH_UNMAPPED and no death wave.

## r799n33y: BAD era fully decoded - the state-6 fork is a key-collision race over mbx_289 frame ORDER

n265 (8x BAD, zero FETCH_UNMAPPED - gate verified) gave the first clean BAD
trace.  BAD is NOT a hang: tid-65 reaches startup state 5 at 37.6 s, state 6
at 37.75 s - IDENTICALLY to GOOD - then calls exit_group(0x7f) at ~37.83 s
(the whole prochmi commits suicide; status 127 = init-fail path).  GOOD
passes through the same state 6 and reaches 7/8/9.

The fork, line by line (BAD n265_1 vs GOOD n264_11):

- tid-65 posts the state-6 NAVIF member request (svc=0x6 func=0xa, R=2,
  rqadd key=0x2).  Both eras identical here.
- GOOD: DAPI's reply PAIR lands while key=2's entry is still pending: the
  data frame (f-accept svc=0x0006, optype 9) FOUND the entry (w6=0x6), the
  completer removed key=2 and the step advanced; the svc-0x0a follow-up
  frame got its own live key; state=7 at 39.19 s.
- BAD: a STALE frame pair for (svc=6, txn=2) - queued in mbx_289 BEFORE the
  request was issued (len 0x22, posted during the registration-era noise) -
  is drained first and consumes/removes key=2 at 37.81 s; DAPI's REAL reply
  (svc=0x000a txn=2) arriving 20 ms later hits `bhrfind lost` (entry gone);
  the pending leg never completes; the FSM's give-up path exits the process.
- So GOOD vs BAD = whether the stale svc-6 optype-9 frame sits in mbx_289 at
  the instant tid-65 registers key=2.  The stale frames come from the
  registration-era traffic our fabrications + DAPI push onto mbx_289 in
  bursts; their drain order is scheduler-lottery - exactly the observed
  era-lottery.

Next-step candidates (not yet done): (a) stop the registration-era optype-9
DATA frames for svc 6/0xa from being fabricated at all (only conf, and only
addressed to the registering client - check cca_server_answer_register's
ServiceStatus shape vs the pumppage accept filter w14=0x00030006);
(b) DAPI-side: make its state-6 reply timing match its own request (it
already does - the problem is OUR stale frames, not DAPI); (c) the tick-gate
(n33x) removes foreign-VM writes into DAPI's heap that perturb the ordering.

## r799n33z: the state-6 poison frame is OUR OWN f74 duplicate - mbx-leg now paced + single-shot

Follow-up to n33y nailed the culprit frame: len 0x22 class-0x45 svc=0x0006
(the `f-accept svc=0x0006 txn=2`) is our `cca_server_answer_sensorfi_data_
request` standing in for app 0x0015 on the f74 readiness lane - and its
+0x1c ordinal follows the ADVANCING `F74_REG_ORD`.  The very same request
(R=1, TGT=0x7 svc-6 regid-3) is ALSO answered genuinely by DAPI (the len
0x98 src-app-0x0007 frame).  At zero latency our duplicate (or its re-fire
at the next ordinal) lands in the window between tid-65 registering the
NEXT request's pending key (R=2 -> key=2) and DAPI's real answer arriving:
the completer "matches key only and REMOVES on any optype hit", so our
frame consumes the foreign entry and DAPI's genuine answer dies as
`bhrfind lost` -> the leg never completes -> FSM suicide (exit_group 127).
GOOD vs BAD was purely whether DAPI's answer won that millisecond race.

Fidelity (rule 1): DAPI is emulated and DOES answer these - our stand-in
must not outrun it.  Fix: the f74 (mbx_7) app-receiver leg is now (a)
single-shot per registration ordinal and (b) scheduled +250 ms later
(`EMU_F74_MBX_DELAY_MS`, namespace-level `ANSWER_SCHED` lane drained from
the scheduler pass; 0 restores the old burst).  The pump-route leg
(`pump_push_completion`) stays immediate; the late frame still feeds the
not-found gnf leg that stores gate +4 = 2 (r799g).

Same round: 0/8 n265 + 0/8 n266 confirmed the tick-gate alone does not
shift the lottery (it removes foreign-VM garbage, keep it), and the era
fork was proven to sit entirely in the state-6 reply ordering.

## r799n33za: the true poisoner - `fabricate_fi_ready_push` kick REFIRE rate (f74 fix was innocent)

n267 (0/6) falsified the f74-duplicate theory: the f74 responder never even
ran in the dying runs (no `f74 REQUEST raw`), yet the len-0x22 class-0x45
svc-0x0006 poison frames kept murdering live pending keys.  The normalized
tid-65 probe diff (GOOD n264_11 vs BAD n267_1 from t=30 s) pinned the fork
to the microsecond of the murder and the message log named the poster:
`fabricated FI ServiceData(svc 0x0006 txn K)` - `fabricate_fi_ready_push`,
the gate-kick completion push (body len 0x22, ordinal at +0x1c = the mint's
key).  Its r799n30 re-fire loop retries EVERY scheduler pass (budget 300),
so pushes sat in mbx_289 spaced 40-90 ms apart; the completer "matches key
only and REMOVES on any optype hit", and svc 0x0006/0x000a requests are
ALSO answered genuinely by DAPI (~100 ms, len 0x98 src app 0x0007).  A
re-fire landing between tid-65's `rqadd key=K` and DAPI's answer consumed
the entry, DAPI's real answer died `bhrfind lost`, and the enavi FSM
called exit_group(127) - 100% of BAD eras (n265: frames at 37.746/37.812 s;
n267: 37.226/37.474/37.560 s).  GOOD = tid-81's clone lane happened to eat
the duplicates before tid-65 registered the colliding key.

Fix: rate-limit `fabricate_fi_ready_push` per transaction key (250 ms
default, `EMU_FI_PUSH_RATELIMIT_MS`, 0 = old burst).  The kick loop
re-tries within milliseconds, so a throttled push still fires whenever the
entry getter is genuinely still 0 - the n175 "one push loses the race"
stall cannot return - while DAPI (~100 ms) now always wins every live key.
The f74 pacing from n33z stays (harmless, more TE-realistic).

## r799n33zb: svc-6 fi-ready leg disabled by default (DAPI-hosted); f02/GpsDataStatus legs stay

The per-key rate-limit (n33za) was necessary but not sufficient - n268
(0/6) still died: the FIRST svc-6 push per key passes the limiter, and one
such push is already enough when its ordinal aliases a live foreign entry.
The DECISIVE experiment was n269 (`EMU_FAKE_FI_READY_PUSH=0`, 0/4 GOOD but
a new, slower signature): state 6 CLOSED (key 2 found + removed 3.6 s later
by DAPI's own txn-2 frame - hosted completion works), and death moved to
41.2 s at the NEXT leg (`svc 0x000a func 0xf02` rqadd key=3 never answered
- the same flag had also switched off the GpsDataStatus supplier, and
SENSORFI 0x0015 is NOT emulated).  GOOD n264_11 shows our f02 lane working
when it stamps the live ordinal (38189: `svc=0x000a txn=3` -> found ->
removed key 3) and being harmless when it stamps the garbage 0x5aa5
(misses).

So the final split by fidelity rule 1:
- svc 0x0006 optype-9 pushes: DAPI-hosted -> skipped by default
  (`EMU_FI_PUSH_SVC6=1` restores for diagnostics; every real provider
  completes a registration with conf only, never an unsolicited data
  frame - our push was the premature-completion murder weapon, n268).
- svc 0x000a / GpsDataStatus (SENSORFI 0x0015) and svc 0x001e legs: keep
  (no emulated host; n269 proved turning them off starves state 8).

## r799n33zc: live key-owner guard on every fabricated completion

n270 (svc-6 push off, 0/6) moved the murder weapon to the svc-0x0a leg:
`f-accept svc=0x000a txn=2` (our gate echo, ordinal recorded legitimately
by its own conf-store) REMOVED tid-65's NAVIF entry key=2 - because the
HMINavServerLib IDFactory is ONE counter across all planes and REUSES
released keys: key 1 was removed at 37.798 s, the gate's next mint grabbed
2, while NAVIF's just-registered `svc6 func=0xa` request was the real live
owner of 2.  Key-only completer => indistinguishable => same
premature-completion death 60 ms later.

Fix: `RQKEY_LIVE` (prochmi.rs) tracks each live key's owning (svc, func)
from the rqadd/rqadd2 probes, drops it on the remove probe, clears on
navif-create.  Every fabricated completion lane (push_gps_answer,
fabricate_fi_ready_push, the sensorfi responder echo, the refire loop) now
calls `rqkey_owned_by_want(key, want_svc)`: a foreign live owner blocks
the push, and the (want_svc, key) pair is FROZEN once rejected (the stolen
key re-mints back to our plane milliseconds later and a repeated push
would re-arm the collision - the answer for our transaction already went
missing once, completing it later with the wrong lifetime is not worth
the risk).  Ownerless keys pass (f240 key-0 members).  EMU_FI_PUSH_OWNER-
GUARD=0 restores unguarded pushes for diagnostics.

## r799n33zd: the era fix exposed the QEMU section cap - guest-call stacks now arena-allocated

With the state-6/9 chain healed (n33zc), runs walked 4->5->6->7->8->9 for
the first time since the era chase began (n271: 4/6 reached state 9!) -
and then the EMULATOR itself aborted mid-run:
`phys_section_add: assert map->sections_nb < TARGET_PAGE_SIZE` at ~42 s
(also present in the old GOOD reference n264_11, which is why GOOD eras
never lived long enough to show their tail).  Diagnostics (TRIGGER log):
the prochmi VM sits at ~528 permanently unmergeable regions (EXEC/shared)
that sever the mergeable [heap] runs, while the state-9 cascade posts
~one guest-call per completion leg - each `spawn_guest_call` mapped its
stack as a FRESH mmu region, and the defragmenter (threshold 500, runs
1081x in a crash run) cannot shrink past the fragments fast enough.

Fix: `Mmu::arena_alloc` - one 16 MiB mapping per arena, stacks bump-cut
from it (1 region per 1024 stacks); `spawn_guest_call` uses it.  Total new
regions for the whole cascade: 2-3.

## r799n33zf: event-driven completion round (RQ_PULSE) + guarded mbx_36 echo lane

n273 (freeze in place): 6/8 crossed state 6 and 5/8 reached state 9 -
the flatview assert gone (arena fix), but the runs died ~2 s into state 9
with the property answers frozen => the reject-freeze is out (n33ze).
n274 (pure per-push owner guard): 4/6 reach state 9, `key 2` now closes
LEGITIMATELY (rqadd owner + its own DAPI frame), and the survivor
signature narrowed to one: the `svc 0x000a func 0xf02` member key
registers (37.325 s), our GpsDataStatus answer posts with the RIGHT
ordinal ~600 ms later (route mbx, `txn 3`) - but tid-65 parked and the
svcdone-refire/vSignalSelf/gps-kick block runs only under the 1 Hz
`TICKER_NEXT` pace: its next pass was due after prochmi's give-up
(37.99 s).  A phase lottery on a 1 Hz clock.

Fix: the svcdone/wake/ticker block now also fires on `RQ_PULSE`, set by
the rqadd probe every time a fresh member key enters SVCDONE_PENDING -
the completion round is event-driven (the 1 Hz SENSORFI cadence stays as
the floor).  Also guarded the last unguarded fabrication lane: the
mbx_36 class-0x45 echo stamps `txn` too; a foreign-owner key defers the
echo and releases the ANSWERED45 tuple so the client's RE-ASK retries
when the transaction is its own again.

## r799n33zż: defragment backoff + pulse throttle (the arena fix traded death-wave for section-leak storm)

n276_1 survived the death wave, walked 4->..->9, then the emulator
ASSERTED again at ~40 s.  Root cause found in the unicorn/QEMU core:
`phys_section_add` counts EVERY MemoryRegion ever created - unmap does
NOT free them - so the 4096 cap is a budget of MAPS, not of live regions.
The r799n33zd arena removed the per-stack maps but exposed a worse
amplifier: the s9 property flood pulses the completion round on every
rqadd (~30 ms), each round fires ~dozens of svcdone guest-calls (64,279
svcdone log lines in 40 s), arenas exhaust -> new 16 MiB maps -> every
map re-triggers a defragment pass (region list hovers at ~1021 because
~527 EXEC/shared regions sever the mergeable runs), and every merge
LEAKS one section (1081 passes -> thousands of leaked sections -> assert).

Fixes: (1) `Mmu::defrag_backoff` - after an unproductive pass (saves<20)
skip merging for 300 map calls; (2) the RQ_PULSE completion round is
throttled to 100 ms (the pulse stays pending, so the first round after a
fresh key still runs promptly); (3) `stall-dump` probe in prochmi::tick
(once at s9+5 s, once at s9+40 s): per-block-reason census of all prochmi
threads incl. tid-65 - n271..n275 survivors sit in state 9 with tid-65
silent and its last calls (Wait(0)+GetFromMQ, timeout=0x8, both native
paths that honour deadlines) rule out IOSC-event sleep; the dump names
where it really is parked.

## r799n33zq: the state-9 silence was a park dead-lock caused by the re-fire budget

n277 (5/8 survive to the kill): state 9 reached, then 50+ s of total
silence and NO stall-dump - because `prochmi::tick` itself stopped.  The
park report settles it: the whole prochmi process loop parked with
`65:blocked(sleep)` (the r753 60 s OSAL-event park, re-armed every wake,
released only by a real OSAL_s32EventPost).  Chain: pending property keys
spent the svcdone re-fire budget (n<8) within ~1 s of the (now 100 ms)
rounds -> `svcdone_open` false -> the vSignalSelf wake lane stopped
spawning -> zero runnable threads -> `run_process_loop` parked on its
doorbell -> the tick that drives the echo rounds, RQ_PULSE and the wake
lane is dead -> the confs that would end the wait can never be echoed.
Self-starvation: OUR retry budget gated the very lane that delivers the
completions the budget retries for.

Fix: re-fire budget and `svcdone_open` threshold 8 -> 200 rounds (~20 s
at the throttled cadence, entries retained <1000 anyway), vSignalSelf
wake budget 30 -> 120.  While completions are pending the wake-lane
guest-calls keep threads runnable, so the loop stays awake by
construction - the park now only happens when the guest is legitimately
idle.

## r799n33zr: key-0 echo was the post-state-9 abort; arena ring-wrap + map-budget watchdog

n279_1 narrowed the post-s9 death to a clean signature: tid-65 consumes
`svc=0x000a txn=0` -> completer miss (`lookup key=0x0 func=0xf02`, no
key-0 svc-0a entry EXISTS - GOOD printed zero key-0 f02 lookups) ->
`handler_res=0` -> `open /dev/tty` -> `exit_group(127)`.  GOOD never
opened /dev/tty (0 opens): that open is the abort path, and it fires only
for a MISS on a key the FSM treats as an expected answer.  Source: the
conf-store gate leg records ordinal 0 for one svc-0x0a store; the echo
becomes a permanent-miss frame that the refire lane re-posts forever
(key 0 never appears in the `remove` probe, so `fi_era_clear_key` never
fires for it).  Fix: `rqkey_owned_by_want` now rejects key 0 for every
lane except the (hosted-off) svc-6 plane - applied at the single choke
point, so the conf-complete leg (also unguarded until now) and the refire
loop (which had `key != 0 &&`) are covered too.

n278_2 also re-crashed `phys_section_add`: with the 200-round budget the
guest-call stacks exhaust the 16 MiB arena every ~20 s and EVERY extra
arena is a permanent unicorn section (they are never freed).  Arenas now
RING-WRAP (1024-slot stack pool reused; guest calls never block and die
in ms, so old slices are long dead), and `map`/`map_shared` carry a
MAP-BUDGET watchdog log so a future leak names itself instead of dying
mid-cascade.

## r799n33zs: the post-s9 abort is FSM round-budget, burned by our own wake storm

n280_1 died on the *innocent* `0x5aa5` kick frame (GOOD consumed 7 of
those and lived, and GOOD printed the identical `found entry=0x0
want_ord=0xf02` misses) - so the miss is not the trigger.  The trigger is
the bRunWhenRunning give-up budget: GOOD's tid-65 spent n=1..3 enavi-chain
passes across 6+ s and reached the final at 44.1 s; our 100 ms pulse-round
`vSignalSelf` wakes drive one pass each and exhaust a ~5-pass budget in
under a second, landing in the abort path (`open /dev/tty` +
`exit_group(127)` - GOOD never opened the tty).  Our wake lane was
literally ticking the FSM to death.

Fix: in state 9 the fabricated wake rides only the 1 Hz tick floor
(`due`), matching GOOD's pass cadence; pulse rounds still run the
svcdone/echo legs (those execute on guest-call clones and do not consume
FSM passes).  The member-era (state 6) pulse wake stays - there the
problem was the opposite (too few wakes before tid-65's f02 patience,
n274_1).

## r799n33zt: soft-unmap + bookkeeping splits kill the flatview-section death

n281/n282: with the FSM pacing fixed, every state-9 survivor now lived to
~38-40 s only to die inside `phys_section_add` again - and the MAP-BUDGET
trajectory proved the budget is consumed by REMAP CHURN, not growth:
PROCNAV.OUT's region count oscillated (444 -> 28 -> 102 between consecutive
100-map checkpoints), i.e. munmap bursts (thread-stack teardown) followed
by re-mmaps of the same ranges; in unicorn's QEMU every re-map permanently
raises `sections_nb` while an unmap frees nothing.  Worse, `split_internal`
did read+unmap+2x map+write per split - 2 leaked sections and up to 32 MiB
of copying each time it touched the 16 MiB arena.

Fixes in `Mmu`:
- `unmap_internal` SOFT-frees non-shared ranges (QEMU mapping stays, range
  parked in a new `freed` pool); `map` reuses an exactly-covering freed
  range (zero-filled, perms-matched) without touching QEMU at all -
  churn converges instead of growing.
- `split_internal` is now pure bookkeeping (the single QEMU mapping already
  covers both halves; no unmap, no re-map, no copy).
- `map_shared` and the fresh-map fallback path hard-unmap overlapping freed
  fragments first (`hard_purge`) so `mem_map`/`mem_map_ptr` never collide
  with stale mappings; shared (host-ptr) regions are never soft-freed.

## r799n34: real provider processes join the boot (procmwlx/procsds/procsql/procvoice)

Direction change per user: instead of chasing the fabrication flags one by one,
boot the processes that on the unit ARE the fabricated partners, then retire
the fabrications behind them.

- `RtosBootConfig::default` now also starts `procmwlx_out.out`, `procsds.out`,
  `procsql_out.out`, `procvoice_out.out` (queue-boot lane spawns their VMs
  unchanged; n286/n287 show all eight processes spawning).  procsds and
  procmwlx come up healthy (scheduler heartbeats, own app loops waiting on
  mbx_27/28 (mwlx, apps 0x1b/0x1c) and mbx_4096..4105 (procsds, apps
  0x1000..0x1009)).  procmwlx has a noisy `/dev/.udev/queue.bin` retry thread
  but its main app loop runs.  procvoice finds its hive after `procvoice.reg`
  joined `DEFAULT_REGISTRY_FILES` (PROCSQL hive already rides base.reg, the
  SDS hives ride ni2k_eur/sdsapp.reg via the navi pack - no other reg work
  was needed).
- Wait-based hosted-app detection (registry.rs `note_hosted_app_queue` /
  `app_is_hosted`): a process blocking in `OSAL_s32MessageQueue(Wait)` on
  `mbx_N` demonstrably consumes app N, so every "answer for an unhosted
  server" lane (generic RegisterConf responder, `..._data_request_if_unhosted`,
  SENSORFI responder, CONNECTCONF lane) stands down for it.  n287_1 flipped
  0x7 (DAPI's CAM queue - the sensorfi mbx_7 stand-in is now OFF), 0x8, 0x1b,
  0x1c, 0x33 and all procsds apps 0x1001..0x1005 automatically.  mbx_21
  (SENSORFI 0x15) and mbx_36 (CSMA 0x24) still have NO waiter in any run -
  those fabrications stay (and stay flag-gated).
- tid hard-coding removed: HMI FSM thread id is published by the startup-step
  probe (`HMI_FSM_TID`, consumed by the stall-dump) - with the extra
  processes it moved 65 -> 120 -> 133.
- n287_1 (era lottery winner): state 9 at 36.5 s with all eight processes,
  first run since the era fixes where the property-completion clones walk the
  FULL completer chain on the live path (`bhrfind res k[6]=0x1f` ->
  `completer -> dispatch -> ev-entry -> bonr -> gnf-run -> vtick -> s9cascade
  cmp1f svc=0x1f r0=1`).  Yet still no `sp2res`/`s9cascade final`, no removes
  past key 0x19, and the FSM (tid-133) aborts 65 ms into state 9: lookup
  `tbl=0x392dfb8 svc=0x1 key=0x5aa5 func=0xf02 optype=0x8` -> `found
  entry=0x0` -> `handler_res=0x0` -> `open /dev/tty` -> `exit_group(0x7f)`.
  GOOD (n264_11) has the SAME lookup shape (svc=0x1/0x5aa5/f02) x60 with
  entry=0x0 misses AND 53 other `handler_res=0` in the same window and lives
  - so the abort's real precondition sits behind the `lr=prochmi_out.out+
  0x86f24` caller, currently being decoded (the on-disk prochmi_out.out is
  encrypted at that offset; the Ghidra `prochmi_out.out` program holds the
  runtime image and is the decoding source).

## r799n34b: startup crash sweep - short argv[0] revives procsql

Boot audit after r799n34 showed procsql_out.out dying inside `main`:
`sql::sem_t::init` -> `OSAL_s32EventCreate(name="sql:evt:/opt/bosch/processes/
procsql_out.out")` -> assert `((0x07<<16)|(0x02<<12)|7) == OSAL_u32ErrorCode()`
(expects 0x72007 ALREADY_EXISTS, sqlbase.hpp) + `invalid != handle` storm +
WRITE_UNMAPPED.  Ghidra decompile of `OSAL_s32EventCreate` (libosal +0x2f1c8)
shows the name is rejected with 0x7200b unless `strlen(name) < 32` - a FULL
argv[0] makes every procsql sqldb IPC name (`sql:evt:`/`sql:sem:` + argv[0])
over-long, so the create fails with the wrong code and sqldb asserts.  The
unit's baselx execs the registry's `PROC_LOCATION` string (short:
"procsql_out.out" -> name 23 chars), and `StartConfigDefault.reg` even ships
the real boot list: `PROCESS\LINUX\ORDER` = PROCMWLX, PROCHMI, PROCSQL,
PROCPHONE, PROCDAPI, PROCMAP, PROCVOICE, PROCSDSTS, PROCSDS, ...

- `RtosBootConfig::default` now uses those PROC_LOCATION names in that ORDER
  for the subset the emulator runs (PROCNAVI is not in LINUX ORDER - Triton
  starts it - and keeps its absolute /opt/bosch/navbin path).
- `process_launcher::spawn_process` passes the message's filename (not the
  resolved elf path) as argv[0] - what `exec(PROC_LOCATION)` yields on the
  unit.  n289_1: procsql alive at 49k passes, `invalid != handle` gone, all
  eight processes heartbeat to the end of the run.
- Remaining startup asserts are pre-existing GOOD-era noise: n264_11 (GOOD)
  carries the same `osalevent ALWAYS`, `osaliosc`, `clGUIWidgetEngine:243`
  assert set and ~900 tolerated Execution-error teardown races; the READ_-
  UNMAPPED storms ride exit_group teardown of prochmi worker clones.  The
  only real crash left is the FSM abort at prochmi+0x86f24 (state 9, 65 ms
  in) - still the r799n34 decode target.

## r799n35: the state-9 "abort" decoded - free() of a non-malloc slot

The r799d-era death (`open /dev/tty` + `exit_group(0x7f)`) was NOT an FSM
logic abort.  Three witnesses were needed: (1) a LIBC code-stub pair at
`abort`/`__assert_fail` (src/libs/libc_trap.rs) showing the caller is
`__libc_message` (lr=0x47db5fc0), not assert/terminate; (2) decoding glibc
2.8 `__libc_message` (libc is plain on disk, link base 0x47d58000) showed it
emits the fatal text with **writev** (svc 0x92) - a syscall path with no
mirror, so the message had never surfaced; (3) a WRITEV-TTY mirror in
uio.rs printed it:

```
*** glibc detected *** prochmi_out.out: free(): invalid pointer: 0x90909000 ***
```

`0x909xxxxx` is the `[pump-completion-pool]` - pump_push_completion posted
fabricated completions whose data slot was a host `heap_alloc` region, and
the pump's orphan-response leg (`bHandleResp: Request not found`, entry=0x0)
`free()`s the message data pointer.  A non-malloc pointer passes glibc's
chunk checks only when the bytes below it look like a valid header - that
was the whole "era lottery".

Fix: the slot ring is now fed by the GUEST's own `malloc` (0x47dc345c,
verified ARM prologue; runtime VA == link VA) called through
`spawn_guest_call_lr` (new, monitor.rs) into a per-slot RWX trampoline that
stores r0 into its scratch word and exits quietly (message.rs
`pump_malloc_trampoline`).  n292_1..3: 3/3 runs reach state 9 and SURVIVE
(zero exit_group, zero free-aborts), slots post as real heap chunks
(0x3939de0...), `sp2res` reappears x2.

Still open (next): the cascade now stalls ~40 s into state 9 - s9-wake
vSignalSelf fires only #0/#1 (gate `fi_era_has_pending(0x1f) || svcdone_open`
goes quiet), sp2a legs return r0=0x0 (pRetrieveRequest misses -> no
enRemoveRequest, removes frozen at key 0x19 vs GOOD 0x1a-0x34), no
`s9cascade final` yet.

## r799n36: host lanes were stealing app-mailbox names - queue-create poison decoded (map-engine reborn)

Priority per user: boot PROCSDSTS (`procsds_ts.out`, LINUX ORDER PROC07,
`sdstsapp.reg` already in the navi pack) and keep pushing.  sdsts boots
(heartbeat x14, creates its own `mbx_4105` = APPID 0x1009).

Then the map-engine blocker (carried from r799n35) was decoded - it was NOT
an OSAL pool exhaustion.  Chain: `OSAL_s32MessageQueueCreate` non-IOSC leg
runs `mq_open("/mbx_1024", O_RDWR|O_CREAT|O_EXCL)`; the emulator's shared
`MqState` answered EEXIST -> assert osalmqueue.cpp:1181 (lr 0x4850f81c =
bl of the assert, verified by disassembly) -> err 0x72003 -> scd_bCreateQueue
fails -> map-engine bInitInstance dies -> no mbx_1024 consumer -> GOOD's
35,742-message svc-0x300 STAT flood channel dead.  n295 census (new
`mq_open census` info log in mqueue.rs + mempool-create/getblock pool-diag
probes) named the thieves: NO guest created mbx_1024 - host lanes did:
`guest_post`/`ensure_queue` **materialized** queues on first post/wait
(synthesized ApplicationInfoStatus, renfi posts, ServiceStatus re-post
pairs, CCS-stamp forwarder, and the monitor's wait lane).  The same poison
killed prochmi's AppId 265 create in n295 (sdsts connected to HMI before
prochmi's own create at t=2690 - a real-unit boot-spacing artifact, and it
did not recur in 3/3 later runs).

Fidelity fixes (all fabricate *less*):
* `guest_post` never creates: a post into a not-yet-created mailbox is
  dropped + logged once per name, exactly like a real ENOENT `mq_send`; the
  synthesizing lanes re-post on their own cadence.
* `ensure_queue` refuses `mbx_*`/`MQB_*` names (sentinel id 0 - matches no
  queue, ids start at MQ_HANDLE_BASE; waiters just see "empty").  Explicit
  guest create requests (monitor OSAL queue-create request, the IOSC leg)
  go through the new `force_create`, which keeps the old create-or-get body.
* monitor post lane + CCS-stamp forwarder + wait lane no longer call
  ensure_queue at all (wait reads `queue_id_for_name`, None -> no message).

n296_1..3: 3/3 `MessageQueueCreate failed`=0, `success for AppId 1024`=1,
zero exit_group, state 9 reached; map-engine BODY THREADS ALIVE (body-pass
dispatch, its class-0x42 svc-0x1e answers now reach mbx_22, STAT admin
reference registered); the STAT self-flood into mbx_1024 restarted in 2/3
runs (4830/5036 posts vs GOOD 35,742, run 1 flood=0 - era still shifts).
`s9cascade final` still 0/3.

Next: the flood is short-lived vs GOOD (GOOD floods until 44 s) - diff the
re-post precondition (r799n14 livelock: unanswered regid-0xfffe STAT answers
must find NO client handler to be re-posted; BAD now has 3 STAT admin
registrations vs GOOD 2, may be claiming them) and the svc-6 fi-ready push
deficit (secondary list in r799n35 notes).

n297_1 follow-up (EMU_FI_PUSH_SVC6=1): no effect (wakes=2, sp2res=0) -
r799n33zb's svc-6 suppression stands, DAPI answers svc-6 genuinely.  The
wake-gate dry-out is now precisely located: the tail conf key (0x26,
func 0x970e, cookies 1/2, optype 8/9 pairs) is re-driven by the svcdone
guest-call lane dozens of times (tids 546..584, retries -> 200) but
`bHandleResponseSVCData` never consumes it, so SVCDONE_PENDING closes the
gate and the 0x1f era gets no new requests.  GOOD's tail instead completed
every key (remove 0x1a..0x34, f-accept svc-6 x48 riding pump-completion
slots r3=0x909xxxx).  Next hypothesis to test: GOOD's completion stream ran
through the OLD host-slot pump ring; since r799n35 the ring is guest-malloc
fed - check whether pump_push_completion still reaches `bHandleResp` FOUND
(entry != 0x0) for the tail keys, and compare the completer's key-matching
word-for-word between a GOOD tail round and a BAD one.

n298_1 (spawn audit, EMU_GUEST_CALL_AUDIT=1): the "workers never run"
theory is FALSIFIED - they land in prochmi's list and DO execute
(pumptrace `found` + `bhrfind res` for their key), then park in
`SHM-WAIT id-word` on the IOSC id word of the object that carries their
conf answer (worker 460: uaddr 0x40110e6c, key /dev/shm#file:0x32+1117804).
The wake machinery works (word 0x40110e6c was woken 7x - tids 193..198
purged key 0x3 at t=35.5s), but NOBODY ever writes/creates the object the
TAIL workers wait on.  GOOD contrast: worker 373 parked 39.9 s on word
0x4010c56c and at 42.1 s a peer thread finished processing the ECHO frame
(`post8`->consumer-> guest `enChkErrInf: Decoded error msg [FID=0x9b1a]
code=0x2a1 -> 0x800002a1` on the HMINavServerLib_1 consumer leg, TRACE tag
0x2700) - the id-word write + futex_wake came out of that processing and
the woken worker then ran subhand -> compres -> remove (key 0x1a purged).
So the missing tail link is NOT the worker lane but the echo CONSUMER:
BAD posts the echoes (`pumptrace post8/notify` into HMINavServerLib_1,
svc 0x20 payloads) and the consumer polls (`Wait timeout=0` cadence alive
at the kill), but the frame never completes through
`enChkErrInf`/deliver - BAD `envigate f-accept` = 24 vs GOOD 130, and BAD
`cca server` census lacks GOOD's `server 0x0020: answered
ServiceRegister(svc 0x0180) from 0x0109 with RegisterConf+ServiceStatus`.
Next step (fresh session): trace one GOOD f-accept frame end-to-end from
its poster to the consumer's enChkErrInf leg and find the equivalent
poster BAD lacks (CCS 0x0180 RegisterConf for the nav-server services vs
`hosted` suppression of the CCS answer lane by the new provider procs).

## r799n36b: the "global coma at t~39.5 s" was a Unicorn section overflow (SIGABRT), not a stall

Every recent BAD run went 100% silent at t~39.5 s regardless of what the
cascade did (n296/n297/n298 all cut logging at 39.1-39.6 s of 75/95 s
budgets).  With stdout redirected to a FILE (no `tr -d '\0'` pipe - the pipe
buffer swallowed the abort banner in the piped runs!) n299_1 caught it:

```
nissan_connect3_emulator: ./qemu/exec.c:837: phys_section_add:
Assertion `map->sections_nb < TARGET_PAGE_SIZE' failed.   (SIGABRT, core)
```

Cause: `guest_call_bhandle_svcdone` heap_alloc'ed THREE fresh unicorn memory
regions (body+stack+tramp) per retry; the svcdone pending-leg re-fires up to
200x per key - 371 spawns in n299 ~= 1100 permanent regions, and unicorn's
static section table caps at 4096 (regions are NEVER freed; r799n33zr already
learned this for arena stacks).  The s9 tail - where svcdone retries pile up
- always died right as it started, poisoning every tail measurement since the
retry legs existed (era lottery partially included "did the poison key's
retry storm hit 4096 early or late").

Fix (fidelity-neutral, host-side): body + tramp now come from two 64-slot
rings allocated ONCE (2 regions total; wrap reuses slots whose parked worker
never completes anyway - same net effect as today's drops), the child stack
comes from the existing arena RING (`arena_alloc`, r799n33zr).  Also tightened
the leg's body-length guard (<0x1e || >0x3c -> skip).

n300_1: full 94.9 s of logging, zero asserts, regions cap 485, wakes 10 (was
2), rqadd max key 0x2d (was 0x26), removes max 0x1a (wall was 0x19; GOOD
0x34), post-40 s the system is ALIVE (svcdone x53, f-accept x52, post8/notify
x26, subhand reached reqid 0x26).  sp2res still 0 -> the next layer of the
tail.  ALWAYS run BAD with `> file 2>&1` at least once per session - pipes
hide SIGABRT banners.

r799n36c: PROCSDSTS spawn staggered (map-engine pattern, host thread, never
sleep in guest context) until prochmi's own mq_open created mbx_265 - kills
the sdsts-vs-HMI O_EXCL race for good (n300_2 lost HMI startup entirely;
n295 lost the queue).  n301_1..3 (all redirect-run, no pipes): 3/3 q_fail=0,
gate reports "mbx_265 already created: true", rqadd max 0x2d/0x2d/0x2e (GOOD
0x3d), removes max 0x2b (was frozen at 0x19 for weeks, GOOD 0x34), sp2res
x6-10 (GOOD x30), final still 0/3.  The tail is now ONE ORDER OF MAGNITUDE
from GOOD rather than dead - the remaining gap is completion ROUNDS, not a
wall.

## r799n36d: tail falsifications after the section-diet (n300-n302 forensics)

With the coma gone the tail became measurable; a full census diff (GOOD
n264_11 vs BAD n301_1/n302_1) falsified the remaining "broken lane" hopes:

- FI fabrication coverage EQUAL (txn 0-45 covered in both; GOOD just keeps
  issuing to 61 because its machine never stops).
- consumer decode count EQUAL (127 vs 128) - the enChkErrInf leg is fine.
- svcdone trio-holder spawns complete (keys 0x1..0x2d, same as GOOD).
- procmwlx alive to kill; post8/notify/notify legs ~equal (173 vs 217).
- GOOD itself ends its log with 9 UN-removed keys (0x35-0x3d): GOOD was
  killed MID-cascade at 45 s - "GOOD" only means the machine kept RUNNING.
- BAD freeze shape (n301_1, t=38.9 s wall): decodes->0, hot-mutex wakes->0
  (769 waits / 242 wakes total), while DAPI keeps posting to
  HMINavServerLib_1 until kill (213 guest posts, overflow x18).  The trio
  round pops ONE frame per pass (cllookup x34 total) and the pops miss the
  handler table (handler_res 2/173 vs GOOD 12/65).

n302_1 (EMU_REALTIME_FACTOR=0.25, 360 s run): conf timeouts dilate 4x but
the machine still freezes at wall t=46 s (right after state-9 entry),
removes 0x2c, stuck keys 0x27/0x2b/0x2d, last round exits EARLIER (sp2a
r0=0).  Wakes were NOT the limitation (~131 fired).  Dilation does not fix
the tail - the trio pass simply does not reach the consumer poll leg.

=> Remaining suspect set (narrowed): (a) the trio's own poll/consume leg
lives in native flow the synthesized vtick does not traverse (GOOD's tid-65
vtick'd NATIVELY 13x, driven by real answers); (b) handler-table entries
for the trio confs are gone (unregistered/timed out) by the time any frame
with their msgref is popped.  Next experiment = design.md step 3: deliver
stamped class-0x45 frames into the client-side path for the PENDING trio
keys instead of the parked worker guest-calls (user-approved), and watch
whether the trio's next pass consumes them (probe: cllookup hit-rate +
stepprobe ev-entry on pending keys).

## r799n36e: the tail blocker is ONE dead-holder shm mutex + a watchdog that never fires

GOOD vs BAD sp2 census t>=39 s: GOOD ran the FULL trio walker until its kill
(sp2o x15, sp2i1 x30, sp2res x30, sp2p1 x30, p2 x24, p3 x22, sp2c x21,
sp2b x23, sp2a x27 - rounds execute on the ANSWER-PROCESSING threads, every
genuine answer arrival re-runs one full round ~every 300 ms).  BAD: ZERO sp2
steps after t=38425 ms despite 50+ synthesized wakes.  The synthesized
vSignalSelf clone cannot drive the walker - only the answer processors can,
and after 38.9 s EVERY answer worker parks on the hot shm mutex word
(0x40110dac, waits 769/wakes 242 overall, wakes->0 exactly at the freeze).

Mechanism: the mutex holder died mid-critical-section (the 0x1c7d118
READ_UNMAPPED worker crash-storm kills workers inside the AIL list lock -
GOOD had MORE crashes (366) but its lock never stayed held: real answer
traffic kept releasing it (230 wakes)).  440+ late waiters should ALL have
been repaired by the r654 shared-futex watchdog (BlockReason::FutexWaitShared
{ deadline: vnow+20 s, watchdog: true } -> scheduler.rs "repaired word"):
the repair path logged ZERO lines in n301/n302 although the prochmi VM was
awake (wakes fired to t=355 s).  WHY the watchdog never fires is now THE bug.

Next session, surgical:
1) one-shot host probe: when waiters-on-one-key count exceeds ~64, log the
   raw shm word value + holder candidates (mem_read via the waiting
   unicorn) - confirm the word is non-zero (locked) and identify the holder
   tid vs the crashed tids.
2) instrument scheduler.rs advance_blocked FutexWaitShared arm: log the
   first expiry check for that key - find why neither the "re-arming"
   (word==0) nor the "repaired word" (word!=0) branch ever runs (suspect
   the deadline never expires because vnow() is *dilated wall* while the
   check only happens on some other path, or the waiter is registered in
   the global shm registry but reaped per-process - audit futex_wait_shared
   ids vs the registry the reaper scans).
3) once the watchdog repairs the word, the parked workers wake -> run the
   sp2 round -> decode -> wake cascade continues - that is the whole
   residual tail in one fix.

## r799n37: state-9 tail NUDGE + OSAL-event force-post - the round machine runs to kill

Root-caused why synthesized vSignalSelf wakes could not restart the tail:
the wake clone signals its OWN action context (r0..r3 = 0), while the enavi
startup-FSM owner sleeps in an OSAL event wait (libosal parks it as
SleepUntil and r753 re-parks infinite waits) that NOTHING reaches - the CSMA
completion that posts that event on the unit is absent.  The owner IS woken
by pump traffic until ~t=50 s (35 polls in n301), then sleeps forever and
the poll/consume leg dies while DAPI keeps genuinely posting answers (278
guest posts, consumer never pops them, queue overflow x31).

Implemented (EMU_S9_NUDGE_SECS=<sec>, default 0 = OFF, every fire logged):
- scheduler::nudge_blocked_thread -> NudgeOutcome{NotBlocked,Nudged(Option<uaddr>)}
  doorbells the FSM owner tid (new FSM_OWNER_TID published by the
  startup-step probe) back to Runnable; SleepUntil nudge alone is useless
  (the guest re-checks and re-parks), so on sleep-only parks we
- libosal_linux::event::force_post_event_for(tid): find the event the owner
  is registered on in the WAITER REGISTRY, OR its requested mask into
  EVENT_BITS and run the library's own wake gate (in_wait && mode<=1) - a
  fabricated OSAL_s32EventPost (absent-partner wake class, logged WARN with
  event name + bit transition).  A futex-parked owner instead gets the
  zero-word token poke (same lost-wake class).

n305_1/2: sp2res x118/120 (GOOD 30!), sp2c rounds cycling to t=92.4 s,
decodes 112/132 (GOOD 128), rqadd 0x33 - the round machine finally ALIVE
for the whole run.  What remains: SIX keys, svc=0xa funcs 0x101/0x102/0x107/
0x125/0x8000/0x8001 (the member-era confs, rqadded already at t=39.4 s),
never removed: svcdone workers for them burn the full 200-retry budget
without completing - their bHandleResponseSVCData leg never finds the real
response BODY (member data).  GOOD's member confs got real answers: GOOD
completed every func=0x10x (its own unfinished 9 are the NEXT batch, svc
0x300/0x2c/0x6 - killed at 45 s mid-cascade).  So the residual is NOT a
wake problem anymore: the member SERVICE RESPONSES must reach the client.
Next: trace one GOOD svc-0xa func=0x102 answer from DAPI (which queue/frame
carries it) and check whether the request even reaches DAPI in BAD (the
conf request POST side), then fix the request delivery OR - if the member
answers are Triton/SD-side data - provide them at wire level.

## r799n38: what the six stuck svc-0xa keys actually are (GOOD worker-1012 trace)

Decoding GOOD's completed member batch (n264_11 tid 1012/1013, t=43801-43844):
- svcdone worker for key 0x2e/func 0x102 spawns, parks (SHM-WAIT), and 31 ms
  later wakes with a REAL msgobj (0x9171e000 = DAPI heap page): the answer
  frame arrived while the worker waited.
- Its completer leg (lr 0x1c7c878) dispatches the traffic-status payload and
  the client library runs FOUR callbacks (tag 0x2714):
  vOnUpdateTrafficMessageSourceAvailable / vOnUpdateAllTrafficMessagesStatus /
  vOnUpdateUserTrafficMessagesStatus / vOnUpdateTrafficMsgMemoryStatistics -
  exactly the stuck func set 0x101/0x102/0x107/0x125.  0x8000/0x8001 are the
  generic get/complete legs.
- So svc 0xa carries the TMC/traffic-status domain (the same service also
  carries GpsDataStatus 0x0f02, which the fabrication lane already fakes for
  the state-8 f02 readiness).  The real answers came from DAPI's traffic
  module - on the unit that module is fed by the TRITON tuner/RDS side
  ("source NOT available" is a legitimate startup answer); nothing in the
  emulator ever delivers tuner state, so BAD's DAPI never answers and its
  svcdone workers burn 200 retries with msgobj-less confs (BAD completer
  never even runs for 0x101/0x102/0x107/0x125 - it DID complete func 0x106).
- The fc_sdp/fc_ts traces seen in BAD are procvoice boot-era init (t<9 s),
  NOT the member request handlers - red herring.

Next (Ghidra-backed): find the CCS svc-0xa func handlers in DAPIAPP.OUT and
what gates their answer (tuner state word?).  If the gate is Triton-side
state, the sanctioned move is a wire-level "traffic source unavailable"
ServiceData+ConfComplete fabrication for funcs {0x101,0x102,0x107,0x125,
0x8000,0x8001} of svc 0xa (flag-gated like EMU_FI_PUSH_SVC6, logged, off by
default; enabled only after golden-run comparison proves no real answer
ever arrives for them).  GOOD's own unfinished 9 keys (0x300/0x2c/0x6 batch)
sit BEHIND this batch - clearing the six should expose the final trio and
`s9cascade final`.
