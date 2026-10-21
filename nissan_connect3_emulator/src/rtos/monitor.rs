//! The Triton monitor image (`triton_mid.bin`) as the Linux-side processes see it.
//!
//! On the unit the core runs `triton_dualos` and the Linux processes trap into the very
//! same RAM: the ARM vectors live at `0x80000000`, and every `svc` from a Linux process
//! lands in the monitor's SWI handler. The service ABI is therefore pointer-based - a
//! request passes a block on the *caller's* stack and the monitor builds the resume frame
//! for the blocked thread - so the monitor cannot be run in a second VM without copying
//! the caller's whole address space. It is mapped into the caller's VM instead.
//!
//! The blob is a raw uImage payload; the emulator reads it straight out of the firmware
//! tree (it is never copied here) and strips the 64-byte header.
//!
//! Facts below come from `docs/reverse_engineering/triton_mid_swi_services.md`, which
//! records how they were recovered.

use crate::emulator::mmu::Mmu;
use crate::emulator::context::Context;
use crate::emulator::memory_map::MQ_NOTIFY_EXIT_STUB;
use crate::emulator::thread::{block_current_thread, BlockReason, GuestThread, ThreadStatus};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use unicorn_engine::unicorn_const::Prot;
use unicorn_engine::{RegisterARM, Unicorn};

/// Where the firmware keeps the monitor. Read-only by rule: nothing here writes to it.
pub const IMAGE_PATH: &str =
    "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/D605/triton_mid.bin";

/// Payload header of the uImage; the raw monitor starts behind it.
const IMAGE_HEADER_SIZE: usize = 64;

/// ARM vectors, and with them the whole monitor, are based here.
pub const MONITOR_BASE: u32 = 0x8000_0000;

/// Code and read-only data end here; the writable part of the image and the monitor's
/// zero-filled RAM follow. The image itself is 0x52_e124 bytes long, so everything above
/// `MONITOR_BASE + image.len()` is RAM the boot code of the real unit pre-fills.
const CODE_END: u32 = 0x8050_0000;
const RAM_END: u32 = 0x8056_0000;

/// SWI dispatcher: range-checks id 4..0x1b, then indexes the service table.
pub const SWI_HANDLER: u32 = 0x800e_aa04;
/// Returns from a service through SPSR: `ldmia sp!,{r3}; ldmia sp!,{r12}; msr spsr_cxsf,r12; ldmia sp!,{r3,r12,pc}^`.
pub const SWI_RETURN: u32 = 0x800e_aad0;
/// Fallback for out-of-range or unregistered ids: returns -26.
pub const SWI_UNHANDLED: u32 = 0x800e_aaf4;

/// Word holding the address of the service table, and the table itself. The table lives
/// past the end of the image, so it is RAM: ids are filled in by the registrar at run
/// time, which is why the ids the Linux side uses were 0 in a boot that never got past
/// the message pump.
pub const SERVICE_TABLE_POINTER: u32 = 0x8053_1ff0;
pub const SERVICE_TABLE: u32 = 0x8052_f678;

/// Generic registrar: `FUN_8002077c(id r0, handler r1)`.
pub const SERVICE_REGISTRAR: u32 = 0x8002_077c;

/// `svc #4` runs a numbered system task off a static table; task 1 is the message pump
/// that feeds the deferred service work, which is what the Linux-side `svc #6` requests
/// are waiting for.
pub const SYSTEM_TASK_TABLE: u32 = 0x8005_3a54;
pub const MESSAGE_PUMP_TASK: u32 = 1;
pub const MESSAGE_PUMP: u32 = 0x8000_9a6c;

/// Per-thread information block and thread control block the monitor reaches through
/// `mrc p15,0,r12,c13,c0,3`. Unicorn does not implement the banked TLS coprocessor
/// registers, so these host the emulated ones.
pub const TIB: u32 = 0x8054_0000;
pub const TCB: u32 = 0x8054_0100;

/// Service stacks taken through `[[0x805118e0]]` by `svc #0` and `svc #4`.
pub const SERVICE_STACK_POINTER: u32 = 0x8051_18e0;
pub const SERVICE_STACKS: u32 = 0x8053_e000;
/// Entry 0 of that table, i.e. the stack the handler switches to. It grows down from
/// here, so it is placed at the far end of the monitor's RAM, away from the table itself.
pub const SERVICE_STACK: u32 = 0x8055_f000;

/// Scratch buffer handed to a system task in R0; the monitor passes a RAM block of its
/// own, the emulator hands over a zeroed one.
pub const SYSTEM_TASK_SCRATCH: u32 = 0x8055_0000;

/// Page holding the replacement stubs for the coprocessor reads the emulator cannot
/// execute. It is deliberately the page right above [`TIB`], so a stub can reach
/// `[TIB + 4]` with one PC-relative load.
pub const TRAMPOLINES: u32 = 0x8054_1000;

/// Globals the boot code of the real unit pre-fills before entering the monitor; the
/// probe harness had to seed the same three to get past the dual-core handoff.
pub const DUAL_CORE_SKIP: u32 = 0x8051_7004;
pub const DUAL_CORE_SKIP_VALUE: u32 = 1;
pub const DEVICE_REGISTRY: u32 = 0x8051_7008;
pub const DEVICE_REGISTRY_VALUE: u32 = 0x7100_0000;

/// Monitor service ids the Linux side calls. `svc #6` is the yield: it stores the reason
/// from R1 into `[TCB + 0x20]` and reschedules, and the thread continues later from the
/// context frame at `[TCB + 0xe0]`.
pub const SVC_YIELD: u32 = 6;
pub const SVC_SYSTEM_TASK: u32 = 4;
pub const TCB_YIELD_REASON: u32 = 0x20;
pub const TCB_CONTEXT_FRAME: u32 = 0xe0;

/// Handlers the monitor's own late registration (`FUN_8002b848`) would have installed for
/// the ids the Linux side uses. That registration never runs here because it happens after
/// the boot phase the image is stuck in, and the dispatcher treats an empty slot as
/// "unimplemented" and returns -26, so the emulator fills the slots with the addresses the
/// image itself contains. See the id -> handler table in
/// `docs/reverse_engineering/triton_mid_swi_services.md`.
pub const SERVICE_HANDLERS: &[(u32, u32)] = &[
    (0x00, 0x8000_05dc),
    (0x03, 0x8003_3514),
    (0x04, 0x8000_0640),
    (0x05, 0x8000_bf74),
    (0x06, 0x8003_7184),
    (0x07, 0x8003_70d8),
    (0x08, 0x8003_6c78),
    (0x09, 0x8003_73a8),
    (0x0a, 0x8000_bfc8),
    (0x1e, 0x8002_03e8),
];

/// Ids the SWI dispatcher accepts; anything else takes its "not implemented" path.
pub const SERVICE_ID_MIN: u32 = 4;
pub const SERVICE_ID_MAX: u32 = 0x1b;

/// Table the id-6 handler branches through: `ldr r2, [0x800750dc + (r1 >> 16) * 4]`.
pub const WORKER_TABLE: u32 = 0x8007_50dc;

/// Service id literals, taken from the literals the `tkse_*` wrappers load into R1 next to
/// themselves. The low byte is the monitor kernel object to wait on, the rest identifies
/// the request.
pub const SERVICE_CREATE_POOL: u32 = 0x0103_010b;
pub const SERVICE_POOL_GROW: u32 = 0x0101_030b;
pub const SERVICE_POOL_FREE: u32 = 0x0102_010b;
pub const SERVICE_GET_TID: u32 = 0x0010_000a;
pub const SERVICE_GET_CFN: u32 = 0x0300_030a;

/// OSAL IO object (`0x6e`): the file-shaped services (open/create/remove/close/
/// iocontrol/read/write) that the native `OSAL_IO*` wrappers drive - PROCNAV's registry
/// client (`reg_tclRegKey`) and SCD service map (`scd`) are the ones that matter. The
/// service ids are the literals the wrapper shims load next to themselves
/// (PROCNAV `0x32b7f4..0x32b818`, see `docs/reverse_engineering` notes on object `0x6e`).
pub const OSAL_IO_OBJECT: u32 = 0x6e;
/// `OSAL_IOOpen(path, u16 flags)`: block `[path ptr, flags @+4 (u16)]`, status carries a
/// handle (`-1` on failure). The client (`OSAL_s32IOControl`, PROCNAV `0x32cfb4`) then
/// demands `*[handle] == OSAL_IO_MAGIC` and takes the generic ioctl path unless
/// `[handle + 0x1c] == 0x22`, so the handle is a real guest object, not a table index.
pub const SERVICE_IO_OPEN: u32 = 0x0001_026e;
/// `OSAL_s32IOClose(handle)`: block `[handle]`, status only.
pub const SERVICE_IO_CLOSE: u32 = 0x0002_016e;
/// `OSAL_IOCreate(path, u16 flags)`: block like open, handle back like open.
pub const SERVICE_IO_CREATE: u32 = 0x0003_026e;
/// `OSAL_s32IORemove(path)`: block `[path ptr]`, status only.
pub const SERVICE_IO_REMOVE: u32 = 0x0004_016e;
/// `OSAL_s32IOControl(handle, request, arg)`: block `[handle, request, arg]`; PROCNAV
/// reaches it through `OSAL_s32IOControl` (`0x32cfb4`), which tail-branches into the
/// `PRC_OSAL_s32IOControl` wrapper (`0x32b6f4`) that packs the three arguments with `stm`.
pub const SERVICE_IO_CONTROL: u32 = 0x0005_036e;
/// `PRC_OSAL_s32IOWrite(handle, buffer, length)`: block `[handle, buffer, length]`.
pub const SERVICE_IO_WRITE: u32 = 0x0006_036e;
/// `PRC_OSAL_s32IORead(handle, buffer, length)`: block `[handle, buffer, length]`.
pub const SERVICE_IO_READ: u32 = 0x0007_036e;

/// Magic word the OSAL user library checks at `*[handle]` before any IO call (`0xD0E1D0E1`,
/// the literal at PROCNAV `0x32d11c`).
const OSAL_IO_MAGIC: u32 = 0xd0e1_d0e1;

/// Device-type word at `handle + 0x1c`. The real unit's registry device answers the request
/// `3` (`bSetU32`) *remotely* - the client only special-cases type `0x22` locally - so any
/// other value takes the generic svc path. Exact real value unknown; only "not 0x22" is
/// pinned by evidence.
const OSAL_IO_REGISTRY_TYPE: u32 = 1;

/// The registry ioctl request codes, taken from the client call sites:
/// `reg_tclRegKey::bQueryU32`/`bQueryString` (`0x273d2c`/`0x273dac`) use `1` with buffer
/// `[name ptr, size, out count, out data ptr]` and require count `1` (u32) / `0` (string);
/// `bSetU32` (`0x273cbc`) uses `3` with the data pointer as input; `scd_bTestLookupIoControl`
/// (`0x34e340`) probes `0xa` and `0xb`; `scd_bGetAppIdFromServiceId` (`0x34f012`) uses `0xb`
/// with the service id at `arg + 0x100` and the app id read back from the same word.
const IO_REQUEST_QUERY: u32 = 1;
const IO_REQUEST_SET: u32 = 3;
const IO_REQUEST_LOOKUP_PROBE: u32 = 0xa;
const IO_REQUEST_SERVICE_TO_APP: u32 = 0xb;
/// Registry subkey enumeration (client `0x2c4738`): the server writes the next child's
/// name into the request struct's inline buffer and advances a per-handle cursor;
/// `-1` status ends the iteration.
const IO_REQUEST_ENUM_SUBKEYS: u32 = 0x3eb;

/// OSAL service object (`sysprogosal`, blob0). Its handler table is registered by the
/// overlay init with `FUN_00027b00(0x64, {table @0x4c134, 99 entries})`; a `svc #6`
/// request `r1 = (cmd << 16) | (sub << 8) | 0x64` runs `table[cmd]`, a thin shim that
/// unpacks the request block and tail-calls the real `OSAL_*` op. See
/// `docs/reverse_engineering/triton_mid_swi_services.md` "Object 0x64 fully decoded".
pub const OSAL_SERVICE_OBJECT: u32 = 0x64;
/// cmd 27: `OSAL_s32SemaphoreCreate` (`osal_core/osalsmphr.cpp:0x2e0`, shim `0xbf08` ->
/// `0x36734`). Block = `{ name ptr @0, out-handle ptr @4, initial count @8, max count u16
/// @0xc }`; the created handle (a pointer to the semaphore control block) is written through
/// `block @4`. PROCNAV's single most-frequent 0x64 op - it stalls without a real handle.
pub const OSAL_SEM_CREATE: u32 = 0x1b;
/// `0x1c`: named-semaphore open (`OSAL_s32SemaphoreOpen`-style, wrapper `0x2ec06c`). Block
/// carries the name at `+0` and an optional handle slot at `+4`; unlike `0x1b` the handle
/// is returned in `r0` directly and `0` means "not found". Names the TTF framework expects
/// to find from earlier bring-up processes (e.g. `AIL_TRACE_SEMAPHORE`) are lazily created,
/// mirroring how the unit's first user of such a lock provisions it.
pub const OSAL_SEM_OPEN: u32 = 0x1c;
/// The other object-0x64 commands PROCNAV drives (each a shim in the `0x4c134` table tail-
/// branching to an `OSAL_*` leaf). The observable contract each must reproduce:
/// - `0x01` trace/log (`0x2e1ac`), returns 0.
/// - `0x02` get the calling task's current-context pointer (`0x288c8`): returns the value the
///   last `0x03` stored for this task, or `0` (the leaf's not-set sentinel) when none.
/// - `0x03` set the calling task's current-context pointer (`0x2e49c`): stores `block[0]`,
///   returns 0. `0x02`/`0x03` are a per-task set/get pair the message/sem ops read back.
/// - `0x0a` IOSC service re-dispatch (`0x2c3e4` -> nested svc #6), status only.
/// - `0x16` get the calling task's id (`0x2df74`): a positive per-task id.
/// - `0x17` registry App-id -> descriptor lookup (`0x44e20`): `-1` for `id < 1` (which is all
///   PROCNAV sends), the leaf itself rejects `id == 0`.
/// - `0x20` release/put the current context (`0x29500`), `0x21` post/acquire it with a param
///   (`0x29414`); both act on the `0x03`-set context and answer with status 0.
/// - `0x43` get/clear a status flag (`0x2cf88`), always returns 0.
pub const OSAL_TRACE: u32 = 0x01;
/// Last-error getter: `OSAL_s32GetLastError()`-style, `block == 0`, the returned code is the
/// monitor error the last failed service on this thread left behind.
pub const OSAL_GET_LAST_ERROR: u32 = 0x02;
pub const OSAL_CTX_SET: u32 = 0x03;
/// Monitor error codes `scd_bTestLookupIoControl` accepts as "this lookup service does not
/// exist here" (function/device not available); anything else keeps SCD in queue mode.
pub const OSAL_ERR_NO_FUNCTION: u32 = 0x72011;
pub const OSAL_IOSC_SERVICE: u32 = 0x0a;
pub const OSAL_GET_TASK_ID: u32 = 0x16;
pub const OSAL_REGISTRY_LOOKUP: u32 = 0x17;
pub const OSAL_CTX_RELEASE: u32 = 0x20;
pub const OSAL_CTX_POST: u32 = 0x21;
pub const OSAL_STATUS_FLAG: u32 = 0x43;
/// The OSAL Event family on object 0x64 (`osal_core/osalevent.cpp`), reached by PROCNAV once
/// the frequent context/sem commands above were answered. Same shape as the semaphore object.
/// - `0x31` `OSAL_EventCreate` (shim `0xbd30` -> `0x58b0`): block = `{ name ptr @0,
///   out-handle ptr @4, event param u16 @8 }`; the created handle (event control block) is
///   written through `block @4`, returns 0.
/// - `0x33` `OSAL_EventDelete` (shim `0xbd28` -> `0x5788`): block = `{ handle @0 }`, returns 0.
/// - `0x34` event close/release (shim `0xbd3c` -> `0x1b59c`): block = `{ handle @0 }`, 0 on the
///   success path PROCNAV follows.
pub const OSAL_EVENT_CREATE: u32 = 0x31;
pub const OSAL_EVENT_DELETE: u32 = 0x33;
pub const OSAL_EVENT_CLOSE: u32 = 0x34;
/// `0x35`: wait on a named event (wrapper `0x2ec3d4`). Block: `[handle, mode, ?, timeout]`
/// with `0xffffffff` meaning forever. PROCNAV's `vStartApp` tail (`0x53e76`) creates the
/// `NAVEND` event and waits on it as its main loop - the unit only signals it when SPM
/// tears the software block down - so an answered wait reads as "shutdown requested" and
/// ProcNav deinitialises every component. The wait must park.
pub const OSAL_EVENT_WAIT: u32 = 0x35;
/// `0x29`: open a named OSAL message queue by name (`scd_OpenQueue`'s leaf). Inline
/// 16-byte name at `block + 0x14`, and the queue handle is written through the out-slot
/// pointer at `block + 0x10`; status in r0. PROCNAV opens `mbx_0` and each started app's
/// `mbx_<appid>` this way.
pub const OSAL_MBX_OPEN: u32 = 0x29;
/// `0x2a`: create a named OSAL message queue (`scd_bCreateQueue`'s leaf, wrapper
/// continuation `0x34f643`). `[+0xc]` = max message size (8 - every payload is an OSAL
/// message *reference* `[type, content_ptr]`), `[+0x10]` = queue depth, `[+0x14]` = inline
/// name. The caller ignores any out-handle (it re-opens by name), answering 0 is enough.
pub const OSAL_MBX_CREATE: u32 = 0x2a;
/// `0x28` (leaf `0x664`, wrapper `0x288c8` region): the runtime-library variant of the
/// queue create used for the rtf task queues (`STAT_TASK1QUEUE`, `TEA_TASK1QUEUE`).
/// Pointer-shaped arguments in a four-word block: `[+0]` = name pointer,
/// `[+4]` = depth, `[+8]` = message size, `[+0xc]` = flags, `[+0x10]` = out-handle.
/// Unlike `0x2a` the caller consumes the handle (the STAT component stores it at
/// `ail+0xc0+0x30` where the delivery pump posts, and `STAT_TASK1`'s context keeps it
/// at `ctx+4` for the receive wait), so an unmodelled create leaves the whole
/// STAT task queue dead (`handle 0xffffffff` waits forever).
pub const OSAL_MBX_CREATE_PTR: u32 = 0x28;
/// `0x2d`: the application message pump's receive (wrapper continuation `0x190a23`).
/// `[+0]` = pointer to the handle slot the open wrote, `[+4]` = 8-byte receive buffer,
/// `[+8]` = buffer size, `[+0xc]` = priority out, `[+0x10]` = timeout (`0xffffffff` =
/// forever). Returns the received byte count (> 0) and only branches on last-error `0x72005`
/// as the benign "no message" code; an answered wait must never leave the queue unfilled.
pub const OSAL_MBX_WAIT: u32 = 0x2d;
/// `0x57`: resolve a received message reference in the receive pump (wrapper `0x2ec8dc`,
/// block `[+0]=1, [+4]=content_ptr, [+8]=0xffff0004`). Answers the **content base** - the
/// amt `amt_tclBaseMessage` accessors read fields at `content+0xb..0x1c` through it - and a
/// `0` answer makes the pump discard the message before the amt dispatcher sees it.
pub const OSAL_MBX_FIELD: u32 = 0x57;
/// cmd 85: the fastpool *capacity* probe in front of every `0x57`-backed
/// `OSAL_pvMemoryAllocate` (`0x30b2f0`). The repost builder's wrapper carries an
/// embedded buffer only after a full deserialise, so answering "does not fit"
/// immediately forces the `0x57` malloc fallback, which the emulator does model.
/// Left un-emulated the probe parks its caller in an 85-tick retry storm.
pub const OSAL_MEM_FITS: u32 = 0x55;
/// cmd 44 on object `0x64` (full service `0x2c0464`): post a built service-data
/// message to the component's pump object (see `complete_osal_service`).
pub const OSAL_IO_POST: u32 = 0x2c;
/// cmd 72 (shim `0xbd80` -> `0x5e88`): a stub leaf that unconditionally returns `3`, so the
/// truthful answer is `3` rather than the catch-all's `0`.
pub const OSAL_UNSUPPORTED3: u32 = 0x48;
/// The OSAL thread family on object 0x64 (`osal_core/osallock.c`/`osalthred.cpp`), reached
/// through the `_Cntxt` wrappers at PROCNAV `0x2ebeb8-0x2ebf3c`. Every one of them is
/// non-blocking and answers in `R0`: the block is `[cb ptr, ctx info, u16 4, continuation]`
/// and the thread CB the block points at is `{ name ptr @0, name len u16 @4, stack size @8,
/// entry @0xc, argument @0x10 }` - the same layout `libosal_linux` reads on the Linux side.
/// - `0x0e` `OSAL_ThreadCreate`: the thread starts out suspended; `0x0f` `OSAL_s32ThreadActivate`
///   takes `{ tid @0 }` and starts it. (The emulator starts created threads right away; see
///   `complete_osal_service`.)
/// - `0x10` `OSAL_ThreadSpawn`: create + start in one call; `R0` is the new thread id, `-1`
///   on failure - `OSAL_bInitAsyncIO` (`0x5e9968`) asserts on `-1` and skips its three
///   `AIO2_%02d` workers when the spawn is answered wrong.
pub const OSAL_THREAD_CREATE: u32 = 0x0e;
pub const OSAL_THREAD_ACTIVATE: u32 = 0x0f;
pub const OSAL_THREAD_SPAWN: u32 = 0x10;
pub const OSAL_THREAD_DELETE: u32 = 0x11;

