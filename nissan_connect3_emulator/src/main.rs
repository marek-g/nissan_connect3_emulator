use crate::emulator::emulator::Emulator;
use crate::file_system::{
    DevFileSystem, MountFileSystem, MountPoint, OsFileSystem, ProcFileSystem, StdFileSystem,
    TmpFileSystem,
};
use std::path::PathBuf;

mod emulator;
mod file_system;
mod os;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
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
        // volatile temp-fs
        MountPoint {
            mount_point: "/var/volatile".to_string(),
            file_system: Box::new(TmpFileSystem::new()),
            is_read_only: false,
        },
        // lib temp-fs
        MountPoint {
            mount_point: "/var/lib".to_string(),
            file_system: Box::new(TmpFileSystem::new()),
            is_read_only: false,
        },
        // shm temp-fs
        MountPoint {
            mount_point: "/dev/shm".to_string(),
            file_system: Box::new(TmpFileSystem::new()),
            is_read_only: false,
        },
        // proc-fs
        MountPoint {
            mount_point: "/proc".to_string(),
            file_system: Box::new(ProcFileSystem::new()),
            is_read_only: false,
        },
        // dev-fs
        MountPoint {
            mount_point: "/dev".to_string(),
            file_system: Box::new(DevFileSystem::new()),
            is_read_only: false,
        },
    ]);

    // environment variables
    let envs = vec![
        ("PATH".to_string(), "/sbin:/bin:/usr/sbin:/usr/bin:/usr/local/bin".to_string()),
        ("RUNLEVEL".to_string(), "S".to_string()),
        (
            "LD_LIBRARY_PATH".to_string(),
            "/usr/lib:/lib:/opt/bosch/processes:/opt/bosch/airbiquity:/usr/lib/qtopia/plugins/gfxdrivers".to_string(),
        ),
    ];

    let mut emulator = Emulator::new(file_system).unwrap();

    /*emulator.run_process(
        "/bin/echo.coreutils".to_string(),
        vec!["Hello".to_string(), "World!".to_string()],
        envs,
    )?;*/
    //emulator.run_process("/bin/date.coreutils".to_string(), vec![], envs)?;
    //emulator.run_process("/bin/pwd.coreutils".to_string(), vec![], envs)?;
    //emulator.run_process("/bin/ls.coreutils".to_string(), vec![], envs)?;
    emulator.run_process(
        "/opt/bosch/processes/procbaselx_out.out".to_string(),
        //"/opt/bosch/processes/proccgs_out.out".to_string(),
        //"/opt/bosch/processes/prochmi_out.out".to_string(),
        //"/opt/bosch/processes/procvoice_out.out".to_string(),
        //"/var/opt/bosch/dynamic/CRYPTNAV/DNL/BIN/NAV/COMMON/DAPIAPP.OUT".to_string(),
        //"/bin/font_demo".to_string(),
        vec![],
        envs,
    )?;

    Ok(())
}
