use crate::emulator::context::Context;
use crate::emulator::thread::{BlockReason, ThreadStatus};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};
use unicorn_engine::unicorn_const::Prot;
use unicorn_engine::{RegisterARM, Unicorn};

const ORIGINAL_BASE: u32 = 0x0000_8000;
const B_IS_HMI_THREAD_RUNNING_GOT: u32 = 0x02a1_faa8;
const HMI_THREAD_RUNNING_TRUE_BYTE: u32 = 0x02a1_faa0;
const CL_HMI_MNGR_S_INITIALIZE: u32 = 0x0134_f540 - ORIGINAL_BASE;
const CL_HMI_MNGR_B_EXECUTE: u32 = 0x0134_ec88 - ORIGINAL_BASE;
const CL_GUI_WIDGET_S_INITIALIZE: u32 = 0x0133_cc6c - ORIGINAL_BASE;
const CL_GUI_WIDGET_B_EXECUTE: u32 = 0x0133_c134 - ORIGINAL_BASE;
const CL_GUI_MAINLOOP: u32 = 0x0133_be70 - ORIGINAL_BASE;
const GUI_LUA_LOAD_SCRIPTS: u32 = 0x0136_9350 - ORIGINAL_BASE;
const GUI_LUA_DOFILE: u32 = 0x0136_9228 - ORIGINAL_BASE;
const GUI_STATEMACHINE_CREATE: u32 = 0x0137_31ac - ORIGINAL_BASE;
const GUI_TOUCH_START: u32 = 0x0134_aef0 - ORIGINAL_BASE;
const GUI_DM_S_INITIALIZE: u32 = 0x0133_df68 - ORIGINAL_BASE;
const GUI_DM_IS_DIRTY: u32 = 0x0133_d8ac - ORIGINAL_BASE;
const GUI_DM_IS_VIEW_DIRTY: u32 = 0x0133_d758 - ORIGINAL_BASE;
const GUI_MENU_GET_VIEW: u32 = 0x0136_42c8 - ORIGINAL_BASE;
const GUI_WIDGET_CALL_DRAW: u32 = 0x0137_af7c - ORIGINAL_BASE;
const GUI_DM_UPDATE: u32 = 0x0133_d99c - ORIGINAL_BASE;
const SVG_INIT_RESOURCE: u32 = 0x00fb_6ad0 - ORIGINAL_BASE;
const SVG_CREATE_SURFACE: u32 = 0x00fb_7058 - ORIGINAL_BASE;
const SVG_CREATE_LAYER_CONTEXT: u32 = 0x00fb_68e4 - ORIGINAL_BASE;
const CL_LUA_DEBUGGER_S_INITIALIZE: u32 = 0x0133_ae10 - ORIGINAL_BASE;
const GUI_DISPLAY_WIDTH: u32 = 800;
const GUI_DISPLAY_HEIGHT: u32 = 480;
const TRAMPOLINE_SIZE: usize = 36;

const CL_HMI_MNGR_C1: u32 = 0x0134_f364 - ORIGINAL_BASE;
const CL_HMI_MNGR_C2: u32 = 0x0134_f5a8 - ORIGINAL_BASE;
const CL_GUI_WIDGET_C1: u32 = 0x0133_cb48 - ORIGINAL_BASE;
const CL_GUI_WIDGET_C2: u32 = 0x0133_ccd0 - ORIGINAL_BASE;
const HMI_MAIN_ENTRY: u32 = 0x00fc_8c48 - ORIGINAL_BASE;
const CL_HMI_MNGR_STATE: u32 = 0x670;
const CL_HMI_MNGR_PENDING_STATE: u32 = 0x671;
const CL_GUI_PREV_APP_STATE: u32 = 0x18;
const CL_GUI_PENDING_POWER_STATE: u32 = 0x1c;
const CL_GUI_STARTED: u32 = 0x20;
const GUI_DM_DIRTY: u32 = 0x3c;
const OSAL_EVENT_BITS: u32 = 0x14;
const HMI_FW_LOOP_EVENT_BIT: u32 = 0x4;

