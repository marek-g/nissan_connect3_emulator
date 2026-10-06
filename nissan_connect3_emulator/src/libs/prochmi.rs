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
const CL_GUI_CHECK_MSGBOX: u32 = 0x0133_bcf4 - ORIGINAL_BASE;
const GUI_LUA_LOAD_SCRIPTS: u32 = 0x0136_9350 - ORIGINAL_BASE;
const GUI_LUA_DOFILE: u32 = 0x0136_9228 - ORIGINAL_BASE;
const GUI_STATEMACHINE_CREATE: u32 = 0x0137_31ac - ORIGINAL_BASE;
const GUI_TOUCH_START: u32 = 0x0134_aef0 - ORIGINAL_BASE;
const GUI_DM_S_INITIALIZE: u32 = 0x0133_df68 - ORIGINAL_BASE;
const GUI_STATE_ADD_TRANSITION: u32 = 0x013a_7d14 - ORIGINAL_BASE;
const GUI_UTIL_QUEUE_READER_C1: u32 = 0x0134_c260 - ORIGINAL_BASE;
const GUI_UTIL_QUEUE_READER_C2: u32 = 0x0134_c204 - ORIGINAL_BASE;
const GUI_UTIL_QUEUE_READER_BEGIN: u32 = 0x0134_c534 - ORIGINAL_BASE;
const GUI_MSGBOX_READER_C1_LR: u32 = 0x0236_8bac - ORIGINAL_BASE;
const GUI_MSGBOX_BEGIN_CALL_LR: u32 = 0x0133_bd64 - ORIGINAL_BASE;
const GUI_MSG_ON_MESSAGE: u32 = 0x0136_52d4 - ORIGINAL_BASE;
const SVG_INIT_RESOURCE: u32 = 0x00fb_6ad0 - ORIGINAL_BASE;
const SVG_CREATE_SURFACE: u32 = 0x00fb_7058 - ORIGINAL_BASE;
const SVG_CREATE_LAYER_CONTEXT: u32 = 0x00fb_68e4 - ORIGINAL_BASE;
const CL_LUA_DEBUGGER_S_INITIALIZE: u32 = 0x0133_ae10 - ORIGINAL_BASE;
const GUI_DISPLAY_WIDTH: u32 = 800;
const GUI_DISPLAY_HEIGHT: u32 = 480;
const TRAMPOLINE_SIZE: usize = 44;

const CL_HMI_MNGR_C1: u32 = 0x0134_f364 - ORIGINAL_BASE;
const CL_HMI_MNGR_C2: u32 = 0x0134_f5a8 - ORIGINAL_BASE;
const CL_GUI_WIDGET_C1: u32 = 0x0133_cb48 - ORIGINAL_BASE;
const CL_GUI_WIDGET_C2: u32 = 0x0133_ccd0 - ORIGINAL_BASE;
const HMI_MAIN_ENTRY: u32 = 0x00fc_8c48 - ORIGINAL_BASE;
const CL_HMI_MNGR_STATE: u32 = 0x670;
const CL_HMI_MNGR_PENDING_STATE: u32 = 0x671;
const CL_GUI_PENDING_POWER_STATE: u32 = 0x1c;
const CL_GUI_STARTED: u32 = 0x20;
const OSAL_EVENT_BITS: u32 = 0x14;
const HMI_FW_LOOP_EVENT_BIT: u32 = 0x4;

