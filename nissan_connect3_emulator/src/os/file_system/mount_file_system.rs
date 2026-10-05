use crate::emulator::context::Context;
use crate::os::file_system::file_info::FileInfo;
use crate::os::file_system::{CloseFileError, FileSystem, OpenFileError, OpenFileFlags};
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

/// strip a single trailing slash ("/var/lib/" -> "/var/lib"), keeping "/" intact
fn strip_trailing_slash(path: &str) -> &str {
    if path.len() > 1 && path.ends_with('/') {
        &path[..path.len() - 1]
    } else {
        path
    }
}

impl MountPoint {
    /// Whether a global path belongs to this mount point (the mount point
    /// itself or anything below it). Matching is done on full path
    /// components, so mounting /var/lib does not claim /var/libfoo.
    pub fn matches(&self, global_path: &str) -> bool {
        if self.mount_point.is_empty() {
            // the std (fd 0-2) file system has no paths
            return false;
        }
        if self.mount_point == "/" {
            return global_path.starts_with("/");
        }
        let global_path = strip_trailing_slash(global_path);
        if global_path == self.mount_point {
            return true;
        }
        let prefix = format!("{}/", self.mount_point);
        global_path.starts_with(&prefix)
    }

    pub fn translate_path(&self, global_path: &str) -> Result<String, ()> {
        if !self.matches(global_path) {
            return Err(());
        }
        if self.mount_point == "/" {
            Ok(global_path.to_string())
        } else {
            let global_path = strip_trailing_slash(global_path);
            let translated = &global_path[self.mount_point.len()..];
            if translated.is_empty() {
                Ok("/".to_string())
            } else {
                Ok(translated.to_string())
            }
        }
    }
}

pub struct MountFsFileData {
    /// global (absolute) path of the opened file
    pub file_path: String,
    /// mount point the file was opened on - part of the inode key so that
    /// identical relative paths on different mounts do not collide in st_ino
    pub mount_point: String,
    pub file_status_flags: u32,
}

///
/// File system that mounts other file systems.
///
pub struct MountFileSystem {
    pub current_working_dir: String,

    mount_points: Vec<MountPoint>,
    inodes: HashMap<(String, String), u64>,
    file_data: HashMap<i32, MountFsFileData>,
}

impl MountFileSystem {
    pub fn new(mut mount_points: Vec<MountPoint>) -> Self {
        // sort mount points from longest to shortest so that nested mounts
        // (e.g. /var/volatile under /) resolve to the most specific one;
        // with component-boundary matching the order of unrelated mount
        // points is irrelevant
        mount_points.sort_by(|a, b| b.mount_point.len().cmp(&a.mount_point.len()));

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
        let absolute_path = self.path_convert_to_absolute(file_path);
        self.resolve_mount(&absolute_path)
    }

