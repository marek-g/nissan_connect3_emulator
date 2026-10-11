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
pub const OSAL_CTX_GET: u32 = 0x02;
pub const OSAL_CTX_SET: u32 = 0x03;
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
/// cmd 72 (shim `0xbd80` -> `0x5e88`): a stub leaf that unconditionally returns `3`, so the
/// truthful answer is `3` rather than the catch-all's `0`.
pub const OSAL_UNSUPPORTED3: u32 = 0x48;
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
    let deadline = if ticks == 0 {
        None
    } else {
        Some(std::time::Instant::now() + TICK * ticks)
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
    /// Thread id -> a dense positive OSAL task id, handed out by `0x16`.
    task_ids: HashMap<u32, u32>,
    next_task_id: u32,
}

/// Process-global OSAL service state, shared by every process's monitor instance (mirrors the
/// `pwr_proxy::global()` `OnceLock<Mutex<..>>` pattern).
fn osal_state() -> &'static Mutex<OsalState> {
    static STATE: OnceLock<Mutex<OsalState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(OsalState::default()))
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
        OSAL_EVENT_CREATE => Some(complete_osal_event_create(unicorn, service, block)),
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
        // `0x02` (`0x288c8`): return this task's stored context, or null when unset.
        OSAL_CTX_GET => {
            let value = osal_state()
                .lock()
                .unwrap()
                .context
                .get(&thread)
                .copied()
                .unwrap_or(OSAL_CONTEXT_UNSET);
            log::info!(
                "monitor: OSAL[{thread}] context get -> {value:#x} at {block:#x}"
            );
            Some(value)
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
        _ => None,
    }
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
        // client speaks iocontrol), and `/dev/trace` records go through write. Accept the
        // write as consumed, report the read as end-of-file.
        SERVICE_IO_WRITE => {
            let handle = read_word(unicorn, block).ok()?;
            let length = read_word(unicorn, block + 8).ok()?;
            log::debug!(
                "monitor: io write of {} bytes to handle {:#x} consumed",
                length,
                handle
            );
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
        IO_REQUEST_LOOKUP_PROBE => {
            log::info!("monitor: registry lookup probe on {:?} answered", path);
            0
        }
        IO_REQUEST_SERVICE_TO_APP => complete_service_to_app(unicorn, argument),
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

/// Request `0xb`: the SCD service -> app lookup PROCNAV uses to find the peer of a commcon
/// service (`scd_bGetAppIdFromServiceId` `0x34f012` at `0x34f084`). Contract pinned from
/// the caller: the service id is a `u32` at `argument + 0x100` and the app id is read back
/// from the same word. On the unit the server resolves it from the registry tree - every
/// app publishes `SERVICEID` under `...\PROCESS\<p>\<app>\SERVICES\<name>` and its own
/// `APPID` one level above - and that is exactly what runs here. A miss fails the call so
/// the caller logs it instead of talking to app 0.
fn complete_service_to_app(unicorn: &mut Unicorn<'_, Context>, argument: u32) -> u32 {
    let slot = argument + 0x100;
    let service_id = read_word(unicorn, slot).unwrap_or(0);
    let namespace = {
        let data = unicorn.get_data();
        data.namespace.clone()
    };
    let namespace = namespace.lock().unwrap();
    let Some(key) = namespace
        .registry
        .find_key_by_u32_value("SERVICEID", service_id)
    else {
        log::warn!(
            "monitor: no registry app publishes SERVICEID {:#x}, lookup fails",
            service_id
        );
        return 0xffff_ffff;
    };
    // `...\PROCESS\<p>\<app>\SERVICES\<svc>` -> `...\PROCESS\<p>\<app>`.
    let Some(app_key) = key.rsplit_once('/').and_then(|(parent, _)| parent.rsplit_once('/').map(|(app, _)| app))
    else {
        log::warn!(
            "monitor: SERVICEID {:#x} found at unexpected path {:?}",
            service_id,
            key
        );
        return 0xffff_ffff;
    };
    let Some(app_id) = namespace.registry.query_u32(app_key, "APPID") else {
        log::warn!(
            "monitor: service {:?} has no APPID for service {:#x}",
            app_key,
            service_id
        );
        return 0xffff_ffff;
    };
    drop(namespace);
    if write_word(unicorn, slot, app_id).is_err() {
        return 0xffff_ffff;
    }
    log::info!(
        "monitor: service {:#x} resolved to app {} via {:?}",
        service_id,
        app_id,
        app_key
    );
    0
}

/// Read a NUL-terminated printable string from guest memory (device paths and value names
/// start with `/` or are arbitrary ASCII, so the identifier filter in [`describe`] is too
/// strict here). Returns `None` only when the memory cannot be read at all.
fn read_c_string(unicorn: &mut Unicorn<'_, Context>, address: u32) -> Option<String> {
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