static PROCHMI_BASE: AtomicU32 = AtomicU32::new(0);
static HMI_MNGR_POINTER: AtomicU32 = AtomicU32::new(0);
static HMI_GUI_POINTER: AtomicU32 = AtomicU32::new(0);
static HMI_EVENT_OBJECT: AtomicU32 = AtomicU32::new(0);
static HMI_MAIN_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static HMI_GUI_FORCE_LOGGED: AtomicBool = AtomicBool::new(false);
static HMI_EVENT_WAKE_LOGGED: AtomicBool = AtomicBool::new(false);
static HMI_MNGR_MISSING_LOGGED: AtomicBool = AtomicBool::new(false);
static SVG_FAKE_HANDLE: AtomicU32 = AtomicU32::new(0);
static SVG_BYPASS_LOGGED: AtomicBool = AtomicBool::new(false);
static GUI_DISPLAY_MANAGER_POINTER: AtomicU32 = AtomicU32::new(0);
static GUI_DISPLAY_DIRTY_COUNT: AtomicU32 = AtomicU32::new(0);
static GUI_VIEW_DIRTY_FORCE_COUNT: AtomicU32 = AtomicU32::new(0);
static GUI_LAST_VIEW: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static PROCHMI_TICK_NEXT: Cell<Option<Instant>> = const { Cell::new(None) };
}

