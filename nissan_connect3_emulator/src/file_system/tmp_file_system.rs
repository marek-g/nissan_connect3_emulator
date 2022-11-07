use crate::emulator::context::Context;
use crate::file_system::{
    CloseFileError, FileDetails, FileSystem, FileSystemType, FileType, OpenFileError, OpenFileFlags,
};
use std::collections::{HashMap, HashSet};
use std::io::SeekFrom;
use std::sync::{Arc, Mutex};
use unicorn_engine::Unicorn;

///
/// File system that stores files in memory.
///
pub struct TmpFileSystem {
    // all paths (to directories and files) should not end on "/"
    // except single one - root ("/")
    files: HashMap<String, Arc<Mutex<TmpFsFileData>>>,
    opened_files: HashMap<i32, TmpFsOpenedFileData>,
}

struct TmpFsFileData {
    pub file_type: FileType,
    pub data: Vec<u8>,
}

struct TmpFsOpenedFileData {
    pub file_data: Arc<Mutex<TmpFsFileData>>,
    pub flags: OpenFileFlags,
    pub pos: usize,
}

impl TmpFileSystem {
    pub fn new() -> Self {
        let mut tmp_fs = Self {
            files: HashMap::new(),
            opened_files: HashMap::new(),
        };
        tmp_fs.insert_entry("/", FileType::Directory, vec![]);
        tmp_fs
    }

    pub fn insert_entry(&mut self, path: &str, file_type: FileType, data: Vec<u8>) {
        self.files.insert(
            path.to_string(),
            Arc::new(Mutex::new(TmpFsFileData { file_type, data })),
        );
    }
}

impl FileSystem for TmpFileSystem {
    fn support_file_paths(&self) -> bool {
        true
    }

    fn file_system_type(&self) -> FileSystemType {
        FileSystemType::Temp
    }

    fn exists(&mut self, file_path: &str) -> bool {
        self.files.contains_key(file_path)
    }

    fn mkdir(&mut self, _file_path: &str, _mode: u32) -> Result<(), OpenFileError> {
        Err(OpenFileError::NoPermission)
    }

    fn read_dir(&mut self, dir_path: &str) -> Result<Vec<String>, ()> {
        let mut dir_path = dir_path.to_string();
        if dir_path != "/" && dir_path.ends_with("/") {
            dir_path.truncate(dir_path.len() - 1);
        }

        if let Some(folder) = self.files.get(&dir_path) {
            if folder.lock().unwrap().file_type != FileType::Directory {
                // this is not a directory
                return Err(());
            }
        } else {
            // there is no such folder
            return Err(());
        }

        let mut folders = HashSet::new();
        for key in self.files.keys() {
            if key.starts_with(&dir_path) {
                let folder = &key[dir_path.len()..];
                if folder.len() > 0 {
                    let folder = folder.split("/").next().unwrap();
                    folders.insert(folder);
                }
            }
        }

        Ok(folders.iter().map(|f| f.to_string()).collect())
    }

    fn open(
        &mut self,
        file_path: &str,
        flags: OpenFileFlags,
        fd: i32,
    ) -> Result<(), OpenFileError> {
        if flags.contains(OpenFileFlags::CREATE) {
            if self.files.contains_key(file_path) {
                if flags.contains(OpenFileFlags::EXCLUSIVE) {
                    return Err(OpenFileError::FileExists);
                }
            } else {
                self.files.insert(
                    file_path.to_string(),
                    Arc::new(Mutex::new(TmpFsFileData {
                        file_type: FileType::File,
                        data: vec![],
                    })),
                );
            }
        } else {
            // file or directory must exists
            if !self.files.contains_key(file_path) {
                return Err(OpenFileError::NoSuchFileOrDirectory);
            }
        }

        if flags.contains(OpenFileFlags::TRUNC) && flags.contains(OpenFileFlags::WRITE) {
            self.files
                .get_mut(file_path)
                .unwrap()
                .lock()
                .unwrap()
                .data
                .clear();
        }

        let file_data = self.files.get_mut(file_path).unwrap();
        let length = file_data.lock().unwrap().data.len();
        let pos = if flags.contains(OpenFileFlags::APPEND) {
            length
        } else {
            0
        };

        self.opened_files.insert(
            fd,
            TmpFsOpenedFileData {
                file_data: file_data.clone(),
                flags,
                pos,
            },
        );

        return Ok(());
    }

    fn close(&mut self, fd: i32) -> Result<(), CloseFileError> {
        match self.opened_files.remove(&fd) {
            None => Err(CloseFileError::FileNotOpened),
            Some(_) => Ok(()),
        }
    }

    fn link(&mut self, old_path: &str, new_path: &str) -> Result<(), OpenFileError> {
        if let Some(file_data) = self.files.get(old_path) {
            self.files.insert(new_path.to_string(), file_data.clone());
            Ok(())
        } else {
            Err(OpenFileError::NoSuchFileOrDirectory)
        }
    }

