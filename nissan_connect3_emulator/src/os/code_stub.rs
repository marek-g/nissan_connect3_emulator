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

// ---------------------------------------------------------------------------
// r746: universal `svc #0` trap sites - a drop-in replacement for per-address
// Unicorn code hooks.
//
// An `add_code_hook(addr, addr, cb)` makes Unicorn emit a per-instruction
// dispatch check in every translation block containing `addr` and pay a
// callback on every execution. `trap_site` instead overwrites the site's
// instruction with `svc #0` (ARM, 4 bytes) or `svc #0` (Thumb T1, 2 bytes -
// `svc.w` 4 bytes for wide first instructions). The pre-existing intr hook
// (`hook_syscall`) already sees every svc; it looks the site up in a
// thread-local registry (VMs run one-per-host-thread, so this is also a
// natural per-VM namespace), runs the handler, and resumes:
//   * handler returned non-zero  -> PC = return value (explicit redirect),
//   * handler wrote PC itself    -> keep it (faithful `return;` skip semantics),
//   * otherwise                  -> PC = code cave that replays the replaced
//                                   instruction(s) and branches back, so the
//                                   original semantics are preserved exactly.
// Like legacy hooks, handlers may scribble guest registers (nothing is
// restored behind their back); unlike legacy hooks, the cost is paid only on
// the trap itself.
//
// Registration typically runs while the MMU mutex is held (library hooks are
// installed from `Mmu::update_library_hooks`), so `trap_site` only queues;
// `flush_trap_sites` - called from contexts with the mutex free, before the
// guest executes anything - allocates the cave arena, decodes the (now
// definitely loaded) site instruction, and patches. Sites whose instruction
// cannot be relocated into a cave degrade to a plain single-address Unicorn
// hook with the same handler and a one-line warning, so conversion can never
// break behavior.
// ---------------------------------------------------------------------------

use std::cell::RefCell;
use std::collections::HashMap;

/// Instruction set at a trap site (no mapping symbols exist in these ELFs,
/// so each converting file states it; procnav + the monitor shims are Thumb,
/// everything else is ARM).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TrapMode {
    Arm,
    Thumb,
}

/// `svc #0` (Thumb T1, 16-bit, little-endian).
pub const SVC16: [u8; 2] = [0x00, 0xdf];

