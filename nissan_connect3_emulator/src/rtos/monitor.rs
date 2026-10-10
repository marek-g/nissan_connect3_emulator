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
use crate::emulator::thread::{GuestThread, ThreadStatus};
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