use crate::emulator::emulator::Emulator;
use crate::file_system::{MountFileSystem, MountPoint, OsFileSystem, StdFileSystem};
use std::path::PathBuf;

mod emulator;
mod file_system;
mod os;

fn main() -> Result<(), Box<dyn std::error::Error + 'static>> {
    pretty_env_logger::init();

    // mounted file systems
    let file_system = MountFileSystem::new(vec![
        // stdin, stdout, stderr
        MountPoint {
            mount_point: "".to_string(),
            file_system: Box::new(StdFileSystem::new()),
            is_read_only: false,
        },
        // firmware
        MountPoint {
            mount_point: "/".to_string(),
            file_system: Box::new(OsFileSystem::new(PathBuf::from(
                "/mnt/hdd_media/ZInternetu/Firmware/NissanConnect/firmware_d605_unpacked",
            ))),
            is_read_only: true,
        },
        // sd-card with maps
        MountPoint {
            mount_point: "/var/opt/bosch/dynamic".to_string(),
            file_system: Box::new(OsFileSystem::new(PathBuf::from(
                "/mnt/hdd_media/ZInternetu/Firmware/NissanConnect/Europe_v7_2022/files",
            ))),
            is_read_only: true,
        },
    ]);

    // environment variables
    let envs = vec![
        ("PATH".to_string(), "/usr/local/bin".to_string()),
        (
            "LD_LIBRARY_PATH".to_string(),
            "/usr/lib:/lib:/opt/bosch/processes:/opt/bosch/airbiquity:/usr/lib/qtopia/plugins/gfxdrivers".to_string(),
        ),
    ];

    let mut emulator = Emulator::new(file_system).unwrap();

    /*emulator.run_elf(
        "/bin/echo.coreutils",
        &vec!["Hello".to_string(), "World!".to_string()],
        &envs,
    )?;*/
    //emulator.run_elf("/bin/date.coreutils", &vec![], &envs)?;
    emulator.run_elf("/bin/pwd.coreutils", &vec![], &envs)?;
    //emulator.run_elf("/bin/ls.coreutils", &vec![], &envs)?;
    //emulator.run_elf("/opt/bosch/processes/procmapengine.out", &vec![], &envs)?;

    Ok(())
}