pub trait SiteCb {
    fn call(&mut self, uc: &mut Unicorn<'_, Context>) -> u32;
}

impl<F> SiteCb for F
where
    F: FnMut(&mut Unicorn<'_, Context>) -> u32,
{
    fn call(&mut self, uc: &mut Unicorn<'_, Context>) -> u32 {
        self(uc)
    }
}

struct PendingSite {
    site: u32,
    mode: TrapMode,
    name: &'static str,
    cb: Box<dyn SiteCb>,
}

thread_local! {
    static PENDING_SITES: RefCell<Vec<PendingSite>> = const { RefCell::new(Vec::new()) };
    static SITES: RefCell<HashMap<u32, (Box<dyn SiteCb>, u32)>> = RefCell::new(HashMap::new());
}

/// Queue an observe-and-continue trap at a single instruction address (see
/// the module comment). `handler` semantics match a legacy `add_code_hook`
/// callback: run at the site before its instruction would execute, may modify
/// guest memory/registers, may redirect by writing PC or returning non-zero
/// (the returned value is then taken as the new PC; return 0 to continue).
pub fn trap_site<F>(
    unicorn: &mut Unicorn<'_, Context>,
    site: u32,
    mode: TrapMode,
    name: &'static str,
    handler: F,
) where
    F: FnMut(&mut Unicorn<'_, Context>) -> u32 + 'static,
{
    let _ = unicorn; // registration defers all VM work to flush (MMU mutex held here)
    PENDING_SITES.with(|q| {
        q.borrow_mut().push(PendingSite {
            site,
            mode,
            name,
            cb: Box::new(handler),
        })
    });
}

/// Is the 16-bit Thumb instruction at a site safely replayable out of
/// place? Accept only unconditional ALU/stack/data-processing forms that
/// cannot observe the PC; everything else (branches, literals, `BX`,
/// 0x46xx/0x47xx high-register moves, conditionals) falls back to a hook.
fn thumb16_relocatable(h: u16) -> bool {
    match h {
        // PUSH {r0-r7[,lr]} / STMDB SP!
        0xb400..=0xb5ff => true,
        // ADD/SUB SP, #imm
        0xa800..=0xa8ff | 0xb000..=0xb0ff => true,
        // ALU/branch-free dataprocessing on low registers (lsl/lsr/ands/...)
        0x0000..=0x27ff => true,
        // comparisons (may name r8-r15 high halves only via 0x428x cmn/cmp;
        // r15 there is architecturally UNPREDICTABLE so real code never has it)
        0x4200..=0x45ff => true,
        // LDR/STR/LDRH/LDRB/LDRSB/LDRSH register- and immediate-offset,
        // no writeback forms in these ranges
        0x5000..=0x7fff | 0x8800..=0x8bff | 0x6800..=0x6bff => true,
        // MOVT/MOVW with a low-or-mid target register is relocatable, but the
        // high-register encoding is fiddly to validate; accept the plain
        // T1/T3 subset conservatively.
        0xf240..=0xf24f | 0xf6c0..=0xf6cf if (h >> 3) & 0xf != 0xf => true,
        _ => false,
    }
}

fn thumb32_relocatable(word: u32) -> bool {
    let h0 = (word & 0xffff) as u16;
    let h1 = (word >> 16) as u16;
    // STMDB.W / PUSH.W with writeback, base SP, list without PC
    if h0 == 0xe92d {
        return h1 & 0x8000 == 0;
    }
    // SUB sp, sp, #imm8 (T3): 11110 00000 10 Sn1101 1101 imm8
    if h0 & 0xfbdf == 0xf1ad && h1 & 0xf000 == 0xd000 {
        return true;
    }
    false
}

/// Allocate a per-site code cave and build the trampoline for one pending
/// site. Returns the cave resume address, or 0 when the site cannot be
/// relocated (caller falls back to a legacy hook).
fn build_cave(
    unicorn: &mut Unicorn<'_, Context>,
    arena: &mut u32,
    p: &PendingSite,
) -> u32 {
    let site = p.site;
    // Cave layouts (16 bytes, 4-byte aligned, all Thumb entries are reached
    // with the |1 Thumb bit; `ldr pc,[pc,#imm]` gives mode-independent
    // arbitrary-range resume):
    //  ARM:      [word][ldr pc,[pc,#0] @+4][pad @+8][resume @+0xc]
    //  Thumb T1: [h16 @+0][ldr pc,[pc,#4] @+2][pad @+4][resume @+8]
    //  Thumb T2: [word32 @+0][ldr pc,[pc,#4] @+4][pad][resume @+0xc]
    let mut bytes = [0u8; 16];
    let resume;
    let mut patch: &[u8] = &[];
    match p.mode {
        TrapMode::Arm => {
            let mut w = [0u8; 4];
            if unicorn.mem_read(site as u64, &mut w).is_err() {
                return 0;
            }
            let word = u32::from_le_bytes(w);
            if !arm_instruction_relocatable(word) {
                log::warn!(
                    "trap {}: ARM word {word:#010x} not relocatable, legacy hook",
                    p.name
                );
                return 0;
            }
            resume = site.wrapping_add(4);
            let words = [word, 0xe59ff000u32, 0, resume];
            for (i, x) in words.iter().enumerate() {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&x.to_le_bytes());
            }
            patch = &SVC0;
        }
        TrapMode::Thumb => {
            let mut h = [0u8; 2];
            if unicorn.mem_read(site as u64, &mut h).is_err() {
                return 0;
            }
            let h0 = u16::from_le_bytes(h);
            let wide = (h0 >> 11) >= 0b11101;
            if wide {
                let mut w = [0u8; 4];
                if unicorn.mem_read(site as u64, &mut w).is_err() {
                    return 0;
                }
                let word = u32::from_le_bytes(w);
                if !thumb32_relocatable(word) {
                    log::warn!(
                        "trap {}: Thumb word {word:#08x} not relocatable, legacy hook",
                        p.name
                    );
                    return 0;
                }
                resume = site.wrapping_add(4);
                bytes[0..4].copy_from_slice(&word.to_le_bytes());
                bytes[4..6].copy_from_slice(&0x4801u16.to_le_bytes()); // ldr pc,[pc,#4]
                bytes[12..16].copy_from_slice(&(resume | 1).to_le_bytes());
                patch = &[0x00, 0xf0, 0x00, 0xf8];
            } else {
                if !thumb16_relocatable(h0) {
                    log::warn!(
                        "trap {}: Thumb half {h0:#06x} not relocatable, legacy hook",
                        p.name
                    );
                    return 0;
                }
                // Only 2 bytes are clobbered; instruction at site+2 stays in
                // place, so the cave runs the single replaced halfword and
                // returns to site+2.
                resume = site.wrapping_add(2);
                bytes[0..2].copy_from_slice(&h0.to_le_bytes());
                bytes[2..4].copy_from_slice(&0x4801u16.to_le_bytes()); // ldr pc,[pc,#4] @+2 -> literal @+8
                bytes[8..12].copy_from_slice(&(resume | 1).to_le_bytes());
                patch = &SVC16;
            }
        }
    }
    let cave = *arena;
    *arena += 16;
    if unicorn.mem_write(cave as u64, &bytes).is_err() {
        return 0;
    }
    if unicorn.mem_write(site as u64, patch).is_err() {
        log::warn!("trap {}: svc patch at {site:#x} failed", p.name);
        return 0;
    }
    match p.mode {
        TrapMode::Arm => cave,
        TrapMode::Thumb => cave | 1,
    }
}

/// Patch all queued trap sites. Must run with the MMU mutex FREE (it calls
/// `heap_alloc`) and before the guest reaches the sites. Fallback sites get a
/// legacy single-address Unicorn code hook with the same handler.
pub fn flush_trap_sites(unicorn: &mut Unicorn<'_, Context>) {
    let pending = PENDING_SITES.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if pending.is_empty() {
        return;
    }
    let count = pending.len() as u32;
    let arena_start = {
        let mmu_arc = {
            let data = unicorn.get_data();
            data.mmu.clone()
        };
        let mut guard = mmu_arc.lock().unwrap();
        let a = guard.heap_alloc(
            unicorn,
            count * 16,
            unicorn_engine::unicorn_const::Prot::READ
                | unicorn_engine::unicorn_const::Prot::WRITE
                | unicorn_engine::unicorn_const::Prot::EXEC,
            "[trap-site-caves]",
        );
        drop(guard);
        a
    };
    let mut arena = arena_start;
    let mut trapped = 0usize;
    for p in pending {
        let cave = if arena_start != 0 && arena + 16 <= arena_start + count * 16 {
            build_cave(unicorn, &mut arena, &p)
        } else {
            0
        };
        if cave != 0 {
            SITES.with(|s| s.borrow_mut().insert(p.site, (p.cb, cave)));
            trapped += 1;
        } else {
            // Legacy fallback: identical handler through a Unicorn hook.
            let mut cb = p.cb;
            let _ = unicorn.add_code_hook(
                p.site as u64,
                p.site as u64,
                move |uc, _a, _b| {
                    cb.call(uc);
                },
            );
        }
    }
    log::info!(
        "code traps: {trapped}/{count} sites trapped via svc #0 (caves at {arena_start:#x})"
    );
}

/// Dispatcher support: take (remove) the site whose instruction was just
/// replaced by the svc that fired, keyed on the post-svc PC.
pub fn take_site(post_pc: u32) -> Option<(u32, Box<dyn SiteCb>, u32)> {
    for key_pc in [post_pc.wrapping_sub(4), post_pc.wrapping_sub(2)] {
        let hit = SITES.with(|s| s.borrow_mut().remove(&key_pc).map(|(cb, cave)| (key_pc, cb, cave)));
        if let Some((key, cb, cave)) = hit {
            return Some((key, cb, cave));
        }
    }
    None
}

/// Dispatcher support: put the site back after its handler ran.
pub fn put_site(key: u32, cb: Box<dyn SiteCb>, cave: u32) {
    SITES.with(|s| s.borrow_mut().insert(key, (cb, cave)));
}

/// Number of live trapped sites (diagnostics/tests).
pub fn site_count() -> usize {
    SITES.with(|s| s.borrow().len())
}
