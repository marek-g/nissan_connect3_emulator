use crate::emulator::context::Context;
use crate::os::file_system::file_info::{FileDetails, FileType};
use crate::os::file_system::file_system::FileSystem;
use crate::os::file_system::{CloseFileError, FileSystemType, OpenFileError, OpenFileFlags};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use unicorn_engine::Unicorn;

struct OpenedFileData {
    pub file: File,
}

///
/// File system that gives access to files hosted on your system.
///
pub struct OsFileSystem {
    host_path: PathBuf,
    opened_files: HashMap<i32, OpenedFileData>,
}

impl FileSystem for OsFileSystem {
    fn support_file_paths(&self) -> bool {
        true
    }

    fn file_system_type(&self) -> FileSystemType {
        FileSystemType::Normal
    }

    fn exists(&mut self, file_path: &str) -> bool {
        if let Some(path) = self.path_transform_to_real(file_path) {
            path.exists()
        } else {
            false
        }
    }

    fn mkdir(&mut self, _file_path: &str, _mode: u32) -> Result<(), OpenFileError> {
        Err(OpenFileError::NoPermission)
    }

    fn read_dir(&mut self, dir_path: &str) -> Result<Vec<String>, ()> {
        let full_path_name = match self.path_transform_to_real(&dir_path) {
            Some(path) => path,
            None => return Err(()),
        };

        if full_path_name.is_dir() {
            if let Ok(read_dir) = full_path_name.read_dir() {
                let mut res = Vec::new();
                for entry in read_dir {
                    if let Ok(entry) = entry {
                        // skip non-UTF8 host file names instead of panicking
                        match entry.file_name().to_str() {
                            Some(name) => res.push(name.to_string()),
                            None => log::warn!("skipping non-UTF8 file name in {}", dir_path),
                        }
                    } else {
                        return Err(());
                    }
                }
                Ok(res)
            } else {
                Err(())
            }
        } else {
            Err(())
        }
    }

    fn open(
        &mut self,
        file_path: &str,
        flags: OpenFileFlags,
        fd: i32,
    ) -> Result<(), OpenFileError> {
        let full_path_name = match self.path_transform_to_real(&file_path) {
            Some(path) => path,
            None => return Err(OpenFileError::NoSuchFileOrDirectory),
        };

        log::debug!(
            "Opening: {}, flags: {:?}",
            full_path_name.display(),
            flags
        );

        let open_options = self.get_open_options(flags);

        if let Ok(file) = open_options.open(full_path_name) {
            let opened_file_data = OpenedFileData { file };
            self.opened_files.insert(fd, opened_file_data);
            Ok(())
        } else {
            Err(OpenFileError::NoSuchFileOrDirectory)
        }
    }

    fn close(&mut self, fd: i32) -> Result<(), CloseFileError> {
        if let Some(_) = self.opened_files.remove(&fd) {
            Ok(())
        } else {
            Err(CloseFileError::FileNotOpened)
        }
    }

    fn read_link(&mut self, file_path: &str) -> Option<String> {
        let real = self.path_transform_to_real(file_path)?;
        // read_link (unlike metadata) does not follow the link, so it also works
        // for a link whose absolute target only exists in the guest mount tree
        std::fs::read_link(real)
            .ok()
            .map(|target| target.to_string_lossy().into_owned())
    }

    fn link(&mut self, _old_path: &str, _new_path: &str) -> Result<(), OpenFileError> {
        Err(OpenFileError::NoPermission)
    }

    fn unlink(&mut self, _file_path: &str) -> Result<(), OpenFileError> {
        Err(OpenFileError::NoPermission)
    }

    fn get_file_details(&mut self, fd: i32) -> Option<FileDetails> {
        if let Some(file) = self.opened_files.get_mut(&fd).map(|el| &mut el.file) {
            // the host file may have vanished between open and stat - do not panic
            let metadata = file.metadata().ok()?;
            Some(FileDetails {
                file_type: if metadata.is_dir() {
                    FileType::Directory
                } else if metadata.is_symlink() {
                    FileType::Link
                } else {
                    FileType::File
                },
                is_readonly: metadata.permissions().readonly(),
                length: metadata.len(),
            })
        } else {
            None
        }
    }