static PROCHMI_BASE: AtomicU32 = AtomicU32::new(0);
static HMI_MNGR_POINTER: AtomicU32 = AtomicU32::new(0);
static HMI_GUI_POINTER: AtomicU32 = AtomicU32::new(0);
static GUI_MESSAGING_POINTER: AtomicU32 = AtomicU32::new(0);
static GUI_INTERNAL_EVENT_SENT_COUNT: AtomicU32 = AtomicU32::new(0);
static GUI_UTIL_STARTUP_STATUS_POSTED: AtomicU32 = AtomicU32::new(0);
static GUI_UTIL_MSGBOX_QUEUE: AtomicU32 = AtomicU32::new(0);
static GUI_ENGINE_ADDRESS: AtomicU32 = AtomicU32::new(0);
static HMI_EVENT_OBJECT: AtomicU32 = AtomicU32::new(0);
static HMI_MAIN_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static HMI_GUI_FORCE_LOGGED: AtomicBool = AtomicBool::new(false);
static HMI_EVENT_WAKE_LOGGED: AtomicBool = AtomicBool::new(false);
static HMI_MNGR_MISSING_LOGGED: AtomicBool = AtomicBool::new(false);
static SVG_FAKE_HANDLE: AtomicU32 = AtomicU32::new(0);
static SVG_BYPASS_LOGGED: AtomicBool = AtomicBool::new(false);
static GUI_INTERNAL_POST_PENDING: AtomicU32 = AtomicU32::new(0);

const GUI_INTERNAL_EVENT_COUNT: u32 = 3;
const GUI_INTERNAL_EVENTS: [u32; 3] = [0x8d, 0x8e, 0x8f];

thread_local! {
    static PROCHMI_TICK_NEXT: Cell<Option<Instant>> = const { Cell::new(None) };
    static PROCHMI_GUI_INTERNAL_NEXT: Cell<Option<Instant>> = const { Cell::new(None) };
    static PROCHMI_DISPLAY_MODE_NEXT: Cell<Option<Instant>> = const { Cell::new(None) };
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
        (
            CL_LUA_DEBUGGER_S_INITIALIZE,
            "clLuaDebugger::s_initialize trampoline",
        ),
        (CL_HMI_MNGR_S_INITIALIZE, "clHMIMngr::s_initialize"),
        (
            CL_GUI_WIDGET_S_INITIALIZE,
            "clGUIWidgetEngine::s_initialize",
        ),
        (GUI_LUA_LOAD_SCRIPTS, "GUI_LUA_Interface::loadScripts"),
        (
            GUI_STATEMACHINE_CREATE,
            "GUI_StateMachineFactory::s_vCreateStateMachines",
        ),
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
            let lua_state = if lua >= 4 { read_u32(uc, lua + 4) } else { 0 };
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
            if gui != 0 {
                GUI_ENGINE_ADDRESS.store(gui, Ordering::Relaxed);
            }
            let mode = read_u8(uc, gui + 0x3d);
            log::info!(
                "PROCHMI: clGUIWidgetEngine::bGUIMainloop this={:#x} display_mode={}",
                gui,
                mode
            );
        })
        .unwrap();

    let check_msgbox = base_address + CL_GUI_CHECK_MSGBOX;
    unicorn
        .add_code_hook(check_msgbox as u64, check_msgbox as u64, |uc, _, _| {
            if GUI_UTIL_STARTUP_STATUS_POSTED.load(Ordering::Relaxed) == 0
                && post_gui_util_startup_anim_status(uc, 1)
            {
                GUI_UTIL_STARTUP_STATUS_POSTED.store(1, Ordering::Relaxed);
                log::info!(
                    "PROCHMI: injected startup animation status before GUI message box poll"
                );
            }
        })
        .unwrap();

    for offset in [
        GUI_UTIL_QUEUE_READER_C1,
        GUI_UTIL_QUEUE_READER_C2,
        GUI_UTIL_QUEUE_READER_BEGIN,
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                if GUI_UTIL_MSGBOX_QUEUE.load(Ordering::Relaxed) != 0 {
                    return;
                }
                let reader = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let mut queue = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                if queue == 0 && offset == GUI_UTIL_QUEUE_READER_BEGIN && reader != 0 {
                    queue = read_u32(uc, reader + 4);
                }
                let captured = (offset == GUI_UTIL_QUEUE_READER_C1
                    && lr == base_address + GUI_MSGBOX_READER_C1_LR)
                    || (offset == GUI_UTIL_QUEUE_READER_BEGIN
                        && lr == base_address + GUI_MSGBOX_BEGIN_CALL_LR);
                if queue != 0 && captured {
                    GUI_UTIL_MSGBOX_QUEUE.store(queue, Ordering::Relaxed);
                    log::info!("PROCHMI: captured GUI_UTIL message box queue={:#x}", queue);
                }
            })
            .unwrap();
    }

    let msg_on_message = base_address + GUI_MSG_ON_MESSAGE;
    unicorn
        .add_code_hook(msg_on_message as u64, msg_on_message as u64, |uc, _, _| {
            let messaging = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            if messaging != 0 {
                GUI_MESSAGING_POINTER.store(messaging, Ordering::Relaxed);
            }
            if GUI_INTERNAL_POST_PENDING.load(Ordering::Relaxed) != 0
                && messaging != 0
                && GUI_INTERNAL_EVENT_SENT_COUNT.load(Ordering::Relaxed) < GUI_INTERNAL_EVENT_COUNT
            {
                post_gui_internal_events(uc);
            }
        })
        .unwrap();

    let state_add_transition = base_address + GUI_STATE_ADD_TRANSITION;
    unicorn
        .add_code_hook(
            state_add_transition as u64,
            state_add_transition as u64,
            |uc, _, _| {
                let state = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let transition = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xffff;
                if state != 0 && read_u16(uc, state + 0x10) == 0x1e5 && transition == 0x1977 {
                    GUI_INTERNAL_POST_PENDING.store(1, Ordering::Relaxed);
                }
            },
        )
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

