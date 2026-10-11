# PROCNAV GPS / position transport — implementation-ready map

Goal: feed PROCNAV a combined-sensor GPS fix so its position-fusion (`posfi`) runs
map-matching and it publishes the CCP/`NavPosition` properties that the map/HMI consume.
This is the RE map (no code) of the receive transport, the message demux, and the
precise work left to build a synthetic GPS publisher.

## What PROCNAV is on the position chain
PROCNAV is the **position server**. `STAT_tclThread::vRelUpregOnPos`
(`PROCNAV_dec.out:0x11e916`) registers its *provided* properties for RELUP on its
out-channel (`*(this+0x290)`):
- `"extrapolated positions"` (type `0x13`)
- `"DeadReckoningPosition"` (type `0x24`)
- `"PositionPathes"` (type `0x11`)
and it emits `posfi_tclMsgNavPositionRelUpReg`. These are what HMI/map read as the
Car-Current-Position. They stay empty until the fusion receives a GPS fix.

## GPS consume path (inside PROCNAV)
- `pc_tclSensorMsgHandler::s32OnStatusMsg(const cmb_sensorfi_tclMsgGpsDataStatus&)`
  @ `0x68ff8c` is the real GPS sink: gates on `bUseGpsTriggerUpdate(msg+0xc)`, copies
  the fix (lat/lon from `msg+0x10..`, speed, heading, time, satellite vector) into a
  `NavGpsBigMsg_t` from `pc_tclDataQueue<NavGpsBigMsg_t>` (this+2 -> `+0xa8`),
  `vGenerateSensorFI4Parameter`, `InsertBack`, then **`vRunCheck()`** (runs the
  fusion / map-match) and `triputil_vRecordGps`. Tight-coupling variant @ `0x690f08`.
- The handler is a virtual override reached only through the amt property demux (it has
  no direct callers; registered as a `pc_tclMessageHandler<cmb_sensorfi_...>`).

## Demux: amt property dispatch
`rtf_tclPropertyDispatcher::s32ProcessMessage(amt_tclServiceData& msg)` @ `0x2e5526`:
- walks the registered `rtf::tclProperty` singly-linked list (`this+0xc` head, node
  `[+0x14]` = `next`),
