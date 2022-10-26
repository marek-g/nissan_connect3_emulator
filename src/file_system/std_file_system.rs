use crate::file_system::file_info::FileDetails;
use crate::file_system::{CloseFileError, FileSystem, OpenFileError, OpenFileFlags};
use std::io;
use std::io::{Read, SeekFrom, Write};

pub struct StdFileSystem;

impl StdFileSystem {
    pub fn new() -> Self {
        Self {}
    }
}

impl FileSystem for StdFileSystem {
    fn support_file_paths(&self) -> bool {
        false
    }

    fn exists(&mut self, _file_path: &str) -> bool {
        false
    }

    fn open(
        &mut self,
        _file_path: &str,
        _flags: OpenFileFlags,
        fd: i32,
    ) -> Result<(), OpenFileError> {
        if fd >= 0 && fd <= 2 {
            Ok(())
        } else {
            Err(OpenFileError::NoSuchFileOrDirectory)
        }
    }

    fn close(&mut self, fd: i32) -> Result<(), CloseFileError> {
        if fd >= 0 && fd <= 2 {
            Ok(())
        } else {
            Err(CloseFileError::FileNotOpened)
        }
    }

    fn get_file_details(&mut self, _fd: i32) -> Option<FileDetails> {
        None
    }

    fn is_open(&self, fd: i32) -> bool {
        if fd >= 0 && fd <= 2 {
            true
        } else {
            false
        }
    }

    fn get_length(&mut self, _fd: i32) -> u64 {
        0
    }

    fn stream_position(&mut self, _fd: i32) -> Result<u64, ()> {
        Ok(0)
    }

    fn seek(&mut self, _fd: i32, _pos: SeekFrom) -> Result<u64, ()> {
        Ok(0)
    }

    fn read(&mut self, fd: i32, content: &mut [u8]) -> Result<u64, ()> {
        if fd == 0 {
            io::stdin().read(content).map(|s| s as u64).map_err(|_| ())
        } else {
            Err(())
        }
    }

    fn write(&mut self, fd: i32, content: &[u8]) -> Result<u64, ()> {
        if fd == 1 {
            io::stdout()
                .write(content)
                .map(|s| s as u64)
                .map_err(|_| ())
        } else if fd == 2 {
            io::stderr()
                .write(content)
                .map(|s| s as u64)
                .map_err(|_| ())
        } else {
            Err(())
        }
    }
}
