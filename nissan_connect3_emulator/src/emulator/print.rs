use crate::emulator::context::Context;
use crate::emulator::mmu::Mmu;
use crate::emulator::utils::unpack_u32;
use capstone::arch::arm::ArchMode;
use capstone::prelude::*;
use capstone::{Capstone, Endian};
use unicorn_engine::{RegisterARM, Unicorn};

pub fn print_mmu(unicorn: &Unicorn<Context>) {
    let data = unicorn.get_data();
    let mmu = data.inner.mmu.lock().unwrap();
    println!("------------------ MMU (emulator regions):");
    println!("{}", mmu.display_mapped());
    //println!("------------------ MMU (unicorn regions):");
    //println!("{}", Mmu::display_mapped_unicorn(unicorn));
}

pub fn print_stack(unicorn: &Unicorn<Context>) {
    let mut sp = unicorn.reg_read(RegisterARM::SP).unwrap();
    let mut fp = unicorn.reg_read(RegisterARM::FP).unwrap();
    println!(
        "------------------ STACK at {:#010x}, FP: {:#010x}:",
        sp, fp
    );
    for i in 0..20 {
        let mut mem = [0u8; 4];
        unicorn.mem_read(sp, &mut mem).unwrap();
        print!("{:#010x} ", unpack_u32(&mem));
        sp += 4;
    }
    println!();
}

pub fn mem_dump(unicorn: &Unicorn<Context>, address: u32, len: u32) {
    println!("------------------ MEM DUMP at {:#010x}:", address);
    let mut mem = vec![0u8; len as usize];
    unicorn.mem_read(address as u64, &mut mem).unwrap();
    for i in 0..len / 4 {
        print!(
            "{:#010x} ",
            unpack_u32(&mem[(4 * i) as usize..(4 * (i + 1)) as usize])
        );
    }
    println!();
}

pub fn disasm(unicorn: &Unicorn<Context>, address: u32, len: u32) {
    let cs = Capstone::new()
        .arm()
        .mode(ArchMode::Arm)
        .endian(Endian::Little)
        .detail(true)
        .build()
        .unwrap();

    let mut vec = vec![0u8; len as usize];
    unicorn.mem_read(address as u64, &mut vec).unwrap();
    let disasm = cs.disasm_all(&vec, address as u64).unwrap();
    println!("------------------ DISASM at {:#010x}:", address);
    println!("{}", disasm);
}