/// TSE kernel objects (`tkse_*`, µITRON-style) on service object `0x13`. The service literals
/// are the ones the PROCNAV wrappers load next to themselves (`tkse_cre_sem` `0x6233c0`,
/// `tkse_del_sem` `0x623408`, `tkse_sig_sem` `0x623428`, `tkse_wai_sem` `0x623450`, and the
/// flag family at `0x5f5220-0x5f52a4`); `service >> 16` is the command, *not* a tick timeout -
/// reading it as ticks parked every call for ~4 s and answering `0` is what made
/// `OSAL_bInitAsyncIO` assert (`cre_flg` must return a **positive** id) and never spawn its
/// workers. Requests are the wrapper's stack frame: the first words are the arguments, and
/// the caller's return address follows them (visible in every parked-request dump).
/// - `0x1000` `tkse_cre_sem(semCB)`: block `[cb]`; `R0` = positive sem id. The OSAL lock
///   (`CreateOsalLock` `0x5f3d50`) is one of these, initial count 1.
/// - `0x1002` `tkse_sig_sem(sem)` / `0x1003` `tkse_wai_sem(sem, count, timeout)`: block
///   `[sem, count, timeout]`; the `LockOsal` wrapper (`0x5f3de0`) waits count 1, timeout
///   `0xffffffff`.
/// - `0x1020` `tkse_cre_flg(flgCB)`: block `[cb]`; `R0` = positive flag id (OSAL checks
///   `> 0`), pattern starts clear.
/// - `0x1022` `tkse_set_flg(flg, pattern)`: block `[flg, pattern]`; wakes matching waiters.
/// - `0x1024` `tkse_wai_flg(flg, pattern, atr, cycle, timeout)`: five-word block; matches
///   when every pattern bit is set, clears them on match. The async-IO worker
///   (`0x5e9430`) waits pattern `1` on its own queue flag with `atr = 0x21`, cycle `-1`,
///   timeout `0` and blocks until `set_flg`.
pub const TSE_OBJECT: u32 = 0x13;
/// TSE task management (µITRON `tk_*`) on service object `0x12` (PROCNAV wrappers
/// `0x5f27c0`-`0x5f2820`). `0x0005_0312` is a `tk_sta_tsk`-style task start when block
/// `[1]` carries an entry: `(interval, entry, context)` - `interval 1` starts the
/// stdin/command loop (`0x5ea540`, behind the stack-alignment thunk `0x5ea41c`), the
/// saturated interval starts the `0x5e8d98` state pump; a bare small word is a plain
/// `OSAL_s32TaskDelay`. `0x0006_0112` is polled by that command loop (`bgt` on `R0`) and
/// ends PROCNAV's init-finalize chain (`0x5e8a80`), whose return address sits in the
/// literal pool - the answer does not matter there, the stubs after it run into data and
/// the kernel's illtrap teardown ends the thread (`INSN_INVALID` at `0x5e8b64`).
pub const TSE_TASK_OBJECT: u32 = 0x12;
pub const TSE_TASK_TERMINATE: u32 = 0x0002_0412;
pub const TSE_TASK_DELAY: u32 = 0x0005_0312;
pub const TSE_TASK_DELETE: u32 = 0x0006_0112;
pub const TSE_SEM_CREATE: u32 = 0x1000_0113;
pub const TSE_SEM_DELETE: u32 = 0x1001_0113;
pub const TSE_SEM_SIGNAL: u32 = 0x1002_0213;
pub const TSE_SEM_WAIT: u32 = 0x1003_0313;
pub const TSE_FLG_CREATE: u32 = 0x1020_0113;
pub const TSE_FLG_DELETE: u32 = 0x1021_0113;
pub const TSE_FLG_SET: u32 = 0x1022_0213;
pub const TSE_FLG_WAIT: u32 = 0x1024_0513;
/// µITRON `E_TMOUT`, the error a timed TSE wait returns when the condition did not hold.
const TSE_E_TMOUT: u32 = 0xffff_fff3;
/// µITRON `E_OBJ`, returned for waits/signals on an id the emulator never created.
const TSE_E_OBJ: u32 = 0xffff_ffff;
/// Bytes of freshly mapped guest memory handed back as an event control block. The real op
/// (`0x58b0`) writes the name at `[cb + 0x10]` up to `0x20` bytes and a type id at `+0x34`, so
/// `0x60` covers the layout the client may read back.
const OSAL_EVENT_BLOCK: u32 = 0x60;
/// Bytes of freshly mapped guest memory handed back as a semaphore control block. The real
/// op (`0x36734`) writes the name at `[cb + 0x15]` up to `0x20` bytes, so `0x40` covers the
/// whole layout the client may read back.
const OSAL_SEM_BLOCK: u32 = 0x40;

/// Object `0x0a` (the bank/mbk + directory service) commands. The dispatcher
/// (`0x8004829c`) keys on `id >> 16`; the four PROCNAV issues during its DS
/// registration are here. Each is served to the observable contract the caller
/// (`0x5ea668`) depends on, since the real leaves (`0x80047b90`, `0x80051178`,
/// `0x800d1d68`, `0x80043a6c`) walk kernel state the image never initialises.
pub const SERVICE_MBK_OPEN: u32 = 0x0008_030a; // cmd 8:  validate + `0x80047b90`
pub const SERVICE_MBK_INFO: u32 = 0x0063_040a; // cmd 63: forward 4 words to `0x80051178`
pub const SERVICE_MBK_LEN: u32 = 0x0201_020a; // cmd 201: path length of the DS entry
pub const SERVICE_MBK_RESOLVE: u32 = 0x0203_020a; // cmd 203: resolve a handle to its object

/// Bytes of freshly mapped, zero-filled memory handed back for a resolved bank
/// object. The reader (`0x5f5af0`) only needs `[obj + 0x1c]` and `[obj + 0x2c]`;
/// a page is more than enough and keeps both reads inside the mapping.
const BANK_OBJECT_BYTES: u32 = 0x1000;

/// Bytes per chunk the pool allocator (`0x5f5d54`) expects the create service to hand back
/// for it to store at `[desc + 0x18]`. It multiplies that word by chunk counts wherever it
/// computes a region end (`end = p + n * chunk - 8`), compares request sizes against
/// `chunk >> 2` to pick its free-list walk direction, and rounds grow requests up to it, so
/// it is the slab granularity - not a pointer. Handing back a pointer here makes the
/// allocator's `mla` land in unmapped space, which is how `0x5f5ed4` died.
pub const POOL_CHUNK: u32 = 0x1000;

/// Largest single grow the emulator will back. The allocator asks for
/// `ceil((aligned_size + 16) / POOL_CHUNK)` chunks; anything wilder is a misread block.
const GROW_MAX_BYTES: u32 = 64 * 1024 * 1024;

/// The monitor image with the uImage header stripped, read once per process.
pub fn image() -> Option<&'static [u8]> {
    static IMAGE: std::sync::OnceLock<Option<Vec<u8>>> = std::sync::OnceLock::new();
    IMAGE
        .get_or_init(|| match std::fs::read(IMAGE_PATH) {
            Ok(bytes) if bytes.len() > IMAGE_HEADER_SIZE => {
                log::info!(
                    "monitor: loaded {} bytes of Triton monitor from {}",
                    bytes.len() - IMAGE_HEADER_SIZE,
                    IMAGE_PATH
                );
                Some(bytes[IMAGE_HEADER_SIZE..].to_vec())
            }
            Ok(bytes) => {
                log::error!("monitor: {} is only {} bytes", IMAGE_PATH, bytes.len());
                None
            }
            Err(error) => {
                log::error!("monitor: cannot read {}: {}", IMAGE_PATH, error);
                None
            }
        })
        .as_deref()
}

/// Map the monitor into the calling process and seed the RAM the boot code would have
/// left behind. Idempotent: the table pointer is the check, because it is zero until the
/// monitor is placed.
pub fn attach(unicorn: &mut Unicorn<'_, Context>) -> bool {
    let Some(image) = image() else {
        return false;
    };
    if read_word(unicorn, SERVICE_TABLE_POINTER) == Ok(SERVICE_TABLE) {
        return true;
    }

    let mmu = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    {
        let mut mmu = mmu.lock().unwrap();
        map_pages(
            &mut mmu,
            unicorn,
            MONITOR_BASE,
            CODE_END - MONITOR_BASE,
            Prot::READ | Prot::EXEC,
        );
        map_pages(
            &mut mmu,
            unicorn,
            CODE_END,
            RAM_END - CODE_END,
            // Executable as well, because [`TRAMPOLINES`] lives in this region.
            Prot::READ | Prot::WRITE | Prot::EXEC,
        );
    }

    // Copy the image, stopping at the code/data boundary because the two regions have
    // different permissions.
    let head = (CODE_END - MONITOR_BASE).min(image.len() as u32) as usize;
    if unicorn
        .mem_write(MONITOR_BASE as u64, &image[..head])
        .is_err()
    {
        log::warn!("monitor: cannot write the code section");
        return false;
    }
    if image.len() > head {
        if unicorn
            .mem_write(CODE_END as u64, &image[head..])
            .is_err()
        {
            log::warn!("monitor: cannot write the data section");
            return false;
        }
    }

    for (address, value) in [
        (SERVICE_TABLE_POINTER, SERVICE_TABLE),
        (DUAL_CORE_SKIP, DUAL_CORE_SKIP_VALUE),
        (DEVICE_REGISTRY, DEVICE_REGISTRY_VALUE),
        (SERVICE_STACK_POINTER, SERVICE_STACKS),
        (SERVICE_STACKS, SERVICE_STACK),
        // Before the monitor stores its tagged form, the thread slot already points at
        // the single thread the emulator gives it.
        (TIB + 4, TCB),
    ] {
        if write_word(unicorn, address, value).is_err() {
            log::warn!("monitor: cannot seed the word at {:#x}", address);
            return false;
        }
    }

    for (id, handler) in SERVICE_HANDLERS {
        if write_word(unicorn, SERVICE_TABLE + 4 * id, *handler).is_err() {
            log::warn!("monitor: cannot register handler {:#x} for id {}", handler, id);
            return false;
        }
    }

    let sites = install_cp15_shim(unicorn);
    spawn_system_tasks(unicorn);

    log::info!(
        "monitor: Triton monitor mapped at {:#x} ({} bytes), service table at {:#x}, message pump at {:#x}, {} coprocessor sites rewritten",
        MONITOR_BASE,
        image.len(),
        SERVICE_TABLE,
        MESSAGE_PUMP,
        sites
    );
    true
}

/// Rewrite the coprocessor accesses the emulator cannot execute.
///
/// The monitor reads its thread slot with `mrc p15,0,r12,c13,c0,3` and stores the thread
/// control block back with the matching `mcr`. Unicorn implements neither the banked TLS
/// registers nor the SPSR semantics, so every coprocessor instruction is replaced at
/// attach time:
///
/// * `mrc ...,c13,c0,3` becomes a branch to a stub loading the [`TIB`] address,
/// * `mrc ...,c13,c0,4` becomes a stub loading the word at `[TIB + 4]`, i.e. the thread
///   control block the monitor stores there,
/// * every other coprocessor access (cache maintenance, wait-for-interrupt, the `mcr`
///   write-back) becomes a `nop`, which leaves `[TIB + 4]` holding the plain thread
///   control block pointer instead of the tagged form the monitor would have written.
///
/// Returns how many sites were rewritten.
pub fn install_cp15_shim(unicorn: &mut Unicorn<'_, Context>) -> usize {
    use capstone::arch::arm::ArchMode;
    use capstone::prelude::*;
    use capstone::{Capstone, Endian};

    let Ok(cs) = Capstone::new().arm().mode(ArchMode::Arm).endian(Endian::Little).build() else {
        return 0;
    };

    let mut rewritten = 0usize;
    let mut stub = TRAMPOLINES;
    let mut chunk = [0u8; 0x4000];

    for page in (MONITOR_BASE..CODE_END).step_by(chunk.len()) {
        let Ok(()) = unicorn.mem_read(page as u64, &mut chunk) else {
            continue;
        };
        // Literal pools and jump tables sit between functions, so decoding stops in the
        // middle of a chunk routinely. Resume after whatever was decoded instead of
        // dropping the rest of the page.
        let mut offset = 0usize;
        while offset + 4 <= chunk.len() {
            let decoded = match cs.disasm_all(&chunk[offset..], (page + offset as u32) as u64) {
                Ok(insns) => insns,
                Err(_) => {
                    offset += 4;
                    continue;
                }
            };
            let mut reached = offset;
            for insn in decoded.iter() {
                reached = (insn.address() as u32 - page) as usize + 4;
                let Some((target, value)) =
                    cp15_action(insn.mnemonic().unwrap_or(""), insn.op_str().unwrap_or(""))
                else {
                    continue;
                };
                let address = insn.address() as u32;
                if rewrite_cp15_site(unicorn, value, target, address, stub) {
                    stub += match value {
                        Cp15::Constant(_) => 12,
                        _ => 8,
                    };
                    rewritten += 1;
                }
            }
            offset = if reached > offset { reached } else { offset + 4 };
        }
    }
    rewritten
}

/// Replace one coprocessor instruction, either with a `nop` or with a branch to a stub
/// placed at `stub`. Returns whether the site was rewritten.
fn rewrite_cp15_site(
    unicorn: &mut Unicorn<'_, Context>,
    action: Cp15,
    target: u32,
    address: u32,
    stub: u32,
) -> bool {
    match action {
        Cp15::Nop => {
            return unicorn
                .mem_write(address as u64, &0xe320_f000u32.to_le_bytes())
                .is_ok();
        }
        Cp15::Constant(value) => {
            // ldr rD, [pc] ; b site + 4 ; .word value
            let loaded = 0xe59f_0000u32 | target << 12;
            let layout = [loaded, branch_to(address + 4, stub + 4), value];
            let bytes: Vec<u8> = layout.iter().flat_map(|word| word.to_le_bytes()).collect();
            if unicorn.mem_write(stub as u64, &bytes).is_err() {
                return false;
            }
        }
        Cp15::Indirect(base) => {
            // The trampoline page is the one right above the thread slot, so the word is
            // always within PC-relative load range.
            let signed = base as i64 - (stub as i64 + 8);
            if signed.unsigned_abs() > 0xfff {
                return false;
            }
            let load = if signed < 0 { 0xe51f_0000 } else { 0xe59f_0000 }
                | target << 12
                | signed.unsigned_abs() as u32;
            let bytes: Vec<u8> = [load, branch_to(address + 4, stub + 4)]
                .iter()
                .flat_map(|word| word.to_le_bytes())
                .collect();
            if unicorn.mem_write(stub as u64, &bytes).is_err() {
                return false;
            }
        }
    }
    unicorn
        .mem_write(address as u64, &branch_to(stub, address).to_le_bytes())
        .is_ok()
}

/// What a coprocessor instruction has to be replaced with.
#[derive(Clone, Copy)]
enum Cp15 {
    Nop,
    Constant(u32),
    Indirect(u32),
}

