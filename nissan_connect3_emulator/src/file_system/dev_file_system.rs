use crate::emulator::context::Context;
use crate::file_system::file_info::FileDetails;
use crate::file_system::{
    CloseFileError, FileSystem, FileSystemType, FileType, OpenFileError, OpenFileFlags,
    TmpFileSystem,
};
use std::io::SeekFrom;
use unicorn_engine::Unicorn;

///
/// Dev file system.
///
pub struct DevFileSystem {
    tmp_fs: TmpFileSystem,
}

impl DevFileSystem {
    pub fn new() -> Self {
        let mut tmp_fs = TmpFileSystem::new();
        tmp_fs.insert_entry(
            "/iosc",
            FileType::File,
            //"rw dualosoff=false".to_string().as_bytes().to_vec(),
            "????????????".to_string().as_bytes().to_vec(),
        );
        tmp_fs.insert_entry(
            "/errmem",
            FileType::File,
            "???????????????".to_string().as_bytes().to_vec(),
        );
        Self { tmp_fs }
    }
}

impl FileSystem for DevFileSystem {
    fn support_file_paths(&self) -> bool {
        true
    }

    fn file_system_type(&self) -> FileSystemType {
        FileSystemType::Dev
    }

    fn exists(&mut self, file_path: &str) -> bool {
        match file_path {
            "/cmdline" => true,
            _ => self.tmp_fs.exists(file_path),
        }
    }

    fn mkdir(&mut self, _file_path: &str, _mode: u32) -> Result<(), OpenFileError> {
        Err(OpenFileError::NoPermission)
    }

    fn read_dir(&mut self, _dir_path: &str) -> Result<Vec<String>, ()> {
        Err(())
    }

    fn open(
        &mut self,
        file_path: &str,
        flags: OpenFileFlags,
        fd: i32,
    ) -> Result<(), OpenFileError> {
        self.tmp_fs.open(file_path, flags, fd)
    }

    fn close(&mut self, fd: i32) -> Result<(), CloseFileError> {
        self.tmp_fs.close(fd)
    }

    fn link(&mut self, _old_path: &str, _new_path: &str) -> Result<(), OpenFileError> {
        Err(OpenFileError::NoPermission)
    }

    fn unlink(&mut self, _file_path: &str) -> Result<(), OpenFileError> {
        Err(OpenFileError::NoPermission)
    }

    fn get_file_details(&mut self, fd: i32) -> Option<FileDetails> {
        self.tmp_fs.get_file_details(fd)
    }

    fn is_open(&self, fd: i32) -> bool {
        self.tmp_fs.is_open(fd)
    }

    fn get_length(&mut self, fd: i32) -> u64 {
        self.tmp_fs.get_length(fd)
    }

    fn stream_position(&mut self, fd: i32) -> Result<u64, ()> {
        self.tmp_fs.stream_position(fd)
    }

    fn seek(&mut self, fd: i32, pos: SeekFrom) -> Result<u64, ()> {
        self.tmp_fs.seek(fd, pos)
    }

    fn read(&mut self, fd: i32, content: &mut [u8]) -> Result<u64, ()> {
        self.tmp_fs.read(fd, content)
    }

    fn write(&mut self, fd: i32, content: &[u8]) -> Result<u64, ()> {
        self.tmp_fs.write(fd, content)
    }

    fn truncate(&mut self, _fd: i32, _length: u32) -> Result<(), ()> {
        Err(())
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
