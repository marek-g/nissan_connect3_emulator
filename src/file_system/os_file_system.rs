use crate::file_system::file_info::{FileDetails, FileType};
use crate::file_system::file_system::FileSystem;
use crate::file_system::{CloseFileError, FileSystemType, OpenFileError, OpenFileFlags};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

struct OpenedFileData {
    pub file: File,
}

///
/// File system provided by host operating system.
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
        let path = self.path_transform_to_real(file_path);
        path.exists()
    }

    fn read_dir(&mut self, dir_path: &str) -> Result<Vec<String>, ()> {
        let full_path_name = self.path_transform_to_real(&dir_path);

        if full_path_name.is_dir() {
            if let Ok(read_dir) = full_path_name.read_dir() {
                let mut res = Vec::new();
                for entry in read_dir {
                    if let Ok(entry) = entry {
                        res.push(entry.file_name().to_str().unwrap().to_string())
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
        let full_path_name = self.path_transform_to_real(&file_path);

        log::debug!(
            "Opening: {}, flags: {:?}",
            full_path_name.to_str().unwrap(),
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

    fn get_file_details(&mut self, fd: i32) -> Option<FileDetails> {
        if let Some(file) = self.opened_files.get_mut(&fd).map(|el| &mut el.file) {
            let metadata = file.metadata().unwrap();
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

    fn is_open(&self, fd: i32) -> bool {
        self.opened_files.contains_key(&fd)
    }

    fn get_length(&mut self, fd: i32) -> u64 {
        if let Some(file) = self.opened_files.get_mut(&fd).map(|el| &mut el.file) {
            file.metadata().unwrap().len()
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
}

impl OsFileSystem {
    pub fn new(host_path: PathBuf) -> Self {
        Self {
            host_path,
            opened_files: HashMap::new(),
        }
    }

    fn path_transform_to_real(&self, guest_path: &str) -> PathBuf {
        if guest_path.starts_with("/") {
            self.host_path.join(&guest_path[1..])
        } else {
            panic!(
                "Only mount file system handle relative paths: {}!",
                guest_path
            );
        }
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
