use crate::emulator::context::Context;
use crate::file_system::file_info::FileDetails;
use bitflags::bitflags;
use std::io::SeekFrom;
use unicorn_engine::Unicorn;

bitflags! {
    pub struct OpenFileFlags: u32 {
        const NONE = 0x00000000;
        const READ = 0x00000001;
        const WRITE = 0x00000002;
        const CREATE = 0x00000004;
        const EXCLUSIVE = 0x00000008;
        const TRUNC = 0x00000010;
        const APPEND = 0x00000020;
        const DIRECTORY = 0x00000040;
        const TEMP_FILE = 0x00000080;
        const NO_FOLLOW = 0x00000100;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpenFileError {
    FileSystemNotMounted,
    NoSuchFileOrDirectory,
    FileExists,
    NoPermission,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CloseFileError {
    FileNotOpened,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileSystemType {
    Normal,
    Dev,
    Proc,
    Temp,
    Stream,
}

pub trait FileSystem {
    fn support_file_paths(&self) -> bool;

    fn file_system_type(&self) -> FileSystemType;

    fn exists(&mut self, file_path: &str) -> bool;

    fn mkdir(&mut self, file_path: &str, mode: u32) -> Result<(), OpenFileError>;

    fn read_dir(&mut self, dir_path: &str) -> Result<Vec<String>, ()>;

    /// Open file from specified path and assign it with the provided `fd` file descriptor id.
    fn open(&mut self, file_path: &str, flags: OpenFileFlags, fd: i32)
        -> Result<(), OpenFileError>;

    /// Close the file.
    fn close(&mut self, fd: i32) -> Result<(), CloseFileError>;

    fn link(&mut self, old_path: &str, new_path: &str) -> Result<(), OpenFileError>;

    fn unlink(&mut self, file_path: &str) -> Result<(), OpenFileError>;

    fn get_file_details(&mut self, fd: i32) -> Option<FileDetails>;

    fn is_open(&self, fd: i32) -> bool;

    fn get_length(&mut self, fd: i32) -> u64;

    fn stream_position(&mut self, fd: i32) -> Result<u64, ()>;

    fn seek(&mut self, fd: i32, pos: SeekFrom) -> Result<u64, ()>;

    fn read(&mut self, fd: i32, content: &mut [u8]) -> Result<u64, ()>;

    fn write(&mut self, fd: i32, content: &[u8]) -> Result<u64, ()>;

    fn truncate(&mut self, fd: i32, length: u32) -> Result<(), ()>;

    fn ioctl(&mut self, unicorn: &mut Unicorn<Context>, fd: i32, request: u32, addr: u32) -> i32;
}