- for each property matches **two u16 fields of the message**:
  `property.u32GetPropertyId() == *(u16*)(msg.d4 + 0x18)` **and**
  `property.u16GetServiceId() == *(u16*)(msg.d4 + 0x14)`
  (`msg.d4 = *(int*)(param_2+4)` is the message's field area),
- on match calls `property->vtable[+8](property, msg)`, which lands in the sensor
  handler's `s32OnStatusMsg`.
So a synthetic GPS message must carry the **exact (serviceId @d4+0x14, propertyId
@d4+0x18)** pair PROCNAV registered for the sensorfi GPS property, on the receiving
thread's dispatcher, plus a body the handler's field reader accepts.

## Receive transport (how the dispatcher is fed)
- The generic msg demux `rtf_tclMessageDispatcher::s32ProcessMessage` @ `0x4d3c38` maps
  the message via `amt_tclMappableMessage::bEnableAccess`, reads `u16GetValue(0,0x14)`,
  then power/property dispatch; virtual (no static callers).
- PROCNAV's native OSAL message wrappers: `OSAL_tclMessageQueue::s32Wait(char* buf,
  uint size, uint* out1, uint* out2)` @ `0x3f1b64` -> `OSAL_s32MessageQueueWait`
  @ `0x2ec2b0` -> `software_interrupt(6)` (monitor). Its svc #6 request block is the
  contiguous stack frame built by the stub:
  `{ r0=handle, buffer@0x04, size@0x08, timeout@0x0c, actual_out@0x10 }`, `r0=&frame`,
  `r2=LR` (continuation), `r1 = *literal` = a per-op **descriptor pointer**
  (e.g. Wait's is `0x002d0564`), *not* a packed service id. The kernel copies the
  received bytes into `buffer` and writes the actual length to `actual_out`.
- PROCNAV's main-thread idle is a kernel **EventFlag** (`tkse_*_flg`) wait, not the
  msgq: `tkse_wai_flg` -> svc #6 `0x10240513`, `tkse_set_flg` -> `0x10220213`,
  `tkse_cal_flg` -> `0x10000113`; object = `service & 0xff = 0x13`, ticks = `>>16`
  (~4096 = the ~4 s loop period; PROCNAV yields on `0x13` ~every 80k log lines). The
  emulator's `yield_thread` always parks these and `complete()` currently returns the
  catch-all 0 ("no message"), so PROCNAV loops forever with an empty queue.

## Concrete message facts (resolved this pass)
`cmb_sensorfi_tclMsgGpsDataStatus` (vtable set in ctor `0x5b09c8`):
- **serviceId = 0xb** (`u16GetServiceId` `0x5b9b5c`) — the sensorfi service.
- **functionId / propertyId = 5** (`u16GetFunctionId` `0x5b9b64`) — GPS-data property.
- **opcode = 6** (`u8GetOpCode` `0x5b9b6c`) — Status (matches `OnStatusMsg`).
- type singleton `s32GetTypeId` -> `&UNK_00050600`.
- layout = `cmb_fi_tclVisitableTypeBase` (this+0..8) + `fi_tclMessageBase` +
  `cmb_fi_tcl_GpsBigdata` (the GPS payload). `oWrite`/`oRead` (`0x5b9b9a`/`0x5b9bac`)
  (de)serialise the `fi_tclTypeBase` then delegate to the visitable `GpsBigdata`;
  `u32GetSize(opcode)` = `cmb_fi_tcl_GpsBigdata::u32GetSize` + base. So the wire =
  `[fi_tclTypeBase (class,0xb,5,6,size)][cmb_fi_tcl_GpsBigdata body]`.
- The dispatcher (`0x2e5526`) matches `d4+0x14 == 0xb` and `d4+0x18 == 5`, so the
  delivered `amt_tclServiceData` field area must carry those two u16 values.

## Registration is CLIENT-side UpReg -> PROCNAV expects a sensorfi property SERVER
PROCNAV does not just listen: it subscribes via commcon RELUP.
`gd_tcl_CCAManagerBase::s32RegisterForProperty` / `s32StartMethod(...fi_tclTypeBase,
serviceId, functionId...)` (and `wdb/...::s32UpRegForProperty`) send a RELUP-register
**method** to the property server over the client-side out-channel; the server then
PUSHES `...Status` messages that arrive through the amt path and hit the property
dispatcher. The `SensorTypeCluster<SensorTypeTraits<...5...>>` ctors (`0x4f6a48` etc.)
build one property handler per sensor type (14,50,66,64,19,20,**5**,38,46,49,23,48,67,
65,2,4) bound to the shared `pc_tclSensorMsgHandler`. No launched process serves
service `0xb`, so PROCNAV's register request currently has no server and no status is
ever pushed -> the GPS pipeline stays empty.

## Transport / addressing (item 1) - RESOLVED, and it moves the earliest blocker upstream
PROCNAV talks to property servers through a **client-side out-channel bound to a commcon
member** (a named remote application/process), not a raw queue:
- Channel creation `pc_tclClientSideOutChannel::bCreate` (`0x1c8818`) ->
  `rtf_tclChannelFactory::poCreateClientSideOutChannel(serviceType, mt1, mt2, ...)`
  (`0x657be`), under an OSAL semaphore, calls
  `pGetCommMemberInfo(factory, type, subtype)` for BOTH the service's member and the
  local member. **If either comm-member lookup returns null, channel creation fails**
  with error class `0x430007` (`0x65842` error path) and the caller gets a null channel.
  `STAT_tclThread::s32CreateSideOutChannel` (`0x11d548`) returns that result as `-1` and
  logs a level-1 trace at `0x11d8d8` - i.e. PROCNAV logs a failure and the sensor property
  handlers are left with no channel, so `s32RegisterForProperty`/`s32StartMethod` never
  even send.
- On success the channel is built from the comm-member record: `memberInfo+0x14` is a
  `u16` **comm member / app id** (the server's routing address), `memberInfo+0x8` a
  sub-object; the send is the channel vtable `+0x10`
  (`channel->vtable[0x10](channel, amt_tclServiceData&, 7, functionId, 3)`) which carries
  the request to that comm member's receive queue (commcon addressing via
  `rtf_tclCommAddress`), the same `target_app`-style id the DAPI hooks already observe.
- The comm-member table is `rtf_tclCommMemberList` (factory `+0x4c`), filled by
  `s32AddClientSideOutChannelParam` / `s32AddServerSideOutChannelParam` and
  `s32GetCommMemberID` (`0x1f71aa`, `0x65d54`), and bootstrapped from the boot
  **directory / bank service** - the same monitor object `0x0a` (mbk) the emulator
  already models. So the resolution PROCNAV needs is a comm-member entry that
  *provides* the sensorfi service (`0xb`).

Net: the earliest failure is not the GPS push but **comm-member resolution for service
`0xb`** at channel-creation time. Until a comm member advertising that service exists in
the directory, PROCNAV never opens the sensor channel and never sends RELUP.

## Remaining work to inject GPS (each RE'd or scoped)
1. **Comm-member / directory entry for service `0xb`** so `pGetCommMemberInfo` succeeds
   and PROCNAV opens the sensor channel: add a comm member (id + app/target id) that
   serves the sensorfi service, in the boot directory the emulator already backs (object
   `0x0a`) plus the `rtf_tclCommMemberList` seed PROCNAV reads. Need to find where PROCNAV
   populates its own comm-member list (which directory queries fill `factory+0x4c`) and the
   exact member-record layout (`+0x8`, `+0x14`).
2. **Server emulation surface**: answer the RELUP register method sent on channel vtable
   `+0x10` and push a `GpsDataStatus` (opcode 6, `d4+0x14=0xb`, `d4+0x18=5`) with a valid
   `cmb_fi_tcl_GpsBigdata` body, delivered to PROCNAV's receive queue + `tkse_set_flg` wake.
3. **`cmb_fi_tcl_GpsBigdata` body offsets**: `cmb_fi_tcl_GpsBigdata::oRead` field visit
   order (client-view `msg+0x10..` in the sink `0x68ff8c` is post-deserialise).
4. **`fi_tclTypeBase` header format** (`operator<<`/`>>`) for a hand-built message.
5. Non-empty `NavPosition` remains **"necessary, not sufficient"** - fusion may also need
   gyro/accel/vehicle-speed and a valid map context.

## Verdict
GPS publishing is a **commcon/amt server-emulation** feature: register a commcon member
that provides the sensorfi service `0xb` (so PROCNAV opens its sensor channel at all),
then act as that server - answer the RELUP register and push a serialised
`cmb_sensorfi_tclMsgGpsDataStatus`. It rides the directory (monitor object `0x0a`) + the
amt comm-member/queue transport the emulator only partially models today, so it is far
bigger than the `DYN*MBX` bridge. Message ids are pinned (service `0xb`, property `5`,
opcode `6`); earliest open RE is the comm-member/directory entry (item 1).

## Provider identity + where to implement (RESOLVED)
`scd_bGetAppIdFromServiceId` resolves service -> app via `scd_tclServiceAppMap`, whose
`bReadData` (`0x34e3ec`) iterates the **amt registry** (`/dev/registry`, emulator model =
`common/registry.rs` in `namespace.rs`) over `PROCESS/<proc>/<app>/SERVICES/<svc>` reading
`APPID` + `SERVICEID`. The emulator already loads `procmw.reg`, which declares the
provider: `PROCESS/PROCMW/SENSOR` has `APPID = 0x2739 (10041)` with
`SERVICES/SENSORS : SERVICEID = 0xb`. So "who provides sensorfi service 0xb" is
**PROCMW / SENSOR app 10041** - already in the registry - but `PROCMW` is **not launched**,
so PROCNAV resolves the app id yet has no live/reachable comm peer and parks on the kernel
event flag (`0x13`). `scd_bTestLookupIoControl` picks the static-map branch when the SCD
queue device can't be opened, else `IOControl(scd_fd, 0xb)`; both bottom out in the same
service->app answer.

Placement: the service->app *data* is the **registry** (`common/registry.rs`, already
present, no new data needed). The *provider behaviour* (be PROCMW/SENSOR: answer PROCNAV's
channel/RELUP send, push GPS) is inter-process commcon delivery, which lives in
**`src/rtos`** (monitor object `0x0a` mailbox/directory + the `interaction`/app-info
announcement path) - NOT the OSAL-service `complete_osal_service` code. So `src/rtos` is
the right place for the provider, seeded off the existing registry data.

## Instrumented runtime ground truth (r173, `EMU_SENSORFI_TRACE=1`)
The static analysis above was partly wrong about *which* branch runs; the diagnostic (reads
`cRam01215110`/`iRam01215114` from PROCNAV's `.bss`, logged on change) settles it:
- PROCNAV opens `/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT` through the **OSAL IO
  service** (a `svc #6`, object `0x6e`, `0x1026e` = open) - the "SCD queue" IS the registry
  over object `0x6e`, path pointer `0x34ed14`. There is no separate scd device and no
  `ioctl` syscall.