pub fn prochmi_add_code_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    PROCHMI_BASE.store(base_address, Ordering::Relaxed);
    install_hmi_mngr_initialize_before_v_start_thread(unicorn, base_address);
    force_hmi_thread_running_true(unicorn);

    for offset in [
        SVG_INIT_RESOURCE,
        SVG_CREATE_SURFACE,
        SVG_CREATE_LAYER_CONTEXT,
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let result = if offset == SVG_INIT_RESOURCE {
                    0
                } else {
                    svg_fake_handle(uc)
                };
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0);
                if !SVG_BYPASS_LOGGED.swap(true, Ordering::Relaxed) {
                    log::info!("PROCHMI: bypassing SVG resource functions");
                }
                uc.reg_write(RegisterARM::R0, result as u64).unwrap();
                uc.reg_write(RegisterARM::PC, lr).unwrap();
            })
            .unwrap();
    }

    for (offset, name) in [
        (CL_LUA_DEBUGGER_S_INITIALIZE, "clLuaDebugger::s_initialize trampoline"),
        (CL_HMI_MNGR_S_INITIALIZE, "clHMIMngr::s_initialize"),
        (CL_GUI_WIDGET_S_INITIALIZE, "clGUIWidgetEngine::s_initialize"),
        (GUI_LUA_LOAD_SCRIPTS, "GUI_LUA_Interface::loadScripts"),
        (GUI_STATEMACHINE_CREATE, "GUI_StateMachineFactory::s_vCreateStateMachines"),
        (GUI_TOUCH_START, "GUI_TouchAdapter_vStart"),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |_, _, _| {
                log::info!("PROCHMI: hit {}", name);
            })
            .unwrap();
    }

    for (offset, store) in [
        (CL_HMI_MNGR_C1, "hmi_mngr"),
        (CL_HMI_MNGR_C2, "hmi_mngr"),
        (CL_GUI_WIDGET_C1, "gui_widget"),
        (CL_GUI_WIDGET_C2, "gui_widget"),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let ptr = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                if ptr != 0 {
                    if store == "hmi_mngr" {
                        HMI_MNGR_POINTER.store(ptr, Ordering::Relaxed);
                    } else {
                        HMI_GUI_POINTER.store(ptr, Ordering::Relaxed);
                    }
                    log::debug!("PROCHMI: recorded {} object {:#x}", store, ptr);
                }
            })
            .unwrap();
    }

    let lua_do_file = base_address + GUI_LUA_DOFILE;
    unicorn
        .add_code_hook(lua_do_file as u64, lua_do_file as u64, |uc, _, _| {
            let lua = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let path = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let path_str = read_cstr(uc, path, 128);
            let lua_state = if lua >= 4 {
                read_u32(uc, lua + 4)
            } else {
                0
            };
            log::info!(
                "PROCHMI: GUI_LUA_Interface::doFile lua={:#x} state={:#x} path={}",
                lua,
                lua_state,
                path_str
            );
        })
        .unwrap();

    let gui_mainloop = base_address + CL_GUI_MAINLOOP;
    unicorn
        .add_code_hook(gui_mainloop as u64, gui_mainloop as u64, |uc, _, _| {
            let gui = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let mode = read_u8(uc, gui + 0x3d);
            if mode != 0 {
                write_u8(uc, gui + 0x3d, 0);
            }
            log::info!(
                "PROCHMI: clGUIWidgetEngine::bGUIMainloop this={:#x} display_mode={}",
                gui,
                mode
            );
        })
        .unwrap();

    let dm_is_dirty = base_address + GUI_DM_IS_DIRTY;
    unicorn
        .add_code_hook(dm_is_dirty as u64, dm_is_dirty as u64, |uc, _, _| {
            let dm = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            GUI_DISPLAY_MANAGER_POINTER.store(dm, Ordering::Relaxed);
            let forced = GUI_DISPLAY_DIRTY_COUNT.fetch_add(1, Ordering::Relaxed);
            if dm != 0 && forced < 20 {
                write_u8(uc, dm + GUI_DM_DIRTY, 1);
                log::info!(
                    "PROCHMI: forcing GUI_DM_DisplayManager dirty {} at {:#x}+{:#x}",
                    forced + 1,
                    dm,
                    GUI_DM_DIRTY
                );
            }
        })
        .unwrap();

    let menu_get_view = base_address + GUI_MENU_GET_VIEW;
    unicorn
        .add_code_hook(menu_get_view as u64, menu_get_view as u64, |uc, _, _| {
            log::info!(
                "PROCHMI: GUI_MenuManager::pGetView menu={:#x} index={}",
                uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                uc.reg_read(RegisterARM::R1).unwrap_or(0) as i32
            );
        })
        .unwrap();

    let dm_is_view_dirty = base_address + GUI_DM_IS_VIEW_DIRTY;
    unicorn
        .add_code_hook(dm_is_view_dirty as u64, dm_is_view_dirty as u64, |uc, _, _| {
            let view = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let view_layer = if view == 0 { 0 } else { read_u32(uc, view + 0x40) };
            GUI_LAST_VIEW.store(view_layer, Ordering::Relaxed);
            let forced = GUI_VIEW_DIRTY_FORCE_COUNT.fetch_add(1, Ordering::Relaxed);
            if view != 0 && forced < 20 {
                log::info!(
                    "PROCHMI: forcing GUI_DM_DisplayManager::isViewDirty view={:#x}",
                    view
                );
                uc.reg_write(RegisterARM::R0, 1).unwrap();
                uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap())
                    .unwrap();
            } else if forced < 40 {
                log::info!(
                    "PROCHMI: GUI_DM_DisplayManager::isViewDirty view={:#x}",
                    view
                );
            }
        })
        .unwrap();

    let dm_update = base_address + GUI_DM_UPDATE;
    unicorn
        .add_code_hook(dm_update as u64, dm_update as u64, |uc, _, _| {
            let dm = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            log::info!(
                "PROCHMI: GUI_DM_DisplayManager::update this={:#x} eam={:#x} view0x40={:#x}",
                dm,
                if dm == 0 {
                    0
                } else {
                    read_u32(uc, dm)
                },
                GUI_LAST_VIEW.load(Ordering::Relaxed)
            );
        })
        .unwrap();

    let widget_call_draw = base_address + GUI_WIDGET_CALL_DRAW;
    unicorn
        .add_code_hook(widget_call_draw as u64, widget_call_draw as u64, |uc, _, _| {
            let widget = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            log::info!(
                "PROCHMI: GUI_Widget::callDraw widget={:#x} vtable={:#x} drawctx={:#x}",
                widget,
                if widget == 0 {
                    0
                } else {
                    read_u32(uc, widget)
                },
                if widget == 0 {
                    0
                } else {
                    read_u32(uc, widget + 0x40)
                }
            );
        })
        .unwrap();

    for (offset, store) in [
        (CL_HMI_MNGR_B_EXECUTE, "hmi_mngr"),
        (CL_GUI_WIDGET_B_EXECUTE, "gui_widget"),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let ptr = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                if ptr != 0 {
                    if store == "hmi_mngr" {
                        HMI_MNGR_POINTER.store(ptr, Ordering::Relaxed);
                    } else {
                        HMI_GUI_POINTER.store(ptr, Ordering::Relaxed);
                    }
                }

                if store == "hmi_mngr" {
                    force_hmi_gui_state(uc);
                    log::info!(
                        "PROCHMI: clHMIMngr::bExecute this={:#x} state={} pending={} gui={:#x}",
                        ptr,
                        read_u8(uc, ptr + CL_HMI_MNGR_STATE),
                        read_u8(uc, ptr + CL_HMI_MNGR_PENDING_STATE),
                        HMI_GUI_POINTER.load(Ordering::Relaxed)
                    );
                } else {
                    log::info!(
                        "PROCHMI: clGUIWidgetEngine::bExecute this={:#x} previous={} pending=0x{:08x} started={}",
                        ptr,
                        read_u8(uc, ptr + 0x18),
                        read_u32(uc, ptr + CL_GUI_PENDING_POWER_STATE),
                        read_u8(uc, ptr + CL_GUI_STARTED)
                    );
                }
            })
            .unwrap();
    }

    let hmi_main_entry = base_address + HMI_MAIN_ENTRY;
    unicorn
        .add_code_hook(hmi_main_entry as u64, hmi_main_entry as u64, |uc, _, _| {
            let tid = uc.get_data().thread_id();
            HMI_MAIN_THREAD_ID.store(tid, Ordering::Relaxed);
            log::info!("PROCHMI: recorded HMI_MAIN thread id {}", tid);
        })
        .unwrap();
}