fn post_gui_message_queue(
    unicorn: &mut Unicorn<'_, Context>,
    queue: u32,
    receiver: u32,
    event_index: u32,
) -> bool {
    let head = read_u32(unicorn, queue + 8);
    let tail = read_u32(unicorn, queue + 0xc);
    let capacity = read_u32(unicorn, queue + 4);
    let entries = read_u32(unicorn, queue);
    if queue == 0 || entries == 0 || capacity == 0 {
        return false;
    }

    let next = if tail + 1 == capacity { 0 } else { tail + 1 };
    if next == head {
        return false;
    }

    let entry = entries + tail * 0x10;
    let _ = write_u32(unicorn, entry, receiver);
    let _ = write_u32(unicorn, entry + 4, 0);
    let _ = write_u32(unicorn, entry + 8, 0);
    let _ = write_u32(unicorn, entry + 0xc, event_index);
    let _ = write_u32(unicorn, queue + 0xc, next);
    true
}

fn post_gui_internal_events(unicorn: &mut Unicorn<'_, Context>) {
    let messaging = GUI_MESSAGING_POINTER.load(Ordering::Relaxed);
    if messaging == 0 {
        return;
    }

    let sent = GUI_INTERNAL_EVENT_SENT_COUNT.load(Ordering::Relaxed) as usize;
    if sent >= GUI_INTERNAL_EVENT_COUNT as usize {
        return;
    }

    let event_index = GUI_INTERNAL_EVENTS[sent];
    let queue = read_u32(unicorn, messaging + 4);
    if post_gui_message_queue(unicorn, queue, messaging, event_index) {
        let count = GUI_INTERNAL_EVENT_SENT_COUNT.fetch_add(1, Ordering::Relaxed);
        log::info!(
            "PROCHMI: posted GUI message event={:#x} queue={:#x} count={}",
            event_index,
            queue,
            count + 1
        );
    }
}

