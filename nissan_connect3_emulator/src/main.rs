use crate::emulator::emulator::{Emulator, ProcessSpec};
use crate::os::file_system::{
    DevFileSystem, MountFileSystem, MountPoint, OsFileSystem, ProcFileSystem, StdFileSystem,
    TmpFileSystem,
};
use std::path::PathBuf;

mod emulator;
mod libs;
mod os;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    pretty_env_logger::init();

    // mounted file systems
    let file_system = MountFileSystem::new(vec![
        // sd-card with maps
        MountPoint {
            mount_point: "/var/opt/bosch/dynamic".to_string(),
            file_system: Box::new(OsFileSystem::new(PathBuf::from(
                "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/NISSAN Connect LCN3 V7 2022_2023",
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
        // firmware
        MountPoint {
            mount_point: "/".to_string(),
            file_system: Box::new(OsFileSystem::new(PathBuf::from(
                "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/D605_unpacked/lx001.tar.gz",
            ))),
            is_read_only: true,
        },
        // stdin, stdout, stderr
        MountPoint {
            mount_point: "".to_string(),
            file_system: Box::new(StdFileSystem::new()),
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
        //("LD_DEBUG".to_string(), "files".to_string()),
    ];

    let emulator = Emulator::new(file_system);

    /*emulator.run_process(
        "/bin/echo.coreutils".to_string(),
        vec!["Hello".to_string(), "World!".to_string()],
        envs,
    )?;*/
    //emulator.run_process("/bin/date.coreutils".to_string(), vec![], envs)?;
    //emulator.run_process("/bin/pwd.coreutils".to_string(), vec![], envs)?;
    //emulator.run_process("/bin/ls.coreutils".to_string(), vec![], envs)?;
    // Run the base process and the GUI/HMI process together: each gets its own VM
    // + address space but they share the file system and kernel namespace so their
    // IPC (message queues / IOSC) works. procbaselx is the process manager; on the
    // real unit it would fork+exec prochmi itself, which the emulator cannot do, so
    // we launch it as a second top-level process instead.
    // Default process set; override with EMU_PROCESSES="path1:path2" to debug a
    // single process or a different combination without editing this file.
    let specs = match std::env::var("EMU_PROCESSES") {
        Ok(list) => list
            .split(':')
            .filter(|s| !s.is_empty())
            .map(|p| ProcessSpec::new(p).envs(envs.clone()))
            .collect(),
        Err(_) => vec![
            ProcessSpec::new("/opt/bosch/processes/procbaselx_out.out").envs(envs.clone()),
            ProcessSpec::new("/opt/bosch/processes/prochmi_out.out").envs(envs),
        ],
    };
    emulator.run_processes(specs)?;

    Ok(())
}