impl Cp15 {
    fn constant(&self) -> u32 {
        match self {
            Cp15::Constant(value) => *value,
            _ => 0,
        }
    }
}

/// Classify one disassembled coprocessor instruction into `(destination register, action)`.
fn cp15_action(mnemonic: &str, ops: &str) -> Option<(u32, Cp15)> {
    if mnemonic != "mrc" && mnemonic != "mrc2" && mnemonic != "mcr" && mnemonic != "mcr2" {
        return None;
    }
    let fields: Vec<&str> = ops.split(',').map(|field| field.trim()).collect();
    // p15, opcode1, Rt, CRn, CRm, opcode2
    if fields.len() != 6 || fields[0] != "p15" {
        return None;
    }
    let destination = fields[2].strip_prefix('r')?.parse::<u32>().ok()?;
    let is_read = mnemonic.starts_with("mrc");
    match (fields[3], fields[5], is_read) {
        ("c13", "3", true) => Some((destination, Cp15::Constant(TIB))),
        ("c13", "4", true) => Some((destination, Cp15::Indirect(TIB + 4))),
        _ => Some((destination, Cp15::Nop)),
    }
}

/// Emulate the effect of a monitor yield (`svc #6` with a positive R1).
///
/// The real handler (`0x800371b4`) takes `R1 & 0xff` as the number of a monitor kernel
/// object - created by the kernel object manager while the monitor initialises - pushes
/// `R1` into the yielding thread's slot and blocks it on that object; `R1 >> 16` is the
/// timeout in ticks. The objects cannot be reconstructed from the image because nothing
/// here runs the code that creates them, so the blocking half is what the emulator
/// reproduces: the thread waits for the object or for the deadline, whichever comes
/// first. Returns whether the thread was parked.
pub fn yield_thread(unicorn: &mut Unicorn<'_, Context>, args: [u32; 4]) -> bool {
    const TICK: std::time::Duration = std::time::Duration::from_millis(1);

    let (block, service, continuation) = (args[0], args[1], args[2]);
    let object = service & 0xff;
    let ticks = service >> 16;
    // A task start/delete on the object-0x12 family is answered in place. `0x50312` is a
    // µITRON `tk_sta_tsk`-style *task start* (`(interval, entry, context)`): both PROCNAV
    // callers (`1` = the stdin/command loop behind the alignment thunk `0x5ea41c`,
    // `0xfffffffe` = the `0x5e8d98` state pump) get a real guest thread. `0x60112` answers
    // "no more work" through `R0`: the command loop (`0x5ea574`) tests it in a `bgt` loop
    // and returns from its entry on 0, while the exit chain (`0x5e8b3c`) ignores the
    // reply and walks into its literal-pool "return address" (the stubs there end in
    // data, so the real kernel's illtrap teardown kills the thread the same way).
    if object == TSE_TASK_OBJECT {
        let self_id = unicorn.get_data().thread_id();
        match service {
            TSE_TASK_DELETE | TSE_TASK_TERMINATE => {
                let tid = read_word(unicorn, block).unwrap_or(0);
                log::info!(
                    "monitor: thread [{self_id}] task {} for thread [{tid}] at {block:#x} answered",
                    if service == TSE_TASK_DELETE { "delete" } else { "terminate" }
                );
                // r752: `ter_tsk(0)` (`tid == 0` in the request block) terminates the
                // *calling* task - the PROCNAV stdin/command loop at 0x5ea41c ends its
                // service cycle with exactly that call. The real monitor kills the task
                // at the svc; it never runs anything after the call. Answering the svc
                // instead let the guest walk its epilogue and die on a `pop {r0,pc}`
                // reading one word past the 16 KiB monitor stack top, which printed a
                // READ_UNMAPPED + full register/MMU/disasm dump at every boot
                // (0x5ea468, thread [20], present since the golden era). Exit at the
                // svc now: same exit code 1 and the same parked `exited(1)` line as
                // before, minus the fault noise.
                if service == TSE_TASK_TERMINATE && tid == 0 {
                    crate::emulator::thread::exit_current_thread(unicorn, 1);
                    return true;
                }
                if tid != 0 && tid != self_id {
                    let mut threads = unicorn.get_data().threads.lock().unwrap();
                    if let Some(target) = threads.iter_mut().find(|t| t.id == tid) {
                        if !matches!(target.status, ThreadStatus::Exited(_)) {
                            target.status = ThreadStatus::Exited(0);
                        }
                    }
                }
                if unicorn.reg_write(RegisterARM::R0 as i32, 0).is_ok() {
                    return true;
                }
            }
            TSE_TASK_DELAY => {
                let interval = read_word(unicorn, block).unwrap_or(0);
                let entry = read_word(unicorn, block + 4).unwrap_or(0);
                if entry >= 0x1000 {
                    let context = read_word(unicorn, block + 8).unwrap_or(0);
                    let tid = spawn_guest_thread(unicorn, entry, context, 0x2800);
                    log::info!(
                        "monitor: thread [{self_id}] TSE task start (interval {interval:#x}, entry {entry:#x}, context {context:#x}) -> thread [{tid}]"
                    );
                    if unicorn.reg_write(RegisterARM::R0 as i32, 0).is_ok() {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    // Requests the emulator can answer without waiting do not belong on a deadline at all:
    // the TSE creates/signals/sets and the OSAL thread family return through `R0` the way
    // the monitor's own leaf would, and a TSE *wait* whose condition already holds (an
    // unlock racing ahead of its lock) finishes in place too. Writing `R0` and letting the
    // `svc` retire reproduces a non-blocking completion exactly.
    let immediate = if object == TSE_OBJECT {
        complete_tse(unicorn, service, block, true)
    } else if object == OSAL_IO_OBJECT {
        // The io-object commands (open/close/create/remove/ioctl/read/write) are the
        // registry client's leaf calls. On the unit the triton side answers them in
        // microseconds and never deschedules the caller; parking every one of them for
        // its nominal 1-5 ticks stretched a ~600-operation SCD registry walk into
        // minutes of serialized host time, which turned PROCNAV's FC_LBS instantiation
        // (and every late CCA answer) into a scheduling lottery (f136 completed the
        // walk at t=27 s, f137-f139 were still walking at t=100 s). Answer in place,
        // exactly like the monitor's own leaf; commands `complete_osal_io` does not
        // model still fall through to the deadline path unchanged.
        complete_osal_io(unicorn, service, block)
    } else if object == OSAL_SERVICE_OBJECT
        && matches!(
            ticks,
            OSAL_THREAD_CREATE
                | OSAL_THREAD_ACTIVATE
                | OSAL_THREAD_SPAWN
                | OSAL_THREAD_DELETE
                | OSAL_MBX_CREATE
                | OSAL_MBX_OPEN
                | OSAL_MBX_FIELD
                | OSAL_MEM_FITS
                | OSAL_IO_POST
                | OSAL_STATUS_FLAG
                | OSAL_TRACE
                | OSAL_IOSC_SERVICE
                | OSAL_CTX_RELEASE
                | OSAL_CTX_POST
                | OSAL_EVENT_DELETE
                | OSAL_EVENT_CLOSE
        )
    {
        complete_osal_service(unicorn, service, block)
    } else if object == OSAL_SERVICE_OBJECT && ticks == OSAL_MBX_WAIT {
        // a queue that already holds a message must not park its waiter
        complete_osal_mbx_wait(unicorn, unicorn.get_data().thread_id(), block, true)
    } else {
        None
    };
    if let Some(status) = immediate {
        log::info!(
            "monitor: thread [{}] service {service:#x} (object {object}) completes in place with {status:#x}: {}",
            unicorn.get_data().thread_id(),
            describe(unicorn, block)
        );
        if unicorn.reg_write(RegisterARM::R0 as i32, status as u64).is_ok() {
            return true;
        }
    }
    // The object-0x13 waits carry their real timeout in the request block (`sem: [2]`,
    // `flg: [4]`); the service's upper half is their *command*, so for them the parked
    // deadline comes from the block - 0 or a saturated word means "wait until woken", and
    // `poll_tse_wait` is what actually releases them. A `tk_dly_tsk` parks on its one
    // block word, which is already a tick count.
    let deadline = if service == TSE_TASK_DELAY {
        // With no entry word this is a plain `OSAL_s32TaskDelay(ticks)` and parks on the
        // block word; task starts were answered in place above.
        match read_word(unicorn, block) {
            Ok(ticks) if ticks != 0 && ticks < 0x0010_0000 => {
                Some(crate::emulator::vtime::deadline(TICK * ticks))
            }
            _ => None,
        }
    } else if matches!(service, TSE_SEM_WAIT | TSE_FLG_WAIT) {
        let timeout = read_word(
            unicorn,
            block + if service == TSE_SEM_WAIT { 0x8 } else { 0x10 },
        )
        .unwrap_or(0xffff_ffff);
        if timeout == 0 || timeout >= 0x8000_0000 {
            None
        } else {
            Some(crate::emulator::vtime::deadline(TICK * timeout))
        }
    } else if service & 0xff == OSAL_SERVICE_OBJECT
        && matches!(service >> 16, OSAL_EVENT_WAIT | OSAL_MBX_WAIT)
    {
        // The OSAL commands are the service's upper half, not a deadline; the event wait
        // carries its real timeout at `block + 0xc` (`0xffffffff` for the NAVEND latch) and
        // the queue wait at `block + 0x10`, both `0xffffffff` for forever.
        let timeout = read_word(unicorn, block + if service >> 16 == OSAL_MBX_WAIT { 0x10 } else { 0xc })
            .unwrap_or(0xffff_ffff);
        if timeout == 0 || timeout >= 0x8000_0000 {
            None
        } else {
            Some(crate::emulator::vtime::deadline(TICK * timeout))
        }
    } else if ticks == 0 {
        None
    } else {
        Some(crate::emulator::vtime::deadline(TICK * ticks))
    };
    log::info!(
        "monitor: thread [{}] yields on service {:#x} (object {}, {} ticks, block {:#x}, continuation {:#x}) with {}",
        unicorn.get_data().thread_id(),
        service,
        object,
        ticks,
        block,
        continuation,
        describe(unicorn, block)
    );
    block_current_thread(
        unicorn,
        BlockReason::MonitorYield {
            service,
            block,
            continuation,
            deadline,
        },
    );
    true
}

/// The request block as hex and as far as it reads like text, because some requests carry
/// a name inline.
fn describe(unicorn: &mut Unicorn<'_, Context>, block: u32) -> String {
    let mut words = [0u32; 8];
    let mut raw = [0u8; 4];
    for (index, word) in words.iter_mut().enumerate() {
        if unicorn
            .mem_read((block.wrapping_add(4 * index as u32)) as u64, &mut raw)
            .is_ok()
        {
            *word = u32::from_le_bytes(raw);
        }
    }
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let text: String = bytes
        .iter()
        .map(|byte| {
            if byte.is_ascii_graphic() || *byte == b' ' {
                *byte as char
            } else {
                '.'
            }
        })
        .collect();
    // Some requests pass a name (a queue like `mbx_0`, a device, a service) as a pointer
    // instead of inline, so the raw word reads as an address. Follow each pointer-shaped
    // word one level and surface anything that looks like a NUL-terminated identifier: this
    // is what reveals which object service carries which queue name.
    let names = dereferenced_names(unicorn, &words);
    let suffix = if names.is_empty() {
        String::new()
    } else {
        format!(" names [{}]", names.join(", "))
    };

    format!(
        "words {} | {}{}",
        words
            .iter()
            .map(|word| format!("{:#x}", word))
            .collect::<Vec<_>>()
            .join(" "),
        text,
        suffix
    )
}

/// For every word in the request block that looks like a pointer into the guest's address
/// space, read what it points at and keep it if it is a plausible identifier string
/// (`[A-Za-z][A-Za-z0-9_./-]{2,31}`, NUL terminated). Returns `w<index>->"<string>"` pairs.
fn dereferenced_names(unicorn: &mut Unicorn<'_, Context>, words: &[u32; 8]) -> Vec<String> {
    let mut found = Vec::new();
    for (index, word) in words.iter().enumerate() {
        // Treat it as a pointer only if it is in the user address range and not a small
        // integer the caller packed into the field.
        if *word < 0x1_0000 || *word >= 0xf000_0000 {
            continue;
        }
        if let Some(name) = read_identifier(unicorn, *word) {
            found.push(format!("w{}->{:?}", index, name));
        }
    }
    found
}

/// Read up to 32 bytes at `address` and return them as a `String` only if they form a
/// NUL-terminated run of identifier characters at least three characters long.
fn read_identifier(unicorn: &mut Unicorn<'_, Context>, address: u32) -> Option<String> {
    let mut bytes = [0u8; 32];
    unicorn.mem_read(address as u64, &mut bytes).ok()?;
    let end = bytes.iter().position(|byte| *byte == 0)?;
    if end < 3 || end >= 32 {
        return None;
    }
    let run = &bytes[..end];
    let printable = |byte: u8| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'.' | b'/' | b'-')
    };
    if !run[0].is_ascii_alphabetic() || !run.iter().all(|byte| printable(*byte)) {
        return None;
    }
    String::from_utf8(run.to_vec()).ok()
}

/// `EMU_SENSORFI_TRACE=1`: surface PROCNAV's service->app lookup state so the sensorfi
/// provider shim can be written against what the guest actually does rather than static
/// inference. `scd_bGetAppIdFromServiceId` (`0x34f012`) branches on `cRam01215110`
/// (`0x1215110`): `0` = the static service->app map it builds by iterating the registry,
/// `1` = query the SCD queue over `OSAL_s32IOControl(handle, 0xb, ...)`. `iRam01215114`
/// (`0x1215114`) is the SCD queue handle from `scd_bCreateQueue`/`scd_OpenQueue`, `-1` when
/// no connection was obtained (which forces mode `0`). Both are PROCNAV `.bss` globals at
/// their Ghidra addresses (the binary runs unrelocated), so they are read straight from the
/// guest. The value is logged only when it changes, so a full run yields a handful of lines.
fn trace_scd_lookup(unicorn: &mut Unicorn<'_, Context>) {
    static ON: OnceLock<bool> = OnceLock::new();
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);
    if !*ON.get_or_init(|| std::env::var_os("EMU_SENSORFI_TRACE").is_some())
        || !unicorn.get_data().elf_path.contains("PROCNAV")
    {
        return;
    }
    // Two brief guest-memory reads; no host lock is held across them and nothing sleeps.
    let mode = read_word(unicorn, 0x1215_110).map(|word| word & 0xff).unwrap_or(0xff);
    let handle = read_word(unicorn, 0x1215_114).unwrap_or(u32::MAX);
    let packed = (u64::from(mode) << 32) | u64::from(handle);
    if LAST.swap(packed, std::sync::atomic::Ordering::Relaxed) == packed {
        return;
    }
    log::warn!(
        "EMU-SCD: PROCNAV service->app lookup mode {} (0=static registry map, 1=SCD-queue ioctl), SCD handle {:#x} ({})",
        mode,
        handle,
        handle as i32,
    );
}

/// Hand a yielded request back, the way the thread that owns the monitor object would
/// when it completes the request: whatever the caller expects lands in its request
/// block, and R0 carries the status the continuation branches on (`cmp r0, #0` with the
/// out-parameter read for `r0 >= 0`).
pub fn complete(unicorn: &mut Unicorn<'_, Context>, service: u32, block: u32) -> u32 {
    trace_scd_lookup(unicorn);
    // The OSAL service object (0x64) dispatches on `cmd = service >> 16` through the 99-entry
    // table blob0 registers; each cmd is answered like the pool/bank services above - by
    // reproducing the observable contract of the real `OSAL_*` leaf in the guest's own RAM.
    // Only the cmds modelled here branch out; every other 0x64 cmd still falls through to the
    // catch-all below, so this stays a narrow, per-cmd slice.
    if service & 0xff == OSAL_SERVICE_OBJECT {
        if let Some(status) = complete_osal_service(unicorn, service, block) {
            return status;
        }
    }
    // The OSAL IO object (0x6e) carries the registry client traffic (`/dev/registry/...`)
    // and `/dev/trace`; every id on it is modelled or consciously acknowledged, so the
    // whole object branches out here.
    if service & 0xff == OSAL_IO_OBJECT {
        if let Some(status) = complete_osal_io(unicorn, service, block) {
            return status;
        }
    }
    // The TSE object (0x13): a timed wait that reaches this completion path has either been
    // woken by its predicate (checked again here) or run out its deadline, so it is answered
    // as an expiry rather than parked a second time. Commands that still refuse to answer
    // (the mbx family) keep falling through to the catch-all.
    if service & 0xff == TSE_OBJECT {
        if let Some(status) = complete_tse(unicorn, service, block, false) {
            return status;
        }
    }
    match service {
        // `0x60c9d0`, called from `0x60d630`, initialises a pool descriptor: it zeroes
        // `[desc+0x18]`/`[desc+0x20]`, stores the caller's grow callback at `[desc+0x24]`,
        // and the resumed continuation (`0x60d674`) reads `[sp, #4]` - `block + 0xc` - and
        // stores that into `[desc+0x18]`, which the allocator then treats as *bytes per
        // chunk*. So this service answers with the slab granularity, not with a buffer;
        // the buffers arrive through the grow service.
        SERVICE_CREATE_POOL => {
            if write_word(unicorn, block + 0xc, POOL_CHUNK).is_err() {
                log::warn!(
                    "monitor: cannot write the chunk size for the pool request at {:#x}",
                    block
                );
                return 0xfff6_0000;
            }
            log::info!(
                "monitor: pool request at {:#x} completed with chunk size {:#x}",
                block,
                POOL_CHUNK
            );
            0
        }
        // When the free list runs dry the allocator tail-calls the grow thunk at
        // `[desc+0x24]` (`0x5f2a28`, and `0x60d600` for the other pool), which repacks
        // `(chunk count, context)` into the `0x60c988` wrapper: the block is
        // `[pointer to the result slot, chunk count, context, continuation]`, the
        // continuation reads `[sp]` - the word `block + 0` points at - and turns a negative
        // status into NULL with `movlt r0, #0`. The allocator initialises the whole region
        // itself - it lays the end sentinel at `p + count * chunk - 8` and threads both
        // nodes through it - so the region must be exactly `count * POOL_CHUNK` bytes of
        // mapped, writable memory and nothing else. The context word is the create
        // caller's first argument (the thunk at `0x5f2a54` passes the 16 MB pool size),
        // so it identifies the pool, not the request.
        SERVICE_POOL_GROW => {
            let slot = read_word(unicorn, block).unwrap_or(0);
            let count = read_word(unicorn, block + 4).unwrap_or(0);
            let total = read_word(unicorn, block + 8).unwrap_or(0);
            let size = count.saturating_mul(POOL_CHUNK);
            if count == 0 || size > GROW_MAX_BYTES {
                log::warn!(
                    "monitor: refusing grow of {} chunks at {:#x} (slot {:#x}, pool {:#x})",
                    count,
                    block,
                    slot,
                    total
                );
                return 0xfff6_0000;
            }
            let mmu = {
                let data = unicorn.get_data();
                data.mmu.clone()
            };
            let region = mmu.lock().unwrap().heap_alloc(
                unicorn,
                size,
                Prot::READ | Prot::WRITE,
                "[monitor-pool]",
            );
            if region == 0 || write_word(unicorn, slot, region).is_err() {
                log::warn!(
                    "monitor: cannot hand out {} bytes for the grow at {:#x} into {:#x}",
                    size,
                    block,
                    slot
                );
                return 0xfff6_0000;
            }
            log::info!(
                "monitor: grow request at {:#x} ({} chunks, pool {:#x}) completed with region {:#x}",
                block,
                count,
                total,
                region
            );
            0
        }
        // `0x60d624` hands back a grown region. The emulator never gives heap memory out
        // again, so this service only has to acknowledge.
        SERVICE_POOL_FREE => {
            log::info!(
                "monitor: free request at {:#x} acknowledged: {}",
                block,
                describe(unicorn, block)
            );
            0
        }
        // Command 203 resolves a handle to the object that owns the caller's incoming
        // message list and writes its address through the output slot at `block + 4`.
        // PROCNAV's DS registration (`0x5ea668`) calls it with `r0 = 0` (the self handle)
        // and `r1 = &obj->0x24`; the continuation (`0x5ea7d4`) then runs
        // `0x5f2ab4 -> 0x5f5af0`, which reads `[obj->0x24 + 0x1c]` (a byte count) and
        // `[obj->0x24 + 0x2c]` (a `u16` element count) and walks the buffer only when the
        // count is non-zero. Nothing has been received at boot, so the truthful state the
        // bank manager hands back is an empty list: a freshly mapped, zero-filled page has
        // a zero count, and `0x5f5af0` returns `0` without dereferencing the buffer.
        SERVICE_MBK_RESOLVE => {
            let out = read_word(unicorn, block + 4).unwrap_or(0);
            if out == 0 {
                log::warn!(
                    "monitor: mailbox resolve at {:#x} names a null output slot",
                    block
                );
                return 0xfff6_0000;
            }
            let mmu = {
                let data = unicorn.get_data();
                data.mmu.clone()
            };
            let object = mmu.lock().unwrap().heap_alloc(
                unicorn,
                BANK_OBJECT_BYTES,
                Prot::READ | Prot::WRITE,
                "[monitor-mbk]",
            );
            if object == 0 || write_word(unicorn, out, object).is_err() {
                log::warn!(
                    "monitor: cannot back the bank object for the resolve at {:#x} into {:#x}",
                    block,
                    out
                );
                return 0xfff6_0000;
            }
            log::info!(
                "monitor: mailbox resolve at {:#x} wrote empty bank object {:#x} into {:#x}",
                block,
                object,
                out
            );
            0
        }
        // The other three bank commands report status only on the paths PROCNAV follows:
        // `0x8030a` (register a bank) and `0x201020a` (ask the bank's path length) are
        // branched on with `cmp r0, #0` alone, and a zero length makes the caller fall back
        // to its statically linked default path (`0x5ea72c` -> `0x5ea734`), which is the
        // same branch the real manager takes for an unregistered entry. Neither output is
        // modelled beyond the status, so they are reported as served, not as emulated.
        SERVICE_MBK_OPEN | SERVICE_MBK_LEN => {
            log::info!(
                "monitor: bank request {:#x} at {:#x} served with status 0: {}",
                service,
                block,
                describe(unicorn, block)
            );
            0
        }
        // `0x63040a` forwards its four request words to `0x80051178`; PROCNAV only stores
        // the returned word into a global (`0x5ea6a0`) and does not dereference it here, so
        // it is answered with a success status until that value's use is decoded.
        SERVICE_MBK_INFO => {
            log::info!(
                "monitor: bank info {:#x} at {:#x} served with status 0: {}",
                service,
                block,
                describe(unicorn, block)
            );
            0
        }
        other => {
            log::info!(
                "monitor: service {:#x} request at {:#x} completed without a result: {}",
                other,
                block,
                describe(unicorn, block)
            );
            0
        }
    }
}

/// Everything the OSAL service object (0x64) shares. The unit keeps this state in the monitor
/// kernel, which every process reads the same way, so it is host-global (not per-process):
/// named objects (semaphores) resolve to the same handle across processes, and each task's
/// "current context" pointer (`0x03` set / `0x02` get) is per task, matching the leaf's
/// `record + 0x60` walk keyed by task id. Kept small and lock-fast: the lock is never held
/// across a memory map, a guest write, or a sleep.
#[derive(Default)]
struct OsalState {
    /// Named semaphore name -> guest control-block handle (process-global).
    sems: HashMap<String, u32>,
    /// Named event name -> guest control-block handle (process-global).
    events: HashMap<String, u32>,
    /// Thread id -> the value the last `0x03` stored for it (the current context).
    context: HashMap<u32, u32>,
    /// Thread id -> the monitor error code the last failed service left behind, served back
    /// by the get-last-error command `0x20`.
    last_error: HashMap<u32, u32>,
    /// Thread id -> a dense positive OSAL task id, handed out by `0x16`.
    task_ids: HashMap<u32, u32>,
    next_task_id: u32,
    /// TSE semaphore id -> current count (object `0x13`, `tkse_*_sem`). Ids are µITRON
    /// ERIDs: small and positive, `0` is never handed out.
    tse_sems: HashMap<u32, u32>,
    /// TSE flag id -> currently set pattern (object `0x13`, `tkse_*_flg`).
    tse_flags: HashMap<u32, u32>,
    /// The next positive TSE id to hand out.
    tse_next_id: u32,
    /// Per-process bump cursor for the `0x57` message-reference arm's
    /// malloc-fallback role (see `complete_osal_service`); absent while the
    /// process has not allocated from its private slab yet.
    mem_slabs: HashMap<u32, u32>,
    /// Named OSAL message queue (`mbx_<appid>`) name -> the monitor handle the open hands
    /// back. Handles are process-global like the host-side `MqState` queues they name; the
    /// base keeps them clear of the TSE id space.
    mbx_handles: HashMap<String, u32>,
    next_mbx_handle: u32,
}

/// Process-global OSAL service state, shared by every process's monitor instance (mirrors the
/// `pwr_proxy::global()` `OnceLock<Mutex<..>>` pattern).
fn osal_state() -> &'static Mutex<OsalState> {
    static STATE: OnceLock<Mutex<OsalState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(OsalState::default()))
}

