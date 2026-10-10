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

/// Hand a yielded request back, the way the thread that owns the monitor object would
/// when it completes the request: whatever the caller expects lands in its request
/// block, and R0 carries the status the continuation branches on (`cmp r0, #0` with the
/// out-parameter read for `r0 >= 0`).
pub fn complete(unicorn: &mut Unicorn<'_, Context>, service: u32, block: u32) -> u32 {
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