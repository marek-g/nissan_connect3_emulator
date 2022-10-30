use crate::file_system::OpenFileError;

pub mod hook_syscall;
pub mod sys_calls_state;

mod fcntl;
mod futex;
mod ioctl;
mod linux;
mod mman;
mod prctl;
mod resource;
mod sched;
mod signal;
mod socket;
mod stat;
mod time;
mod uio;
mod unistd;
mod utsname;

trait SysCallError {
    fn to_syscall_error(self) -> u32;
}

impl SysCallError for OpenFileError {
    fn to_syscall_error(self) -> u32 {
        match self {
            OpenFileError::FileSystemNotMounted => -2i32 as u32, // -EXDEV
            OpenFileError::NoSuchFileOrDirectory => -2i32 as u32, // -ENOENT
            OpenFileError::FileExists => -17i32 as u32,          // -EEXIST
            OpenFileError::NoPermission => -1i32 as u32,         // -EPERM
        }
    }
}