/// Record the monitor last-error a failing service leaves behind, so the
/// following get-last-error command (`0x20`) hands it back.  Also used by the
/// libosal traps that deny a service outside the monitor (r791).
pub fn note_osal_last_error(thread: u32, code: u32) {
    osal_state().lock().unwrap().last_error.insert(thread, code);
}

/// The `OSAL_NOT_SET` sentinel the context leaf (`0x288c8`) hands back when the task has no
/// context stored: a null pointer is the value PROCNAV's callers already branch on.
const OSAL_CONTEXT_UNSET: u32 = 0;

/// Serve an object-0x64 command the way the shim in the `0x4c134` table + its `OSAL_*` leaf do,
/// reproducing only the observable contract (arguments read from the request block, the `R0`
/// status the continuation branches on, and any value written back through an out-parameter).
/// Returns `Some(status)` for a command it models and `None` for an unmodelled one, which then
/// falls through to the generic catch-all. It never parks the thread and never holds the shared
/// lock across a guest write or memory map.
fn complete_osal_service(
    unicorn: &mut Unicorn<'_, Context>,
    service: u32,
    block: u32,
) -> Option<u32> {
    let thread = unicorn.get_data().thread_id();
    match service >> 16 {
        OSAL_SEM_CREATE => Some(complete_osal_sem_create(unicorn, service, block)),
        OSAL_SEM_OPEN => Some(complete_osal_sem_open(unicorn, block)),
        OSAL_EVENT_CREATE => Some(complete_osal_event_create(unicorn, service, block)),
        OSAL_MBX_CREATE => Some(complete_osal_mbx_create(unicorn, thread, block)),
        OSAL_MBX_CREATE_PTR if service == 0x2806_64 => {
            Some(complete_osal_mbx_create_ptr(unicorn, thread, block))
        }
        OSAL_MBX_OPEN => Some(complete_osal_mbx_open(unicorn, thread, block)),
        OSAL_MBX_WAIT => complete_osal_mbx_wait(unicorn, thread, block, false),
        // `0x57`: resolve a received message reference `{1, content}` to its data pointer.
        // r212 field-helper traces (0x258b6a = `amt_tclBaseMessage::u8GetValue(4)`, the pump
        // stores the answer at `[block+4]` and double-dereferences through it) show the real
        // OSAL answers the **content base**: the amt accessors (`u16GetValue` @ 0x2e3418 =
        // `[[this+4]+idx]`) read serviceId u16 @ data+0x14, propertyId @ +0x18, opCode @ +0x1a,
        // class/type @ +0xb - the same envelope the snapshot bodies carry from offset 0.
        // Answering `content + 8` (r211) shifted every field by 8 and silently mis-dispatched.
        // The same command doubles as `OSAL_pvMemoryAllocate` (`0x2ec8dc` wrapper, used by the
        // service-data repost builder `0x2b7a6c` -> `0x30b2f0`): there `[block+4]` is a size,
        // not a pointer, and the caller only branches on NULL, so a non-null slab block is
        // bumped out of a private guest page. That repost is what feeds the component
        // dispatcher queue (`[adapter]+0x54`), the only in-image path to `s32ProcessMessage`.
        OSAL_MBX_FIELD => {
            let content = read_word(unicorn, block + 4).unwrap_or(0);
            if (0x1000..0xf000_0000).contains(&content) {
                // A receive buffer that holds a posted reference `{1, inner}`
                // (what our queue model delivers into the waiter's slot) is
                // dereferenced once: the real OSAL wait hands the *token* to
                // the channel, the channel stores it, and `bEnableAccess`
                // (`0x30b27e` -> this wrapper) resolves token -> data before
                // `u8GetValue` reads the class byte at `data + 0xb` (r379:
                // answering the buffer itself made STAT_TASK1 read zeros there
                // and reject every message as "Unknown type of message").
                let mut answer = content;
                let mut via = "";
                if read_word(unicorn, content).unwrap_or(0) == 1 {
                    let inner = read_word(unicorn, content.wrapping_add(4)).unwrap_or(0);
                    if (0x1000..0xf000_0000).contains(&inner) {
                        answer = inner;
                        via = " (dereferenced)";
                    }
                }
                log::info!(
                    "monitor: OSAL[{thread}] message reference {content:#x} -> data {answer:#x}{via}"
                );
                Some(answer)
            } else if (0x30b2f0..0x30b340).contains(&(read_word(unicorn, block + 0xc)
                .unwrap_or(0)
                & !1))
            {
                // malloc role: callers are the `0x30b2f0` wrapper family (continuation
                // right after `blx 0x2ec8dc`). `block[8]` is the *embedded-slab* request the
                // guest computed (often garbage like `0xffff0004`), so clamp the bump.
                let pid = unicorn.get_data().process_id;
                let size = (read_word(unicorn, block + 8).unwrap_or(0) & 0xff).max(4);
                let known = osal_state().lock().unwrap().mem_slabs.get(&pid).copied();
                let slab = match known {
                    Some(slab) => slab,
                    None => {
                        let mmu = {
                            let data = unicorn.get_data();
                            data.mmu.clone()
                        };
                        let addr = {
                            let mut mmu = mmu.lock().unwrap();
                            mmu.heap_alloc(
                                unicorn,
                                0x1000,
                                Prot::READ | Prot::WRITE,
                                "[osal-mem-57]",
                            )
                        };
                        osal_state()
                            .lock()
                            .unwrap()
                            .mem_slabs
                            .insert(pid, addr);
                        addr
                    }
                };
                let step = ((size + 0xf) & !0xf).max(0x10);
                let (block_ptr, exhausted) = {
                    let mut state = osal_state().lock().unwrap();
                    let block = state.mem_slabs.get(&pid).copied().unwrap_or(slab);
                    let next = block.wrapping_add(step);
                    let exhausted = next >= slab + 0x1000;
                    if !exhausted {
                        state.mem_slabs.insert(pid, next);
                    }
                    (block, exhausted)
                };
                if exhausted {
                    log::warn!(
                        "monitor: OSAL[{thread}] service {service:#x} alloc out of slab space (block {content:#x})"
                    );
                    Some(0)
                } else {
                    log::info!(
                        "monitor: OSAL[{thread}] service {service:#x} alloc(size {size:#x}) -> {block_ptr:#x}"
                    );
                    Some(block_ptr)
                }
            } else {
                log::info!(
                    "monitor: OSAL[{thread}] message reference {content:#x} -> data 0x0"
                );
                Some(0)
            }
        }
        // `0x55`: capacity probe, see `OSAL_MEM_FITS`. "Does not fit" pushes the
        // caller into the `0x57` malloc fallback, which hands out real blocks.
        OSAL_MEM_FITS => {
            log::debug!(
                "monitor: OSAL[{thread}] service {service:#x} capacity probe at {block:#x} -> not fitted"
            );
            Some(0)
        }
        // `0x2c0464` (traced leaf `0x19089e`): post the built service-data message to
        // the component's pump object. Block = `{pump, &wrapper+0xc, 8, 2, cont,
        // data, trace-str, svc}`. `data` (`block[5]`) is the 0x18-byte serialized
        // body: `w0 = src app | dst app << 16` - the kernel routes it to
        // `mbx_{dst}`. Capped: a consumer that misses its ownership chain re-posts
        // the same body back "to the kernel", so without a real owner the exchange
        // would ping-pong; after eight hops the candidate list is declared exhausted
        // (`-1`), matching a device where nobody subscribed.
        0x2c if service == 0x2c04_64 => {
            // Two callers share this service.  The STAT component's delivery pump
            // (`0x577d1`, wrapper veneer `0x2ec28c`) posts an 8-byte message
            // reference by HANDLE: block = `{handle, ref_ptr, 8, sub-id}`.  The
            // amt repost chain (`0x249647`) instead passes `{pump, .., body_ptr}`
            // with a 0x18-byte routing body at `[+0x14]`.  A 0x4000_xxxx handle
            // in `[+0]` disambiguates.
            let w0 = read_word(unicorn, block).unwrap_or(0);
            if (0x4000_0000..0x5000_0000).contains(&w0) {
                let ref_ptr = read_word(unicorn, block.wrapping_add(4)).unwrap_or(0);
                let content_ptr = read_word(unicorn, ref_ptr.wrapping_add(4)).unwrap_or(0);
                let name = osal_state()
                    .lock()
                    .unwrap()
                    .mbx_handles
                    .iter()
                    .find(|(_, h)| **h == w0)
                    .map(|(n, _)| n.clone());
                let mut posted = false;
                if let Some(name) = &name {
                    // The pump posts the 8-byte OSAL message *reference*
                    // `{1, content}` (wrapper +0xc holds it, getter `0x30b237`);
                    // copy the message body out of the poster's frame because
                    // the waiter runs later, on another stack, and resolves a
                    // `{1, copy}` reference through `0x57` to `copy`.
                    let content_ptr = read_word(unicorn, ref_ptr.wrapping_add(4)).unwrap_or(0);
                    // Word-by-word so a body that sits near the end of a mapped
                    // region still copies up to the boundary instead of failing
                    // wholesale (a rejected post drops the message silently).
                    let mut content = vec![0u8; 0x200];
                    let mut copied = 0usize;
                    for chunk in 0..(content.len() / 4) {
                        let ok = unicorn
                            .mem_read(
                                (content_ptr as usize + chunk * 4) as u64,
                                &mut content[chunk * 4..chunk * 4 + 4],
                            )
                            .is_ok();
                        if !ok {
                            break;
                        }
                        copied = (chunk + 1) * 4;
                    }
                    if copied >= 8 {
                        let namespace = unicorn.get_data().namespace.clone();
                        let mmu_arc = {
                            let data = unicorn.get_data();
                            data.mmu.clone()
                        };
                        let copy = {
                            let mut alloc = mmu_arc.lock().unwrap();
                            alloc.heap_alloc(
                                unicorn,
                                0x200,
                                unicorn_engine::unicorn_const::Prot::READ
                                    | unicorn_engine::unicorn_const::Prot::WRITE,
                                "[osal-post-copy]",
                            )
                        };
                        if unicorn.mem_write(copy as u64, &content).is_ok() {
                            let mut item = Vec::with_capacity(8);
                            item.extend_from_slice(&1u32.to_le_bytes());
                            item.extend_from_slice(&copy.to_le_bytes());
                            let mut guard = namespace.lock().unwrap();
                            crate::common::osal_queues::OsalQueueService::ensure_queue(
                                &mut guard.mq, name,
                            );
                            posted = crate::common::osal_queues::OsalQueueService::guest_post(
                                &mut guard.mq,
                                name,
                                {
                                    let mut b = Vec::with_capacity(16);
                                    b.extend_from_slice(&1u32.to_le_bytes());
                                    b.extend_from_slice(&8u32.to_le_bytes());
                                    b.extend_from_slice(&item);
                                    b
                                },
                                0,
                            );
                            if posted {
                                guard.notify_waiters();
                            }
                        }
                    }
                }
                log::info!(
                    "monitor: OSAL[{thread}] post on handle {w0:#x} ({name:?}) ref {ref_ptr:#010x} content {content_ptr:#010x} -> {}",
                    if posted { "queued" } else { "failed" }
                );
                return Some(if posted { 0 } else { -1i32 as u32 });
            }
            let pump = w0;
            let body = read_word(unicorn, block.wrapping_add(0x14)).unwrap_or(0);
            let w0 = read_word(unicorn, body).unwrap_or(0);
            let (src_app, dst_app) = (w0 & 0xffff, (w0 >> 16) & 0xffff);
            static SENT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = SENT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // r799n12: the routing body's word 3 (body+0x0c) is the reply-
            // queue slot the *kernel* stamps with the posting application's
            // queue handle - the receiver stores it into its server port
            // (+0x20) and posts service-data answers through it.  We used to
            // forward the body verbatim (word 3 = 0xffffffff), so every
            // answer leg died on `ail_bIpcMessagePost(app, queue=-1)` and the
            // svc-0x1e ServiceData replies never left procmapengine - the
            // same kernel stamp also covers app ids above 0xff (`mbx_1024`
            // is decimal app 1024), so the dst range check becomes "any
            // non-self app".  The stamp comes from our own queue table, i.e.
            // it is exactly the value the real kernel would know.
            if n < 40 && dst_app != 0 && dst_app != src_app {
                let namespace = unicorn.get_data().namespace.clone();
                let mut guard = namespace.lock().unwrap();
                let queue = format!("mbx_{dst_app}");
                crate::common::osal_queues::OsalQueueService::ensure_queue(&mut guard.mq, &queue);
                let mut content = vec![0u8; 0x18];
                let blob = if unicorn.mem_read(body as u64, &mut content).is_ok() {
                    let src_queue = format!("mbx_{src_app}");
                    crate::common::osal_queues::OsalQueueService::ensure_queue(
                        &mut guard.mq,
                        &src_queue,
                    );
                    let handle = osal_mbx_handle(&src_queue);
                    content[0x0c..0x10].copy_from_slice(&handle.to_le_bytes());
                    let mut b = Vec::with_capacity(8 + 0x18);
                    b.extend_from_slice(&1u32.to_le_bytes());
                    b.extend_from_slice(&0x18u32.to_le_bytes());
                    b.extend_from_slice(&content);
                    b
                } else {
                    crate::libs::procnav::gps_status_message(n as u16, 0x45)
                };
                if crate::common::osal_queues::OsalQueueService::guest_post(
                    &mut guard.mq,
                    &queue,
                    blob,
                    0,
                ) {
                    guard.notify_waiters();
                    log::info!(
                        "monitor: OSAL[{thread}] pump post on {pump:#x}: body {body:#010x} src {src_app:#x} -> routed to {queue}"
                    );
                }
            } else {
                log::debug!(
                    "monitor: OSAL[{thread}] pump post on {pump:#x}: body {body:#010x} src {src_app:#x} dst {dst_app:#x} -> no candidate (-1)"
                );
            }
            Some(-1i32 as u32)
        }
        // `0x48` (shim `0xbd80` -> leaf `0x5e88`): a stub that always returns 3.
        OSAL_UNSUPPORTED3 => {
            log::info!(
                "monitor: OSAL[{thread}] service {service:#x} (cmd 0x48) at {block:#x} -> 3"
            );
            Some(3)
        }
        // `0x03` (`0x2e49c`): store `block[0]` as this task's current context, return 0.
        OSAL_CTX_SET => {
            let value = read_word(unicorn, block).unwrap_or(0);
            osal_state().lock().unwrap().context.insert(thread, value);
            log::info!(
                "monitor: OSAL[{thread}] context set to {value:#x} at {block:#x}"
            );
            Some(0)
        }
        // `0x02`: the last-error getter. Every observed call site (`0x2ebd5c` and the SCD
        // probe family) passes `block == 0` and branches on the returned monitor error code;
        // `scd_bTestLookupIoControl` only believes a refused lookup when it carries `0x72011`
        // or `0x72015` (function/device not available), which is what switches PROCNAV to
        // its static SCD registry map.
        OSAL_GET_LAST_ERROR => {
            let error = osal_state()
                .lock()
                .unwrap()
                .last_error
                .get(&thread)
                .copied()
                .unwrap_or(0);
            log::info!("monitor: OSAL[{thread}] last error get -> {error:#x}");
            Some(error)
        }
        // `0x16` (`0x2df74`): a positive, stable per-task id (assigned densely on first ask).
        OSAL_GET_TASK_ID => {
            let mut state = osal_state().lock().unwrap();
            let existing = state.task_ids.get(&thread).copied();
            let id = match existing {
                Some(id) => id,
                None => {
                    state.next_task_id += 1;
                    let id = state.next_task_id;
                    state.task_ids.insert(thread, id);
                    id
                }
            };
            drop(state);
            log::info!("monitor: OSAL[{thread}] task id get -> {id}");
            Some(id)
        }
        // `0x17` (`0x44e20`): App-id -> registry descriptor lookup. PROCNAV only ever asks for
        // `id == 0`, which the leaf itself rejects (`id < 1`), so the truthful answer is -1.
        OSAL_REGISTRY_LOOKUP => {
            let id = read_word(unicorn, block).unwrap_or(0) as i32;
            if id < 1 {
                log::info!(
                    "monitor: OSAL[{thread}] registry lookup id {id} at {block:#x} -> not found"
                );
                return Some(0xffff_ffff);
            }
            // A non-zero id we cannot resolve from the (unmodelled) registry is also "not found".
            log::warn!(
                "monitor: OSAL[{thread}] registry lookup id {id:#x} at {block:#x}: registry not modelled -> not found"
            );
            Some(0xffff_ffff)
        }
        // Status-only commands whose leaf answers 0 for every path PROCNAV follows: the IOSC
        // re-dispatch, the trace/log op, the always-zero status flag, and the two context
        // post/release ops that act on the `0x03`-set context. Named here so they are served
        // on their decoded contract instead of by the blind catch-all.
        OSAL_IOSC_SERVICE | OSAL_TRACE | OSAL_STATUS_FLAG | OSAL_CTX_RELEASE | OSAL_CTX_POST
        | OSAL_EVENT_DELETE | OSAL_EVENT_CLOSE => {
            log::info!(
                "monitor: OSAL[{thread}] service {service:#x} (cmd {:#x}) at {block:#x} served with status 0: {}",
                service >> 16,
                describe(unicorn, block)
            );
            Some(0)
        }
        // The thread family. These are non-blocking and answered in place (see
        // `yield_thread`); `ThreadCreate` semantics are only half modelled - the real op
        // starts the thread suspended for `0x0f` to release, the emulator starts it right
        // away, which is safe because every created entry parks on its own wait object
        // until there is work. Nothing in the launched set used `0x0e`/`0x0f` yet; `0x10`
        // is the one PROCNAV's async-IO init drives once `cre_flg` hands back real ids.
        OSAL_THREAD_CREATE | OSAL_THREAD_SPAWN => {
            let cb = read_word(unicorn, block).unwrap_or(0);
            let tid = start_osal_thread(
                unicorn,
                cb,
                if service >> 16 == OSAL_THREAD_CREATE { "create" } else { "spawn" },
            );
            Some(tid)
        }
        OSAL_THREAD_ACTIVATE => {
            let tid = read_word(unicorn, block).unwrap_or(0);
            log::info!("monitor: OSAL[{thread}] thread activate for thread [{tid}] at {block:#x}");
            Some(0)
        }
        OSAL_THREAD_DELETE => {
            // taking the target away is the whole point of this call: a parked worker
            // must not be woken by a later signal, so the delete marks it exited.
            let tid = read_word(unicorn, block).unwrap_or(0);
            let self_id = thread;
            if tid == 0 || tid == self_id {
                log::info!("monitor: OSAL[{thread}] thread delete ends the thread at {block:#x}");
                crate::emulator::thread::exit_current_thread(unicorn, 0);
                return Some(0);
            }
            let mut threads = unicorn.get_data().threads.lock().unwrap();
            if let Some(target) = threads.iter_mut().find(|t| t.id == tid) {
                if !matches!(target.status, ThreadStatus::Exited(_)) {
                    target.status = ThreadStatus::Exited(0);
                }
                drop(threads);
                log::info!("monitor: OSAL[{thread}] thread delete for thread [{tid}] at {block:#x}");
                Some(0)
            } else {
                drop(threads);
                log::warn!("monitor: OSAL[{thread}] thread delete for unknown thread [{tid}]");
                Some(0xffff_ffff)
            }
        }
        _ => None,
    }
}

