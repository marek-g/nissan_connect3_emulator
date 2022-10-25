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

    //let pwd_path = root_path.join("bin/echo.coreutils");
    //let pwd_path = root_path.join("bin/date.coreutils");
    //let pwd_path = root_path.join("bin/pwd.coreutils");
    //let pwd_path = root_path.join("bin/ls.coreutils");
    let pwd_path = root_path.join("opt/bosch/processes/procmapengine.out");
    let pwd_bin = std::fs::read(pwd_path.clone())?;
    //display_binary_information(&pwd_bin);
    //emulator.run_elf("/bin/pwd.coreutils", &pwd_bin, &Vec::new(), &Vec::new())?;
    emulator.run_elf(
        "/opt/bosch/processes/procmapengine.out",
        &pwd_bin,
        &vec![],
        &Vec::new(),
    )?;

    Ok(())
}
