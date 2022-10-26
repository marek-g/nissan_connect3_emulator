use crate::file_system::file_info::FileInfo;
use crate::file_system::{CloseFileError, FileSystem, OpenFileError, OpenFileFlags};
use path_absolutize::Absolutize;
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::Path;

pub struct MountPoint {
    pub mount_point: String,
    pub file_system: Box<dyn FileSystem>,
    pub is_read_only: bool,
}

pub struct MountFsFileData {
    pub file_path: String,
    pub file_status_flags: u32,
}

///
/// File system that mounts other file systems.
///
pub struct MountFileSystem {
    pub current_working_dir: String,

    mount_points: Vec<MountPoint>,
    inodes: HashMap<String, u64>,
    file_data: HashMap<i32, MountFsFileData>,
}

impl MountFileSystem {
    pub fn new(mut mount_points: Vec<MountPoint>) -> Self {
        // sort mount points from longest to shortest to allow matching paths in order
        mount_points.sort_by(|a, b| b.mount_point.cmp(&a.mount_point));

        Self {
            current_working_dir: "/bin".to_string(),

            mount_points,
            inodes: HashMap::new(),
            file_data: HashMap::new(),
        }
    }

    pub fn get_mount_point(&self, fd: i32) -> Option<&MountPoint> {
        self.mount_points
            .iter()
            .find(|mp| mp.file_system.is_open(fd))
    }

    pub fn get_mount_point_mut(&mut self, fd: i32) -> Option<&mut MountPoint> {
        self.mount_points
            .iter_mut()
            .find(|mp| mp.file_system.is_open(fd))
    }

    pub fn get_mount_point_from_filepath_mut(
        &mut self,
        filepath: &str,
    ) -> Option<(&mut MountPoint, String)> {
        let filepath = self.path_convert_to_absolute(filepath);
        self.mount_points
            .iter_mut()
            .filter(|mp| mp.file_system.support_file_paths())
            .find(|mp| filepath.starts_with(&mp.mount_point))
            .map(|mp| {
                let filepath = filepath[mp.mount_point.len() - 1..].to_string();
                (mp, filepath)
            })
    }

    pub fn exists(&mut self, filepath: &str) -> bool {
        if let Some((mount_point, filepath)) = self.get_mount_point_from_filepath_mut(filepath) {
            mount_point.file_system.exists(&filepath)
        } else {
            false
        }
    }

    pub fn open(&mut self, file_path: &str, flags: OpenFileFlags) -> Result<i32, OpenFileError> {
        let fd = self.get_unique_fd();
        if let Some((mount_point, file_path)) = self.get_mount_point_from_filepath_mut(file_path) {
            if mount_point.is_read_only
                && (flags.contains(OpenFileFlags::WRITE)
                    || flags.contains(OpenFileFlags::CREATE)
                        && flags.contains(OpenFileFlags::EXCLUSIVE)
                    || flags.contains(OpenFileFlags::TEMP_FILE))
            {
                log::warn!(
                    "Open file for saving ignored for readonly file system! File: ({}), flags: {:?}",
                    file_path, flags
                );
                return Err(OpenFileError::NoPermission);
            }

            let res = mount_point
                .file_system
                .open(&file_path, flags, fd)
                .map(|_| fd);

            if res.is_ok() {
                let mount_fs_file_data = MountFsFileData {
                    file_path: file_path.to_string(),
                    file_status_flags: 0,
                };

                self.file_data.insert(fd, mount_fs_file_data);
            }

            res
        } else {
            Err(OpenFileError::FileSystemNotMounted)
        }
    }

    pub fn close(&mut self, fd: i32) -> Result<(), CloseFileError> {
        let res = if let Some(mount_point) = self.get_mount_point_mut(fd) {
            mount_point.file_system.close(fd)
        } else {
            Err(CloseFileError::FileNotOpened)
        };

        self.file_data.remove(&fd);

        res
    }