/// Serve a TSE kernel-object request (object `0x13`) from the emulator-side µITRON state.
/// `at_entry` tells the two waits apart: called while the request arrives (thread still in
/// the `svc`) an unsatisfied wait returns `None` so the caller can park the thread, while
/// called from the completion path the same wait has run out of luck and answers `E_TMOUT`.
/// A wait on an id that was never created answers `E_OBJ` right away instead of parking -
/// that is what a real µITRON kernel says, and it keeps a caller holding a stale id from
/// sleeping forever. The lock is never held across a memory map or a guest write.
fn complete_tse(unicorn: &mut Unicorn<'_, Context>, service: u32, block: u32, at_entry: bool) -> Option<u32> {
    let thread = unicorn.get_data().thread_id();
    match service {
        TSE_SEM_CREATE => {
            let id = {
                let mut state = osal_state().lock().unwrap();
                state.tse_next_id += 1;
                let id = state.tse_next_id;
                // `CreateOsalLock` (`0x5f3d50`) builds its locks from this call and expects
                // `LockOsal` to take the first unit right away, so the count starts at 1.
                state.tse_sems.insert(id, 1);
                id
            };
            log::info!("monitor: TSE[{thread}] semaphore create -> id {id}");
            Some(id)
        }
        TSE_SEM_DELETE => {
            let id = read_word(unicorn, block).unwrap_or(0);
            osal_state().lock().unwrap().tse_sems.remove(&id);
            Some(0)
        }
        TSE_SEM_SIGNAL => {
            let id = read_word(unicorn, block).unwrap_or(0);
            let mut state = osal_state().lock().unwrap();
            match state.tse_sems.get_mut(&id) {
                Some(count) => *count += 1,
                None => {
                    drop(state);
                    log::warn!("monitor: TSE[{thread}] signal on unknown semaphore {id}");
                    return Some(TSE_E_OBJ);
                }
            }
            Some(0)
        }
        TSE_SEM_WAIT => {
            let id = read_word(unicorn, block).unwrap_or(0);
            let mut state = osal_state().lock().unwrap();
            match state.tse_sems.get_mut(&id) {
                Some(count) if *count > 0 => {
                    *count -= 1;
                    Some(0)
                }
                Some(_) => {
                    if at_entry {
                        None
                    } else {
                        log::warn!("monitor: TSE[{thread}] semaphore {id} wait timed out");
                        Some(TSE_E_TMOUT)
                    }
                }
                None => {
                    drop(state);
                    log::warn!("monitor: TSE[{thread}] wait on unknown semaphore {id}");
                    Some(TSE_E_OBJ)
                }
            }
        }
        TSE_FLG_CREATE => {
            // The flag name lives inline in the CB as two 4-character words (the async-IO
            // workers name theirs `AIO2_%02d` split across them); read it for the log only.
            let cb = read_word(unicorn, block).unwrap_or(0);
            let (lo, hi) = (
                read_word(unicorn, cb + 0xc).unwrap_or(0),
                read_word(unicorn, cb + 0x10).unwrap_or(0),
            );
            let name: String = lo
                .to_le_bytes()
                .into_iter()
                .chain(hi.to_le_bytes())
                .map(|byte| if byte.is_ascii_graphic() { byte as char } else { '.' })
                .collect();
            let id = {
                let mut state = osal_state().lock().unwrap();
                state.tse_next_id += 1;
                let id = state.tse_next_id;
                state.tse_flags.insert(id, 0);
                id
            };
            log::info!("monitor: TSE[{thread}] flag create (name {name:?}) -> id {id}");
            Some(id)
        }
        TSE_FLG_DELETE => {
            let id = read_word(unicorn, block).unwrap_or(0);
            osal_state().lock().unwrap().tse_flags.remove(&id);
            Some(0)
        }
        TSE_FLG_SET => {
            let flag = read_word(unicorn, block).unwrap_or(0);
            let pattern = read_word(unicorn, block + 4).unwrap_or(0);
            let mut state = osal_state().lock().unwrap();
            match state.tse_flags.get_mut(&flag) {
                Some(bits) => *bits |= pattern,
                None => {
                    drop(state);
                    log::warn!("monitor: TSE[{thread}] set {pattern:#x} on unknown flag {flag}");
                    return Some(TSE_E_OBJ);
                }
            }
            Some(0)
        }
        TSE_FLG_WAIT => {
            let flag = read_word(unicorn, block).unwrap_or(0);
            let pattern = read_word(unicorn, block + 4).unwrap_or(0);
            let mut state = osal_state().lock().unwrap();
            match state.tse_flags.get_mut(&flag) {
                Some(bits) if *bits & pattern == pattern => {
                    *bits &= !pattern;
                    Some(0)
                }
                Some(_) => {
                    if at_entry {
                        None
                    } else {
                        log::warn!("monitor: TSE[{thread}] flag {flag} pattern {pattern:#x} wait timed out");
                        Some(TSE_E_TMOUT)
                    }
                }
                None => {
                    drop(state);
                    log::warn!("monitor: TSE[{thread}] wait on unknown flag {flag}");
                    Some(TSE_E_OBJ)
                }
            }
        }
        _ => None,
    }
}

/// Re-check a parked TSE wait from the scheduler: `Some(0)` once the semaphore can be taken
/// or the flag pattern matches (and the unit is consumed), `None` while it must keep waiting.
/// Only the two wait services are looked at; everything parked elsewise is untouched.
pub fn poll_tse_wait(unicorn: &mut Unicorn<'_, Context>, service: u32, block: u32) -> Option<u32> {
    if !matches!(service, TSE_SEM_WAIT | TSE_FLG_WAIT) {
        return None;
    }
    complete_tse(unicorn, service, block, true)
}

/// Host-side raise of a TSE flag pattern (the emulator playing the role of a sender that
/// delivers a message and signals the receiving app's wake flag). Waiters are released by
/// `poll_tse_wait` on the next scheduler pass.
pub fn raise_tse_flag(flag: u32, pattern: u32) {
    osal_state()
        .lock()
        .unwrap()
        .tse_flags
        .entry(flag)
        .and_modify(|bits| *bits |= pattern)
        .or_insert(pattern);
}

/// Start a guest thread for an OSAL create/spawn request: a fresh zero-filled stack from the
/// process heap, `R0` = argument, `LR` = 0 (returning from the entry ends the thread through
/// the scheduler's null-pc guard), and the entry's ARM/Thumb bit folded into `CPSR`. Mirrors
/// what `clone` does for Linux-side threads; the entry runs in the caller's address space.
/// Build (once per process) the OSAL task-loop stub in guest memory and return
/// its address. The stub mirrors the SCD task-entry trampoline
/// (`ldr r1,[r0]; ldr r1,[r1,#0x28]; bx r1` - call `vRun()` on the task
/// descriptor) and adds the re-run loop the real RTOS glue provides: the task
/// bodies are coroutines whose completion epilogue pops their entry-time `r3`
/// and branches to it, so `r3` must point at a continuation that calls
/// `vRun()` again with the descriptor restored in `r0`. Without it every
/// `*_TASK*` thread returned from its first iteration into pc=0 and the
/// scheduler terminated it (`Exited(139)`).
fn guest_task_loop_stub(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let stub = {
        let data = unicorn.get_data();
        data.inner.task_loop_stub()
    };
    if stub != 0 {
        return stub;
    }
    let mmu = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    let addr = {
        let mut mmu = mmu.lock().unwrap();
        mmu.heap_alloc(unicorn, 0x1000, Prot::READ | Prot::WRITE | Prot::EXEC, "[osal-task-stub]")
    };
    // THUMB: ldr r1,[r0]; ldr r1,[r1,#0x28]; push {r4}; mov r4,r0; blx r1;
    //        mov r0,r4; ldr r1,[r0]; ldr r1,[r1,#0x28]; blx r1
    // The bodies re-enter through their entry-time `r3` = the re-run site at
    // stub+0xa: restore the descriptor from r4 (callee-saved), reload the
    // vmethod (bodies clobber r1) and call vRun() again.
    let code: &[u8] = &[
        0x01, 0x68, 0x89, 0x6a, 0x10, 0xb4, 0x04, 0x46, 0x88, 0x47, 0x20, 0x46, 0x01, 0x68, 0x89,
        0x6a, 0x88, 0x47,
    ];
    if unicorn.mem_write(addr as u64, code).is_err() {
        log::warn!("monitor: cannot write the OSAL task-loop stub at {addr:#x}");
        return 0;
    }
    log::info!("monitor: OSAL task-loop stub at {addr:#010x}");
    // trace every first call and re-run through the stub (cheap: only task
    // threads pass through here, and only on log level debug)
    for (label, off) in [("first", 0u32), ("re-run", 0xa)] {
        let tag = label.to_string();
        crate::os::code_stub::trap_site(
            unicorn,
            addr + off,
            crate::os::code_stub::TrapMode::Thumb,
            "rtos/monitor.rs:1796",
            move |uc: &mut Unicorn<'_, Context>| {
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0);
                let tid = uc.get_data().thread_id();
                log::debug!("monitor: task-stub {tag}[{tid}] r0={r0:#010x} r4={r4:#010x}");
                0
            },
        );
    }
    let data = unicorn.get_data();
    data.inner.set_task_loop_stub(addr);
    addr
}

/// True when `entry` is the SCD task trampoline `ldr r1,[r0]; ldr r1,[r1,#0x28];
/// bx r1` - the entries whose `vRun()` bodies re-enter through `bx r3`. Other
/// OSAL entries (e.g. the AE engines calling slot 0x10) run their own pump
/// loops and must keep their original entry semantics.
fn is_scd_task_trampoline(unicorn: &mut Unicorn<'_, Context>, entry: u32) -> bool {
    let mut bytes = [0u8; 6];
    if unicorn.mem_read((entry & !1) as u64, &mut bytes).is_err() {
        return false;
    }
    let half = |i: usize| u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]);
    let ldr = half(1);
    let imm5 = (ldr >> 6) & 0x1f;
    let rn = (ldr >> 3) & 7;
    let rd = ldr & 7;
    half(0) == 0x6801 && (ldr >> 11) == 0b01101 && imm5 * 4 == 0x28 && rn == 1 && rd == 1
        && half(2) == 0x4708
}