    fn get_file_details_for_path(&mut self, file_path: &str) -> Option<FileDetails> {
        let full_path_name = self.path_transform_to_real(file_path)?;
        let metadata = std::fs::metadata(full_path_name).ok()?;
        Some(FileDetails {
            file_type: if metadata.is_dir() {
                FileType::Directory
            } else if metadata.is_symlink() {
                FileType::Link
            } else {
                FileType::File
            },
            is_readonly: metadata.permissions().readonly(),
            length: metadata.len(),
        })
    }

    fn is_open(&self, fd: i32) -> bool {
        self.opened_files.contains_key(&fd)
    }

    fn get_length(&mut self, fd: i32) -> u64 {
        if let Some(file) = self.opened_files.get_mut(&fd).map(|el| &mut el.file) {
            // the host file may have vanished between open and stat - do not panic
            file.metadata().map(|metadata| metadata.len()).unwrap_or(0)
        } else {
            0
        }
    }

    fn stream_position(&mut self, fd: i32) -> Result<u64, ()> {
        if let Some(file) = self.opened_files.get_mut(&fd).map(|el| &mut el.file) {
            file.stream_position().map_err(|_| ())
        } else {
            Err(())
        }
    }

    fn seek(&mut self, fd: i32, pos: SeekFrom) -> Result<u64, ()> {
        if let Some(file) = self.opened_files.get_mut(&fd).map(|el| &mut el.file) {
            file.seek(pos).map_err(|_| ())
        } else {
            Err(())
        }
    }

    fn read(&mut self, fd: i32, content: &mut [u8]) -> Result<u64, ()> {
        if let Some(file) = self.opened_files.get_mut(&fd).map(|el| &mut el.file) {
            file.read(content).map(|s| s as u64).map_err(|_| ())
        } else {
            Err(())
        }
    }

    fn write(&mut self, fd: i32, content: &[u8]) -> Result<u64, ()> {
        if let Some(file_info) = self.opened_files.get_mut(&fd) {
            file_info
                .file
                .write(content)
                .map(|s| s as u64)
                .map_err(|_| ())
        } else {
            Err(())
        }
    }

    fn truncate(&mut self, _fd: i32, _length: u32) -> Result<(), ()> {
        Err(())
    }

    fn ioctl(
        &mut self,
        _unicorn: &mut Unicorn<'_, Context>,
        _fd: i32,
        _request: u32,
        _addr: u32,
    ) -> i32 {
        -1i32
    }
}

impl OsFileSystem {
    pub fn new(host_path: PathBuf) -> Self {
        Self {
            host_path: normalize_path(&host_path),
            opened_files: HashMap::new(),
        }
    }

    /// Translate a guest path to the corresponding host path. Returns None for
    /// relative paths (only the mount file system resolves those) and for
    /// paths that would escape the mounted host root.
    fn path_transform_to_real(&self, guest_path: &str) -> Option<PathBuf> {
        if !guest_path.starts_with("/") {
            log::warn!(
                "Only mount file system handles relative paths - refusing: {}",
                guest_path
            );
            return None;
        }

        let joined = self.host_path.join(&guest_path[1..]);
        // defensively check that the result stays under host_path (a path with
        // '..' components would otherwise escape the mounted root)
        let normalized = normalize_path(&joined);
        if !normalized.starts_with(&self.host_path) {
            log::warn!("guest path escapes the host root: {}", guest_path);
            return None;
        }

        Some(normalized)
    }

    fn get_open_options(&self, flags: OpenFileFlags) -> OpenOptions {
        let mut open_options = OpenOptions::new();

        open_options.read(flags.contains(OpenFileFlags::READ));
        open_options.write(flags.contains(OpenFileFlags::WRITE));
        open_options.append(flags.contains(OpenFileFlags::APPEND));
        open_options.create(
            flags.contains(OpenFileFlags::CREATE) && !flags.contains(OpenFileFlags::EXCLUSIVE),
        );
        open_options.create_new(
            flags.contains(OpenFileFlags::CREATE) && flags.contains(OpenFileFlags::EXCLUSIVE),
        );
        open_options.truncate(flags.contains(OpenFileFlags::TRUNC));

        open_options
    }
}

/// Lexically remove `.` and `..` components (no symlink resolution).
fn normalize_path(path: &std::path::Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                result.pop();
            }
            _ => result.push(component.as_os_str()),
        }
    }
    result
}