fn post_gui_util_startup_anim_status(unicorn: &mut Unicorn<'_, Context>, status: u32) -> bool {
    let queue = GUI_UTIL_MSGBOX_QUEUE.load(Ordering::Relaxed);
    if queue == 0 {
        return false;
    }

    let buffer = read_u32(unicorn, queue + 4);
    let read = read_u32(unicorn, queue + 8);
    let write = read_u32(unicorn, queue + 0xc);
    let wrap = read_u32(unicorn, queue + 0x10);
    let max = read_u32(unicorn, queue + 0x14);
    const MSG_SIZE: u32 = 12;

    if buffer == 0 || max < MSG_SIZE {
        log::info!(
            "PROCHMI: GUI_UTIL startup animation queue not ready queue={:#x} buffer={:#x} read={} write={} wrap={} max={}",
            queue,
            buffer,
            read,
            write,
            wrap,
            max
        );
        return false;
    }

    let mut offset = write;
    if max.saturating_sub(offset) < MSG_SIZE {
        if offset < read || read <= MSG_SIZE {
            log::info!(
                "PROCHMI: GUI_UTIL startup animation queue has no room queue={:#x} read={} write={} wrap={} max={}",
                queue,
                read,
                write,
                wrap,
                max
            );
            return false;
        }
        let _ = write_u32(unicorn, queue + 0x10, offset);
        offset = 0;
    }

    if max.saturating_sub(offset) < MSG_SIZE {
        return false;
    }

    let next = offset + MSG_SIZE;
    let _ = write_u32(unicorn, buffer + offset, next);
    let _ = write_u32(unicorn, buffer + offset + 4, 6);
    let _ = write_u32(unicorn, buffer + offset + 8, status);
    let callback = read_u32(unicorn, queue + 0x30);
    let context = read_u32(unicorn, queue + 0x34);
    let _ = write_u32(unicorn, queue + 0xc, next);

    log::info!(
        "PROCHMI: posted GUI_UTIL startup animation status queue={:#x} buffer={:#x} read={} write={} next={} status={} callback={:#x} context={:#x}",
        queue,
        buffer,
        read,
        write,
        next,
        status,
        callback,
        context
    );
    true
}

pub fn tick(unicorn: &mut Unicorn<'_, Context>) {
    let now = Instant::now();
    if GUI_INTERNAL_POST_PENDING.load(Ordering::Relaxed) != 0
        && GUI_MESSAGING_POINTER.load(Ordering::Relaxed) != 0
        && GUI_INTERNAL_EVENT_SENT_COUNT.load(Ordering::Relaxed) < GUI_INTERNAL_EVENT_COUNT
    {
        post_gui_internal_events(unicorn);
    }

    if GUI_ENGINE_ADDRESS.load(Ordering::Relaxed) != 0
        && GUI_UTIL_STARTUP_STATUS_POSTED.load(Ordering::Relaxed) < 1
    {
        if post_gui_util_startup_anim_status(unicorn, 1) {
            GUI_UTIL_STARTUP_STATUS_POSTED.fetch_add(1, Ordering::Relaxed);
        }
    }

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
    let fake =
        mmu_arc
            .lock()
            .unwrap()
            .heap_alloc(unicorn, 0x1000, Prot::READ | Prot::WRITE, "[svg-fake]");
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
    trampoline_bytes.extend_from_slice(&[0xc8, 0x00, 0xa0, 0xe3]); // mov r0, #200
    trampoline_bytes.extend_from_slice(&[0x00, 0x01, 0xa0, 0xe1]); // mov r0, r0, lsl #2
    trampoline_bytes.extend_from_slice(&[0xf0, 0x10, 0xa0, 0xe3]); // mov r1, #240
    trampoline_bytes.extend_from_slice(&[0x81, 0x10, 0xa0, 0xe1]); // mov r1, r1, lsl #1
    trampoline_bytes.extend_from_slice(&arm_bl(lua_initialize + 28, display_initialize));
    trampoline_bytes.extend_from_slice(&[0x11, 0x40, 0xbd, 0xe8]); // pop {r0, r4, lr}
    trampoline_bytes.extend_from_slice(&[0x00, 0x00, 0xa0, 0xe3]); // mov r0, #0
    trampoline_bytes.extend_from_slice(&[0x1e, 0xff, 0x2f, 0xe1]); // bx lr

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

fn read_u16(unicorn: &Unicorn<'_, Context>, addr: u32) -> u32 {
    if addr == 0 {
        return 0;
    }
    let mut buf = [0_u8; 2];
    match unicorn.mem_read(addr as u64, &mut buf) {
        Ok(()) => u16::from_le_bytes(buf) as u32,
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