fn spawn_guest_thread(unicorn: &mut Unicorn<'_, Context>, entry: u32, arg: u32, stack_size: u32) -> u32 {
    let size = (stack_size.max(0x4000) + 0xfff) & !0xfff;
    let mmu = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    let stack = {
        let mut mmu = mmu.lock().unwrap();
        mmu.heap_alloc(unicorn, size, Prot::READ | Prot::WRITE, "osal-thread-stack")
    };
    let sp = (stack + size - 8) & !7;
    let stub = if is_scd_task_trampoline(unicorn, entry) {
        guest_task_loop_stub(unicorn)
    } else {
        0
    };
    let parent_context = unicorn.context_init().unwrap();
    unicorn
        .reg_write(RegisterARM::R0 as i32, arg as u64)
        .unwrap();
    unicorn.reg_write(RegisterARM::R13 as i32, sp as u64).unwrap();
    // r754: task bodies that *return* (the PROCNAV state pump `0x5e8d98` is
    // just `vMsgHandler`, which falls through once its context word is not a
    // known message id) used to branch to LR=0: the scheduler reported
    // `ERROR cannot resume at pc=0x0` and SIGSEGV-terminated the task every
    // boot.  The real µITRON kernel gives a task body a return point that
    // tears the task down, and this emulator already has the equivalent:
    // the kernel-trap `exit` stub (same one glibc-style mq-notify threads
    // return through).  LR = stub turns the task's plain `return` into a
    // normal, quiet thread exit instead of a null-page illtrap.
    unicorn
        .reg_write(RegisterARM::R14 as i32, MQ_NOTIFY_EXIT_STUB as u64)
        .unwrap();
    // SCD task bodies are coroutines that return through `bx r3` into their
    // re-run continuation; with r3 = 0 the first completion branch jumped to
    // the null page and the scheduler terminated the thread (Exited(139)).
    // The stub duplicates the entry trampoline (`bx [descriptor->vt+0x28]`)
    // plus the re-run loop, so start the thread there instead.
    let start = if stub != 0 {
        unicorn.reg_write(RegisterARM::R3 as i32, (stub + 0xa) as u64).unwrap();
        stub
    } else {
        entry & !1
    };
    unicorn.reg_write(RegisterARM::PC as i32, start as u64).unwrap();
    let cpsr = unicorn.reg_read(RegisterARM::CPSR).unwrap_or(0) as u32;
    let cpsr = if entry & 1 != 0 { cpsr | 0x20 } else { cpsr & !0x20 };
    unicorn.reg_write(RegisterARM::CPSR as i32, cpsr as u64).unwrap();
    let child_context = unicorn.context_init().unwrap();
    unicorn.context_restore(&parent_context).unwrap();

    let tid = {
        let data = unicorn.get_data();
        let tid = data.inner.next_thread_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        data.threads.lock().unwrap().push(GuestThread {
            id: tid,
            rtos_task: true,
            status: ThreadStatus::Runnable,
            cpu_context: Some(child_context),
            pc: start,
            pending_result: None,
            clear_child_tid: None,
        });
        tid
    };
    tid
}

/// r799d: spawn a guest thread that CALLS a guest function with the given
/// r0..r3 and exits through the kernel teardown stub when it returns.  Lets
/// a fabricated TE-side delivery run the client's OWN native OSAL post
/// (real queue ring, real waiter wake-up) instead of re-implementing the
/// queue semantics host-side - hosting the queue bridge-side broke the
/// consumer loop twice (f263-f265, f275; see the note in osal-queues.rs).
pub fn spawn_guest_call(
    unicorn: &mut Unicorn<'_, Context>,
    entry: u32,
    args: [u32; 4],
    stack_size: u32,
) -> u32 {
    if entry < 0x1000 {
        return 0xffff_ffff;
    }
    let size = (stack_size.max(0x4000) + 0xfff) & !0xfff;
    let mmu = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    let stack = {
        let mut mmu = mmu.lock().unwrap();
        mmu.heap_alloc(unicorn, size, Prot::READ | Prot::WRITE, "guest-call-stack")
    };
    if stack == 0 {
        return 0xffff_ffff;
    }
    let sp = (stack + size - 8) & !7;
    let parent_context = unicorn.context_init().unwrap();
    for (reg, value) in [
        (RegisterARM::R0, args[0]),
        (RegisterARM::R1, args[1]),
        (RegisterARM::R2, args[2]),
        (RegisterARM::R3, args[3]),
        (RegisterARM::R13, sp),
    ] {
        unicorn
            .reg_write(reg as i32, value as u64)
            .unwrap();
    }
    // return = quiet thread exit (see spawn_guest_thread's r754 comment).
    unicorn
        .reg_write(RegisterARM::R14 as i32, MQ_NOTIFY_EXIT_STUB as u64)
        .unwrap();
    let start = entry & !1;
    unicorn
        .reg_write(RegisterARM::PC as i32, start as u64)
        .unwrap();
    let cpsr = unicorn.reg_read(RegisterARM::CPSR).unwrap_or(0) as u32;
    let cpsr = if entry & 1 != 0 { cpsr | 0x20 } else { cpsr & !0x20 };
    unicorn.reg_write(RegisterARM::CPSR as i32, cpsr as u64).unwrap();
    let child_context = unicorn.context_init().unwrap();
    unicorn.context_restore(&parent_context).unwrap();

    let tid = {
        let data = unicorn.get_data();
        let tid = data
            .inner
            .next_thread_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        data.threads.lock().unwrap().push(GuestThread {
            id: tid,
            rtos_task: true,
            status: ThreadStatus::Runnable,
            cpu_context: Some(child_context),
            pc: start,
            pending_result: None,
            clear_child_tid: None,
        });
        tid
    };
    tid
}

/// Read and start one OSAL thread CB (`{ name, len, stack, entry, argument }`, see the
/// `OSAL_THREAD_*` docs). Returns the new thread id, or `-1` when the CB carries no entry -
/// the callers (`OSAL_bInitAsyncIO` among them) branch on exactly that.
fn start_osal_thread(unicorn: &mut Unicorn<'_, Context>, cb: u32, verb: &str) -> u32 {
    if cb < 0x1000 {
        log::warn!("monitor: OSAL thread {verb} on bad CB {cb:#x}");
        return 0xffff_ffff;
    }
    let name_ptr = read_word(unicorn, cb).unwrap_or(0);
    let stack = read_word(unicorn, cb + 8).unwrap_or(0);
    let entry = read_word(unicorn, cb + 0xc).unwrap_or(0);
    let arg = read_word(unicorn, cb + 0x10).unwrap_or(0);
    if entry < 0x1000 {
        log::warn!("monitor: OSAL thread {verb} CB {cb:#x} carries entry {entry:#x}");
        return 0xffff_ffff;
    }
    let name = read_c_string(unicorn, name_ptr).unwrap_or_default();
    let tid = spawn_guest_thread(unicorn, entry, arg, stack);
    // The real µITRON kernel releases a created task without letting it run
    // through: the creator keeps the CPU until it blocks.  Creator-side init
    // relies on that (STAT_tclThread.cpp): `s32Initialize` vStart()s the STAT
    // tasks, then still fills the object members vEntry() null-checks.  An
    // immediate Runnable start lets the child win the race and both STAT
    // threads exit on `NaviThread::vEntry() nullpointer` (f258/f260: the
    // +0x4c producer at lr 0x4d3732 lands ~40 ms after the threads died).
    // A small start delay keeps the creator ahead; EMU_OSAL_START_DELAY_MS=0
    // disables it for control runs.
    let delay_ms = std::env::var("EMU_OSAL_START_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(60);
    if delay_ms > 0 && tid != 0xffff_ffff {
        if let Some(target) = unicorn
            .get_data()
            .threads
            .lock()
            .unwrap()
            .iter_mut()
            .find(|t| t.id == tid)
        {
            target.status = ThreadStatus::Blocked(BlockReason::SleepUntil(
                std::time::Instant::now() + std::time::Duration::from_millis(delay_ms),
            ));
        }
    }
    log::info!(
        "monitor: OSAL thread {verb} (name {name:?}, entry {entry:#x}, arg {arg:#x}, stack {stack:#x}) -> thread [{tid}]{}",
        if delay_ms > 0 {
            format!(" (start delayed {delay_ms} ms, EMU_OSAL_START_DELAY_MS=0 disables)")
        } else {
            String::new()
        },
    );
    tid
}

/// Serve `OSAL_EventCreate` (cmd 49) the way `0x58b0` does: a named event whose handle (the
/// control-block pointer) lands in the request block's output slot at `block @4`. Mirrors
/// [`complete_osal_sem_create`]: the name is resolved through the shared registry so a name
/// created by one process resolves to the same handle in another, and the hook never holds the
/// lock across the map or a write.
fn complete_osal_event_create(
    unicorn: &mut Unicorn<'_, Context>,
    service: u32,
    block: u32,
) -> u32 {
    let name = read_word(unicorn, block).unwrap_or(0);
    let out = read_word(unicorn, block + 4).unwrap_or(0);
    let param = (read_word(unicorn, block + 8).unwrap_or(0) & 0xffff) as u16;
    if out == 0 {
        log::warn!(
            "monitor: event create at {:#x} names a null output slot",
            block
        );
        return 0xfff6_0000;
    }
    let name = read_identifier(unicorn, name);
    let shared = name
        .as_ref()
        .and_then(|n| osal_state().lock().unwrap().events.get(n).copied());
    let handle = match shared {
        Some(handle) => {
            log::info!(
                "monitor: event create {:?} at {:#x} reuses shared handle {:#x}",
                name.as_deref().unwrap_or("?"),
                block,
                handle
            );
            handle
        }
        None => {
            let mmu = {
                let data = unicorn.get_data();
                data.mmu.clone()
            };
            let control = mmu.lock().unwrap().heap_alloc(
                unicorn,
                OSAL_EVENT_BLOCK,
                Prot::READ | Prot::WRITE,
                "[monitor-osal-event]",
            );
            if control == 0 {
                log::warn!(
                    "monitor: cannot back the event object for the create at {:#x}",
                    block
                );
                return 0xfff6_0000;
            }
            // Mirror the control-block `0x58b0` lays down: state byte +4 = 1, event param u16
            // at +6, max u16 +8 = 1, type id `0xc9` at +0x34, name copied 0x20 bytes at +0x10.
            let mut object = [0u8; OSAL_EVENT_BLOCK as usize];
            object[0x04] = 1;
            object[0x06..0x08].copy_from_slice(&param.to_le_bytes());
            object[0x08..0x0a].copy_from_slice(&1u16.to_le_bytes());
            object[0x34..0x38].copy_from_slice(&0xc9u32.to_le_bytes());
            if let Some(name) = &name {
                for (dest, byte) in object[0x10..0x30].iter_mut().zip(name.as_bytes()) {
                    *dest = *byte;
                }
            }
            if unicorn.mem_write(control as u64, &object).is_err() {
                log::warn!(
                    "monitor: cannot initialise the event object at {:#x}",
                    control
                );
                return 0xfff6_0000;
            }
            if let Some(name) = &name {
                osal_state()
                    .lock()
                    .unwrap()
                    .events
                    .insert(name.clone(), control);
            }
            log::info!(
                "monitor: event create {:?} (param {}) at {:#x} -> handle {:#x}",
                name.as_deref().unwrap_or("?"),
                param,
                block,
                control
            );
            control
        }
    };
    if write_word(unicorn, out, handle).is_err() {
        log::warn!(
            "monitor: cannot write event handle {:#x} into {:#x} (service {:#x})",
            handle,
            out,
            service
        );
        return 0xfff6_0000;
    }
    0
}

/// Serve `OSAL_s32SemaphoreCreate` (cmd 27) the way `0x36734` does, in the caller's own RAM:
/// a named, counting semaphore whose handle (the control-block pointer) lands in the request
/// block's output slot. The name is resolved through the shared registry so a name created by
/// one process resolves to the same handle in another. The real op never blocks, so this hook
/// neither parks the thread nor holds a lock across the map.
fn complete_osal_sem_create(
    unicorn: &mut Unicorn<'_, Context>,
    service: u32,
    block: u32,
) -> u32 {
    let name = read_word(unicorn, block).unwrap_or(0);
    let out = read_word(unicorn, block + 4).unwrap_or(0);
    let initial = read_word(unicorn, block + 8).unwrap_or(0) as u16;
    let max = (read_word(unicorn, block + 0xc).unwrap_or(0) & 0xffff) as u16;
    if out == 0 {
        log::warn!(
            "monitor: semaphore create at {:#x} names a null output slot",
            block
        );
        return 0xfff6_0000;
    }
    let name = read_identifier(unicorn, name);
    // Reuse the shared handle if this process is resolving a name another already created.
    let shared = name
        .as_ref()
        .and_then(|n| osal_state().lock().unwrap().sems.get(n).copied());
    let handle = match shared {
        Some(handle) => {
            log::info!(
                "monitor: semaphore create {:?} at {:#x} reuses shared handle {:#x}",
                name.as_deref().unwrap_or("?"),
                block,
                handle
            );
            handle
        }
        None => {
            let mmu = {
                let data = unicorn.get_data();
                data.mmu.clone()
            };
            let control = mmu.lock().unwrap().heap_alloc(
                unicorn,
                OSAL_SEM_BLOCK,
                Prot::READ | Prot::WRITE,
                "[monitor-osal-sem]",
            );
            if control == 0 {
                log::warn!(
                    "monitor: cannot back the semaphore object for the create at {:#x}",
                    block
                );
                return 0xfff6_0000;
            }
            // Mirror the control-block `0x36734` lays down: type 1 at +4, `+6 = 0xffff`,
            // max at +8, initial at +0xc, state byte +0xa = 1, name copied 0x20 bytes at +0x15.
            let mut object = [0u8; OSAL_SEM_BLOCK as usize];
            object[0x04..0x06].copy_from_slice(&1u16.to_le_bytes());
            object[0x06..0x08].copy_from_slice(&0xffffu16.to_le_bytes());
            object[0x08..0x0a].copy_from_slice(&max.to_le_bytes());
            object[0x0a] = 1;
            object[0x0c..0x0e].copy_from_slice(&initial.to_le_bytes());
            if let Some(name) = &name {
                for (dest, byte) in object[0x15..0x35].iter_mut().zip(name.as_bytes()) {
                    *dest = *byte;
                }
            }
            if unicorn.mem_write(control as u64, &object).is_err() {
                log::warn!(
                    "monitor: cannot initialise the semaphore object at {:#x}",
                    control
                );
                return 0xfff6_0000;
            }
            if let Some(name) = &name {
                osal_state()
                    .lock()
                    .unwrap()
                    .sems
                    .insert(name.clone(), control);
            }
            log::info!(
                "monitor: semaphore create {:?} (init {}, max {}) at {:#x} -> handle {:#x}",
                name.as_deref().unwrap_or("?"),
                initial,
                max,
                block,
                control
            );
            control
        }
    };
    if write_word(unicorn, out, handle).is_err() {
        log::warn!(
            "monitor: cannot write semaphore handle {:#x} into {:#x} (service {:#x})",
            handle,
            out,
            service
        );
        return 0xfff6_0000;
    }
    0
}

/// Request `0x2a`: create an OSAL message queue. Only the shape matters to the guest -
/// every payload is an 8-byte OSAL message reference and the SCD framework re-opens the
/// queue by name - so the emulator backs it with a host-side `MqState` queue of the same
/// name (the very queues the libosal-side `mbx_` bridge posts to) and answers status 0.
fn complete_osal_mbx_create(unicorn: &mut Unicorn<'_, Context>, thread: u32, block: u32) -> u32 {
    let Some(name) = read_identifier(unicorn, block + 0x14) else {
        log::warn!("monitor: OSAL[{thread}] queue create with unreadable name at {:#x}", block + 0x14);
        return 0;
    };
    let (size, count) = (
        read_word(unicorn, block + 0xc).unwrap_or(8),
        read_word(unicorn, block + 0x10).unwrap_or(0),
    );
    let namespace = unicorn.get_data().namespace.clone();
    let mut guard = namespace.lock().unwrap();
    let queue_id = crate::common::osal_queues::OsalQueueService::ensure_queue(&mut guard.mq, &name);
    guard.mq.grow_msgsize(queue_id, size.max(8) as i64 * count.max(1) as i64);
    drop(guard);
    log::info!("monitor: OSAL[{thread}] queue create name {name:?} size {size} count {count}");
    // Dump the creating thread's stack: the mbx_* queues are all created by
    // the same libosal wrapper, so the guest function that requested the
    // queue (its return addresses) is the only way to name the queue owner.
    dump_wait_stack(unicorn, thread, "create");
    0
}

/// Log up to 16 stack words of the current guest thread so return addresses
/// into the process image can be matched against the Ghidra listing.
fn dump_wait_stack(unicorn: &mut Unicorn<'_, Context>, thread: u32, tag: &str) {
    if let Ok(sp) = unicorn.reg_read(RegisterARM::R13) {
        let sp = sp as u32;
        let mut words = Vec::new();
        for i in 0..16u32 {
            if let Ok(w) = read_word(unicorn, sp + i * 4) {
                words.push(format!("{w:#010x}"));
            }
        }
        log::info!("monitor: OSAL[{thread}] {tag} stack sp={sp:#x}: {words:?}");
    }
}

/// Handle token for a named queue (stable per name, allocated on first use by
/// either the create or the open path so both agree on the same value).
pub fn osal_mbx_handle(name: &str) -> u32 {
    let mut state = osal_state().lock().unwrap();
    match state.mbx_handles.get(name).copied() {
        Some(handle) => handle,
        None => {
            let handle = 0x4000_0000 + state.next_mbx_handle;
            state.next_mbx_handle += 1;
            state.mbx_handles.insert(name.to_string(), handle);
            handle
        }
    }
}

/// Request `0x28` leaf `0x664`: the rtf task-queue create with pointer-shaped
/// arguments (see [`OSAL_MBX_CREATE_PTR`]). Allocates the per-name handle token
/// and writes it through `[block + 0x10]`, which is where the STAT component
/// later reads the post target and `STAT_TASK1`'s context the wait target.
fn complete_osal_mbx_create_ptr(unicorn: &mut Unicorn<'_, Context>, thread: u32, block: u32) -> u32 {
    let name_ptr = match read_word(unicorn, block) {
        Ok(value) if (0x1_0000..0xf000_0000).contains(&value) => value,
        _ => 0,
    };
    let Some(name) = read_identifier(unicorn, name_ptr) else {
        log::warn!(
            "monitor: OSAL[{thread}] queue create(ptr) with unreadable name at {name_ptr:#x}"
        );
        return 0;
    };
    let (count, size) = (
        read_word(unicorn, block + 4).unwrap_or(0),
        read_word(unicorn, block + 8).unwrap_or(8),
    );
    let out = read_word(unicorn, block + 0x10).unwrap_or(0);
    let handle = osal_mbx_handle(&name);
    {
        let namespace = unicorn.get_data().namespace.clone();
        let mut guard = namespace.lock().unwrap();
        let queue_id =
            crate::common::osal_queues::OsalQueueService::ensure_queue(&mut guard.mq, &name);
        guard.mq.grow_msgsize(queue_id, size.max(8) as i64 * count.max(1) as i64);
    }
    if write_word(unicorn, out, handle).is_err() {
        log::warn!(
            "monitor: OSAL[{thread}] cannot write queue create(ptr) handle {handle:#x} into {out:#x}"
        );
        return 0;
    }
    log::info!(
        "monitor: OSAL[{thread}] queue create(ptr) name {name:?} size {size} count {count} -> handle {handle:#x}"
    );
    0
}

/// Request `0x29`: open an OSAL message queue by name. The handle is a monitor-side token
/// (stable per name) written through the out-slot pointer at `block + 0x10`; the pump
/// later passes a pointer to that slot back to the wait.
fn complete_osal_mbx_open(unicorn: &mut Unicorn<'_, Context>, thread: u32, block: u32) -> u32 {
    // Leaf 3 (`0x290364`, the rtf task-queue open, used for `STAT_TASK1QUEUE`)
    // passes the name as a direct pointer in `[+0]` and the out-slot in `[+8]`;
    // the SCD leaf (`0x290164`) inlines the name at `[+0x14]` with the out-slot
    // pointer at `[+0x10]`.
    let direct_name = match read_word(unicorn, block) {
        Ok(value) if (0x1_0000..0xf000_0000).contains(&value) => read_identifier(unicorn, value),
        _ => None,
    };
    let (name, out) = match direct_name {
        Some(name) => (name, read_word(unicorn, block + 8).unwrap_or(0)),
        None => {
            let Some(name) = read_identifier(unicorn, block + 0x14) else {
                log::warn!("monitor: OSAL[{thread}] queue open with unreadable name at {:#x}", block + 0x14);
                return 0;
            };
            (name, match read_word(unicorn, block + 0x10) {
                Ok(value) if (0x1000..0xf000_0000).contains(&value) => value,
                _ => 0,
            })
        }
    };
    let handle = osal_mbx_handle(&name);
    // One-shot block dump: the open callers use at least two layouts and the AE
    // pumps starve on slots the write-back above apparently misses.
    {
        static DUMPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if DUMPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 24 {
            let raw: Vec<u32> = (0..8u32)
                .map(|i| read_word(unicorn, block + 4 * i).unwrap_or(0))
                .collect();
            log::info!(
                "monitor: OSAL[{thread}] queue open {name:?} handle {handle:#x} out {out:#x} block {block:#x} raw {:08x?}",
                raw
            );
        }
    }
    if out == 0 || write_word(unicorn, out, handle).is_err() {
        log::warn!("monitor: OSAL[{thread}] queue open {name:?}: cannot write handle to {out:#x}");
        return 0;
    }
    // The PWR proxy answers every app that opens its `mbx_<id>` queue with the
    // START_CONF power message (mode 3) - the CCA INIT every component's
    // `ail_bHandleMsgPowerMessage` runs `bOnInit()` on and promotes the adapter
    // to ACTIVE. Linux processes get this from the libosal hook; the statically
    // linked PROCNAV reaches the same proxy through this monitor path.
    crate::rtos::pwr_proxy::register_app_queue_open(&name);
    log::info!("monitor: OSAL[{thread}] queue open name {name:?} -> handle {handle:#x}");
    0
}

/// Request `0x2d`: the app message pump's receive. Delivery materializes a snapshotted
/// host-queue message into this guest's VM (the same path the libosal `mbx_` bridge uses),
/// so a Linux-side poster's payload arrives pointer-correct. `at_entry` answers only when
/// a message can be handed over right now; from the completion path an empty queue is the
/// expired wait and answers `-1` with the benign last-error `0x72005` the pump tolerates.
fn complete_osal_mbx_wait(
    unicorn: &mut Unicorn<'_, Context>,
    thread: u32,
    block: u32,
    at_entry: bool,
) -> Option<u32> {
    let slot = read_word(unicorn, block).unwrap_or(0);
    // The rtf task-queue waits (`STAT_TASK1`, wrapper `0x3f1b65` -> veneer
    // `0x2ec2b0`) pass the handle by value in `[+0]`; the AE pumps pass a
    // pointer to the open's write-back slot.  Monitor handle tokens live in the
    // 0x4000_0000 range, guest slot pointers never do, so the value disambiguates.
    let handle = if (0x4000_0000..0x5000_0000).contains(&slot) {
        slot
    } else {
        read_word(unicorn, slot).unwrap_or(0)
    };
    let (buf, size) = (read_word(unicorn, block + 4).unwrap_or(0), read_word(unicorn, block + 8).unwrap_or(0));
    // The AE pumps carry the target app id in the request block itself (`w7`,
    // offset 0x1c). Their `w0` slot on the bring-up stack gets re-written by the
    // guest (app ids, zeroes) after the open write-back, so the id is the only
    // stable routing information; the handle table stays the fallback.
    let app_id = read_word(unicorn, block + 0x1c).unwrap_or(0);
    let (name, from_pump) = if (1..=0xff).contains(&app_id) {
        (Some(format!("mbx_{app_id}")), true)
    } else {
        (
            osal_state()
                .lock()
                .unwrap()
                .mbx_handles
                .iter()
                .find(|(_, h)| **h == handle)
                .map(|(n, _)| n.clone()),
            false,
        )
    };
    if buf == 0 || buf > 0xf000_0000 || size < 8 {
        // A wait without a usable receive buffer can never complete; answer it
        // like an empty queue instead of parking forever.
        {
            static GUARDS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if GUARDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 24 {
                log::info!(
                    "monitor: OSAL[{thread}] queue wait rejected: slot {slot:#x} handle {handle:#x} buf {buf:#x} size {size:#x}"
                );
            }
        }
        if !at_entry {
            osal_state().lock().unwrap().last_error.insert(thread, 0x72005);
            return Some(-1i32 as u32);
        }
        return None;
    }
    let Some(name) = name else {
        if !at_entry {
            osal_state().lock().unwrap().last_error.insert(thread, 0x72005);
            return Some(-1i32 as u32);
        }
        return None;
    };
    // One-line map of which pump thread consumes which queue: the parked AE waits
    // are otherwise invisible and the GPS provider has to target the right `mbx_`.
    {
        static SEEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<(u32, String)>>> =
            std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));
        let mut seen = SEEN.lock().unwrap();
        if seen.insert((thread, name.clone())) {
            log::info!("monitor: OSAL[{thread}] pump waits on {name} (handle {handle:#x})");
            // Dump the waiter's stack so the return addresses name the guest
            // function parking here (runtimes map PROCNAV/prochmi VAs 1:1 to
            // their Ghidra images, so these words are directly decompilable).
            if let Ok(sp) = unicorn.reg_read(RegisterARM::R13) {
                let sp = sp as u32;
                let mut words = Vec::new();
                for i in 0..16u32 {
                    if let Ok(w) = read_word(unicorn, sp + i * 4) {
                        words.push(format!("{w:#010x}"));
                    }
                }
                log::info!("monitor: OSAL[{thread}] wait stack sp={sp:#x}: {words:?}");
            }
        }
    }
    let prio_ptr = read_word(unicorn, block + 0xc).unwrap_or(0);
    // Proxy-queued power messages take precedence over bridge traffic, exactly
    // like the libosal-side wait hook (`pwr_proxy_service_handle`). The body is
    // pre-encoded in snapshot format so `materialize_snapshot` handles it.
    //
    // Only the AE pump waits (app-id resolved) may consume a power message. The
    // transient bring-up wait (thread 18/16, handle-resolved) shares the queue
    // name but must not steal the single START_CONF: if it does, the component
    // adapter's `bOnInit` never runs and the adapter stays in INITIALIZED.
    let message = match if from_pump {
        take_pwr_power_message(&name)
    } else {
        None
    } {
        Some(message) => Some(message),
        None => {
            let namespace = unicorn.get_data().namespace.clone();
            let mut guard = namespace.lock().unwrap();
            let queue_id = crate::common::osal_queues::OsalQueueService::ensure_queue(
                &mut guard.mq,
                &name,
            );
            let message = crate::common::osal_queues::OsalQueueService::pop_guest_message(
                &mut guard.mq,
                queue_id,
                usize::MAX,
            );
            if message.is_some() {
                guard.notify_waiters();
            }
            message
        }
    };
    let message = message?;
    match crate::libs::libosal_linux::message::materialize_snapshot(
        unicorn,
        &message.data,
        buf,
        prio_ptr,
        message.priority,
    ) {
        Some(count) => {
            let dump: Vec<String> = message
                .data
                .iter()
                .skip(8)
                .take(0x20)
                .map(|b| format!("{b:02x}"))
                .collect();
            log::info!(
                "monitor: OSAL[{thread}] queue wait handle {handle:#x} ({name}) delivered {count} bytes body: {}",
                dump.join(" ")
            );
            Some(count)
        }
        None => {
            log::warn!(
                "monitor: OSAL[{thread}] queue wait ({name}): cannot materialize message into {buf:#x}"
            );
            if at_entry {
                None
            } else {
                osal_state().lock().unwrap().last_error.insert(thread, 0x72005);
                Some(-1i32 as u32)
            }
        }
    }
}