    /// resolve an already-absolute global path to the (mount point, translated path)
    fn resolve_mount(&mut self, absolute_path: &str) -> Option<(&mut MountPoint, String)> {
        self.mount_points
            .iter_mut()
            .filter(|mp| mp.file_system.support_file_paths())
            .find(|mp| mp.matches(absolute_path))
            .map(|mp| {
                let translated_path = mp.translate_path(absolute_path).unwrap();
                (mp, translated_path)
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
        let mut path = self.path_convert_to_absolute(file_path);

        // follow symbolic links: the stored target is a *guest* path, so each hop
        // must be re-resolved through the mount table (a bare host follow would
        // resolve an absolute target against the host root and miss it)
        for _ in 0..8 {
            let absolute_path = path.clone();
            if let Some((mount_point, translated_path)) = self.resolve_mount(&absolute_path) {
                if mount_point.is_read_only
                    && (flags.contains(OpenFileFlags::WRITE)
                        || flags.contains(OpenFileFlags::CREATE)
                            && flags.contains(OpenFileFlags::EXCLUSIVE)
                        || flags.contains(OpenFileFlags::TEMP_FILE))
                {
                    log::warn!(
                        "Open file for saving ignored for readonly file system! File: ({}), flags: {:?}",
                        translated_path, flags
                    );
                    return Err(OpenFileError::NoPermission);
                }

                if let Some(target) = mount_point.file_system.read_link(&translated_path) {
                    path = if target.starts_with('/') {
                        target
                    } else {
                        let parent = absolute_path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
                        format!("{}/{}", parent, target)
                    };
                    continue;
                }

                let res = mount_point
                    .file_system
                    .open(&translated_path, flags, fd)
                    .map(|_| fd);

                if res.is_ok() {
                    // store the global path (not the mount-relative one) so that
                    // dirfd-based operations (openat, getdents) re-resolve entries
                    // against the file's real location
                    let mount_fs_file_data = MountFsFileData {
                        file_path: absolute_path.clone(),
                        mount_point: mount_point.mount_point.clone(),
                        file_status_flags: 0,
                    };

                    self.file_data.insert(fd, mount_fs_file_data);
                }

                return res;
            } else {
                return Err(OpenFileError::FileSystemNotMounted);
            }
        }

        Err(OpenFileError::NoSuchFileOrDirectory)
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

    /// Duplicate `old_fd` onto `new_fd` (a not-yet-used descriptor), making both
    /// refer to the same file. Implemented by re-opening the source file's path
    /// on the same mount under `new_fd`. The offset is not shared with the
    /// original (unlike a real `dup`), which is fine for the head-unit's use of
    /// dup (saving/redirecting standard descriptors during setup). Returns
    /// `false` if the source is not a path-backed, openable file (e.g. a stdio
    /// descriptor or a driver fd), in which case the caller reports an error.
    pub fn dup2(&mut self, old_fd: i32, new_fd: i32) -> bool {
        let (path, flags) = match self.file_data.get(&old_fd) {
            Some(data) => (data.file_path.clone(), OpenFileFlags::READ),
            None => return false,
        };
        let absolute_path = self.path_convert_to_absolute(&path);
        // free the target descriptor first (dup2 closes it if open)
        if self.is_open(new_fd) {
            let _ = self.close(new_fd);
        }
        if self.is_open(new_fd) {
            let _ = self.close(new_fd);
        }
        let mount_name = {
            let (mount_point, translated_path) = match self.resolve_mount(&absolute_path) {
                Some(pair) => pair,
                None => return false,
            };
            if mount_point
                .file_system
                .open(&translated_path, flags, new_fd)
                .is_err()
            {
                return false;
            }
            mount_point.mount_point.clone()
        };
        self.file_data.insert(
            new_fd,
            MountFsFileData {
                file_path: absolute_path,
                mount_point: mount_name,
                file_status_flags: 0,
            },
        );
        true
    }

    /// Allocate the next free descriptor and duplicate `old_fd` onto it. Returns
    /// the new descriptor, or `None` if `old_fd` cannot be duplicated.
    pub fn dup(&mut self, old_fd: i32) -> Option<i32> {
        if !self.is_open(old_fd) {
            return None;
        }
        let new_fd = self.get_unique_fd();
        if self.dup2(old_fd, new_fd) {
            Some(new_fd)
        } else {
            None
        }
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

    pub fn symlink(&mut self, target: &str, link_path: &str) -> Result<(), OpenFileError> {
        if let Some((mount_point, file_path)) = self.get_mount_point_from_filepath_mut(link_path) {
            mount_point.file_system.symlink(target, &file_path)
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
        let mut mount_name = String::new();
        let mut file_status_flags = 0;

        if let Some(file_data) = self.file_data.get(&fd) {
            file_path = file_data.file_path.clone();
            mount_name = file_data.mount_point.clone();
            file_status_flags = file_data.file_status_flags;
        }

        // pull the (owned) file details out and drop the mutable borrow of the
        // mount point before calling the inode lookup below
        let file_details = self
            .get_mount_point_mut(fd)
            .and_then(|mount_point| mount_point.file_system.get_file_details(fd));

        if let Some(file_details) = file_details {
            let inode = self.get_inode_for_filepath(&mount_name, &file_path);
            Some(FileInfo {
                file_details,
                file_path,
                inode,
                file_status_flags,
            })
        } else {
            None
        }
    }

    pub fn get_file_info_from_filepath(&mut self, file_path: &str) -> Option<FileInfo> {
        let absolute_path = self.path_convert_to_absolute(file_path);

        // stat by path - no need (or side effect) of opening the file
        let mount_and_details =
            self.resolve_mount(&absolute_path)
                .and_then(|(mount_point, translated_path)| {
                    mount_point
                        .file_system
                        .get_file_details_for_path(&translated_path)
                        .map(|details| (mount_point.mount_point.clone(), details))
                });

        if let Some((mount_name, file_details)) = mount_and_details {
            let inode = self.get_inode_for_filepath(&mount_name, &absolute_path);
            Some(FileInfo {
                file_details,
                file_path: absolute_path,
                inode,
                file_status_flags: 0,
            })
        } else {
            None
        }
    }

    pub fn set_file_status_flags(&mut self, fd: i32, status_flags: u32) -> Result<(), ()> {
        if let Some(file_data) = self.file_data.get_mut(&fd) {
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
                    Ok(0) => {
                        // a 0-byte result (e.g. the file shrank between get_length and
                        // the read) would spin this loop forever - treat it as an error
                        return Err(());
                    }
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
                        Ok(0) => {
                            // a 0-byte result would spin this loop forever - treat it
                            // as an error
                            return Err(());
                        }
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
        unicorn: &mut Unicorn<'_, Context>,
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

    fn get_inode_for_filepath(&mut self, mount_point: &str, file_path: &str) -> u64 {
        let next_inode = self.inodes.len() as u64 + 1;
        let entry = self
            .inodes
            .entry((mount_point.to_string(), file_path.to_string()))
            .or_insert(next_inode);
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
