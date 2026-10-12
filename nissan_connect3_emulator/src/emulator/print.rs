use crate::emulator::context::Context;
use crate::emulator::utils::unpack_u32;
use capstone::arch::arm::ArchMode;
use capstone::prelude::*;
use capstone::{Capstone, Endian};
use unicorn_engine::{RegisterARM, Unicorn};

pub fn print_mmu(unicorn: &Unicorn<'_, Context>) {
    let data = unicorn.get_data();
    let mmu = data.inner.mmu.lock().unwrap();
    // fault autopsy must survive a SIGTERM'd process (stdout is block-buffered)
    log::error!("------------------ MMU (emulator regions):\n{}", mmu.display_mapped());
}

pub fn print_stack(unicorn: &Unicorn<'_, Context>) {
    let mut sp = unicorn.reg_read(RegisterARM::SP).unwrap();
    let start = sp;
    let fp = unicorn.reg_read(RegisterARM::FP).unwrap();
    let mut line = String::new();
    for _ in 0..20 {
        let mut mem = [0u8; 4];
        // a thread dying on a corrupt/unmapped stack must not take the whole host
        // thread down while printing its own autopsy
        if unicorn.mem_read(sp, &mut mem).is_err() {
            line.push_str(&format!("<unmapped at {sp:#010x}>"));
            break;
        }
        line.push_str(&format!("{:#010x} ", unpack_u32(&mem)));
        sp += 4;
    }
    log::error!("------------------ STACK at {start:#010x}, FP: {fp:#010x}: {line}");
}

pub fn disasm(unicorn: &Unicorn<'_, Context>, address: u32, len: u32) {
    let cs = Capstone::new()
        .arm()
        .mode(ArchMode::Arm)
        .endian(Endian::Little)
        .detail(true)
        .build()
        .unwrap();

    let mut vec = vec![0u8; len as usize];
    if unicorn.mem_read(address as u64, &mut vec).is_err() {
        log::error!("------------------ DISASM {address:#010x}: unmapped");
        return;
    }
    let disasm = cs.disasm_all(&vec, address as u64).unwrap();
    log::error!("------------------ DISASM at {:#010x}:\n{}", address, disasm);
}
