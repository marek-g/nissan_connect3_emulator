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