pub fn should_force_hmi_event(unicorn: &Unicorn<'_, Context>, requested_mask: u32) -> bool {
    if requested_mask == 0 {
        return false;
    }

    let hmi = HMI_MNGR_POINTER.load(Ordering::Relaxed);
    if hmi == 0 {
        return false;
    }

    let state = read_u8(unicorn, hmi + CL_HMI_MNGR_STATE);
    if state != 3 {
        return false;
    }

    if read_u8(unicorn, hmi + CL_HMI_MNGR_PENDING_STATE) != 0 {
        return true;
    }

    let gui = HMI_GUI_POINTER.load(Ordering::Relaxed);
    if gui == 0 {
        return true;
    }

    let started = read_u8(unicorn, gui + CL_GUI_STARTED);
    let pending = read_u32(unicorn, gui + CL_GUI_PENDING_POWER_STATE);
    started == 0 || pending != 0
}

pub fn note_hmi_event_object(object: u32) {
    if object != 0 {
        HMI_EVENT_OBJECT.store(object, Ordering::Relaxed);
    }
}

pub fn force_hmi_gui_state(unicorn: &mut Unicorn<'_, Context>) -> bool {
    let hmi = HMI_MNGR_POINTER.load(Ordering::Relaxed);
    if hmi == 0 {
        if !HMI_MNGR_MISSING_LOGGED.swap(true, Ordering::Relaxed) {
            log::info!("PROCHMI: clHMIMngr object pointer is still unknown");
        }
        return false;
    }

    let state = read_u8(unicorn, hmi + CL_HMI_MNGR_STATE);
    if state != 3 {
        let _ = write_u8(unicorn, hmi + CL_HMI_MNGR_STATE, 3);
        let _ = write_u8(unicorn, hmi + CL_HMI_MNGR_PENDING_STATE, 1);
        log::info!(
            "PROCHMI: forced HMI framework app state {} -> NORMAL(3)",
            state
        );
    }

    let gui = HMI_GUI_POINTER.load(Ordering::Relaxed);
    if gui != 0 {
        let started = read_u8(unicorn, gui + CL_GUI_STARTED);
        let pending = read_u32(unicorn, gui + CL_GUI_PENDING_POWER_STATE);
        if started == 0 && (pending >> 24) as u8 != 0x10 {
            let forced = (pending & 0x00ff_ffff) | 0x1000_0000;
            let _ = write_u32(unicorn, gui + CL_GUI_PENDING_POWER_STATE, forced);
            if !HMI_GUI_FORCE_LOGGED.swap(true, Ordering::Relaxed) {
                log::info!(
                    "PROCHMI: forced GUI pending power state 0x{:08x} -> 0x{:08x}",
                    pending,
                    forced
                );
            }
        }
    }

    true
}