    fn unlink(&mut self, file_path: &str) -> Result<(), OpenFileError> {
        self.files
            .remove(file_path)
            .map(|_file_data| ())
            .ok_or(OpenFileError::NoSuchFileOrDirectory)
    }

    fn get_file_details(&mut self, fd: i32) -> Option<FileDetails> {
        if let Some(opened_file) = self.opened_files.get_mut(&fd) {
            let file_data = opened_file.file_data.lock().unwrap();
            return Some(FileDetails {
                file_type: file_data.file_type.clone(),
                is_readonly: false,
                length: file_data.data.len() as u64,
            });
        }
        return None;
    }

    fn is_open(&self, fd: i32) -> bool {
        self.opened_files.contains_key(&fd)
    }

    fn get_length(&mut self, fd: i32) -> u64 {
        if let Some(opened_file) = self.opened_files.get(&fd) {
            return opened_file.file_data.lock().unwrap().data.len() as u64;
        }
        return 0;
    }

    fn stream_position(&mut self, fd: i32) -> Result<u64, ()> {
        if let Some(opened_file) = self.opened_files.get(&fd) {
            Ok(opened_file.pos as u64)
        } else {
            Err(())
        }
    }

    fn seek(&mut self, fd: i32, pos: SeekFrom) -> Result<u64, ()> {
        if let Some(opened_file) = self.opened_files.get_mut(&fd) {
            match pos {
                SeekFrom::Start(pos) => {
                    if pos <= opened_file.file_data.lock().unwrap().data.len() as u64 {
                        opened_file.pos = pos as usize;
                        return Ok(opened_file.pos as u64);
                    }
                }
                SeekFrom::End(offset) => {
                    if offset <= 0
                        && opened_file.file_data.lock().unwrap().data.len() as i64 + offset >= 0
                    {
                        opened_file.pos = (opened_file.file_data.lock().unwrap().data.len() as i64
                            + offset) as usize;
                        return Ok(opened_file.pos as u64);
                    }
                }
                SeekFrom::Current(offset) => {
                    if opened_file.pos as i64 + offset >= 0
                        && opened_file.pos as i64 + offset
                            <= opened_file.file_data.lock().unwrap().data.len() as i64
                    {
                        opened_file.pos = (opened_file.pos as i64 + offset) as usize;
                        return Ok(opened_file.pos as u64);
                    }
                }
            }
        }
        return Err(());
    }

    fn read(&mut self, fd: i32, content: &mut [u8]) -> Result<u64, ()> {
        if let Some(opened_file) = self.opened_files.get_mut(&fd) {
            if !opened_file.flags.contains(OpenFileFlags::READ) {
                return Err(());
            }

            let bytes_to_read = (opened_file.file_data.lock().unwrap().data.len()
                - opened_file.pos)
                .min(content.len());
            content[0..bytes_to_read].copy_from_slice(
                &opened_file.file_data.lock().unwrap().data
                    [opened_file.pos..opened_file.pos + bytes_to_read],
            );
            opened_file.pos += bytes_to_read;
            return Ok(bytes_to_read as u64);
        }
        return Err(());
    }

    fn write(&mut self, fd: i32, content: &[u8]) -> Result<u64, ()> {
        if let Some(opened_file) = self.opened_files.get_mut(&fd) {
            if !opened_file.flags.contains(OpenFileFlags::WRITE) {
                return Err(());
            }

            let bytes_to_override = (opened_file.file_data.lock().unwrap().data.len()
                - opened_file.pos)
                .min(content.len());
            opened_file.file_data.lock().unwrap().data
                [opened_file.pos..opened_file.pos + bytes_to_override]
                .copy_from_slice(&content[0..bytes_to_override]);
            let bytes_to_append = content.len() - bytes_to_override;
            opened_file
                .file_data
                .lock()
                .unwrap()
                .data
                .extend_from_slice(
                    &content[bytes_to_override..bytes_to_override + bytes_to_append],
                );
            opened_file.pos += content.len();
            return Ok(content.len() as u64);
        }
        return Err(());
    }

    fn truncate(&mut self, fd: i32, length: u32) -> Result<(), ()> {
        if let Some(opened_file) = self.opened_files.get_mut(&fd) {
            if !opened_file.flags.contains(OpenFileFlags::WRITE) {
                return Err(());
            }

            opened_file
                .file_data
                .lock()
                .unwrap()
                .data
                .resize(length as usize, 0u8);

            Ok(())
        } else {
            Err(())
        }
    }

    fn ioctl(
        &mut self,
        _unicorn: &mut Unicorn<Context>,
        _fd: i32,
        _request: u32,
        _addr: u32,
    ) -> i32 {
        todo!()
    }
}
