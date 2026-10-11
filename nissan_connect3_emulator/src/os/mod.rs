pub mod code_stub;
pub mod dev;
pub mod file_system;
pub(crate) mod osal_queue;
pub(crate) mod syscalls;

use crate::emulator::context::Context;
pub use crate::libs::libosal_linux::libosal_add_code_hooks;
use crate::libs::dapi::dapiapp_add_code_hooks;
use crate::libs::gl_stub::{libegl_add_code_hooks, libgles2_add_code_hooks};
use crate::libs::libsvg_resource::libsvg_resource_add_code_hooks;
use crate::libs::libtrace::libtrace_add_code_hooks;
use crate::libs::procmapengine::procmapengine_add_code_hooks;
use crate::libs::procnav::procnav_add_code_hooks;
use crate::libs::prochmi::prochmi_add_code_hooks;
pub use syscalls::hook_syscall::hook_syscall;
pub use syscalls::sys_calls_state::SysCallsState;
use unicorn_engine::Unicorn;

/// Optional write-watch on absolute guest addresses: `EMU_WATCH=0x0122f598,0x...`.
/// Answers the question a fault dump cannot: did the code that is supposed to initialize
/// this word ever run, and from which instruction did it last get written?
fn install_memory_watch(unicorn: &mut Unicorn<'_, Context>) {
    use std::collections::HashSet;
    use std::sync::{LazyLock, Mutex};
    use unicorn_engine::unicorn_const::HookType;
    use unicorn_engine::RegisterARM;

    static WATCHED: LazyLock<Mutex<HashSet<(String, u32)>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));
    static HITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    let Ok(spec) = std::env::var("EMU_WATCH") else {
        return;
    };
    let elf_path = unicorn.get_data().elf_path.clone();
    let mut seen = WATCHED.lock().unwrap();
    for item in spec.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let Ok(addr) = u32::from_str_radix(item.trim_start_matches("0x"), 16) else {
            log::warn!("WATCH: cannot parse {item}");
            continue;
        };
        if !seen.insert((elf_path.clone(), addr)) {
            continue;
        }
        let result = unicorn.add_mem_hook(
            HookType::MEM_WRITE,
            addr as u64,
            (addr + 3) as u64,
            move |uc, _type, address, size, value| {
                if HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 200 {
                    log::warn!(
                        "WATCH [{}] {} write to {:#x} size={} value={:#x} pc={:#x} lr={:#x} sp={:#x}",
                        uc.get_data().inner.thread_id(),
                        uc.get_data().elf_path,
                        address,
                        size,
                        value,
                        uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32,
                        uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                        uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32,
                    );
                }
                true
            },
        );
        log::info!(
            "WATCH: write hook on {:#x} for {}: {}",
            addr,
            elf_path,
            result.is_ok()
        );
    }
}

pub fn add_library_hook(unicorn: &mut Unicorn<'_, Context>, library: &str, base_address: u32) {
    install_memory_watch(unicorn);
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
        "/opt/bosch/processes/DAPIAPP.OUT" => dapiapp_add_code_hooks(unicorn, base_address),
        "/opt/bosch/navbin/PROCNAV.OUT" => procnav_add_code_hooks(unicorn, base_address),
        _ => return,
    }

    log::info!(
        "[{}] Added library hooks for {} at base address {:#x}.",
        unicorn.get_data().inner.thread_id(),
        library,
        base_address
    );
}
