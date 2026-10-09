use crate::emulator::emulator::{Emulator, ProcessSpec};
use crate::os::file_system::{
    DevFileSystem, FileType, MountFileSystem, MountPoint, OsFileSystem, ProcFileSystem,
    StdFileSystem, TmpFileSystem,
};
use std::io;
use std::path::{Path, PathBuf};

mod common;
mod emulator;
mod gpu;
mod libs;
mod os;
mod rtos;

fn seed_dynamic_ffs() -> io::Result<()> {
    const SOURCE: &str =
        "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/D605_unpacked/lx001.tar.gz/var/opt/bosch/dynamic/ffs";
    const DESTINATION: &str = "/tmp/opencode/nissan_emu/ffs_dynamic";

    let source = PathBuf::from(SOURCE);
    let destination = PathBuf::from(DESTINATION);
    std::fs::create_dir_all(&destination)?;
    seed_dynamic_ffs_recursive(&source, &destination)
}

fn seed_dynamic_ffs_recursive(source: &Path, destination: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());

        if file_type.is_dir() {
            std::fs::create_dir_all(&destination_path)?;
            seed_dynamic_ffs_recursive(&source_path, &destination_path)?;
        } else if file_type.is_file() && !destination_path.exists() {
            std::fs::copy(&source_path, &destination_path)?;
        }
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    pretty_env_logger::init();
    seed_dynamic_ffs()?;

    // mounted file systems
    let map_card_path =
        "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/NISSAN Connect LCN3 V7 2022_2023";
    let file_system = MountFileSystem::new(vec![
        // sd-card with maps
        MountPoint {
            mount_point: "/var/opt/bosch/dynamic".to_string(),
            file_system: Box::new(OsFileSystem::new(PathBuf::from(map_card_path))),
            is_read_only: true,
        },
        // OSAL exposes the inserted navigation SD card under /shared/cryptnav
        MountPoint {
            mount_point: "/shared".to_string(),
            file_system: {
                let mut shared_fs = TmpFileSystem::new();
                shared_fs.insert_entry("/cryptnav", FileType::Directory, vec![]);
                Box::new(shared_fs)
            },
            is_read_only: false,
        },
        MountPoint {
            mount_point: "/shared/cryptnav".to_string(),
            file_system: Box::new(OsFileSystem::new(PathBuf::from(map_card_path))),
            is_read_only: true,
        },
        // dynamic process binaries (e.g. DAPIAPP.OUT): the map card's mount above
        // shadows the firmware's /var/opt/bosch/dynamic, but the stock binaries
        // symlink /opt/bosch/processes/*.OUT into /var/opt/bosch/dynamic/processes
        // which only the firmware root actually contains. Mount that directory
        // over the (map-card) mount so those symlinks resolve to real ELFs.
        MountPoint {
            mount_point: "/var/opt/bosch/dynamic/processes".to_string(),
            file_system: Box::new(OsFileSystem::new(PathBuf::from(
                "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/D605_unpacked/lx001.tar.gz/var/opt/bosch/dynamic/processes",
            ))),
            is_read_only: true,
        },
        // Dynamic FFS is writable on the unit. The SD-card mount above is
        // read-only, so shadow only the mutable FFS tree with a host overlay.
        MountPoint {
            mount_point: "/var/opt/bosch/dynamic/ffs".to_string(),
            file_system: Box::new(OsFileSystem::new(PathBuf::from(
                "/tmp/opencode/nissan_emu/ffs_dynamic",
            ))),
            is_read_only: false,
        },
        // Keep the software watchdog from rebooting the emulated process.
        MountPoint {
            mount_point: "/opt/bosch/disable_reset.txt".to_string(),
            file_system: {
                let mut fs = TmpFileSystem::new();
                fs.insert_entry("/", FileType::File, b"emulator\n".to_vec());
                Box::new(fs)
            },
            is_read_only: false,
        },
        // DAPIAPP reads the cryptnav configuration tree (POI_MAPPING.DAT and
        // friends) from the internal navdata flash; on the unit that tree is
        // populated from the navigation SD card. Serve it straight from the
        // card's CRYPTNAV volume so DAPDEVM can validate the dataset.
        MountPoint {
            mount_point: "/var/opt/bosch/navdata/cryptnav".to_string(),
            file_system: Box::new(OsFileSystem::new(PathBuf::from(format!(
                "{}/CRYPTNAV",
                map_card_path
            )))),
            is_read_only: true,
        },
        // volatile temp-fs
        MountPoint {
            mount_point: "/var/volatile".to_string(),
            file_system: Box::new(TmpFileSystem::new()),
            is_read_only: false,
        },
        // The firmware /etc holds runtime configuration the guest expects to
        // read (svg_config.ini for libsvg-common, localtime for libc, and so
        // on). Real unit has /etc on the rootfs; we expose the unpacked
        // firmware copy read-only.
        MountPoint {
            mount_point: "/etc".to_string(),
            file_system: Box::new(OsFileSystem::new(PathBuf::from(
                "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/D605_unpacked/lx001.tar.gz/etc",
            ))),
            is_read_only: true,
        },
        // tmp-fs: OSAL/PRM expects /tmp to be writable and automount-ready.
        // One emulated USB storage device is enough to get past the PRM table
        // startup checks without a host-side USB backend.
        MountPoint {
            mount_point: "/tmp".to_string(),
            file_system: {
                let mut tmp_fs = TmpFileSystem::new();
                tmp_fs.insert_entry("/.automount", FileType::Directory, vec![]);
                tmp_fs.insert_entry(
                    "/.automount/sda",
                    FileType::File,
                    b"/dev/media/sda1".to_vec(),
                );
                tmp_fs.insert_entry("/.automount/mmcblk1p1", FileType::Directory, vec![]);
                Box::new(tmp_fs)
            },
            is_read_only: false,
        },
        // fake removable-media tree created by the emulated automounter
        MountPoint {
            mount_point: "/dev/media".to_string(),
            file_system: {
                let mut media_fs = TmpFileSystem::new();
                media_fs.insert_entry("/sda1", FileType::Directory, vec![]);
                media_fs.insert_entry("/sda1/CRYPTNAV", FileType::Directory, vec![]);
                Box::new(media_fs)
            },
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
        // sysfs SD-card device node
        MountPoint {
            mount_point: "/sys".to_string(),
            file_system: {
                let mut sys_fs = TmpFileSystem::new();
                sys_fs.insert_entry("/block", FileType::Directory, vec![]);
                sys_fs.insert_entry("/block/mmcblk1", FileType::Directory, vec![]);
                sys_fs.insert_entry("/block/mmcblk1/device", FileType::Directory, vec![]);
                sys_fs.insert_entry(
                    "/block/mmcblk1/device/cid",
                    FileType::File,
                    b"5d5342303031364712e055a86c013301".to_vec(),
                );
                sys_fs.insert_entry(
                    "/block/mmcblk1/device/csd",
                    FileType::File,
                    b"400e00325b59000000000000000000f7".to_vec(),
                );
                sys_fs.insert_entry(
                    "/block/mmcblk1/device/scr",
                    FileType::File,
                    b"0235800000000000".to_vec(),
                );
                sys_fs.insert_entry(
                    "/block/mmcblk1/device/manfid",
                    FileType::File,
                    b"5d".to_vec(),
                );
                sys_fs.insert_entry(
                    "/block/mmcblk1/device/serial",
                    FileType::File,
                    b"12e055a8".to_vec(),
                );
                sys_fs.insert_entry("/block/mmcblk1/device/ro", FileType::File, b"0".to_vec());
                sys_fs.insert_entry(
                    "/block/mmcblk1/size",
                    FileType::File,
                    b"134217728".to_vec(),
                );
                Box::new(sys_fs)
            },
            is_read_only: true,
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
        MountPoint {
            mount_point: "/dev/ffs2".to_string(),
            file_system: Box::new(OsFileSystem::new(PathBuf::from(
                "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/D605_unpacked/lx001.tar.gz/var/opt/bosch",
            ))),
            is_read_only: true,
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
    // Start the base process only: the in-crate RTOS backend now plays the
    // boot-controller role that the real triton_dualos side has on the unit,
    // injecting/launching configured Linux start-process commands once the OSAL
    // callback queues exist. Use EMU_RTOS=off to disable it, or EMU_RTOS_START
    // to choose the logical process(es) it should start.
    // Default process set; override with EMU_PROCESSES="path1:path2" to debug a
    // single process or a different combination without editing this file.
    let specs = match std::env::var("EMU_PROCESSES") {
        Ok(list) => list
            .split(':')
            .filter(|s| !s.is_empty())
            .map(|p| ProcessSpec::new(p).envs(envs.clone()))
            .collect(),
        Err(_) => vec![ProcessSpec::new("/opt/bosch/processes/procbaselx_out.out").envs(envs)],
    };
    emulator.run_processes(specs)?;

    Ok(())
}
