use crate::emulator::context::Context;
use crate::file_system::file_info::FileInfo;
use crate::file_system::{CloseFileError, FileSystem, OpenFileError, OpenFileFlags};
use path_absolutize::Absolutize;
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::Path;
use unicorn_engine::Unicorn;

pub struct MountPoint {
    pub mount_point: String,
    pub file_system: Box<dyn FileSystem + Send + Sync>,
    pub is_read_only: bool,
}

impl MountPoint {
    pub fn translate_path(&self, global_path: &str) -> Result<String, ()> {
        if global_path.starts_with(&self.mount_point) {
            let mut start_index = self.mount_point.len();
            if self.mount_point.ends_with("/") {
                start_index -= 1;
            }
            Ok(global_path[start_index..].to_string())
        } else {
            Err(())
        }
    }
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
            current_working_dir: "/".to_string(),

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
        file_path: &str,
    ) -> Option<(&mut MountPoint, String)> {
        let file_path = self.path_convert_to_absolute(file_path);
        self.mount_points
            .iter_mut()
            .filter(|mp| mp.file_system.support_file_paths())
            .find(|mp| file_path.starts_with(&mp.mount_point))
            .map(|mp| {
                let file_path = mp.translate_path(&file_path).unwrap();
                (mp, file_path)
            })
    }

    pub fn read_dir(&mut self, dir_path: &str) -> Result<Vec<String>, ()> {
        if let Some((mount_point, file_path)) = self.get_mount_point_from_filepath_mut(dir_path) {
            mount_point.file_system.read_dir(&file_path)
        } else {
            Err(())
        }
    }

    pub fn exists(&mut self, file_path: &str) -> bool {
        if let Some((mount_point, file_path)) = self.get_mount_point_from_filepath_mut(file_path) {
            mount_point.file_system.exists(&file_path)
        } else {
            false
        }
    }

    pub fn mkdir(&mut self, file_path: &str, mode: u32) -> Result<(), OpenFileError> {
        if let Some((mount_point, file_path)) = self.get_mount_point_from_filepath_mut(file_path) {
            mount_point.file_system.mkdir(&file_path, mode)
        } else {
            Err(OpenFileError::FileSystemNotMounted)
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

    pub fn link(&mut self, old_path: &str, new_path: &str) -> Result<(), OpenFileError> {
        if let Some((mount_point, old_file_path)) = self.get_mount_point_from_filepath_mut(old_path)
        {
            if let Ok(new_file_path) = mount_point.translate_path(new_path) {
                mount_point.file_system.link(&old_file_path, &new_file_path)
            } else {
                Err(OpenFileError::FileSystemNotMounted)
            }
        } else {
            Err(OpenFileError::FileSystemNotMounted)
        }
    }

    pub fn unlink(&mut self, file_path: &str) -> Result<(), OpenFileError> {
        if let Some((mount_point, file_path)) = self.get_mount_point_from_filepath_mut(file_path) {
            mount_point.file_system.unlink(&file_path)
        } else {
            Err(OpenFileError::FileSystemNotMounted)
        }
    }

    pub fn get_file_info(&mut self, fd: i32) -> Option<FileInfo> {
        let mut file_path = String::new();
        let mut file_status_flags = 0;

        if let Some(file_data) = self.file_data.get(&fd) {
            file_path = file_data.file_path.clone();
            file_status_flags = file_data.file_status_flags;
        }

        if let Some(mount_point) = self.get_mount_point_mut(fd) {
            if let Some(file_details) = mount_point.file_system.get_file_details(fd) {
                let inode = self.get_inode_for_filepath(file_path.clone());
                Some(FileInfo {
                    file_details,
                    file_path,
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

    pub fn get_file_info_from_filepath(&mut self, file_path: &str) -> Option<FileInfo> {
        let file_path = self.path_convert_to_absolute(file_path);
        if let Ok(fd) = self.open(&file_path, OpenFileFlags::READ) {
            let res = self.get_file_info(fd);
            self.close(fd).unwrap();
            res
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

    pub fn ftruncate(&mut self, fd: i32, length: u32) -> Result<(), ()> {
        if let Some(mount_point) = self.get_mount_point_mut(fd) {
            mount_point.file_system.truncate(fd, length)
        } else {
            Err(())
        }
    }

    pub fn ioctl(
        &mut self,
        unicorn: &mut Unicorn<Context>,
        fd: i32,
        request: u32,
        addr: u32,
    ) -> i32 {
        if let Some(mount_point) = self.get_mount_point_mut(fd) {
            mount_point.file_system.ioctl(unicorn, fd, request, addr)
        } else {
            -1i32
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

    fn get_inode_for_filepath(&mut self, file_path: String) -> u64 {
        let next_inode = self.inodes.len() as u64 + 1;
        let entry = self.inodes.entry(file_path).or_insert(next_inode);
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