pub fn tick(unicorn: &mut Unicorn<'_, Context>) {
    let now = Instant::now();
    let ready = PROCHMI_TICK_NEXT.with(|cell| match cell.get() {
        None => {
            cell.set(Some(now + Duration::from_millis(100)));
            false
        }
        Some(next) => now >= next,
    });
    if !ready {
        return;
    }

    if !force_hmi_gui_state(unicorn) {
        return;
    }
    PROCHMI_TICK_NEXT.with(|cell| cell.set(Some(now + Duration::from_millis(100))));

    let event = HMI_EVENT_OBJECT.load(Ordering::Relaxed);
    if event == 0 {
        log::debug!("PROCHMI: HMI_FW_LOOP event object is still unknown");
        return;
    }

    let bits_addr = event + OSAL_EVENT_BITS;
    let bits = read_u32(unicorn, bits_addr);
    let _ = write_u32(unicorn, bits_addr, bits | HMI_FW_LOOP_EVENT_BIT);

    let tid = HMI_MAIN_THREAD_ID.load(Ordering::Relaxed);
    if tid == 0 {
        return;
    }

    let mut woke = false;
    {
        let mut threads = unicorn.get_data().threads.lock().unwrap();
        if let Some(thread) = threads.iter_mut().find(|t| t.id == tid) {
            if matches!(
                thread.status,
                ThreadStatus::Blocked(BlockReason::FutexWait { .. })
                    | ThreadStatus::Blocked(BlockReason::FutexWaitShared { .. })
            ) {
                thread.status = ThreadStatus::Runnable;
                thread.pending_result = Some(0);
                woke = true;
            }
        }
    }

    if woke && !HMI_EVENT_WAKE_LOGGED.swap(true, Ordering::Relaxed) {
        log::info!("PROCHMI: forced HMI_MAIN event wake for HMI_FW_LOOP");
    }
}

fn svg_fake_handle(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let current = SVG_FAKE_HANDLE.load(Ordering::Relaxed);
    if current != 0 {
        return current;
    }

    let mmu_arc = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    let fake = mmu_arc.lock().unwrap().heap_alloc(
        unicorn,
        0x1000,
        Prot::READ | Prot::WRITE,
        "[svg-fake]",
    );
    SVG_FAKE_HANDLE.store(fake, Ordering::Relaxed);
    log::info!("PROCHMI: allocated SVG fake resource surface {:#x}", fake);
    fake
}