    pub fn get_file_info(&mut self, fd: i32) -> Option<FileInfo> {
        let mut filepath = String::new();
        let mut file_status_flags = 0;

        if let Some(file_data) = self.file_data.get(&fd) {
            filepath = file_data.file_path.clone();
            file_status_flags = file_data.file_status_flags;
        }

        if let Some(mount_point) = self.get_mount_point_mut(fd) {
            if let Some(file_details) = mount_point.file_system.get_file_details(fd) {
                let inode = self.get_inode_for_filepath(filepath.clone());
                Some(FileInfo {
                    file_details,
                    filepath,
                    inode,
                    file_status_flags,
                })
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn set_file_status_flags(&mut self, fd: i32, status_flags: u32) -> Result<(), ()> {
        if let Some(mut file_data) = self.file_data.get_mut(&fd) {
            file_data.file_status_flags = status_flags;
            Ok(())
        } else {
            Err(())
        }
    }

    pub fn is_open(&self, fd: i32) -> bool {
        self.mount_points
            .iter()
            .any(|mp| mp.file_system.is_open(fd))
    }

    pub fn get_length(&mut self, fd: i32) -> u64 {
        if let Some(mount_point) = self.get_mount_point_mut(fd) {
            mount_point.file_system.get_length(fd)
        } else {
            0
        }
    }

    pub fn stream_position(&mut self, fd: i32) -> Result<u64, ()> {
        if let Some(mount_point) = self.get_mount_point_mut(fd) {
            mount_point.file_system.stream_position(fd)
        } else {
            Err(())
        }
    }

    pub fn seek(&mut self, fd: i32, pos: SeekFrom) -> Result<u64, ()> {
        if let Some(mount_point) = self.get_mount_point_mut(fd) {
            mount_point.file_system.seek(fd, pos)
        } else {
            Err(())
        }
    }

    pub fn read(&mut self, fd: i32, content: &mut [u8]) -> Result<u64, ()> {
        if let Some(mount_point) = self.get_mount_point_mut(fd) {
            mount_point.file_system.read(fd, content)
        } else {
            Err(())
        }
    }

    pub fn read_all(&mut self, fd: i32, content: &mut [u8]) -> Result<(), ()> {
        if let Some(mount_point) = self.get_mount_point_mut(fd) {
            let len = content.len();
            let mut bytes_to_read = len;
            while bytes_to_read > 0 {
                match mount_point
                    .file_system
                    .read(fd, &mut content[len - bytes_to_read..])
                {
                    Ok(bytes) => bytes_to_read -= bytes as usize,
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        } else {
            Err(())
        }
    }

    pub fn write(&mut self, fd: i32, content: &[u8]) -> Result<u64, ()> {
        if let Some(mount_point) = self.get_mount_point_mut(fd) {
            if mount_point.is_read_only {
                log::warn!("skipped writing to read only file system");
                Err(())
            } else {
                mount_point.file_system.write(fd, content)
            }
        } else {
            Err(())
        }
    }

    pub fn write_all(&mut self, fd: i32, content: &[u8]) -> Result<(), ()> {
        if let Some(mount_point) = self.get_mount_point_mut(fd) {
            if mount_point.is_read_only {
                log::warn!("skipped writing to read only file system");
                Err(())
            } else {
                let len = content.len();
                let mut bytes_to_write = len;
                while bytes_to_write > 0 {
                    match mount_point
                        .file_system
                        .write(fd, &content[len - bytes_to_write..])
                    {
                        Ok(bytes) => bytes_to_write -= bytes as usize,
                        Err(e) => return Err(e),
                    }
                }
                Ok(())
            }
        } else {
            Err(())
        }
    }
}

impl MountFileSystem {
    fn get_unique_fd(&self) -> i32 {
        let mut fd = 0i32;
        while let Some(_) = self.get_mount_point(fd) {
            fd += 1;
        }
        fd
    }

    fn get_inode_for_filepath(&mut self, filepath: String) -> u64 {
        let next_inode = self.inodes.len() as u64 + 1;
        let entry = self.inodes.entry(filepath).or_insert(next_inode);
        *entry
    }

    fn path_convert_to_absolute(&self, path: &str) -> String {
        if path.starts_with("/") {
            Path::new(path)
                .absolutize()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
        } else if path.starts_with("~") {
            panic!("home dir not implemented yet");
        } else {
            Path::new(&self.current_working_dir)
                .join(path)
                .absolutize()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
        }
    }
}
