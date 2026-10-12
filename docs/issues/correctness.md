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
