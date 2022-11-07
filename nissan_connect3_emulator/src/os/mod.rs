mod libosal_linux;
mod syscalls;

pub use libosal_linux::libosal_add_code_hooks;
pub use syscalls::hook_syscall::hook_syscall;
pub use syscalls::sys_calls_state::SysCallsState;
