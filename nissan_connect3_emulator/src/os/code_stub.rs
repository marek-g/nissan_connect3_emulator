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
    if (word >> 25) & 0b111 == 0b101 {
        return false; // B / BL (any condition: bits 27:25 == 101)
    }
    if (word >> 24) & 0x0f == 0x0f {
        return false; // SWI / SVC (bits 27:24 all set - also rejects svc-patched stub sites)
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    static SITES: RefCell<HashMap<u32, (Box<dyn SiteCb>, u32, &'static str)>> = RefCell::new(HashMap::new());
    /// Legacy-fallback handlers keyed by their exact site address.  Each
    /// fallback rides a narrow (`begin == end`) Unicorn code hook at the site
    /// - the golden-era mechanism whose instruction-boundary semantics the
    /// handlers assume (skip + PC redirect).  Because fallbacks register late
    /// (after the retry budget) the page is usually already translated
    /// without the bounded check, so `flush_trap_sites` calls
    /// `ctl_remove_cache` on the page right after registering, forcing
    /// re-translation with `HOOK_EXISTS_BOUNDED` emitted (see r748 notes in
    /// docs/issues/design.md - a page-range hook cannot substitute: it fires
    /// mid-TB and the guest then executes the replaced instruction anyway).
    static FALLBACKS: RefCell<HashMap<u32, Box<dyn SiteCb>>> = RefCell::new(HashMap::new());
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
    // The T1 cave returns through `ldr r1,[pc,#4]` + `bx r1`, so the replayed
    // instruction must not write r1 (the loaded resume would clobber its
    // result).  r1-writing forms per allow-listed class (Rd fields below):
    let writes_r1 = match h {
        // PUSH {..}: bit1 = r1 in the list
        0xb400..=0xb5ff => h & 0x2 != 0,
        // ADD/SUB SP,#imm: Rd = bits 10:8 (SP for 0xa8xx? those are ADD Rd,SP - real Rd field)
        0xa800..=0xa8ff => (h >> 8) & 7 == 1,
        0xb000..=0xb0ff => false, // ADD/SUB SP,SP,#imm
        // LSL/LSR/ASR/MOV/ADD/SUB/CMP imm & 3-register forms: Rd = bits 10:8
        0x0000..=0x27ff => (h >> 8) & 7 == 1,
        // ALU hi-op / MUL / REV / CMP-CMN-TST-NOT-NEG reg: Rn/Rd = bits 5:3
        0x4200..=0x45ff => (h >> 3) & 7 == 1,
        // STR/LDR half,word,byte immediate: Rt = bits 2:0 (reject loads AND
        // stores naming r1: cheap and conservative)
        r @ (0x5000..=0x7fff | 0x8800..=0x8bff | 0x6800..=0x6bff) => r & 7 == 1,
        _ => true,
    };
    if writes_r1 {
        return false;
    }
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
    // STMDB.W / PUSH.W with writeback, base SP, list without PC.  The T2 cave
    // also returns via `ldr r1` + `bx r1`, so a list containing r1 is rejected.
    if h0 == 0xe92d {
        return h1 & 0x8002 == 0;
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
/// Why `build_cave` did not patch a site.
enum CaveFail {
    /// The word may not be the final instruction yet (the loader has mapped
    /// the page but not written it) - retry on a later flush.
    Retry,
    /// The word is a real instruction but structurally unrelocatable (branch,
    /// `BX`, `MRS`/`MSR`, PC operand, `SVC`...). Retrying is pointless; go to
    /// the narrow-hook fallback *immediately*, because leaving the site
    /// unhooked for 64 flushes runs the guest function completely bare
    /// during that window (the golden-era hooks were live from load).
    Structural,
}

/// Detect the *true* instruction mode at `site` and auto-correct a wrong
/// registration stamp.  The converter stamps whole files (ARM or Thumb), but
/// binaries contain interworking islands and linker thunks in the other
/// mode: an ARM `svc #0` word patched onto Thumb code decodes there as
/// `movs r0,r0` plus an undefined 32-bit word, and a Thumb16 `svc` spliced
/// into ARM code corrupts the instruction pair - both keep running (silently
/// wrong) instead of faulting.  The probe decodes the site in both modes and
/// flips the stamp only when the stamped mode cannot decode *any* valid
/// instruction while the other mode can.  Genuinely ambiguous words (an ARM
/// `STMDB sp!,{..}` push also being a legal Thumb32 encoding) keep the
/// stamp, which is what every shipped site has been verified against.
fn probe_true_mode(unicorn: &Unicorn<'_, Context>, site: u32, stamped: TrapMode) -> TrapMode {
    use capstone::arch::arm::ArchMode;
    use capstone::prelude::*;
    use capstone::{Capstone, Endian};

    let mut code = [0u8; 8];
    if unicorn.mem_read(site as u64, &mut code).is_err() {
        return stamped;
    }
    let decodes = |mode: ArchMode| -> bool {
        let Ok(cs) = Capstone::new()
            .arm()
            .mode(mode)
            .endian(Endian::Little)
            .build()
        else {
            return false;
        };
        cs.disasm_all(&code, site as u64)
            .map(|insns| insns.iter().next().is_some())
            .unwrap_or(false)
    };
    let (arm_ok, thumb_ok) = (decodes(ArchMode::Arm), decodes(ArchMode::Thumb));
    let true_mode = match (arm_ok, thumb_ok) {
        (true, _) => TrapMode::Arm,
        (false, true) => TrapMode::Thumb,
        (false, false) => return stamped,
    };
    if true_mode != stamped {
        log::warn!(
            "trap: site {site:#x} stamp {stamped:?} is really {true_mode:?} (auto-correcting)"
        );
    }
    true_mode
}

fn build_cave(
    unicorn: &mut Unicorn<'_, Context>,
    arena: &mut u32,
    p: &PendingSite,
) -> Result<u32, CaveFail> {
    let site = p.site;
    // Cave layouts (16 bytes, 4-byte aligned, all Thumb entries are reached
    // with the |1 Thumb bit).  Thumb16 has NO `ldr pc,[pc,#imm]` (Rt=15 is not
    // encodable), so the return is `ldr r1,[pc,#4]` + `bx r1` (BX honours the
    // |1 mode bit); the replayed instruction must not depend on r1:
    //  ARM:      [word][ldr pc,[pc,#0] @+4][pad @+8][resume @+0xc]
    //  Thumb T1: [h16 @+0][ldr r1 @+2][bx r1 @+4][pad @+6][resume|1 @+8]
    //  Thumb T2: [word32 @+0][ldr r1 @+4][bx r1 @+6][pad @+8][resume|1 @+0xc]
    let mut bytes = [0u8; 16];
    let resume;
    let mut patch: &[u8] = &[];
    let mode = probe_true_mode(unicorn, site, p.mode);
    match mode {
        TrapMode::Arm => {
            let mut w = [0u8; 4];
            if unicorn.mem_read(site as u64, &mut w).is_err() {
                return Err(CaveFail::Retry);
            }
            let word = u32::from_le_bytes(w);
            if !arm_instruction_relocatable(word) {
                log::debug!(
                    "trap {}: ARM word {word:#010x} not relocatable",
                    p.name
                );
                return Err(if word == 0 {
                    CaveFail::Retry // unwritten page: retry after the loader
                } else {
                    CaveFail::Structural
                });
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
                return Err(CaveFail::Retry);
            }
            let h0 = u16::from_le_bytes(h);
            let wide = (h0 >> 11) >= 0b11101;
            if wide {
                let mut w = [0u8; 4];
                if unicorn.mem_read(site as u64, &mut w).is_err() {
                    return Err(CaveFail::Retry);
                }
                let word = u32::from_le_bytes(w);
                if !thumb32_relocatable(word) {
                    log::debug!("trap {}: Thumb word {word:#08x} not relocatable", p.name);
                    return Err(if word == 0 {
                        CaveFail::Retry
                    } else {
                        CaveFail::Structural
                    });
                }
                resume = site.wrapping_add(4);
                bytes[0..4].copy_from_slice(&word.to_le_bytes());
                bytes[4..6].copy_from_slice(&0x4801u16.to_le_bytes()); // ldr r1,[pc,#4]
                bytes[6..8].copy_from_slice(&0x4708u16.to_le_bytes()); // bx  r1
                bytes[12..16].copy_from_slice(&(resume | 1).to_le_bytes());
                patch = &[0x00, 0xf0, 0x00, 0xf8];
            } else {
                if !thumb16_relocatable(h0) {
                    log::debug!("trap {}: Thumb half {h0:#06x} not relocatable", p.name);
                    return Err(if h0 == 0 {
                        CaveFail::Retry
                    } else {
                        CaveFail::Structural
                    });
                }
                // Only 2 bytes are clobbered; instruction at site+2 stays in
                // place, so the cave runs the single replaced halfword and
                // returns to site+2.
                resume = site.wrapping_add(2);
                bytes[0..2].copy_from_slice(&h0.to_le_bytes());
                bytes[2..4].copy_from_slice(&0x4801u16.to_le_bytes()); // ldr r1,[pc,#4] @+2 -> literal @+8
                bytes[4..6].copy_from_slice(&0x4708u16.to_le_bytes()); // bx  r1 (honours the |1 mode bit)
                bytes[8..12].copy_from_slice(&(resume | 1).to_le_bytes());
                patch = &SVC16;
            }
        }
    }
    let cave = *arena;
    *arena += 16;
    if unicorn.mem_write(cave as u64, &bytes).is_err() {
        return Err(CaveFail::Retry);
    }
    if unicorn.mem_write(site as u64, patch).is_err() {
        log::warn!("trap {}: svc patch at {site:#x} failed", p.name);
        return Err(CaveFail::Retry);
    }
    // The patched page may already have translated blocks (the site itself was
    // not executed yet, but QEMU translates whole runs of instructions, and a
    // block covering the site can exist as part of a block whose earlier part
    // ran - or a guest demand-paged page load may have rewritten our svc, in
    // which case re-patching here is exactly what keeps the trap alive).
    // Drop the page's translations so the *next* fetch re-reads the patched
    // bytes; host-side mem_write alone never invalidates them.
    if unicorn
        .ctl_remove_cache((site & !0xfff) as u64, ((site & !0xfff) + 0x1000) as u64)
        .is_err()
    {
        log::warn!("trap {}: TB invalidate at {site:#x} failed", p.name);
    }
    // Confirm the patch really landed (a readback that differs means the page
    // was remapped/reloaded over us).
    {
        let mut back = [0u8; 4];
        let _ = unicorn.mem_read(site as u64, &mut back);
        log::debug!(
            "trap {}: patched {site:#x} readback {:02x?} (want {:02x?}) thread [{}]",
            p.name,
            back,
            patch,
            unicorn.get_data().inner.thread_id()
        );
    }
    match mode {
        TrapMode::Arm => Ok(cave),
        TrapMode::Thumb => Ok(cave | 1),
    }
}

/// Patch all queued trap sites. Must run with the MMU mutex FREE (it calls
/// `heap_alloc`) and before the guest reaches the sites. A site whose word is
/// not yet the real instruction (the loader may map a segment's pages before
/// writing all of their contents) is retried on the next flush; only after
/// `RETRY_LIMIT` flushes - by which time the guest has surely loaded and even
/// run the page - does it degrade to a legacy Unicorn code hook.
pub fn flush_trap_sites(unicorn: &mut Unicorn<'_, Context>) {
    let mut pending = PENDING_SITES.with(|q| std::mem::take(&mut *q.borrow_mut()));
    let retry: Vec<(PendingSite, u32)> = RETRY_SITES.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if pending.is_empty() && retry.is_empty() {
        return;
    }
    let retry_counts: Vec<u32> = retry.iter().map(|(_, n)| *n).collect();
    let fresh_count = pending.len();
    for (p, _) in retry {
        pending.push(p);
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
    // Debug: watch the arena page so guest writes into a live cave (which the
    // earlier 0x16-into-literal corruption looked exactly like) are logged.
    {
        let res = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            (arena_start & !0xfff) as u64,
            (((arena_start + count * 16) | 0xfff) + 1) as u64 - 1,
            move |uc, _t, address, size, value| {
                log::error!(
                    "ARENA WRITE: {:#x} size {:#x} value {:#x} thread [{}]",
                    address,
                    size,
                    value,
                    uc.get_data().inner.thread_id()
                );
                true
            },
        );
        if let Err(e) = res {
            log::warn!("arena watch failed: {e:?}");
        }
    }
    let mut trapped = 0usize;
    let mut new_retry: Vec<(PendingSite, u32)> = Vec::new();
    for (idx, p) in pending.into_iter().enumerate() {
        // Retries were appended behind the fresh sites; their attempt count
        // lives in `retry_counts` (same order as the trailing items).
        let attempts = if idx >= fresh_count {
            retry_counts[idx - fresh_count]
        } else {
            0
        };
        let disabled = trap_disabled(p.name);
        let duplicate = SITES.with(|s| s.borrow().contains_key(&p.site));
        let mut structural_reject = false;
        let cave = if !disabled
            && !duplicate
            && arena_start != 0
            && arena + 16 <= arena_start + count * 16
        {
            match build_cave(unicorn, &mut arena, &p) {
                Ok(cave) => cave,
                Err(CaveFail::Structural) => {
                    structural_reject = true;
                    0
                }
                Err(CaveFail::Retry) => 0,
            }
        } else {
            0
        };
        if cave != 0 {
            ALL_SITES.with(|s| s.borrow_mut().insert(p.site, cave));
            SITES.with(|s| s.borrow_mut().insert(p.site, (p.cb, cave, p.name)));
            trapped += 1;
            continue;
        }
        if duplicate {
            // unicorn would run BOTH legacy callbacks; a trap could only carry
            // one handler, so chain this extra registration as a legacy hook
            // (it still fires alongside the svc trap that owns the instruction).
            log::warn!("trap {}: duplicate site at {:#x}, legacy hook", p.name, p.site);
        } else if !disabled && attempts < RETRY_LIMIT && !structural_reject {
            // The site word may not be final yet (the loader has mapped the
            // page but not written its contents - segments arrive in several
            // mmaps); retry on the next flush. A *structural* rejection goes
            // straight to the legacy hook: an unhooked window would run the
            // guest function bare, and the golden-era hooks were live from
            // load.
            new_retry.push((p, attempts + 1));
            continue;
        } else if !disabled {
            log::warn!(
                "trap {}: site {:#x} -> legacy hook ({}, in {} [{}])",
                p.name,
                p.site,
                if structural_reject {
                    "instruction not relocatable".to_string()
                } else {
                    format!("not relocatable after {RETRY_LIMIT} flushes")
                },
                unicorn.get_data().elf_path,
                unicorn.get_data().inner.thread_id()
            );
        }
        if disabled {
            // EMU_TRAPS_DISABLE names the pure-legacy files (procnav probes,
            // rtos stub traces): their addresses are owned by the xp-watch
            // page hooks / stub registrar, so installing a fallback here too
            // would double-dispatch every hit with different semantics.
            continue;
        }
        // Legacy fallback: the golden-era narrow (begin == end) Unicorn hook
        // at the exact site address. Narrow hooks fire *at the instruction
        // boundary*, so the handler's PC writes and skip semantics survive
        // (a page-range hook fires mid-TB: the replaced instruction and the
        // rest of the TB still execute and clobber the handler's redirect -
        // that silently ran stub function bodies whose trap rejected the
        // prologue, e.g. OSAL_s32MessageQueueCreate asserting on procmap
        // boot). They die only when the page was already translated without
        // the bounded-hook check, so drop the page's TBs right after
        // registering to force re-translation with the check emitted.
        FALLBACKS.with(|m| m.borrow_mut().insert(p.site, p.cb));
        log::info!("trap {}: fallback hook installed at {:#x}", p.name, p.site);
        let res = unicorn.add_code_hook(p.site as u64, p.site as u64, move |uc, a, _| {
            let a = a as u32;
            let cb = FALLBACKS.with(|m| m.borrow_mut().remove(&a));
            if let Some(mut cb) = cb {
                let ret = cb.call(uc);
                FALLBACKS.with(|m| m.borrow_mut().insert(a, cb));
                if ret != 0 {
                    let _ = uc.reg_write(unicorn_engine::RegisterARM::PC, ret as u64);
                }
            }
        });
        if res.is_err() {
            log::warn!("trap {}: fallback hook install failed at {:#x}", p.name, p.site);
        }
        let page = p.site & !0xfff;
        let _ = unicorn.ctl_remove_cache(page as u64, page as u64 + 0x1000);
    }
    RETRY_SITES.with(|q| *q.borrow_mut() = new_retry);
    log::info!(
        "code traps: {}/{} sites trapped via svc #0 (caves at {:#x}) in {} [{}]",
        trapped,
        count,
        arena_start,
        unicorn.get_data().elf_path,
        unicorn.get_data().inner.thread_id()
    );
}

thread_local! {
    /// Sites rejected by `build_cave` (word not yet a real instruction
    /// because the loader has not finished writing the page), kept pending
    /// conversion until a later flush sees a sane word. See flush_trap_sites.
    static RETRY_SITES: RefCell<Vec<(PendingSite, u32)>> = const { RefCell::new(Vec::new()) };
}

/// Number of flush passes a rejected site waits before it degrades to a
/// legacy Unicorn hook.
const RETRY_LIMIT: u32 = 64;

thread_local! {
    /// Registry of every successfully patched site address -> its cave. Kept
    /// for the lifetime of the VM even while the site is temporarily taken by
    /// the dispatcher; lets the interrupt hook recognise (and safely replay)
    /// its own `svc #0` whose SITES entry went missing.
    static ALL_SITES: RefCell<HashMap<u32, u32>> = RefCell::new(HashMap::new());
}

pub fn site_registered(addr: u32) -> bool {
    ALL_SITES.with(|s| s.borrow().contains_key(&addr))
}

pub fn site_registered_cave(addr: u32) -> u32 {
    ALL_SITES.with(|s| *s.borrow().get(&addr).unwrap_or(&0))
}

/// Dispatcher support: take (remove) the site whose instruction was just
/// replaced by the svc that fired, keyed on the post-svc PC.
pub fn take_site(post_pc: u32) -> Option<(u32, Box<dyn SiteCb>, u32, &'static str)> {
    for key_pc in [post_pc.wrapping_sub(4), post_pc.wrapping_sub(2)] {
        let hit = SITES
            .with(|s| s.borrow_mut().remove(&key_pc).map(|(cb, cave, name)| (key_pc, cb, cave, name)));
        if let Some((key, cb, cave, name)) = hit {
            return Some((key, cb, cave, name));
        }
    }
    None
}

/// A site is busy while its handler runs (`take_site` removed it). If guest
/// code executed *by the handler* (`call_guest_function`) re-executes that
/// exact site, the trap must NOT run the handler again nor fall through to
/// syscall emulation: the instruction must simply replay. Returns the cave
/// for such a re-entrant hit.
pub fn site_cave(post_pc: u32) -> Option<u32> {
    for key_pc in [post_pc.wrapping_sub(4), post_pc.wrapping_sub(2)] {
        let hit = SITES.with(|s| s.borrow().get(&key_pc).map(|(_, cave, _)| *cave));
        if hit.is_some() {
            return hit;
        }
    }
    None
}

thread_local! {
    /// Nesting depth of site-handler execution on this VM thread.
    static IN_SITE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

pub fn in_site_handler() -> bool {
    IN_SITE.with(|d| d.get() > 0)
}

thread_local! {
    /// Debug aid: the name of the last site handler executed on this VM
    /// thread, reported by the scheduler when a guest execution fault hits.
    static LAST_SITE: std::cell::RefCell<Option<(&'static str, u32)>> = const { std::cell::RefCell::new(None) };
}

pub fn note_site(name: &'static str, addr: u32) {
    LAST_SITE.with(|s| *s.borrow_mut() = Some((name, addr)));
    SITE_RING.with(|r| {
        let mut r = r.borrow_mut();
        if r.len() >= 48 {
            r.remove(0);
        }
        r.push_back((name, addr));
    });
}

pub fn last_site() -> Option<(&'static str, u32)> {
    LAST_SITE.with(|s| *s.borrow())
}

thread_local! {
    /// Debug aid: the last ~48 site hits on this VM thread, dumped with the
    /// scheduler's fault context so the exact trapped path is visible.
    static SITE_RING: std::cell::RefCell<std::collections::VecDeque<(&'static str, u32)>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

pub fn dump_site_ring() {
    SITE_RING.with(|r| {
        let r = r.borrow();
        for (n, a) in r.iter() {
            log::error!("site ring: {n} @ {a:#x}");
        }
    });
}

pub fn site_handler_enter() {
    IN_SITE.with(|d| d.set(d.get() + 1));
}

pub fn site_handler_leave() {
    IN_SITE.with(|d| d.set(d.get() - 1));
}

/// Kill-switch for bisecting regressions: `EMU_TRAPS_DISABLE=<name prefix>`
/// (comma separated) makes matching `trap_site` registrations fall back to a
/// legacy Unicorn code hook instead of the svc trampoline.
fn trap_disabled(name: &str) -> bool {
    std::env::var("EMU_TRAPS_DISABLE").map_or(false, |v| {
        v.split(',').any(|p| !p.is_empty() && name.starts_with(p))
    })
}

/// Dispatcher support: put the site back after its handler ran.
pub fn put_site(key: u32, cb: Box<dyn SiteCb>, cave: u32, name: &'static str) {
    SITES.with(|s| s.borrow_mut().insert(key, (cb, cave, name)));
}

/// Number of live trapped sites (diagnostics/tests).
pub fn site_count() -> usize {
    SITES.with(|s| s.borrow().len())
}
