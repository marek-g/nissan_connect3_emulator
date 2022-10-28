use crate::emulator::context::Context;
use crate::file_system::{
    CloseFileError, FileDetails, FileSystem, FileSystemType, FileType, OpenFileError, OpenFileFlags,
};
use std::collections::{HashMap, HashSet};
use std::io::SeekFrom;
use unicorn_engine::Unicorn;

///
/// File system that stores files in memory.
///
pub struct TmpFileSystem {
    // all paths (to directories and files) should not end on "/"
    // except single one - root ("/")
    files: HashMap<String, TmpFsFileData>,
    opened_files: HashMap<i32, TmpFsOpenedFileData>,
}

struct TmpFsFileData {
    pub file_type: FileType,
    pub data: Vec<u8>,
}

struct TmpFsOpenedFileData {
    pub path: String,
}

impl TmpFileSystem {
    pub fn new() -> Self {
        let mut files = HashMap::new();
        files.insert(
            "/".to_string(),
            TmpFsFileData {
                file_type: FileType::Directory,
                data: vec![],
            },
        );
        Self {
            files,
            opened_files: HashMap::new(),
        }
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

    fn read_dir(&mut self, dir_path: &str) -> Result<Vec<String>, ()> {
        let mut dir_path = dir_path.to_string();
        if dir_path != "/" && dir_path.ends_with("/") {
            dir_path.truncate(dir_path.len() - 1);
        }

        if let Some(folder) = self.files.get(&dir_path) {
            if folder.file_type != FileType::Directory {
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
        if !flags.contains(OpenFileFlags::CREATE) {
            // file or directory must exists
            if !self.files.contains_key(file_path) {
                return Err(OpenFileError::NoSuchFileOrDirectory);
            }

            self.opened_files.insert(
                fd,
                TmpFsOpenedFileData {
                    path: file_path.to_string(),
                },
            );
        } else {
            panic!("not implemented!");
        }

        return Ok(());
    }

    fn close(&mut self, fd: i32) -> Result<(), CloseFileError> {
        match self.opened_files.remove(&fd) {
            None => Err(CloseFileError::FileNotOpened),
            Some(_) => Ok(()),
        }
    }

    fn get_file_details(&mut self, fd: i32) -> Option<FileDetails> {
        todo!()
    }

    fn is_open(&self, fd: i32) -> bool {
        self.opened_files.contains_key(&fd)
    }

    fn get_length(&mut self, fd: i32) -> u64 {
        todo!()
    }

    fn stream_position(&mut self, fd: i32) -> Result<u64, ()> {
        todo!()
    }

    fn seek(&mut self, fd: i32, pos: SeekFrom) -> Result<u64, ()> {
        todo!()
    }

    fn read(&mut self, fd: i32, content: &mut [u8]) -> Result<u64, ()> {
        todo!()
    }

    fn write(&mut self, fd: i32, content: &[u8]) -> Result<u64, ()> {
        todo!()
    }

    fn ioctl(&mut self, unicorn: &mut Unicorn<Context>, fd: i32, request: u32, addr: u32) -> i32 {
        todo!()
    }
}
