pub mod code_stub;
pub mod dev;
pub mod file_system;
pub(crate) mod osal_queue;
pub(crate) mod syscalls;

use crate::emulator::context::Context;
pub use crate::libs::libosal_linux::libosal_add_code_hooks;
use crate::libs::gl_stub::{libegl_add_code_hooks, libgles2_add_code_hooks};
use crate::libs::libsvg_resource::libsvg_resource_add_code_hooks;
use crate::libs::libtrace::libtrace_add_code_hooks;
use crate::libs::procmapengine::procmapengine_add_code_hooks;
use crate::libs::prochmi::prochmi_add_code_hooks;
pub use syscalls::hook_syscall::hook_syscall;
pub use syscalls::sys_calls_state::SysCallsState;
use unicorn_engine::Unicorn;

pub fn add_library_hook(unicorn: &mut Unicorn<'_, Context>, library: &str, base_address: u32) {
    // Code hooks change guest behavior (stubbed bodies are skipped), so they
    // are enabled by default and can be turned off with EMU_CODE_HOOKS_DISABLED=1.
    if std::env::var("EMU_CODE_HOOKS_DISABLED")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return;
    }

    match library {
        "/usr/lib/libtrace.so" => libtrace_add_code_hooks(unicorn, base_address),
        "/opt/bosch/processes/libosal_linux_so.so" => libosal_add_code_hooks(unicorn, base_address),
        "/usr/lib/libEGL.so" => libegl_add_code_hooks(unicorn, base_address),
        "/usr/lib/libGLESv2.so" => libgles2_add_code_hooks(unicorn, base_address),
        "/usr/lib/libsvg-resource.so" => libsvg_resource_add_code_hooks(unicorn, base_address),
        "/opt/bosch/processes/prochmi_out.out" => prochmi_add_code_hooks(unicorn, base_address),
        "/opt/bosch/processes/procmapengine.out" => {
            procmapengine_add_code_hooks(unicorn, base_address)
        }
        _ => return,
    }

    log::info!(
        "[{}] Added library hooks for {} at base address {:#x}.",
        unicorn.get_data().inner.thread_id(),
        library,
        base_address
    );
}
