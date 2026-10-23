//! r799n35: witness for the fatal `abort()` deaths.
//!
//! The state-9 FSM dies `open /dev/tty` + `exit_group(0x7f)` and the r799d
//! chain dump shows only libc return addresses (`0x47d8635c` = `abort+0x108`),
//! because by `_exit` time every application frame has been popped.  These
//! stubs fire at the TOP of `abort`/`__assert_fail` - while the caller's
//! frames are still on the stack - and log a module-resolved stack scan, so
//! the next run names the exact guest code that pulls the trigger.  After
//! witnessing, the death proceeds exactly as before (`exit_group(127)`, the
//! same status glibc's abort chain ends in), so no behaviour changes for
//! survivors; the witness is pure logging.

use crate::emulator::context::Context;
use crate::os::code_stub::add_code_stub;
use unicorn_engine::{RegisterARM, Unicorn};

/// glibc 2.8 links its first (r-x) LOAD at 0x47d58000 and the emulator maps
/// it at that same VA (the r799d death chain carries `abort+0x108` verbatim),
/// so the stub sites are plain `base + (link_va - 0x47d58000)`.
const LINK_BASE: u32 = 0x47d5_8000;
const ABORT: u32 = 0x47d8_6254 - LINK_BASE;
const ASSERT_FAIL: u32 = 0x47d7_b354 - LINK_BASE;

pub fn libc_add_code_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    add_code_stub(
        unicorn,
        "LIBC",
        base_address + ABORT,
        "abort",
        abort_witness,
    );
    add_code_stub(
        unicorn,
        "LIBC",
        base_address + ASSERT_FAIL,
        "__assert_fail",
        assert_fail_witness,
    );
}

fn c_string(uc: &mut Unicorn<'_, Context>, addr: u32) -> String {
    if addr == 0 {
        return "<null>".to_string();
    }
    let mut bytes = Vec::with_capacity(64);
    for i in 0..192u32 {
        let mut b = [0u8; 1];
        if uc.mem_read((addr.wrapping_add(i)) as u64, &mut b).is_err() {
            break;
        }
        if b[0] == 0 {
            break;
        }
        bytes.push(b[0]);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Resolve stack words through the VM's file-backed EXEC regions, so the
/// death sites read as `module+0xoffset` (skipped: unowned words, already
/// listed entries, and after 10 distinct owners).
fn stack_witness(uc: &mut Unicorn<'_, Context>) -> String {
    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
    let mut out: Vec<String> = Vec::new();
    for i in 0..512u32 {
        let mut w = [0u8; 4];
        if uc.mem_read((sp.wrapping_add(i * 4)) as u64, &mut w).is_err() {
            break;
        }
        let w = u32::from_le_bytes(w);
        let owner = {
            let mmu = uc.get_data().mmu.lock().unwrap();
            mmu.executable_location(w)
                .map(|(l, o)| format!("{l}+0x{o:x}"))
        };
        if let Some(entry) = owner {
            if !out.contains(&entry) {
                out.push(entry);
                if out.len() >= 10 {
                    break;
                }
            }
        }
    }
    out.join(" <- ")
}

fn abort_witness(uc: &mut Unicorn<'_, Context>) -> u32 {
    let thread = uc.get_data().inner.thread_id();
    let elf = uc.get_data().elf_path.clone();
    let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
    log::warn!(
        "GLIBC-TRAP [{}] {elf}: abort() lr={lr:#x} t={}ms stack: {}",
        thread,
        crate::libs::dapi::emu_t0_elapsed_ms(),
        stack_witness(uc)
    );
    crate::os::syscalls::unistd::exit_group(uc, 0x7f)
}

fn assert_fail_witness(uc: &mut Unicorn<'_, Context>) -> u32 {
    let thread = uc.get_data().inner.thread_id();
    let elf = uc.get_data().elf_path.clone();
    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
    let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
    let (expr, file, func) = {
        (c_string(uc, r0), c_string(uc, r1), c_string(uc, r3))
    };
    log::warn!(
        "GLIBC-TRAP [{}] {elf}: Assertion `{}' failed at {}:{} in {}() t={}ms stack: {}",
        thread,
        expr,
        file,
        r2,
        func,
        crate::libs::dapi::emu_t0_elapsed_ms(),
        stack_witness(uc)
    );
    crate::os::syscalls::unistd::exit_group(uc, 0x7f)
}