/// Scheduler poll for a parked queue wait: releases it as soon as a message lands in the
/// host queue (posted by a libosal-side process or an emulator provider).
pub fn poll_osal_mbx_wait(unicorn: &mut Unicorn<'_, Context>, service: u32, block: u32) -> Option<u32> {
    if service & 0xff != OSAL_SERVICE_OBJECT || service >> 16 != OSAL_MBX_WAIT {
        return None;
    }
    let thread = unicorn.get_data().thread_id();
    complete_osal_mbx_wait(unicorn, thread, block, true)
}

/// Consume the PWR proxy's next pending PowerMessage for this queue as a
/// snapshot-format blob (`[u32 flag, u32 len, body]`) ready for
/// `materialize_snapshot`, mirroring the libosal-side injection hook.
fn take_pwr_power_message(name: &str) -> Option<crate::common::osal_queues::OsalMessage> {
    let app_id = crate::rtos::pwr_proxy::parse_mbx_app_id(name)?;
    let message = crate::rtos::pwr_proxy::take_pending_for(app_id)?;
    let mut blob = Vec::with_capacity(8 + message.body.len());
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&(message.body.len() as u32).to_le_bytes());
    blob.extend_from_slice(&message.body);
    log::info!(
        "monitor: PWR proxy delivers type {:#x} to {}-wait",
        message.power_type,
        name
    );
    Some(crate::common::osal_queues::OsalMessage {
        data: blob,
        priority: 0,
    })
}

/// Request `0x1c`: resolve a named semaphore and hand the control-block handle back in the
/// service result. Known names reuse their shared handle; names the unit would have found
/// provisioned by an earlier bring-up process get created here (binary count), so the
/// caller's framework lock always exists in the emulator.
fn complete_osal_sem_open(unicorn: &mut Unicorn<'_, Context>, block: u32) -> u32 {
    let name_ptr = read_word(unicorn, block).unwrap_or(0);
    let out = read_word(unicorn, block + 4).unwrap_or(0);
    let name = read_identifier(unicorn, name_ptr);
    let shared = name
        .as_ref()
        .and_then(|n| osal_state().lock().unwrap().sems.get(n).copied());
    let handle = match shared {
        Some(handle) => handle,
        None => {
            let Some(name) = name else {
                log::warn!("monitor: semaphore open at {block:#x} names nothing");
                return 0;
            };
            let mmu = {
                let data = unicorn.get_data();
                data.mmu.clone()
            };
            let control = mmu.lock().unwrap().heap_alloc(
                unicorn,
                OSAL_SEM_BLOCK,
                Prot::READ | Prot::WRITE,
                "[monitor-osal-sem]",
            );
            if control == 0 {
                log::warn!(
                    "monitor: cannot back the semaphore object for opening {name:?} at {block:#x}"
                );
                return 0;
            }
            let mut object = [0u8; OSAL_SEM_BLOCK as usize];
            object[0x04..0x06].copy_from_slice(&1u16.to_le_bytes());
            object[0x06..0x08].copy_from_slice(&0xffffu16.to_le_bytes());
            object[0x08..0x0a].copy_from_slice(&1u16.to_le_bytes());
            object[0x0a] = 1;
            object[0x0c..0x0e].copy_from_slice(&1u16.to_le_bytes());
            for (dest, byte) in object[0x15..0x35].iter_mut().zip(name.as_bytes()) {
                *dest = *byte;
            }
            if unicorn.mem_write(control as u64, &object).is_err() {
                log::warn!(
                    "monitor: cannot initialise the semaphore object at {control:#x}"
                );
                return 0;
            }
            osal_state().lock().unwrap().sems.insert(name.clone(), control);
            log::info!("monitor: semaphore open {name:?} created handle {control:#x}");
            control
        }
    };
    if out >= 0x1_0000 {
        let _ = write_word(unicorn, out, handle);
    }
    log::info!("monitor: semaphore open handle {handle:#x} for block {block:#x}");
    handle
}

/// Serve an OSAL IO request on object `0x6e` the way the monitor's device layer would,
/// backed by the emulator's shared registry (`SystemNamespace::registry`). Like the 0x64
/// unit this reproduces the observable contract of the client side that was decoded from
/// PROCNAV (wrappers at `0x32b66c..0x32b77c`, validation in `OSAL_s32IOControl` `0x32cfb4`):
/// open/create hand back a guest handle object `{ +0: magic, +0x1c: type, +0x20: path }`,
/// iocontrol dispatches on the request word, and everything else is acknowledged. The path
/// travels *inside* the handle object, so handles from different processes can never be
/// confused and no host-side table has to be kept in step with guest memory.
///
/// Returns `None` only for services on this object that are not modelled (the async pair),
/// which keeps the catch-all log for them.
fn complete_osal_io(unicorn: &mut Unicorn<'_, Context>, service: u32, block: u32) -> Option<u32> {
    match service {
        SERVICE_IO_OPEN | SERVICE_IO_CREATE => {
            let path_address = read_word(unicorn, block).ok()?;
            let path = match read_c_string(unicorn, path_address) {
                Some(path) => path,
                None => {
                    // The caller handed over an unusable path pointer (uninitialised key
                    // object). The device could not have opened such a name either, so the
                    // honest status is the client's "open failed" value.
                    log::info!(
                        "monitor: io open at {:#x} names unreadable path {:#x}, failing",
                        block,
                        path_address
                    );
                    return Some(0xffff_ffff);
                }
            };
            let flags = read_word(unicorn, block + 4).unwrap_or(0) & 0xffff;
            Some(complete_io_open_create(unicorn, service, block, path_address, &path, flags))
        }
        SERVICE_IO_CLOSE => {
            let handle = read_word(unicorn, block).ok()?;
            log::info!(
                "monitor: io close of handle {:#x} at {:#x} acknowledged",
                handle,
                block
            );
            // Return the handle page to the guest's block pool so a later open
            // reuses it instead of mapping a fresh QEMU section. Only addresses
            // the pool handed out dynamically may be released: an unknown
            // address (double close) must never reach the static-slot path.
            {
                let state = {
                    let data = unicorn.get_data();
                    data.sys_calls_state.clone()
                };
                let mut guard = state.lock().unwrap();
                if guard.osal_messages.is_dynamic(handle) {
                    guard.osal_messages.release(handle);
                }
            }
            Some(0)
        }
        SERVICE_IO_REMOVE => {
            let path_address = read_word(unicorn, block).ok()?;
            log::info!(
                "monitor: io remove of {:?} at {:#x} acknowledged",
                read_c_string(unicorn, path_address),
                block
            );
            Some(0)
        }
        SERVICE_IO_CONTROL => {
            let handle = read_word(unicorn, block).ok()?;
            let request = read_word(unicorn, block + 4).ok()?;
            let argument = read_word(unicorn, block + 8).ok()?;
            Some(complete_io_control(unicorn, handle, request, argument))
        }
        // Nothing on this unit reads or writes registry keys through the byte API (the
        // client speaks iocontrol). `/dev/trace` records arrive here: `OSAL_TraceOutput`
        // (`0x2c4780`) writes a 0x10-byte header `{handle, severity, tag, string ptr}`,
        // so surface the message text - it is the applications' own diagnostics and the
        // best window into why bring-up decides to roll back.
        SERVICE_IO_WRITE => {
            let handle = read_word(unicorn, block).ok()?;
            let buffer = read_word(unicorn, block + 4).ok()?;
            let length = read_word(unicorn, block + 8).ok()?;
            let text = if length >= 0x10 {
                read_word(unicorn, buffer + 0xc)
                    .ok()
                    .and_then(|ptr| read_c_string(unicorn, ptr))
            } else {
                None
            };
            match text {
                Some(text) => {
                    let severity = read_word(unicorn, buffer + 4).unwrap_or(0);
                    log::info!("monitor: trace[{handle:#x}] sev {severity:#x}: {text}");
                }
                None => {
                    log::debug!(
                        "monitor: io write of {} bytes to handle {:#x} consumed",
                        length,
                        handle
                    );
                }
            }
            Some(length)
        }
        SERVICE_IO_READ => {
            let handle = read_word(unicorn, block).ok()?;
            log::debug!("monitor: io read from handle {:#x} reports end of file", handle);
            Some(0)
        }
        _ => None,
    }
}

