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

## Remaining work to inject GPS (each must be RE'd before coding — all are amt-internal)
1. **Message type id**: the amt type base (`rfoGetTypeBase`) of
   `cmb_sensorfi_tclMsgGpsDataStatus` — the class id the property handler dispatch keys on.
2. **Registration pair**: the `(serviceId, propertyId)` PROCNAV registers for the
   sensorfi GPS property (the values the dispatcher compares against `d4+0x14`/`d4+0x18`),
   and which dispatcher/thread list head (`this+0xc`) it lives in.
3. **Serialized body layout**: how the GPS fields are read from the message
   (`oRead`/`fi_tclInContext`) — the concrete `cmb_sensorfi_tclMsgGpsDataStatus` wire
   format (the handler's `msg+0xc/0x10/0x14..` reads are the client-side view, not the
   serialized offsets).
4. **Transport to reach the dispatcher**: reconstruct an `amt_tclServiceData` from bytes
   and route it to `s32ProcessMessage` — i.e. either model the OSAL MessageQueue
   (object 0x13 family) end-to-end so PROCNAV's own recv loop dequeues the bytes, or
   drive the amt receive path directly. Then `tkse_set_flg` to wake the loop promptly.
5. Even after GPS is consumed, producing a non-empty `NavPosition` is **"necessary, not
   sufficient"** — the fusion may also need gyro/accel/vehicle-speed and a valid map
   context before it extrapolates a position.

## Verdict
The GPS publisher is a **framework-level** feature (construct valid amt property messages
+ native OSAL msgq / event-flag modelling), not a bounded hook like the `DYN*MBX` bridge.
Items 1–4 are several more RE passes into the amt/rtf message framework before any safe,
non-guessing implementation, with real regression risk.