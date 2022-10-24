use crate::emulator::emulator::Emulator;
use crate::emulator::file_system::FileSystem;
use std::path::PathBuf;
use unicorn_engine::unicorn_const::{Arch, Mode, Permission, SECOND_SCALE};
use unicorn_engine::{RegisterARM, Unicorn};
use xmas_elf::sections;
use xmas_elf::{header, program, ElfFile};

mod emulator;
mod os;

fn main() -> Result<(), Box<dyn std::error::Error + 'static>> {
    pretty_env_logger::init();

    let root_path =
        PathBuf::from("/mnt/hdd_media/ZInternetu/Firmware/NissanConnect/firmware_d605_unpacked");
    let sd_card_path =
        PathBuf::from("/mnt/hdd_media/ZInternetu/Firmware/NissanConnect/Europe_v7_2022/files");

    //let dapiapp_path = sd_card_path.join("CRYPTNAV/DNL/BIN/NAV/COMMON/DAPIAPP.OUT");
    //let dapiapp_bin = std::fs::read(dapiapp_path)?;

    //let procmapengine_path = root_path.join("opt/bosch/processes/procmapengine.out");
    //let procmapengine_bin = std::fs::read(procmapengine_path)?;

    let file_system = FileSystem::new(root_path.clone(), sd_card_path);
    let mut emulator = Emulator::new(file_system).unwrap();

    //let pwd_path = root_path.join("bin/date.coreutils");
    //let pwd_path = root_path.join("bin/pwd.coreutils");
    let pwd_path = root_path.join("opt/bosch/processes/procmapengine.out");
    let pwd_bin = std::fs::read(pwd_path.clone())?;
    //display_binary_information(&pwd_bin);
    emulator.run_elf("/bin/pwd.coreutils", &pwd_bin, &Vec::new(), &Vec::new())?;

    /*let arm_code32: Vec<u8> = vec![0x17, 0x00, 0x40, 0xe2]; // sub r0, #23
    let mut unicorn =
        Unicorn::new(Arch::ARM, Mode::LITTLE_ENDIAN).expect("failed to initialize Unicorn");
    let emu = &mut unicorn;
    emu.mem_map(0x1000, 0x4000, Permission::ALL)
        .expect("failed to map code page");
    emu.mem_write(0x1000, &arm_code32)
        .expect("failed to write instructions");

    emu.reg_write(RegisterARM::R0, 123)
        .expect("failed write R0");
    emu.reg_write(RegisterARM::R5, 1337)
        .expect("failed write R5");

    let _ = emu.emu_start(
        0x1000,
        (0x1000 + arm_code32.len()) as u64,
        10 * SECOND_SCALE,
        1000,
    );
    assert_eq!(emu.reg_read(RegisterARM::R0), Ok(100));
    assert_eq!(emu.reg_read(RegisterARM::R5), Ok(1337));*/

    Ok(())
}

fn display_binary_information(buf: &Vec<u8>) {
    let elf_file = ElfFile::new(buf).unwrap();
    println!("{}", elf_file.header);
    header::sanity_check(&elf_file).unwrap();

    let mut sect_iter = elf_file.section_iter();
    // Skip the first (dummy) section
    sect_iter.next();
    println!("sections");
    for sect in sect_iter {
        println!("{}", sect.get_name(&elf_file).unwrap());
        println!("{:?}", sect.get_type());
        // println!("{}", sect);
        sections::sanity_check(sect, &elf_file).unwrap();

        // if sect.get_type() == ShType::StrTab {
        //     println!("{:?}", sect.get_data(&elf_file).to_strings().unwrap());
        // }

        // if sect.get_type() == ShType::SymTab {
        //     if let sections::SectionData::SymbolTable64(data) = sect.get_data(&elf_file) {
        //         for datum in data {
        //             println!("{}", datum.get_name(&elf_file));
        //         }
        //     } else {
        //         unreachable!();
        //     }
        // }
    }
    let ph_iter = elf_file.program_iter();
    println!("\nprogram headers");
    for sect in ph_iter {
        println!("{:?}", sect.get_type());
        program::sanity_check(sect, &elf_file).unwrap();
    }

    match elf_file.program_header(5) {
        Ok(sect) => {
            println!("{}", sect);
            match sect.get_data(&elf_file) {
                Ok(program::SegmentData::Note64(header, ptr)) => {
                    println!("{}: {:?}", header.name(ptr), header.desc(ptr))
                }
                Ok(_) => (),
                Err(err) => println!("Error: {}", err),
            }
        }
        Err(err) => println!("Error: {}", err),
    }

    // let sect = elf_file.find_section_by_name(".rodata.const2794").unwrap();
    // println!("{}", sect);
}
