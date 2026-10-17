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