- The emulator's `complete()` catch-all answers that open with status `0` and a handle of
  **`0` (null)**. PROCNAV stores it as `iRam01215114 = 0`, and `scd_bTestLookupIoControl`
  still sets `cRam01215110 = 1` (the null-handle error `0x72002` is "unexpected enough" to
  imply the queue supports ioctl), so **mode 1 (SCD-queue ioctl) is live**.
- With mode 1 + a **null handle**, PROCNAV's `OSAL_s32IOControl(handle, 0xb, buf)` fails its
  own magic check (`*handle == 0xD0E1D0E1`; `handle == 0` is the explicit null-handle error
  `0x72002`). So *every* service->app lookup fails, `scd_bGetAppIdFromServiceId` returns 0,
  PROCNAV never learns app `10041`, never creates the sensorfi client channel, and idles.

So the emulator is handing PROCNAV a **malformed registry handle** for every `0x6e` open,
not just scd. The provider is not reached at all today.

## Corrected implementation plan (src/rtos, object `0x6e`)
To get PROCNAV to resolve service `0xb` -> app `10041` we must model the OSAL registry
handle + its ioctl, in `complete()` for object `0x6e`:
1. On `0x1026e` open whose path resolves under `/dev/registry`, return a **valid OSAL device
   handle**: a guest object whose `*handle == 0xD0E1D0E1` (the magic `OSAL_s32IOControl`
   checks at `0x32cfb4`) so the ioctl dispatch is entered instead of rejected.
