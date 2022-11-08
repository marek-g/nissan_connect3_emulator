mod libosal_linux;
mod libtrace;
mod syscalls;

use crate::emulator::context::Context;
use crate::os::libtrace::libtrace_add_code_hooks;
pub use libosal_linux::libosal_add_code_hooks;
pub use syscalls::hook_syscall::hook_syscall;
pub use syscalls::sys_calls_state::SysCallsState;
use unicorn_engine::unicorn_const::Permission;
use unicorn_engine::Unicorn;

pub fn add_library_hook(unicorn: &mut Unicorn<Context>, library: &str, base_address: u32) {
    match library {
        "/usr/lib/libtrace.so" => libtrace_add_code_hooks(unicorn, base_address),
        "/usr/lib/libosal_linux_so.so" => libosal_add_code_hooks(unicorn, base_address),
        _ => return,
    }

    log::info!(
        "[{}] Added library hooks for {} at base address {:#x}.",
        unicorn.get_data().inner.thread_id,
        library,
        base_address
    );
}
