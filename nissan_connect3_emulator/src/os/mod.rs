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

macro_rules! add_code_hook {
    ($unicorn:ident, $lib:literal, $address:expr, $func:ident) => {
        $unicorn
            .add_code_hook($address as u64, $address as u64, |uc, addr, _| {
                log::trace!(
                    "{:#x}: [{}] [{} HOOK] {}() [IN]",
                    uc.reg_read(RegisterARM::PC).unwrap(),
                    uc.get_data().inner.thread_id,
                    $lib,
                    stringify!($func)
                );
                let res = $func(uc);
                log::trace!(
                    "{:#x}: [{}] [{} HOOK] {}() => {}",
                    uc.reg_read(RegisterARM::PC).unwrap(),
                    uc.get_data().inner.thread_id,
                    $lib,
                    stringify!($func),
                    res
                );
                uc.reg_write(RegisterARM::R0, res as u64).unwrap();
                uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap())
                    .unwrap();
            })
            .unwrap();
    };
}

pub(crate) use add_code_hook;

pub fn add_library_hook(unicorn: &mut Unicorn<Context>, library: &str, base_address: u32) {
    match library {
        "/usr/lib/libtrace.so" => libtrace_add_code_hooks(unicorn, base_address),
        "/opt/bosch/processes/libosal_linux_so.so" => libosal_add_code_hooks(unicorn, base_address),
        _ => return,
    }

    log::info!(
        "[{}] Added library hooks for {} at base address {:#x}.",
        unicorn.get_data().inner.thread_id,
        library,
        base_address
    );
}