2. Serve `OSAL_s32IOControl(handle, 0xb, buf)` from the emulator registry: write the count
   (`1`) and the provider `APPID` (`10041`) into `buf` (`scd_bGetAppIdFromServiceId` reads
   `*param_3 = (u16)*(buf + 0x100)`) so PROCNAV resolves app `10041`.
3. Then observe (re-instrument) PROCNAV's comm-member/addressing for app `10041`: whether it
   still needs a live mbk/directory record (`0x0a`) and a provider to answer its RELUP,
   before pushing `GpsDataStatus` (opcode `6`, `d4+0x14=0xb`, `d4+0x18=5`).
Open RE before coding step 1: the OSAL handle-object layout the ioctl dispatch consumes
(magic at `+0x00`, type at `param_1[7]`/`+0x1c`) and how `func_0x0062ef88` turns the ioctl
into a `svc #6` (the resume frame + status contract).

## Scope after instrumentation (bigger than a one-off ioctl)
Both lookup branches bottom out in the same prerequisite: PROCNAV must be able to READ the
registry. `scd_tclServiceAppMap::bReadData` (`0x34e3ec`, the mode-0 static map) uses the
`reg_tclProcess/Application/Service` client iterators, which - like DAPI/procmapengine -
reach the registry through the same OSAL IO opens. For PROCNAV (native) those are `svc #6`
object `0x6e` (`0x1026e`), which the emulator answers with a **null handle**, so PROCNAV
currently cannot read the registry by EITHER branch. Therefore the real work is modeling
PROCNAV's **OSAL registry IO service on object `0x6e`** (open a `/dev/registry` path -> a
valid OSAL handle, read/ioctl/close) against the emulator's existing registry
(`common/registry.rs`); the sensorfi `IOControl(0xb)` answer (and every later service/app
lookup, comm-member resolution, etc.) rides on top of that. This is a core OSAL-IO feature,
not a sensorfi-specific shim - sensorfi is just the first thing it unblocks.