/// `0x1026e`/`0x3026e`: hand back a handle object for the path. Registry keys must exist in
/// the emulator's registry for open (create inserts them first), mirroring the device's
/// "no such key" failure; the client treats `-1` as exactly that (`reg_tclRegKey::bOpen`
/// only branches on it, `scd_init` falls back to its static map). Every other device path
/// seen so far (`/dev/trace`) is modelled loosely: it gets a handle too, because the old
/// catch-all already reported those opens as successful and its ioctls are acknowledged.
fn complete_io_open_create(
    unicorn: &mut Unicorn<'_, Context>,
    service: u32,
    block: u32,
    path_address: u32,
    path: &str,
    flags: u32,
) -> u32 {
    let is_registry = path
        .to_ascii_lowercase()
        .starts_with("/dev/registry/");
    let namespace = {
        let data = unicorn.get_data();
        data.namespace.clone()
    };
    if is_registry {
        let exists = namespace.lock().unwrap().registry.has_path(path);
        if service == SERVICE_IO_CREATE {
            let created = namespace
                .lock()
                .unwrap()
                .registry
                .create_key(path);
            log::info!("monitor: io create {:?} -> {:?}", path, created);
        } else if !exists {
            let mut raw = [0u8; 40];
            let raw = match unicorn.mem_read(u64::from(path_address), &mut raw) {
                Ok(()) => format!("{:02x?}", &raw[..]),
                Err(_) => "unreadable".to_string(),
            };
            log::info!(
                "monitor: io open of {:?} (flags {:#x}) at {:#x} (path ptr {:#x}) fails: no such key, raw {}",
                path,
                flags,
                block,
                path_address,
                raw
            );
            return 0xffff_ffff;
        }
    } else if service == SERVICE_IO_CREATE || path.to_ascii_lowercase() != "/dev/trace" {
        log::warn!(
            "monitor: io {:?} of {:?} is not a modelled device, handing back a plain handle",
            if service == SERVICE_IO_CREATE {
                "create"
            } else {
                "open"
            },
            path
        );
    }
    match alloc_io_handle(unicorn, path) {
        Some(handle) => {
            log::info!(
                "monitor: io open of {:?} (flags {:#x}) completed with handle {:#x}",
                path,
                flags,
                handle
            );
            handle
        }
        None => {
            log::warn!("monitor: cannot allocate the io handle object for {:?}", path);
            0xffff_ffff
        }
    }
}

/// Build the guest-side handle object: one zero-filled page with the magic the user library
/// demands at `+0`, a device type at `+0x1c` that keeps iocontrol on the generic path, and
/// the device path at `+0x20` (iocontrol reads it back to route the request).
fn alloc_io_handle(unicorn: &mut Unicorn<'_, Context>, path: &str) -> Option<u32> {
    // The handle is a full page, and a busy bring-up opens hundreds of keys in
    // open/ioctl/close cycles. QEMU memory sections never collapse, so every fresh
    // mapping counts towards the hard 4096-section abort - recycle the guest's
    // closed handle pages through the same pool the message snapshots use.
    let state = {
        let data = unicorn.get_data();
        data.sys_calls_state.clone()
    };
    let reused = state.lock().unwrap().osal_messages.take_freed(0x1000);
    let object = match reused {
        Some(object) => object,
        None => {
            // No guard may be live here: `heap_alloc` re-enters the very lock.
            let mmu = {
                let data = unicorn.get_data();
                data.mmu.clone()
            };
            let object = mmu.lock().unwrap().heap_alloc(
                unicorn,
                0x1000,
                Prot::READ | Prot::WRITE,
                "[monitor-osal-io]",
            );
            if object != 0 {
                state
                    .lock()
                    .unwrap()
                    .osal_messages
                    .mark_dynamic(object, 0x1000);
            }
            object
        }
    };
    if object == 0 {
        return None;
    }
    if write_word(unicorn, object, OSAL_IO_MAGIC).is_err()
        || write_word(unicorn, object + 0x1c, OSAL_IO_REGISTRY_TYPE).is_err()
    {
        return None;
    }
    let mut bytes = path.as_bytes().to_vec();
    bytes.truncate(0x100);
    bytes.push(0);
    if unicorn
        .mem_write(u64::from(object) + 0x20, &bytes)
        .is_err()
    {
        return None;
    }
    Some(object)
}

/// `0x5036e`: route the request by the device recorded in the handle object. Registry
/// requests run against the shared registry; anything else is acknowledged, matching how
/// the old catch-all answered these before real handles existed (and the SCD probes must
/// not error, or `scd_bTestLookupIoControl` drops PROCNAV to its static lookup mode).
fn complete_io_control(
    unicorn: &mut Unicorn<'_, Context>,
    handle: u32,
    request: u32,
    argument: u32,
) -> u32 {
    if handle < 0x1_0000
        || read_word(unicorn, handle).unwrap_or(0) != OSAL_IO_MAGIC
    {
        log::warn!(
            "monitor: io control {:#x} on handle {:#x} without the OSAL magic fails",
            request,
            handle
        );
        return 0xffff_ffff;
    }
    let path = read_c_string(unicorn, handle + 0x20).unwrap_or_default();
    if !path
        .to_ascii_lowercase()
        .starts_with("/dev/registry/")
    {
        // `OSAL` trace gate (PROCNAV `0x566be`): `ioctl(trace_handle, 3, {out enabled u8,
        // tag u16, level u8})` - the caller emits nothing unless the server writes `1`
        // back at `argument + 0`, so every trace print (including the SCD bring-up
        // diagnostics) silently drops when this only acknowledges.
        if request == IO_REQUEST_SET
            && path.to_ascii_lowercase().contains("trace")
            && write_word(unicorn, argument, 1).is_ok()
        {
            return 0;
        }
        log::info!(
            "monitor: io control {:#x} on device {:?} acknowledged",
            request,
            path
        );
        return 0;
    }
    match request {
        IO_REQUEST_QUERY => complete_registry_query(unicorn, &path, argument),
        IO_REQUEST_SET => complete_registry_set(unicorn, &path, argument),
        IO_REQUEST_ENUM_SUBKEYS => complete_registry_enum(unicorn, &path, handle, argument),
        // Both lookup requests are refused: answering them puts PROCNAV in SCD mode 1,
        // where every answer must come from an IOSC pool queue owned by the (unemulated)
        // SPM server. Refused probes keep mode 0 - the static map `scd_bGetAppIdFromServiceId`
        // (`0x34f012`) reads straight from the registry tree, which the emulated registry
        // serves natively.
        IO_REQUEST_LOOKUP_PROBE | IO_REQUEST_SERVICE_TO_APP => {
            // A `-1` with the bare `0x72011` last-error is what the SCD probe classifies as
            // "no SCD server here"; record it so the following get-last-error (`0x20064`)
            // answers accordingly.
            let thread = unicorn.get_data().thread_id();
            osal_state()
                .lock()
                .unwrap()
                .last_error
                .insert(thread, OSAL_ERR_NO_FUNCTION);
            log::info!(
                "monitor: registry lookup ({:#x}) on {path:?} refused: static mode",
                request
            );
            0xffff_ffff
        }
        other => {
            log::warn!(
                "monitor: registry io control {:#x} on {:?} at {:#x} acknowledged without a result: {}",
                other,
                path,
                argument,
                describe(unicorn, argument)
            );
            0
        }
    }
}

/// Request `1`: read the value `buf[0]` (name) of key `path`, up to `buf[1]` bytes, into
/// Request `0x3eb`: enumerate the child keys of an open registry handle. The ioctl
/// argument is the caller's request object itself (`+0` its io handle, `+4` a zero flag,
/// `+8..` a 0x100-byte name buffer) and the client hands out `object + 8` as the name
/// pointer (`0x2c475a`), so the answer is written into that inline buffer and the walk
/// position is read back from it too: the call returns the subkey following the name
/// currently stored there, `-1` when the name is the last child. Empty or stale buffer
/// content restarts the list, which matches a freshly allocated iterator, and the caller
/// may move to a new request object (and io handle) at any point - the walk lives entirely
/// in the name it passes over. Status `0` keeps the loop going, `-1` ends it
/// (`adds r0, #1; bne` at `0x2c4752`).
fn complete_registry_enum(
    unicorn: &mut Unicorn<'_, Context>,
    path: &str,
    _handle: u32,
    argument: u32,
) -> u32 {
    let names = {
        let namespace = {
            let data = unicorn.get_data();
            data.namespace.clone()
        };
        let namespace = namespace.lock().unwrap();
        namespace.registry.subkeys(path)
    };
    let current = read_c_string(unicorn, argument + 8).unwrap_or_default();
    let index = match names.iter().position(|name| *name == current) {
        Some(seen) => seen + 1,
        None => 0,
    };
    let Some(name) = names.get(index).cloned() else {
        log::info!(
            "monitor: registry enum {path:?} after {current:?} exhausted ({} subkeys)",
            names.len()
        );
        return 0xffff_ffff;
    };
    let mut bytes = name.as_bytes().to_vec();
    bytes.push(0);
    if unicorn
        .mem_write(u64::from(argument + 8), &bytes)
        .is_err()
    {
        return 0xffff_ffff;
    }
    log::info!("monitor: registry enum {path:?} after {current:?} -> {name:?}");
    0
}

/// the buffer at `*[buf + 0xc]`, and write the match indicator at `buf + 8`. The two
/// clients disagree on what that indicator means - `bQueryU32` requires `1`,
/// `bQueryString` requires `0` - so it is answered to satisfy each: one value for a DWORD,
/// no overflow indicator for a string. A missing value answers "no match" with status 0,
/// which is how the real server lets the client take its default branch without failing
/// the io call itself.
fn complete_registry_query(unicorn: &mut Unicorn<'_, Context>, path: &str, buffer: u32) -> u32 {
    let name_address = read_word(unicorn, buffer).unwrap_or(0);
    let size = read_word(unicorn, buffer + 4).unwrap_or(0);
    let data = read_word(unicorn, buffer + 0xc).unwrap_or(0);
    let name = read_c_string(unicorn, name_address).unwrap_or_default();
    let value = {
        let namespace = {
            let data = unicorn.get_data();
            data.namespace.clone()
        };
        let found = namespace.lock().unwrap().registry.query_value(path, &name);
        found
    };
    let Some(value) = value else {
        log::info!(
            "monitor: registry query {:?}\\{:?} (size {}) misses, count 0",
            path,
            name,
            size
        );
        let _ = write_word(unicorn, buffer + 8, 0);
        return 0;
    };
    match value {
        crate::common::registry::RegistryValue::U32(number) => {
            if data != 0 && write_word(unicorn, data, number).is_err() {
                return 0xffff_ffff;
            }
            let _ = write_word(unicorn, buffer + 8, 1);
            log::info!(
                "monitor: registry query {:?}\\{:?} = {:#x}",
                path,
                name,
                number
            );
        }
        crate::common::registry::RegistryValue::String(text) => {
            if data != 0 {
                let capacity = size.max(1).min(0x1000) as usize;
                let mut bytes = text.as_bytes().to_vec();
                bytes.truncate(capacity - 1);
                bytes.push(0);
                if unicorn.mem_write(u64::from(data), &bytes).is_err() {
                    return 0xffff_ffff;
                }
            }
            let _ = write_word(unicorn, buffer + 8, 0);
            log::info!("monitor: registry query {:?}\\{:?} = {:?}", path, name, text);
        }
    }
    0
}

/// Request `3` (`bSetU32`/`bSetString`): store `*[buf + 0xc]` (a DWORD when `buf[1] == 4`,
/// else a NUL-terminated string) as value `buf[0]` of key `path`. PROCNAV marks processes
/// and configuration through this, so the write lands in the shared registry like every
/// other mutation.
fn complete_registry_set(unicorn: &mut Unicorn<'_, Context>, path: &str, buffer: u32) -> u32 {
    let name_address = read_word(unicorn, buffer).unwrap_or(0);
    let size = read_word(unicorn, buffer + 4).unwrap_or(0);
    let data = read_word(unicorn, buffer + 0xc).unwrap_or(0);
    let name = read_c_string(unicorn, name_address).unwrap_or_default();
    if data == 0 || name.is_empty() {
        return 0xffff_ffff;
    }
    let value = if size == 4 {
        crate::common::registry::RegistryValue::U32(read_word(unicorn, data).unwrap_or(0))
    } else {
        crate::common::registry::RegistryValue::String(
            read_c_string(unicorn, data).unwrap_or_default(),
        )
    };
    let display = value.display();
    {
        let namespace = {
            let data = unicorn.get_data();
            data.namespace.clone()
        };
        namespace
            .lock()
            .unwrap()
            .registry
            .set_value(path, &name, value);
    }
    log::info!("monitor: registry set {:?}\\{:?} = {}", path, name, display);
    0
}


/// Read a NUL-terminated printable string from guest memory (device paths and value names
/// start with `/` or are arbitrary ASCII, so the identifier filter in [`describe`] is too
/// strict here). Returns `None` only when the memory cannot be read at all.
pub(crate) fn read_c_string(unicorn: &mut Unicorn<'_, Context>, address: u32) -> Option<String> {
    if address < 0x1_0000 {
        return None;
    }
    let mut bytes = vec![0u8; 260];
    unicorn
        .mem_read(u64::from(address), &mut bytes)
        .ok()?;
    let end = bytes.iter().position(|byte| *byte == 0)?;
    if end >= 256 {
        return None;
    }
    let run = &bytes[..end];
    if !run
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
    {
        return None;
    }
    String::from_utf8(run.to_vec()).ok()
}

/// Enter the monitor's own SWI dispatcher for `svc #id`, the way the ARM vector would.
///
/// Unicorn reports the trap with PC already past the instruction and without touching the
/// saved program status, so the return address is the reported PC and the saved status is
/// the current one without the interrupt mask. The dispatcher expects the five words the
/// vector trampoline left below the caller's stack: `R3`, the saved status, `R3` again,
/// `R12` and the link register, and it finishes with a multiple load that ends in the
/// program counter, which lands on that link register.
///
/// Returns whether the monitor took the call over.
pub fn enter(unicorn: &mut Unicorn<'_, Context>, id: u32, return_address: u32) -> bool {
    if !(SERVICE_ID_MIN..=SERVICE_ID_MAX).contains(&id) || !attach(unicorn) {
        return false;
    }

    let args1 = unicorn.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
    let Ok(status) = unicorn.reg_read(RegisterARM::CPSR) else {
        return false;
    };
    let status = (status as u32) & !0x80;
    let r3 = unicorn.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
    let r12 = unicorn.reg_read(RegisterARM::R12).unwrap_or(0) as u32;
    let Ok(sp) = unicorn.reg_read(RegisterARM::SP) else {
        return false;
    };
    let frame = (sp as u32).wrapping_sub(0x14) & !7;

    for (offset, word) in [
        (0u32, r3),
        (4, status),
        (8, r3),
        (0xc, r12),
        (0x10, return_address),
    ] {
        if write_word(unicorn, frame + offset, word).is_err() {
            log::warn!("monitor: cannot build the trap frame at {:#x}", frame + offset);
            return false;
        }
    }

    for (register, value) in [
        (RegisterARM::R14 as i32, return_address as u64),
        (RegisterARM::R13 as i32, frame as u64),
        (RegisterARM::PC as i32, SWI_HANDLER as u64),
    ] {
        if unicorn.reg_write(register, value).is_err() {
            return false;
        }
    }

    let index = args1.wrapping_shr(16);
    let worker = read_word(unicorn, WORKER_TABLE + 4 * index).unwrap_or(0);
    log::info!(
        "monitor: id {} r1={:#x} index={:#x} worker={:#x} handler={:#x}",
        id,
        args1,
        index,
        worker,
        read_word(unicorn, SERVICE_TABLE + 4 * id).unwrap_or(0)
    );
    log::trace!(
        "monitor: id {} entering the dispatcher at {:#x} with the frame at {:#x}",
        id,
        SWI_HANDLER,
        frame
    );
    true
}

/// Start the monitor's system tasks as ordinary guest threads, the way `svc #4` would:
/// the task is entered on the service stack with the scratch block in R0 and the task
/// number tagged into R1. Task 1, the message pump, is the producer behind the deferred
/// work the Linux side waits for with `svc #6`, so it has to run on the same core as the
/// process it serves.
fn spawn_system_tasks(unicorn: &mut Unicorn<'_, Context>) {
    let parent_context = match unicorn.context_init() {
        Ok(context) => context,
        Err(_) => return,
    };
    for task in [MESSAGE_PUMP_TASK] {
        let registers = [
            (RegisterARM::SP as i32, SERVICE_STACK as u64),
            (RegisterARM::R0 as i32, SYSTEM_TASK_SCRATCH as u64),
            (RegisterARM::R1 as i32, (0x8000_0000u64 | (task as u64) << 16) | 0),
            (RegisterARM::R14 as i32, MQ_NOTIFY_EXIT_STUB as u64),
            (RegisterARM::PC as i32, MESSAGE_PUMP as u64),
        ];
        let mut usable = true;
        for (register, value) in registers {
            if unicorn.reg_write(register, value).is_err() {
                usable = false;
            }
        }
        if !usable {
            return;
        }
        let Ok(task_context) = unicorn.context_init() else {
            break;
        };
        let tid = unicorn
            .get_data()
            .next_thread_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        unicorn.get_data().threads.lock().unwrap().push(GuestThread {
            id: tid,
            rtos_task: true,
            status: ThreadStatus::Runnable,
            cpu_context: Some(task_context),
            pc: MESSAGE_PUMP,
            pending_result: None,
            clear_child_tid: None,
        });
        log::info!(
            "monitor: system task {} running as thread [{}] at {:#x} on stack {:#x}",
            task,
            tid,
            MESSAGE_PUMP,
            SERVICE_STACK
        );
    }
    let _ = unicorn.context_restore(&parent_context);
}

/// `b` from `from` to `to`, both 4-byte aligned and within 32 MB of each other.
fn branch_to(to: u32, from: u32) -> u32 {
    let words = ((to as i64 - (from as i64 + 8)) >> 2) as i32;
    0xea00_0000u32 | (words as u32 & 0x00ff_ffff)
}

fn map_pages(
    mmu: &mut Mmu,
    unicorn: &mut Unicorn<'_, Context>,
    address: u32,
    size: u32,
    perms: Prot,
) {
    mmu.map(unicorn, address, size, perms, "[monitor]", "triton_mid");
}

fn read_word(unicorn: &mut Unicorn<'_, Context>, address: u32) -> Result<u32, ()> {
    let mut bytes = [0u8; 4];
    unicorn.mem_read(address as u64, &mut bytes).map_err(|_| ())?;
    Ok(u32::from_le_bytes(bytes))
}

fn write_word(unicorn: &mut Unicorn<'_, Context>, address: u32, value: u32) -> Result<(), ()> {
    unicorn
        .mem_write(address as u64, &value.to_le_bytes())
        .map_err(|_| ())
}