fn install_hmi_mngr_initialize_before_v_start_thread(
    unicorn: &mut Unicorn<'_, Context>,
    base_address: u32,
) {
    let lua_initialize = base_address + CL_LUA_DEBUGGER_S_INITIALIZE;
    let hmi_initialize = base_address + CL_HMI_MNGR_S_INITIALIZE;
    let gui_initialize = base_address + CL_GUI_WIDGET_S_INITIALIZE;
    let display_initialize = base_address + GUI_DM_S_INITIALIZE;

    let mut trampoline_bytes = Vec::with_capacity(TRAMPOLINE_SIZE);
    trampoline_bytes.extend_from_slice(&[0x11, 0x40, 0x2d, 0xe9]); // push {r0, r4, lr}
    trampoline_bytes.extend_from_slice(&arm_bl(lua_initialize + 4, hmi_initialize));
    trampoline_bytes.extend_from_slice(&arm_bl(lua_initialize + 8, gui_initialize));
    trampoline_bytes.extend_from_slice(&arm_bl(
        lua_initialize + 20,
        display_initialize,
    ));
    trampoline_bytes.extend_from_slice(&[0x11, 0x40, 0xbd, 0xe8]); // pop {r0, r4, lr}
    trampoline_bytes.extend_from_slice(&[0x00, 0x00, 0xa0, 0xe3]); // mov r0, #0
    trampoline_bytes.extend_from_slice(&[0x1e, 0xff, 0x2f, 0xe1]); // bx lr
    trampoline_bytes.splice(
        12..12,
        [0x20, 0x00, 0x03, 0xe3]
            .into_iter()
            .chain([0xe0, 0x00, 0x01, 0xe3]),
    );

    assert_eq!(trampoline_bytes.len(), TRAMPOLINE_SIZE);
    unicorn
        .mem_write(lua_initialize as u64, &trampoline_bytes)
        .unwrap();

    log::info!(
        "PROCHMI: clLuaDebugger::s_initialize({:#x}) -> clHMIMngr::s_initialize({:#x}) + clGUIWidgetEngine::s_initialize({:#x}) + GUI_DM_DisplayManager::s_initialize({}x{}) ({:#x}) (debugger disabled)",
        lua_initialize,
        hmi_initialize,
        gui_initialize,
        GUI_DISPLAY_WIDTH,
        GUI_DISPLAY_HEIGHT,
        display_initialize
    );
}

fn force_hmi_thread_running_true(unicorn: &mut Unicorn<'_, Context>) {
    unicorn
        .mem_write(
            B_IS_HMI_THREAD_RUNNING_GOT as u64,
            &HMI_THREAD_RUNNING_TRUE_BYTE.to_le_bytes(),
        )
        .unwrap();

    log::debug!(
        "PROCHMI: bIsHmiThreadRunning GOT slot {:#x} -> true byte {:#x}",
        B_IS_HMI_THREAD_RUNNING_GOT,
        HMI_THREAD_RUNNING_TRUE_BYTE
    );
}

fn arm_bl(from: u32, to: u32) -> [u8; 4] {
    arm_branch(from, to, 0xeb00_0000)
}

fn arm_b(from: u32, to: u32) -> [u8; 4] {
    arm_branch(from, to, 0xea00_0000)
}

fn arm_branch(from: u32, to: u32, opcode: u32) -> [u8; 4] {
    let offset = to
        .wrapping_sub(from + 8)
        .checked_shr(2)
        .expect("ARM branch offset must be word aligned");
    (opcode | (offset & 0x00ff_ffff)).to_le_bytes()
}

fn read_u32(unicorn: &Unicorn<'_, Context>, addr: u32) -> u32 {
    if addr == 0 {
        return 0;
    }
    let mut buf = [0_u8; 4];
    match unicorn.mem_read(addr as u64, &mut buf) {
        Ok(()) => u32::from_le_bytes(buf),
        Err(_) => 0,
    }
}

fn read_u8(unicorn: &Unicorn<'_, Context>, addr: u32) -> u8 {
    if addr == 0 {
        return 0;
    }
    let mut buf = [0_u8; 1];
    match unicorn.mem_read(addr as u64, &mut buf) {
        Ok(()) => buf[0],
        Err(_) => 0,
    }
}

fn write_u32(unicorn: &mut Unicorn<'_, Context>, addr: u32, value: u32) -> bool {
    addr != 0 && unicorn.mem_write(addr as u64, &value.to_le_bytes()).is_ok()
}

fn write_u8(unicorn: &mut Unicorn<'_, Context>, addr: u32, value: u8) -> bool {
    addr != 0 && unicorn.mem_write(addr as u64, &[value]).is_ok()
}

fn read_cstr(unicorn: &Unicorn<'_, Context>, addr: u32, max_len: usize) -> String {
    if addr == 0 {
        return String::new();
    }
    let mut out = Vec::with_capacity(max_len);
    for index in 0..max_len {
        let mut buf = [0_u8; 1];
        match unicorn.mem_read((addr + index as u32) as u64, &mut buf) {
            Ok(()) if buf[0] != 0 => out.push(buf[0]),
            Ok(_) => break,
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}