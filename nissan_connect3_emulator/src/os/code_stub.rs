use crate::emulator::context::Context;
use std::cell::Cell;
use unicorn_engine::Unicorn;

/// Host-side replacement for a stubbed guest function. Returns the value to place
/// in R0; the dispatcher then jumps back to the caller (PC = LR).
pub type CodeStubHandler = fn(&mut Unicorn<'_, Context>) -> u32;

thread_local! {
    static CURRENT_STUB_NAME: Cell<&'static str> = const { Cell::new("") };
}

pub fn current_stub_name() -> &'static str {
    CURRENT_STUB_NAME.with(|name| name.get())
}

pub fn set_current_stub_name(name: &'static str) {
    CURRENT_STUB_NAME.with(|cell| cell.set(name));
}

#[derive(Clone, Copy)]
pub struct CodeStub {
    pub lib: &'static str,
    pub name: &'static str,
    pub handler: CodeStubHandler,
    /// Observer sites (`add_code_observer`) resume execution at this address
    /// (a code cave replaying the replaced instruction) instead of the
    /// stub-return semantics (R0 = result, PC = LR).
    pub resume: Option<u32>,
}

/// `svc #0` (ARM, little-endian). Patched over a stubbed function's first
/// instruction so that calling it traps into the single intr hook instead of
/// running the original body.
pub const SVC0: [u8; 4] = [0x00, 0x00, 0x00, 0xef];

/// Replace the function at `address` with a `svc #0` trap and record how to
/// emulate it. Unlike `add_code_hook!` this registers no per-instruction Unicorn
/// code hook: the (already-present) intr hook dispatches on PC when the svc fires.
pub fn add_code_stub(
    unicorn: &mut Unicorn<'_, Context>,
    lib: &'static str,
    address: u32,
    name: &'static str,
    handler: CodeStubHandler,
) {
    unicorn.mem_write(address as u64, &SVC0).unwrap();
    unicorn
        .get_data()
        .code_stubs
        .lock()
        .unwrap()
        .insert(address, CodeStub { lib, name, handler, resume: None });
}

/// Conservative relocatability test for the instruction that `add_code_observer`
/// wants to move into a cave: accept only unconditional ARM instructions that
/// cannot observe the PC - no branches (immediate or `BX`/`BLX` register), no
/// `MRS`/`MSR`, no coprocessor/SWI, and no register list naming R15. Data-processing,
/// `PUSH`/`STMDB sp!`, `ADD`/`SUB`/`MOV` against SP etc. all pass.
fn arm_instruction_relocatable(word: u32) -> bool {
    let cond = word >> 28;
    if cond != 0b1110 {
        return false; // only unconditional instructions
    }
    let rd = (word >> 16) & 0xf;
    let rn = (word >> 12) & 0xf;
    let rm = word & 0xf;
    if rd == 15 || rn == 15 || rm == 15 {
        return false; // PC appears as operand or base
    }
    if (word >> 25) & 0b101 == 0b101 && (word >> 25) & 0b100 == 0 {
        // B/BL immediate class: cond 0101 ll - reject any 25:24 == 01 with
        // the L bit forms; branches specifically are (word>>26)==0b0101.
    }
    if (word >> 26) == 0b0101 {
        return false; // B/BL
    }
    if word & 0x0ffffff0 == 0x012fff10 || word & 0x0fffffff == 0x012fff30 {
        return false; // BX / BLX register
    }
    if word & 0x0fb0_0fff == 0x0100_0000 || word & 0x0fb0_0fff == 0x0120_0000 {
        return false; // MRS / MSR
    }
    if (word >> 24) & 0xe == 0xc {
        return false; // CDP / LDC / STC / MRC coprocessor space
    }
    if (word >> 4) & 0xffff == 0b0001_0010_1111_1111_0001 {
        return false; // BLX immediate (ARMv5)
    }
    true
}

/// Install an observer at a mid-function ARM instruction address without
/// paying Unicorn's per-instruction page-hook cost: the instruction word is
/// copied into a 16-byte RWX cave (`cave+0` instruction, `+4 ldr pc,[pc,#0]`,
/// `+0xc` resume target = `address + 4`) and the original site is patched with
/// `svc #0`. When the trap fires the intr-hook dispatcher runs `handler`
/// (observer logic: reads, guest mem_writes) and resumes at the cave, so the
/// replaced instruction executes exactly once and control returns to
/// `address + 4`. Returns false (nothing patched) when the instruction cannot
/// be relocated safely or the cave write fails - the caller must then keep a
/// page-range hook as fallback. Must run from a context that does NOT hold
/// the MMU mutex: the cave allocation takes it (`heap_alloc`).
pub fn add_code_observer(
    unicorn: &mut Unicorn<'_, Context>,
    lib: &'static str,
    address: u32,
    name: &'static str,
    cave: u32,
    handler: CodeStubHandler,
) -> bool {
    let mut word = [0u8; 4];
    if unicorn.mem_read(address as u64, &mut word).is_err() {
        log::warn!("observer {name} at {address:#x}: site not readable");
        return false;
    }
    let original = u32::from_le_bytes(word);
    if !arm_instruction_relocatable(original) {
        log::warn!(
            "observer {name} at {address:#x}: instruction {original:#010x} is not relocatable, keeping page hook"
        );
        return false;
    }
    let resume = address.wrapping_add(4);
    let cave_words = [original, 0xe59ff000u32, 0, resume]; // instr; ldr pc,[pc,#0]; pad; target
    let mut bytes = [0u8; 16];
    for (i, w) in cave_words.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    if unicorn.mem_write(cave as u64, &bytes).is_err() {
        log::warn!("observer {name}: cave write at {cave:#x} failed");
        return false;
    }
    unicorn.mem_write(address as u64, &SVC0).unwrap();
    unicorn
        .get_data()
        .code_stubs
        .lock()
        .unwrap()
        .insert(address, CodeStub { lib, name, handler, resume: Some(cave) });
    log::info!("observer {name} installed at {address:#x} (cave {cave:#x}, orig {original:#010x})");
    true
}
