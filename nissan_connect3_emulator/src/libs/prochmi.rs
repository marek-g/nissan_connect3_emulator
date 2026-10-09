use crate::emulator::context::Context;
use crate::emulator::thread::{BlockReason, ThreadStatus};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;
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
const GUI_LUA_PREPARE_CALL: u32 = 0x0136_8c5c - ORIGINAL_BASE;
const GUI_STATEMACHINE_CREATE: u32 = 0x0137_31ac - ORIGINAL_BASE;
const GUI_TOUCH_START: u32 = 0x0134_aef0 - ORIGINAL_BASE;
const GUI_DM_S_INITIALIZE: u32 = 0x0133_df68 - ORIGINAL_BASE;
const GUI_STATE_ADD_TRANSITION: u32 = 0x013a_7d14 - ORIGINAL_BASE;
const GUI_STATEMACHINE_ON_MESSAGE: u32 = 0x0137_17a8 - ORIGINAL_BASE;
const GUI_STATEMACHINE_PERFORM_TRANSITION: u32 = 0x0137_1390 - ORIGINAL_BASE;
const GUI_STATEMACHINE_DO_TRANSITION: u32 = 0x0137_1170 - ORIGINAL_BASE;
const GUI_UTIL_QUEUE_READER_C1: u32 = 0x0134_c260 - ORIGINAL_BASE;
const GUI_UTIL_QUEUE_READER_C2: u32 = 0x0134_c204 - ORIGINAL_BASE;
const GUI_UTIL_QUEUE_READER_BEGIN: u32 = 0x0134_c534 - ORIGINAL_BASE;
const GUI_MSGBOX_READER_C1_LR: u32 = 0x0236_8bac - ORIGINAL_BASE;
const GUI_MSGBOX_BEGIN_CALL_LR: u32 = 0x0133_bd64 - ORIGINAL_BASE;
const GUI_MSG_ON_MESSAGE: u32 = 0x0136_52d4 - ORIGINAL_BASE;
const GUI_MSG_EVENT_NAME_RETURN: u32 = 0x0136_5408 - ORIGINAL_BASE;
const GUI_MSG_FIRE_INTERNAL_EVENT: u32 = 0x0136_5868 - ORIGINAL_BASE;
const HSI_CM_BASE_SEND_SYSTEM_EVENT: u32 = 0x0180_a8bc - ORIGINAL_BASE;
const CL_HMI_MNGR_PERFORM_NEW_APP_STATE: u32 = 0x0134_e92c - ORIGINAL_BASE;
const CL_HSI_CM_STARTUP_UPDATE_STATUS: u32 = 0x0184_bf7c - ORIGINAL_BASE;
const CL_HSI_CM_STARTUP_CHECK_STATUS: u32 = 0x0184_bdd0 - ORIGINAL_BASE;
const SVG_INIT_RESOURCE: u32 = 0x00fb_6ad0 - ORIGINAL_BASE;
const SVG_CREATE_SURFACE: u32 = 0x00fb_7058 - ORIGINAL_BASE;
const SVG_CREATE_LAYER_CONTEXT: u32 = 0x00fb_68e4 - ORIGINAL_BASE;
const SVG_GET_LAYER_BY_NAME: u32 = 0x00fb_68f0 - ORIGINAL_BASE;
const SVG_GET_LAYER_STATUS: u32 = 0x00fb_6ac4 - ORIGINAL_BASE;
const SVG_GET_SURFACE_STATUS: u32 = 0x00fb_64b8 - ORIGINAL_BASE;
const SVG_GET_LAYER_ERROR: u32 = 0x00fb_6c38 - ORIGINAL_BASE;
const SVG_GET_RESOURCE_ERROR: u32 = 0x00fb_6a10 - ORIGINAL_BASE;
const SVG_APPLY_LAYER_IN_SYNC: u32 = 0x00fb_6e90 - ORIGINAL_BASE;
const SVG_WAIT_LAYER_VSYNC: u32 = 0x00fb_6cc8 - ORIGINAL_BASE;
const SVG_MAP_LAYER_HANDLE: u32 = 0x5f4c_0001;
const SVG_MAP_SURFACE_HANDLE: u32 = 0x5f4d_4150;
const SVG_MAP_WIDTH: u16 = 800;
const SVG_MAP_HEIGHT: u16 = 480;
const SVG_MAP_PITCH: u16 = SVG_MAP_WIDTH * 4;
const GUI_GL_LAYER_SYNC_COPY_LAYER: u32 = 0x0134_359c - ORIGINAL_BASE;
const GUI_GL_LAYER_SYNC_GET_LAYERS: u32 = 0x0134_2e38 - ORIGINAL_BASE;
const GUI_GL_LAYER_COPY_COPY: u32 = 0x0134_2b98 - ORIGINAL_BASE;
const GUI_GL_LAYER_COPY_PERFORM_COPY: u32 = 0x0134_2754 - ORIGINAL_BASE;
const GUI_GL_LAYER_SYNC_S_INITIALIZE: u32 = 0x0134_3e44 - ORIGINAL_BASE;
const GUI_GL_LAYER_SYNC_ON_SET_LAYER_NAMES: u32 = 0x0134_342c - ORIGINAL_BASE;
const GUI_GL_LAYER_SYNC_APPLY_PENDING: u32 = 0x0134_2dc0 - ORIGINAL_BASE;
const GUI_GL_LAYER_SYNC_REQUEST_VIEW_STATUS: u32 = 0x0134_3b50 - ORIGINAL_BASE;
const GUI_GL_LAYER_SYNC_ON_VIEW_STATUS_CHANGED: u32 = 0x0134_3d14 - ORIGINAL_BASE;
const GUI_GL_LAYER_SYNC_GET_VIEW_STATUS: u32 = 0x0134_2d0c - ORIGINAL_BASE;
const GUI_DM_EA_MANAGER_REQUEST_EA_SHOW: u32 = 0x0133_eef4 - ORIGINAL_BASE;
const GUI_DM_EA_MANAGER_REQUEST_EA_STATE_CHANGE: u32 = 0x0133_f1d0 - ORIGINAL_BASE;
const GUI_DM_EA_MANAGER_ON_VIEW_STATUS_CHANGED: u32 = 0x0133_e9b8 - ORIGINAL_BASE;
const GUI_DM_EA_MANAGER_REQUEST_EA_HIDE: u32 = 0x0133_e378 - ORIGINAL_BASE;
const GUI_DM_EA_MANAGER_REQUEST_DISPLAY_MODE_CHANGE: u32 = 0x0133_f778 - ORIGINAL_BASE;

const GUI_GL_TEXTURE_CONSTRUCTOR: u32 = 0x0134_a844 - ORIGINAL_BASE;
const GUI_GL_OPENGL_MIX_LAYERS: u32 = 0x0134_6bb0 - ORIGINAL_BASE;
const GUI_DM_EA_MANAGER_IS_BLOCKED: u32 = 0x0133_ede4 - ORIGINAL_BASE;
const GUI_DM_EA_MANAGER_GET_BACKGROUND: u32 = 0x0133_f528 - ORIGINAL_BASE;
const GOT_GUI_DM_DISPLAY_MANAGER_SINGLETON: u32 = 0x02a1_d970 - ORIGINAL_BASE;
const GOT_GUI_MENU_MANAGER_INSTANCE: u32 = 0x02a1_3f50 - ORIGINAL_BASE;
const GUI_DM_DISPLAY_MANAGER_IS_DIRTY: u32 = 0x0133_d8ac - ORIGINAL_BASE;

const GUI_DM_DISPLAY_MANAGER_UPDATE: u32 = 0x0133_d99c - ORIGINAL_BASE;

const GUI_DISPLAY_MANAGER_THREAD: u32 = 0x0133_be70 - ORIGINAL_BASE;
const GUI_MAINLOOP_TRACE_START: u32 = 0x0133_be70 - ORIGINAL_BASE;
const GUI_MAINLOOP_TRACE_END: u32 = 0x0133_c07c - ORIGINAL_BASE;
const GUI_GL_CONTEXT_INITIALIZE: u32 = 0x0134_8b04 - ORIGINAL_BASE;
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
const GUI_STATE_SM_CURRENT_LEAF: u32 = 8;
const GUI_STATE_SM_PENDING_TRANSITION: u32 = 0xc;
const GUI_STATE_STATE_ID: u32 = 0x10;
const GUI_MESSAGE_EVENT_ID: u32 = 0xc;
const GUI_TRANSITION_EVENT_ID: u32 = 8;
const GUI_TRANSITION_SOURCE_STATE: u32 = 0xa;
const GUI_TRANSITION_TARGET_STATE: u32 = 0xc;
const GUI_TRANSITION_TYPE: u32 = 0xe;
const SYSDLG_NAV_STARTING_UP_STATE: u32 = 0x1e5;
const SYSDLG_NAV_STARTING_UP_TRANSITION: u32 = 0x1977;

const CL_HMI_NAV_SERVER_HANDLER_C1: u32 = 0x015e_871c - ORIGINAL_BASE;
const CL_HMI_NAV_SERVER_HANDLER_C2: u32 = 0x015e_9e5c - ORIGINAL_BASE;
const CL_HMI_NAV_SERVER_INIT: u32 = 0x015e_0824 - ORIGINAL_BASE;
const CL_HMI_NAV_HANDLE_INIT_MAP_SCREEN: u32 = 0x015b_0884 - ORIGINAL_BASE;
const CL_HMI_NAV_FORCE_MAP_INIT: u32 = 0x015a_d8b8 - ORIGINAL_BASE;
const CL_HMI_NAV_REQUEST_CREATE_RENDER_VIEW: u32 = 0x015a_dfbc - ORIGINAL_BASE;
const ENAVI_RENDER_VIEW_C1: u32 = 0x016a_5bd4 - ORIGINAL_BASE;
const ENAVI_RENDER_VIEW_CREATE_PIXMAP_REQUEST: u32 = 0x016a_558c - ORIGINAL_BASE;
const ENAVI_RENDER_VIEW_CREATE_INTERNAL_REQUEST: u32 = 0x016a_58c4 - ORIGINAL_BASE;
const ENAVI_RENDER_VIEW_REGISTER_SINK: u32 = 0x016a_2824 - ORIGINAL_BASE;

const CL_HMI_NAV_DISPLAY_MODE: u32 = 0x148;
const CL_HMI_NAV_NAV_SERVER: u32 = 0x15c;
const CL_HMI_NAV_TRACE: u32 = 0x160;
const CL_HMI_NAV_RENDER_VIEW: u32 = 0x808;
const CL_HMI_NAV_SINK_IMPLEMENTATION: u32 = 0x9a8;
const CL_HMI_NAV_MAP_DRAW_FLAG: u32 = 0x1c74;
const DAT_ENAVI_FI_CLIENT: u32 = 0x0597_3de0 - ORIGINAL_BASE;

const CL_HSI_CM_MANAGER_PHSI_BASE_GET: u32 = 0x0182_79f4 - ORIGINAL_BASE;
const CL_HSI_MNGR_BINIT_FIS: u32 = 0x0135_2cbc - ORIGINAL_BASE;
const HSI_CM_STARTUP_BEXECUTE_MESSAGE: u32 = 0x0184_b958 - ORIGINAL_BASE;
const HSI_FACTORY_CREATE_POST_ASSIGN: u32 = 0x0185_1edc - ORIGINAL_BASE;
const CL_HMI_MNGR_CMMNGR_OFFSET: u32 = 0x640;
const CL_HMI_MNGR_EVENT_ADAPTER_OFFSET: u32 = 0x610;
const HSI_CM_BASE_EVENT_ENGINE: u32 = 0x30;
const HSI_CM_STARTUP_ID: u32 = 2;
const HSI_CM_MANAGER_COMPONENT_ARRAY_OFFSET: u32 = 4;
const HSI_POWER_STATE_MESSAGE: u32 = 0x2715;
const HSI_POWER_STATE_DEFAULT: u32 = 0x12;
const HSI_POWER_STATE_PENDING_CREATE: u32 = 1;
const HSI_POWER_STATE_PENDING_SEND: u32 = 2;
const GUEST_CALL_STUB_SIZE: u32 = 16;

static PROCHMI_BASE: AtomicU32 = AtomicU32::new(0);
static HMI_MNGR_POINTER: AtomicU32 = AtomicU32::new(0);
static HMI_GUI_POINTER: AtomicU32 = AtomicU32::new(0);
static GUI_MESSAGING_POINTER: AtomicU32 = AtomicU32::new(0);
static GUI_INTERNAL_EVENT_SENT_COUNT: AtomicU32 = AtomicU32::new(0);
static GUI_UTIL_STARTUP_STATUS_POSTED: AtomicU32 = AtomicU32::new(0);
// The startup animation status (GUI message type 6) that a real boot
// animation process reports over the message queue. Injecting it makes
// prochmi treat the startup animation as started (its GUI svg layer stays
// visible via clGUIWidgetEngine::onGuiStartupAnimStatus). Off by default;
// set EMU_GUI_STARTUP_ANIM=1 to re-enable.
fn gui_startup_anim_enabled() -> bool {
    std::env::var("EMU_GUI_STARTUP_ANIM")
        .map(|v| v != "0")
        .unwrap_or(false)
}
static GUI_UTIL_MSGBOX_QUEUE: AtomicU32 = AtomicU32::new(0);
static GUI_ENGINE_ADDRESS: AtomicU32 = AtomicU32::new(0);
static HMI_EVENT_OBJECT: AtomicU32 = AtomicU32::new(0);
static HMI_MAIN_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static HMI_GUI_FORCE_LOGGED: AtomicBool = AtomicBool::new(false);
static HMI_EVENT_WAKE_LOGGED: AtomicBool = AtomicBool::new(false);
static HMI_MNGR_MISSING_LOGGED: AtomicBool = AtomicBool::new(false);
static SVG_FAKE_HANDLE: AtomicU32 = AtomicU32::new(0);
static SVG_BYPASS_LOGGED: AtomicBool = AtomicBool::new(false);
static SVG_HOOK_HIT_COUNT: AtomicU32 = AtomicU32::new(0);
// Staged emulation of procmapengine's LayerSync CCA replies. prochmi's
// GUI_GL_LayerSync::requestViewStatus(EA=0) asks the map application for
// its view; on real hardware procmap answers over the DAPI/FI transport
// with a serialized message burst on the GUI message queue (see
// GUI_MessageBoxGUI/System::sendGuiLSync* senders in prochmi_out.out):
//   type 3    SetLayerNames(ea, "MAP_View1", "")   -> onSetLayerNames
//   type 2    ViewStatusChanged(ea, VISIBLE=2)     -> onViewStatusChanged
//   type 1002 SetView(ea, visible, x, y, w, h)     -> setLayersVisible path
// The stages advance one message per clGUIWidgetEngine::bCheckMsgBox tick;
// 9 means the burst completed.
static LSYNC_MAP_STAGE: AtomicU32 = AtomicU32::new(0);
// GUI_DM_EAManager singleton and the active EAWStatus entry captured from
// GUI_DM_EAManager::updateEAShow calls (r0 = manager, r1 = entry with
// +0x00 covering widget, +0x08 EA, +0x24 state machine).
static LSYNC_EA_MANAGER: AtomicU32 = AtomicU32::new(0);
static LSYNC_EA_ENTRY: AtomicU32 = AtomicU32::new(0);
static LSYNC_HIDE_TICKS: AtomicU32 = AtomicU32::new(0);
static LSYNC_REFRESH_FRAMES: AtomicU32 = AtomicU32::new(0);
static LSYNC_MSGBOX_TICKS: AtomicU32 = AtomicU32::new(0);
static GL_LAYER_COPY_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

static HMI_MAINLOOP_DIAG_COUNT: AtomicU32 = AtomicU32::new(0);
static HMI_FIS_INIT_DONE: AtomicBool = AtomicBool::new(false);
static HMI_VIEW_CLEAN_DIAG_COUNT: AtomicU32 = AtomicU32::new(0);
static HMI_VIEW_DIRTY_FORCE_COUNT: AtomicU32 = AtomicU32::new(0);
static HMI_MAINLOOP_TRACE_ARMED: AtomicBool = AtomicBool::new(false);
static HMI_MAINLOOP_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

static GL_CONTEXT_CTOR_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

static HMI_VIEW_CONTEXT_ASSIGN_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static HMI_WIDGET_CALL_DRAW_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static HMI_CONTEXT_VIRTUAL_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static HMI_UPDATE_TRACE_ARMED: AtomicBool = AtomicBool::new(false);
static HMI_UPDATE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static GUI_GL_OPENGLCONTEXT_CTOR: u32 = 0x0134_8e40 - ORIGINAL_BASE;
static GUI_GL_CONTEXT_IS_DIRTY: u32 = 0x0134_6ff0 - ORIGINAL_BASE;
static GUI_GL_CONTEXT_DRAW_BEGIN: u32 = 0x0134_8980 - ORIGINAL_BASE;
static GUI_GL_CONTEXT_GET_DIRTY_REGION: u32 = 0x0134_8fc4 - ORIGINAL_BASE;
static GUI_WIDGET_CALL_DRAW: u32 = 0x0137_af7c - ORIGINAL_BASE;
static GUI_DM_EA_MANAGER_UPDATE: u32 = 0x0133_f608 - ORIGINAL_BASE;
static GUI_DM_EA_MANAGER_HIDE: u32 = 0x0133_e790 - ORIGINAL_BASE;
static GUI_DM_EA_MANAGER_ABORT: u32 = 0x0133_e744 - ORIGINAL_BASE;
static GUI_DM_EA_MANAGER_SHOW: u32 = 0x0133_ea84 - ORIGINAL_BASE;
static HMI_UPDATE_TRACE_START: u32 = GUI_DM_DISPLAY_MANAGER_UPDATE;
static HMI_UPDATE_TRACE_END: u32 = 0x0133_dd20 - ORIGINAL_BASE;
static GUI_GL_OPENGLCONTEXT_STORE_VPTR: u32 = 0x0134_8e80 - ORIGINAL_BASE;
static GUI_GL_OPENGLCONTEXT_INIT: u32 = GUI_GL_CONTEXT_INITIALIZE;
static GUI_GL_INVISIBLE_CONTEXT_CTOR: u32 = 0x0134_911c - ORIGINAL_BASE;
static GUI_MENU_MANAGER_VIEW_CHANGE_STORE_CONTEXT: u32 = 0x0136_4770 - ORIGINAL_BASE;
static GUI_MENU_MANAGER_OVERLAY_STORE_CONTEXT: u32 = 0x0136_4964 - ORIGINAL_BASE;
const GOT_GUI_GL_OPENGLCONTEXT_VTABLE: u32 = 0x02a1_db34 - ORIGINAL_BASE;
static GUI_INTERNAL_POST_PENDING: AtomicU32 = AtomicU32::new(0);
static NAV_STATE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static LUA_CALL_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static GUI_EVENT_NAME_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static GUI_FIRE_EVENT_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static HSI_CM_EVENT_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static CL_HMI_MNGR_APP_STATE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static HSI_POWER_STATE_PENDING: AtomicU32 = AtomicU32::new(0);
static HSI_CM_STARTUP_COMPONENT: AtomicU32 = AtomicU32::new(0);
static PROCHMI_GUEST_CALL_STUB: AtomicU32 = AtomicU32::new(0);
static HSI_POWER_STATE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static HMI_NAV_HANDLER: AtomicU32 = AtomicU32::new(0);
static HMI_NAV_HANDLER_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

const GUI_INTERNAL_EVENT_COUNT: u32 = 3;
const GUI_INTERNAL_EVENTS: [u32; 3] = [0x8d, 0x8e, 0x8f];

const NAV_STATE_TRACE_LIMIT: u32 = 128;

fn nav_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("EMU_PROCHMI_NAV_TRACE").is_some())
}

fn prochmi_internal_events_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("EMU_PROCHMI_NO_FAKE_INTERNAL_EVENTS").is_none())
}

fn lua_call_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("EMU_PROCHMI_LUA_TRACE").is_some())
}

fn lua_call_trace_allowed() -> bool {
    if !lua_call_trace_enabled() {
        return false;
    }
    LUA_CALL_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < NAV_STATE_TRACE_LIMIT
}

fn gui_event_name_trace_allowed() -> bool {
    if std::env::var_os("EMU_PROCHMI_GUI_EVENT_TRACE").is_none() {
        return false;
    }
    GUI_EVENT_NAME_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < NAV_STATE_TRACE_LIMIT
}

fn gui_fire_event_trace_allowed() -> bool {
    if std::env::var_os("EMU_PROCHMI_GUI_FIRE_TRACE").is_none() {
        return false;
    }
    GUI_FIRE_EVENT_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < NAV_STATE_TRACE_LIMIT
}

fn hsi_cm_event_trace_allowed() -> bool {
    if std::env::var_os("EMU_PROCHMI_CM_EVENT_TRACE").is_none() {
        return false;
    }
    HSI_CM_EVENT_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < NAV_STATE_TRACE_LIMIT
}

fn hmi_app_state_trace_allowed() -> bool {
    if std::env::var_os("EMU_PROCHMI_HMI_STATE_TRACE").is_none() {
        return false;
    }
    CL_HMI_MNGR_APP_STATE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < NAV_STATE_TRACE_LIMIT
}

fn hsi_power_state_stub() -> Option<u32> {
    static ENABLED: OnceLock<Option<u32>> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if std::env::var_os("EMU_PROCHMI_NO_HSI_POWER_STATE_EMULATION").is_some() {
            return None;
        }
        let requested = std::env::var("EMU_PROCHMI_EMULATE_HSI_POWER_STATE").ok();
        if requested.is_none() {
            return None;
        }
        let value = requested.unwrap();
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("1") || value.eq_ignore_ascii_case("true") {
            return Some(HSI_POWER_STATE_DEFAULT);
        }
        let text = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
            .unwrap_or(value);
        if text.is_empty() {
            return Some(HSI_POWER_STATE_DEFAULT);
        }
        if value.starts_with("0x") || value.starts_with("0X") {
            return u32::from_str_radix(text, 16).ok();
        }
        text.parse::<u32>().ok()
    })
}

fn hsi_power_state_trace_allowed() -> bool {
    if hsi_power_state_stub().is_none() {
        return false;
    }
    HSI_POWER_STATE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < NAV_STATE_TRACE_LIMIT
}

fn nav_trace_allowed() -> bool {
    if !nav_trace_enabled() {
        return false;
    }
    NAV_STATE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < NAV_STATE_TRACE_LIMIT
}

fn hmi_nav_handler_trace_allowed() -> bool {
    HMI_NAV_HANDLER_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < NAV_STATE_TRACE_LIMIT
}

fn log_hmi_nav_handler_state(
    unicorn: &mut Unicorn<'_, Context>,
    handler: u32,
    label: &str,
    force: bool,
) -> bool {
    if handler == 0 {
        if force {
            log::info!("PROCHMI: {} saw null clHmiNavServerHandler", label);
        }
        return false;
    }
    let previous = HMI_NAV_HANDLER.swap(handler, Ordering::Relaxed);
    let should_log = force || previous == 0 || hmi_nav_handler_trace_allowed();
    if should_log {
        log::info!(
            "PROCHMI: {} handler={:#x} mode={} nav={} trace={} render_view={} sink={} fi={} flag={}",
            label,
            handler,
            read_u32(unicorn, handler + CL_HMI_NAV_DISPLAY_MODE),
            read_u32(unicorn, handler + CL_HMI_NAV_NAV_SERVER),
            read_u32(unicorn, handler + CL_HMI_NAV_TRACE),
            read_u32(unicorn, handler + CL_HMI_NAV_RENDER_VIEW),
            handler + CL_HMI_NAV_SINK_IMPLEMENTATION,
            read_u32(
                unicorn,
                PROCHMI_BASE.load(Ordering::Relaxed) + DAT_ENAVI_FI_CLIENT,
            ),
            read_u8(unicorn, handler + CL_HMI_NAV_MAP_DRAW_FLAG),
        );
    }
    true
}

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

    install_svg_map_surface_hooks(unicorn, base_address);

    force_display_manager_dirty(unicorn, base_address);
    force_ea_manager_background(unicorn, base_address);
    install_gl_layer_copy_trace_hooks(unicorn, base_address);

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

    for (offset, label) in [
        (CL_HMI_NAV_SERVER_HANDLER_C1, "clHmiNavServerHandler C1"),
        (CL_HMI_NAV_SERVER_HANDLER_C2, "clHmiNavServerHandler C2"),
        (CL_HMI_NAV_SERVER_INIT, "clHmiNavServerHandler vHMIInitialization"),
        (CL_HMI_NAV_HANDLE_INIT_MAP_SCREEN, "clHmiNavServerHandler vHandleInitMapScreen"),
        (CL_HMI_NAV_FORCE_MAP_INIT, "clHmiNavServerHandler vForceMapInitialization"),
        (
            CL_HMI_NAV_REQUEST_CREATE_RENDER_VIEW,
            "clHmiNavServerHandler vHandleOnRequestCreateRenderView",
        ),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let handler = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let force = matches!(
                    label,
                    "clHmiNavServerHandler vHMIInitialization"
                        | "clHmiNavServerHandler vHandleInitMapScreen"
                        | "clHmiNavServerHandler vForceMapInitialization"
                        | "clHmiNavServerHandler vHandleOnRequestCreateRenderView"
                );
                log_hmi_nav_handler_state(uc, handler, label, force || nav_trace_enabled());
            })
            .unwrap();
    }

    for (offset, label) in [
        (
            ENAVI_RENDER_VIEW_C1,
            "enavi_tclRenderView constructor",
        ),
        (
            ENAVI_RENDER_VIEW_CREATE_PIXMAP_REQUEST,
            "enavi_tclRenderView bRequestCreateRenderPixmapView",
        ),
        (
            ENAVI_RENDER_VIEW_CREATE_INTERNAL_REQUEST,
            "enavi_tclRenderView bRequestCreateRenderViewInternal",
        ),
        (
            ENAVI_RENDER_VIEW_REGISTER_SINK,
            "enavi_tclRenderView bRegisterSinkInterface",
        ),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let render_view = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let sink = if offset == ENAVI_RENDER_VIEW_REGISTER_SINK {
                    uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32
                } else {
                    0
                };
                let width = if matches!(
                    offset,
                    ENAVI_RENDER_VIEW_CREATE_PIXMAP_REQUEST | ENAVI_RENDER_VIEW_CREATE_INTERNAL_REQUEST
                ) {
                    read_u32(uc, uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32)
                } else {
                    0
                };
                if hmi_nav_handler_trace_allowed() || nav_trace_enabled() {
                    log::info!(
                        "PROCHMI: {} this={:#x} sink={:#x} stack0={:#x}",
                        label,
                        render_view,
                        sink,
                        width
                    );
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

    let lua_prepare_call = base_address + GUI_LUA_PREPARE_CALL;
    unicorn
        .add_code_hook(lua_prepare_call as u64, lua_prepare_call as u64, |uc, _, _| {
            if !lua_call_trace_allowed() {
                return;
            }
            let module = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let function = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let module_name = read_cstr(uc, module, 96);
            let function_name = read_cstr(uc, function, 128);
            log::info!(
                "PROCHMI: GUI_LUA_Interface::prepareCall module={} function={}",
                module_name,
                function_name
            );
        })
        .unwrap();

    let gui_mainloop = base_address + CL_GUI_MAINLOOP;
    unicorn
        .        add_code_hook(gui_mainloop as u64, gui_mainloop as u64, |uc, address, _| {
            if maybe_force_hmi_fi_init(uc, address as u32) {
                return;
            }
            if maybe_inject_hsi_power_state(uc, address as u32) {
                return;
            }

            let gui = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            if gui != 0 {
                GUI_ENGINE_ADDRESS.store(gui, Ordering::Relaxed);
            }
            let mode = read_u8(uc, gui + 0x3d);
            let diag_count = HMI_MAINLOOP_DIAG_COUNT.fetch_add(1, Ordering::Relaxed);
            let hmi = HMI_MNGR_POINTER.load(Ordering::Relaxed);
            if hmi != 0 && diag_count < 20 {
                log::info!(
                    "PROCHMI: HMI manager diag count={} hmi={:#x} adapter={:#x} cfg={:#x} trace={:#x} fi_factory={:#x}",
                    diag_count,
                    hmi,
                    read_u32(uc, hmi + 8),
                    read_u32(uc, hmi + 0xc),
                    read_u32(uc, hmi + 0x10),
                    read_u32(uc, hmi + 0x18),
                );
            }
            let handler = HMI_NAV_HANDLER.load(Ordering::Relaxed);
            if handler != 0 && diag_count < 20 {
                log::info!(
                    "PROCHMI: nav handler diag count={} handler={:#x} mode={:#x} nav={:#x} trace={:#x} render={:#x} fi={:#x} flag={:#x}",
                    diag_count,
                    handler,
                    read_u32(uc, handler + 0x148),
                    read_u32(uc, handler + 0x15c),
                    read_u32(uc, handler + 0x160),
                    read_u32(uc, handler + 0x808),
                    read_u32(uc, PROCHMI_BASE.load(Ordering::Relaxed) + DAT_ENAVI_FI_CLIENT),
                    read_u32(uc, handler + 0x1c74),
                );
            }
            log::info!(
                "PROCHMI: clGUIWidgetEngine::bGUIMainloop this={:#x} display_mode={}",
                gui,
                mode
            );
        })
        .unwrap();

    let check_msgbox = base_address + CL_GUI_CHECK_MSGBOX;
    unicorn
        .add_code_hook(check_msgbox as u64, check_msgbox as u64, |uc, address, _| {
            let msgbox_ticks = LSYNC_MSGBOX_TICKS.fetch_add(1, Ordering::Relaxed);
            if msgbox_ticks % 2000 == 1 {
                log::info!("PROCHMI: bCheckMsgBox tick {}", msgbox_ticks);
            }
            if GUI_UTIL_STARTUP_STATUS_POSTED.load(Ordering::Relaxed) == 0
                && gui_startup_anim_enabled()
                && post_gui_util_startup_anim_status(uc, 1)
            {
                GUI_UTIL_STARTUP_STATUS_POSTED.store(1, Ordering::Relaxed);
                log::info!(
                    "PROCHMI: injected startup animation status before GUI message box poll"
                );
            }
            // Stages >=10 mutate registers via guest calls; they must not run
            // from this hook because the stub return re-enters bCheckMsgBox's
            // own code hook. They run from the DisplayManager::update hook
            // with original_pc = caller LR.
            if LSYNC_MAP_STAGE.load(Ordering::Relaxed) < 10 {
                post_lsync_map_announcement(uc, address as u32);
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

    let hmi_perform_new_app_state = base_address + CL_HMI_MNGR_PERFORM_NEW_APP_STATE;
    unicorn
        .add_code_hook(
            hmi_perform_new_app_state as u64,
            hmi_perform_new_app_state as u64,
            |uc, _, _| {
                if !hmi_app_state_trace_allowed() {
                    return;
                }
                let obj = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let caller = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                let state = if obj != 0 {
                    read_u8(uc, obj + 0x670)
                } else {
                    0
                };
                let pending = if obj != 0 {
                    read_u8(uc, obj + 0x671)
                } else {
                    0
                };
                log::info!(
                    "PROCHMI: clHMIMngr::vPerformNewAppState obj={:#x} state={} pending={} caller={:#x}",
                    obj,
                    state,
                    pending,
                    caller
                );
            },
        )
        .unwrap();

    let hsi_send_system_event = base_address + HSI_CM_BASE_SEND_SYSTEM_EVENT;
    unicorn
        .add_code_hook(hsi_send_system_event as u64, hsi_send_system_event as u64, |uc, _, _| {
            if !hsi_cm_event_trace_allowed() {
                return;
            }
            let obj = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let event = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let caller = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            let event_engine = if obj != 0 {
                read_u32(uc, obj + 0x30)
            } else {
                0
            };
            log::info!(
                "PROCHMI: clHSI_CMBase::vSendSystemEvent obj={:#x} event={:#x} engine={:#x} caller={:#x}",
                obj,
                event,
                event_engine,
                caller
            );
        })
        .unwrap();

    let cm_startup_update_status = base_address + CL_HSI_CM_STARTUP_UPDATE_STATUS;
    unicorn
        .add_code_hook(
            cm_startup_update_status as u64,
            cm_startup_update_status as u64,
            |uc, _, _| {
                if !hsi_cm_event_trace_allowed() {
                    return;
                }
                let obj = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let caller = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                let msg = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let id = if msg != 0 { read_u32(uc, msg) } else { 0 };
                let field1 = if msg != 0 { read_u32(uc, msg + 4) } else { 0 };
                let status = if msg != 0 { read_u32(uc, msg + 8) } else { 0 };
                log::info!(
                    "PROCHMI: clHSI_CMStartup::vUpdateStatus obj={:#x} id={:#x} field1={:#x} status={:#x} caller={:#x}",
                    obj,
                    id,
                    field1,
                    status,
                    caller
                );
            },
        )
        .unwrap();

    let cm_startup_check_status = base_address + CL_HSI_CM_STARTUP_CHECK_STATUS;
    unicorn
        .add_code_hook(
            cm_startup_check_status as u64,
            cm_startup_check_status as u64,
            |uc, _, _| {
                if !hsi_cm_event_trace_allowed() {
                    return;
                }
                let obj = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let service_id = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let caller = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                let begin = if obj != 0 { read_u32(uc, obj + 0x70) } else { 0 };
                let end = if obj != 0 { read_u32(uc, obj + 0x74) } else { 0 };
                let count = if end > begin { (end - begin) / 4 } else { 0 };
                log::info!(
                    "PROCHMI: clHSI_CMStartup::bCheckStatus obj={:#x} service={:#x} groups={} begin={:#x} end={:#x} caller={:#x}",
                    obj,
                    service_id,
                    count,
                    begin,
                    end,
                    caller
                );
            },
        )
        .unwrap();

    let gui_msg_event_name = base_address + GUI_MSG_EVENT_NAME_RETURN;
    unicorn
        .add_code_hook(gui_msg_event_name as u64, gui_msg_event_name as u64, |uc, _, _| {
            if !gui_event_name_trace_allowed() {
                return;
            }
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let event = if sp != 0 {
                read_u16(uc, sp + 0x4c)
            } else {
                0
            };
            let name_ptr = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let name = read_cstr(uc, name_ptr, 160);
            log::info!(
                "PROCHMI: GUI_Messaging event id={:#x} name={} name_ptr={:#x}",
                event,
                name,
                name_ptr
            );
        })
        .unwrap();

    let gui_fire_internal_event = base_address + GUI_MSG_FIRE_INTERNAL_EVENT;
    unicorn
        .add_code_hook(
            gui_fire_internal_event as u64,
            gui_fire_internal_event as u64,
            |uc, _, _| {
                if !gui_fire_event_trace_allowed() {
                    return;
                }
                let receiver = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let message = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let target = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let caller = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                let event = if message != 0 {
                    read_u16(uc, message + GUI_MESSAGE_EVENT_ID)
                } else {
                    0
                };
                let m4 = read_u32(uc, message + 4);
                let m8 = read_u32(uc, message + 8);
                log::info!(
                    "PROCHMI: GUI_Messaging::vFireInternalEvent receiver={:#x} msg={:#x} target={:#x} event={:#x} caller={:#x} [0x{:08x},0x{:08x}]",
                    receiver,
                    message,
                    target,
                    event,
                    caller,
                    m4,
                    m8
                );
            },
        )
        .unwrap();

    let hsi_factory_post_assign = base_address + HSI_FACTORY_CREATE_POST_ASSIGN;
    unicorn
        .add_code_hook(
            hsi_factory_post_assign as u64,
            hsi_factory_post_assign as u64,
            |uc, _, _| {
                let object = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                let component_id = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                let factory = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                if component_id != HSI_CM_STARTUP_ID || object == 0 {
                    return;
                }

                HSI_CM_STARTUP_COMPONENT.store(object, Ordering::Relaxed);
                let hmi = HMI_MNGR_POINTER.load(Ordering::Relaxed);
                if hmi != 0 && read_u32(uc, object + HSI_CM_BASE_EVENT_ENGINE) == 0 {
                    if write_u32(uc, object + HSI_CM_BASE_EVENT_ENGINE, hmi + CL_HMI_MNGR_EVENT_ADAPTER_OFFSET)
                        && hsi_power_state_trace_allowed()
                    {
                        log::info!(
                            "PROCHMI: HSI startup event-engine {:#x} -> {:#x}",
                            object,
                            hmi + CL_HMI_MNGR_EVENT_ADAPTER_OFFSET
                        );
                    }
                }

                if hsi_power_state_trace_allowed() {
                    log::info!(
                        "PROCHMI: clHSI_CMStartup created obj={:#x} factory={:#x} pending={}",
                        object,
                        factory,
                        HSI_POWER_STATE_PENDING.load(Ordering::Relaxed)
                    );
                }
            },
        )
        .unwrap();

    let state_on_message = base_address + GUI_STATEMACHINE_ON_MESSAGE;
    unicorn
        .add_code_hook(state_on_message as u64, state_on_message as u64, |uc, _, _| {
            if !nav_trace_enabled() {
                return;
            }
            let sm = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let message = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let caller = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            let leaf = if sm != 0 {
                read_u32(uc, sm + GUI_STATE_SM_CURRENT_LEAF)
            } else {
                0
            };
            let state_id = if leaf != 0 {
                read_u16(uc, leaf + GUI_STATE_STATE_ID)
            } else {
                0
            };
            if state_id != SYSDLG_NAV_STARTING_UP_STATE {
                return;
            }
            let event = if message != 0 {
                read_u16(uc, message + GUI_MESSAGE_EVENT_ID)
            } else {
                0
            };
            let m0 = read_u32(uc, message);
            let m4 = read_u32(uc, message + 4);
            let m8 = read_u32(uc, message + 8);
            let m10 = read_u32(uc, message + 0x10);
            if nav_trace_allowed() {
                log::info!(
                    "PROCHMI: nav startup bOnMessage sm={:#x} leaf={:#x} event={:#x} msg={:#x} caller={:#x} [0x{:08x},0x{:08x},0x{:08x},0x{:08x}]",
                    sm,
                    leaf,
                    event,
                    message,
                    caller,
                    m0,
                    m4,
                    m8,
                    m10
                );
            }
        })
        .unwrap();

    let state_perform_transition = base_address + GUI_STATEMACHINE_PERFORM_TRANSITION;
    unicorn
        .add_code_hook(
            state_perform_transition as u64,
            state_perform_transition as u64,
            |uc, _, _| {
                if !nav_trace_enabled() {
                    return;
                }
                let sm = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let leaf = if sm != 0 {
                    read_u32(uc, sm + GUI_STATE_SM_CURRENT_LEAF)
                } else {
                    0
                };
                let state_id = if leaf != 0 {
                    read_u16(uc, leaf + GUI_STATE_STATE_ID)
                } else {
                    0
                };
                let transition = read_u32(uc, sm + GUI_STATE_SM_PENDING_TRANSITION);
                if transition == 0 {
                    return;
                }
                let event = read_u16(uc, transition + GUI_TRANSITION_EVENT_ID);
                let source = read_u16(uc, transition + GUI_TRANSITION_SOURCE_STATE);
                let target = read_u16(uc, transition + GUI_TRANSITION_TARGET_STATE);
                let kind = read_u16(uc, transition + GUI_TRANSITION_TYPE);
                if nav_trace_allowed() {
                    log::info!(
                        "PROCHMI: GUI_StateMachine performTransition sm={:#x} leaf={:#x} state={:#x} trans={:#x} event={:#x} src={:#x} tgt={:#x} kind={:#x}",
                        sm,
                        leaf,
                        state_id,
                        transition,
                        event,
                        source,
                        target,
                        kind
                    );
                }
            },
        )
        .unwrap();

    let state_do_transition = base_address + GUI_STATEMACHINE_DO_TRANSITION;
    unicorn
        .add_code_hook(state_do_transition as u64, state_do_transition as u64, |uc, _, _| {
            if !nav_trace_enabled() {
                return;
            }
            let sm = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let transition = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xffff;
            let leaf = if sm != 0 {
                read_u32(uc, sm + GUI_STATE_SM_CURRENT_LEAF)
            } else {
                0
            };
            let state_id = if leaf != 0 {
                read_u16(uc, leaf + GUI_STATE_STATE_ID)
            } else {
                0
            };
            if state_id != SYSDLG_NAV_STARTING_UP_STATE && transition != SYSDLG_NAV_STARTING_UP_TRANSITION {
                return;
            }
            if nav_trace_allowed() {
                log::info!(
                    "PROCHMI: nav startup doTransition sm={:#x} leaf={:#x} transition_id={:#x}",
                    sm,
                    leaf,
                    transition
                );
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
                let state_id = if state != 0 {
                    read_u16(uc, state + GUI_STATE_STATE_ID)
                } else {
                    0
                };
                if state_id == SYSDLG_NAV_STARTING_UP_STATE {
                    log::debug!(
                        "PROCHMI: GUI_State::vAddTransition state={:#x} transition={:#x}",
                        state,
                        transition
                    );
                }
                if state_id == SYSDLG_NAV_STARTING_UP_STATE && transition == SYSDLG_NAV_STARTING_UP_TRANSITION {
                    if let Some(power_state) = hsi_power_state_stub() {
                        HSI_POWER_STATE_PENDING.store(HSI_POWER_STATE_PENDING_CREATE, Ordering::Relaxed);
                        if nav_trace_allowed() || hsi_power_state_trace_allowed() {
                            log::info!(
                                "PROCHMI: armed HSI power-state emulation state={:#x} transition={:#x} hsi_state={:#x}",
                                state,
                                transition,
                                power_state
                            );
                        }
                    } else if prochmi_internal_events_enabled() {
                        GUI_INTERNAL_POST_PENDING.store(1, Ordering::Relaxed);
                        if nav_trace_allowed() {
                            log::info!(
                                "PROCHMI: armed startup-nav internal events at state={:#x} transition={:#x}",
                                state,
                                transition
                            );
                        }
                    }
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
    if !prochmi_internal_events_enabled() {
        return;
    }

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

// Appends one serialized GUI_UTIL_Queue message (payload already encoded in
// GUI_UTIL_QueueWriter format: consecutive u32 ints, strings as u32 length
// followed by length+1 raw bytes) to the GUI message box queue prochmi polls
// from clGUIWidgetEngine::bCheckMsgBox. Message framing matches
// GUI_UTIL_QueueReader::messageBegin: a u32 next-message offset header
// followed by the payload.
fn post_gui_util_message(unicorn: &mut Unicorn<'_, Context>, payload: &[u8], label: &str) -> bool {
    let queue = GUI_UTIL_MSGBOX_QUEUE.load(Ordering::Relaxed);
    if queue == 0 {
        return false;
    }

    let buffer = read_u32(unicorn, queue + 4);
    let read = read_u32(unicorn, queue + 8);
    let write = read_u32(unicorn, queue + 0xc);
    let max = read_u32(unicorn, queue + 0x14);
    let msg_size = 4 + payload.len() as u32;

    if buffer == 0 || max < msg_size {
        return false;
    }

    let mut offset = write;
    if max.saturating_sub(offset) < msg_size {
        if offset < read || read <= msg_size {
            return false;
        }
        let _ = write_u32(unicorn, queue + 0x10, offset);
        offset = 0;
    }
    if max.saturating_sub(offset) < msg_size {
        return false;
    }

    let next = offset + msg_size;
    if unicorn
        .mem_write((buffer + offset) as u64, &next.to_le_bytes())
        .is_err()
        || unicorn
            .mem_write((buffer + offset + 4) as u64, payload)
            .is_err()
    {
        return false;
    }
    let _ = write_u32(unicorn, queue + 0xc, next);

    log::info!(
        "PROCHMI: posted GUI_UTIL {} queue={:#x} buffer={:#x} read={} write={} next={}",
        label,
        queue,
        buffer,
        read,
        write,
        next
    );
    true
}

fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn push_string(buf: &mut Vec<u8>, value: &str) {
    push_u32(buf, value.len() as u32);
    buf.extend_from_slice(value.as_bytes());
    buf.push(0);
}

// Advances one stage of the emulated procmap LayerSync reply burst per call.
fn post_lsync_map_announcement(unicorn: &mut Unicorn<'_, Context>, original_pc: u32) {
    match LSYNC_MAP_STAGE.load(Ordering::Relaxed) {
        0 => {}
        1 => {
            // type 3: GUI_MessageBoxGUI::sendGuiLSyncSetLayerNames(ea=0, names)
            let mut payload = Vec::new();
            push_u32(&mut payload, 3);
            push_u32(&mut payload, 0);
            push_string(&mut payload, "MAP_View1");
            push_string(&mut payload, "");
            if post_gui_util_message(unicorn, &payload, "lsync SetLayerNames(MAP_View1)") {
                LSYNC_MAP_STAGE.store(2, Ordering::Relaxed);
            }
        }
        2 => {
            // type 2: GUI_MessageBoxGUI::sendGuiLSyncViewStatusChanged(ea=0, VISIBLE)
            let mut payload = Vec::new();
            push_u32(&mut payload, 2);
            push_u32(&mut payload, 0);
            push_u32(&mut payload, 2);
            if post_gui_util_message(unicorn, &payload, "lsync ViewStatusChanged(VISIBLE)") {
                LSYNC_MAP_STAGE.store(3, Ordering::Relaxed);
            }
        }
        3 => {
            // type 1002: GUI_MessageBoxSystem::sendGuiLSyncSetView(ea=0, visible, 0,0,800,480)
            let mut payload = Vec::new();
            push_u32(&mut payload, 1002);
            push_u32(&mut payload, 0);
            push_u32(&mut payload, 1);
            push_u32(&mut payload, 0);
            push_u32(&mut payload, 0);
            push_u32(&mut payload, 800);
            push_u32(&mut payload, 480);
            if post_gui_util_message(unicorn, &payload, "lsync SetView(fullscreen)") {
                LSYNC_MAP_STAGE.store(10, Ordering::Relaxed);
                log::info!("PROCHMI: emulated procmap LayerSync reply burst complete");
            }
        }
        // On the real unit the HMI hides the map view as soon as a blocking
        // popup takes over; EAManager then snapshots the map layer into
        // EAManager's GUI_GL_LayerCopy (updateEAHide state 5 ->
        // LayerSync::copyLayer). Drive that hide request ourselves once the
        // emulated show flow has completed.
        10 => {
            let entry = LSYNC_EA_ENTRY.load(Ordering::Relaxed);
            let manager = LSYNC_EA_MANAGER.load(Ordering::Relaxed);
            if entry != 0 && manager != 0 && read_u32(unicorn, entry + 0x24) == 3 {
                // Equivalent of requestEAHide(): mark the EAWStatus slot's
                // command field (slot base = entry - 0xc, +0x2c) as HIDE and
                // dirty the manager. A guest call into requestEAHide proved
                // fatal for the GUI thread, so poke the state directly.
                let _ = write_u32(unicorn, entry + 0x20, 1);
                let _ = write_u8(unicorn, manager + 0x12c, 1);
                LSYNC_MAP_STAGE.store(11, Ordering::Relaxed);
                log::info!(
                    "PROCHMI: flagged EA0 hide for map snapshot manager={:#x} entry={:#x}",
                    manager,
                    entry
                );
            }
        }
        // EAManager::update keeps being pumped by the natural GUI main loop;
        // only the clTimerHelper hide expiry is missing under emulation.
        // Emulate timer event 1 (state 4 -> 5), whose updateEAHide case 5
        // runs LayerSync::copyLayer to snapshot the map layer.
        11 => {
            let entry = LSYNC_EA_ENTRY.load(Ordering::Relaxed);
            let manager = LSYNC_EA_MANAGER.load(Ordering::Relaxed);
            let ticks = LSYNC_HIDE_TICKS.fetch_add(1, Ordering::Relaxed);
            if manager == 0 {
                LSYNC_MAP_STAGE.store(99, Ordering::Relaxed);
                log::info!("PROCHMI: EA hide monitor gave up (no manager)");
            } else if read_u32(unicorn, manager + 0x130) != 0 {
                LSYNC_MAP_STAGE.store(99, Ordering::Relaxed);
                log::info!(
                    "PROCHMI: map layer snapshot texture ready manager={:#x} ticks={}",
                    manager,
                    ticks
                );
            } else if ticks > 2000 {
                LSYNC_MAP_STAGE.store(99, Ordering::Relaxed);
                log::info!("PROCHMI: EA hide monitor timed out ticks={}", ticks);
            } else if read_u32(unicorn, entry + 0x24) == 4 && ticks > 20 {
                let _ = write_u32(unicorn, entry + 0x24, 5);
                let _ = write_u8(unicorn, manager + 0x12c, 1);
                log::info!(
                    "PROCHMI: emulated EA hide timer expiry; copyLayer snapshot armed entry={:#x}",
                    entry
                );
            }
        }
        _ => {}
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
        && gui_startup_anim_enabled()
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
            match thread.status {
                ThreadStatus::Blocked(BlockReason::FutexWait { .. })
                | ThreadStatus::Blocked(BlockReason::FutexWaitShared { .. }) => {
                    thread.status = ThreadStatus::Runnable;
                    thread.pending_result = Some(0);
                    woke = true;
                }
                ThreadStatus::Blocked(BlockReason::SleepUntil(_)) => {
                    thread.status = ThreadStatus::Runnable;
                    thread.pending_result = None;
                    woke = true;
                }
                _ => {}
            }
        }
    }

    if woke && !HMI_EVENT_WAKE_LOGGED.swap(true, Ordering::Relaxed) {
        log::info!("PROCHMI: forced HMI_MAIN event wake for HMI_FW_LOOP");
    }
}

fn ensure_guest_call_stub(unicorn: &mut Unicorn<'_, Context>) -> Option<u32> {
    let current = PROCHMI_GUEST_CALL_STUB.load(Ordering::Relaxed);
    if current != 0 {
        return Some(current);
    }

    let mmu_arc = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    let addr = mmu_arc
        .lock()
        .unwrap()
        .heap_alloc(
            unicorn,
            GUEST_CALL_STUB_SIZE,
            Prot::READ | Prot::WRITE | Prot::EXEC,
            "[prochmi-call-stub]",
        );
    let stub_code = [
        0x34, 0xc0, 0x8c, 0xe2, // add r12, sp, #0x34
        0xff, 0x4f, 0xbd, 0xe8, // pop {r0-r11,lr}
        0x00, 0xa0, 0xb0, 0xe8, // ldmia r12, {sp,pc}
    ];
    if addr == 0 || unicorn.mem_write(addr as u64, &stub_code).is_err() {
        log::warn!("PROCHMI: failed to allocate ARM r0-r11+sp+pc guest-call stub");
        return None;
    }

    PROCHMI_GUEST_CALL_STUB.store(addr, Ordering::Relaxed);
    log::info!("PROCHMI: allocated guest-call stub at {:#x}", addr);
    Some(addr)
}

fn call_guest_function(
    unicorn: &mut Unicorn<'_, Context>,
    original_pc: u32,
    function: u32,
    args: [u32; 4],
) -> bool {
    if original_pc == 0 || function == 0 {
        return false;
    }

    let Some(stub) = ensure_guest_call_stub(unicorn) else {
        return false;
    };

    let sp = unicorn.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
    if sp < 0x1000 {
        log::warn!("PROCHMI: refusing guest call from invalid SP {:#x}", sp);
        return false;
    }

    let saved = [
        unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
        unicorn.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
        unicorn.reg_read(RegisterARM::R2).unwrap_or(0) as u32,
        unicorn.reg_read(RegisterARM::R3).unwrap_or(0) as u32,
        unicorn.reg_read(RegisterARM::R4).unwrap_or(0) as u32,
        unicorn.reg_read(RegisterARM::R5).unwrap_or(0) as u32,
        unicorn.reg_read(RegisterARM::R6).unwrap_or(0) as u32,
        unicorn.reg_read(RegisterARM::R7).unwrap_or(0) as u32,
        unicorn.reg_read(RegisterARM::R8).unwrap_or(0) as u32,
        unicorn.reg_read(RegisterARM::R9).unwrap_or(0) as u32,
        unicorn.reg_read(RegisterARM::R10).unwrap_or(0) as u32,
        unicorn.reg_read(RegisterARM::R11).unwrap_or(0) as u32,
        unicorn.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
        sp,
        original_pc,
        0,
    ];
    let new_sp = sp.wrapping_sub(saved.len() as u32 * 4) & !7;
    if !saved
        .iter()
        .enumerate()
        .all(|(index, value)| write_u32(unicorn, new_sp + (index as u32 * 4), *value))
    {
        log::warn!(
            "PROCHMI: failed to save caller state at {:#x} before guest call {:#x}",
            new_sp,
            function
        );
        return false;
    }

    unicorn
        .reg_write(RegisterARM::R13, new_sp as u64)
        .unwrap_or_default();
    for (register, value) in [
        (RegisterARM::R0, args[0]),
        (RegisterARM::R1, args[1]),
        (RegisterARM::R2, args[2]),
        (RegisterARM::R3, args[3]),
        (RegisterARM::R14, stub),
        (RegisterARM::PC, function),
    ] {
        let _ = unicorn.reg_write(register, value as u64);
    }

    true
}

fn maybe_force_hmi_fi_init(unicorn: &mut Unicorn<'_, Context>, original_pc: u32) -> bool {
    if true || HMI_FIS_INIT_DONE.load(Ordering::Relaxed) {
        return false;
    }
    let hmi = HMI_MNGR_POINTER.load(Ordering::Relaxed);
    if hmi == 0 {
        return false;
    }
    let factory = read_u32(unicorn, hmi + 0x18);
    if factory == 0 {
        let count = HMI_MAINLOOP_DIAG_COUNT.load(Ordering::Relaxed);
        if count < 5 {
            log::info!("PROCHMI: FI init waiting for HSI FI factory hmi={:#x}", hmi);
        }
        return false;
    }
    let base = PROCHMI_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return false;
    }
    let function = base + CL_HSI_MNGR_BINIT_FIS;
    if call_guest_function(unicorn, original_pc, function, [hmi, 0, 0, 0]) {
        HMI_FIS_INIT_DONE.store(true, Ordering::Relaxed);
        log::info!(
            "PROCHMI: forced clHSIMngr::bInitFIs hmi={:#x} factory={:#x}",
            hmi,
            factory
        );
        return true;
    }
    false
}

fn maybe_inject_hsi_power_state(unicorn: &mut Unicorn<'_, Context>, original_pc: u32) -> bool {
    let Some(power_state) = hsi_power_state_stub() else {
        return false;
    };
    let phase = HSI_POWER_STATE_PENDING.load(Ordering::Relaxed);
    if phase == 0 {
        return false;
    }

    let base = PROCHMI_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return false;
    }

    let hmi = HMI_MNGR_POINTER.load(Ordering::Relaxed);
    if hmi == 0 {
        if hsi_power_state_trace_allowed() {
            log::warn!("PROCHMI: HSI power-state emulation waiting for clHMIMngr pointer");
        }
        return false;
    }

    let manager = hmi + CL_HMI_MNGR_CMMNGR_OFFSET;
    let factory = read_u32(unicorn, manager + HSI_CM_MANAGER_COMPONENT_ARRAY_OFFSET);
    if factory == 0 {
        if hsi_power_state_trace_allowed() {
            log::warn!("PROCHMI: HSI power-state emulation waiting for HSI factory at {:#x}", manager + 4);
        }
        return false;
    }

    let mut component = HSI_CM_STARTUP_COMPONENT.load(Ordering::Relaxed);
    if component == 0 {
        let array_slot =
            factory + HSI_CM_MANAGER_COMPONENT_ARRAY_OFFSET + (HSI_CM_STARTUP_ID * 4);
        component = read_u32(unicorn, array_slot);
        if component != 0 {
            HSI_CM_STARTUP_COMPONENT.store(component, Ordering::Relaxed);
        }
    }

    if component == 0 {
        if phase == HSI_POWER_STATE_PENDING_CREATE {
            let function = base + CL_HSI_CM_MANAGER_PHSI_BASE_GET;
            if call_guest_function(unicorn, original_pc, function, [manager, HSI_CM_STARTUP_ID, 0, 0]) {
                HSI_POWER_STATE_PENDING.store(HSI_POWER_STATE_PENDING_SEND, Ordering::Relaxed);
                log::info!(
                    "PROCHMI: created HSI startup component via clHSI_CMMngr::pHSI_BaseGet manager={:#x} id={}",
                    manager,
                    HSI_CM_STARTUP_ID
                );
                return true;
            }

            HSI_POWER_STATE_PENDING.store(0, Ordering::Relaxed);
            log::warn!("PROCHMI: failed to call clHSI_CMMngr::pHSI_BaseGet for HSI startup component");
            return false;
        }

        HSI_POWER_STATE_PENDING.store(0, Ordering::Relaxed);
        log::warn!("PROCHMI: HSI startup component was not created by pHSI_BaseGet");
        return false;
    }

    let adapter = hmi + CL_HMI_MNGR_EVENT_ADAPTER_OFFSET;
    if read_u32(unicorn, component + HSI_CM_BASE_EVENT_ENGINE) == 0 {
        let _ = write_u32(unicorn, component + HSI_CM_BASE_EVENT_ENGINE, adapter);
    }

    let function = base + HSI_CM_STARTUP_BEXECUTE_MESSAGE;
    if call_guest_function(
        unicorn,
        original_pc,
        function,
        [component, HSI_POWER_STATE_MESSAGE, power_state, 0],
    ) {
        HSI_POWER_STATE_PENDING.store(0, Ordering::Relaxed);
        log::info!(
            "PROCHMI: simulated HSI power-state message obj={:#x} msg={:#x} hsi_state={:#x}",
            component,
            HSI_POWER_STATE_MESSAGE,
            power_state
        );
        return true;
    }

    HSI_POWER_STATE_PENDING.store(0, Ordering::Relaxed);
    log::warn!("PROCHMI: failed to call clHSI_CMStartup::bExecuteMessage for HSI power-state");
    false
}

fn install_svg_map_surface_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    for (offset, kind) in [
        (SVG_GET_LAYER_BY_NAME, "layer_by_name"),
        (SVG_GET_LAYER_STATUS, "layer_status"),
        (SVG_GET_SURFACE_STATUS, "surface_status"),
        (SVG_GET_LAYER_ERROR, "layer_error"),
        (SVG_GET_RESOURCE_ERROR, "resource_error"),
        (SVG_APPLY_LAYER_IN_SYNC, "apply_in_sync"),
        (SVG_WAIT_LAYER_VSYNC, "wait_vsync"),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let result = match kind {
                    "layer_by_name" | "layer_status" => SVG_MAP_LAYER_HANDLE,
                    "surface_status" => SVG_MAP_SURFACE_HANDLE,
                    _ => 0,
                };
                if kind == "layer_status" {
                    let status = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    write_svg_layer_status(uc, status);
                } else if kind == "surface_status" {
                    let status = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    write_svg_surface_status(uc, status);
                }
                let name_arg = if kind == "layer_by_name" {
                    let ptr = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let len = (uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32).min(64) as usize;
                    let mut buf = vec![0u8; len];
                    if uc.mem_read(ptr as u64, &mut buf).is_ok() {
                        String::from_utf8_lossy(&buf)
                            .trim_end_matches('\0')
                            .to_string()
                    } else {
                        "<unread>".to_string()
                    }
                } else {
                    String::new()
                };
                let count = SVG_HOOK_HIT_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 20 {
                    log::info!(
                        "PROCHMI: SVG hook {} fired count={} r0={:#x} r1={:#x} r2={:#x} name_arg={} ret={:#x}",
                        kind,
                        count,
                        uc.reg_read(RegisterARM::R0).unwrap_or(0),
                        uc.reg_read(RegisterARM::R1).unwrap_or(0),
                        uc.reg_read(RegisterARM::R2).unwrap_or(0),
                        name_arg,
                        result
                    );
                }
                if !SVG_BYPASS_LOGGED.swap(true, Ordering::Relaxed) {
                    log::info!("PROCHMI: bypassing SVG layer-sync resource functions");
                }
                uc.reg_write(RegisterARM::R0, result as u64).unwrap();
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0);
                uc.reg_write(RegisterARM::PC, lr).unwrap();
            })
            .unwrap();
    }
}

fn force_ea_manager_background(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let addr = base_address + GUI_DM_EA_MANAGER_GET_BACKGROUND;
    unicorn
        .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
            let ea = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let layer_copy = ea + 0x130;
            let texture = read_u32(uc, layer_copy);
            if texture != 0 {
                let count = GL_LAYER_COPY_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 40 {
                    log::info!(
                        "PROCHMI: forced EA background count={} ea={:#x} layer_copy={:#x} texture={:#x}",
                        count,
                        ea,
                        layer_copy,
                        texture
                    );
                }
                HMI_MAINLOOP_TRACE_ARMED.store(true, Ordering::Relaxed);
                uc.reg_write(RegisterARM::R0, layer_copy as u64).unwrap();
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0);
                uc.reg_write(RegisterARM::PC, lr).unwrap();
            }
        })
        .unwrap();
}





fn force_display_manager_dirty(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let addr = base_address + GUI_DM_DISPLAY_MANAGER_IS_DIRTY;
    unicorn
        .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
            uc.reg_write(RegisterARM::R0, 1).unwrap();
            let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0);
            uc.reg_write(RegisterARM::PC, lr).unwrap();
        })
        .unwrap();
}



fn force_hmi_mainloop_display_update(unicorn: &mut Unicorn<'_, Context>) {
    let base = PROCHMI_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    let engine = unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    if engine != 0 {
        let _ = write_u8(unicorn, engine + 0x3d, 0);
    }
    let singleton_slot = read_u32(unicorn, base + GOT_GUI_DM_DISPLAY_MANAGER_SINGLETON);
    let display_manager = read_u32(unicorn, singleton_slot);
    if display_manager != 0 {
        let _ = write_u8(unicorn, display_manager + 0x3c, 1);
    }
}

fn force_hmi_view_draw_regions(unicorn: &mut Unicorn<'_, Context>) {
    if HMI_VIEW_DIRTY_FORCE_COUNT.load(Ordering::Relaxed) >= 120 {
        return;
    }

    let base = PROCHMI_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    let instance_slot = read_u32(unicorn, base + GOT_GUI_MENU_MANAGER_INSTANCE);
    let menu_manager = read_u32(unicorn, instance_slot);
    if menu_manager == 0 {
        return;
    }

    for index in 0..4u32 {
        let widget = read_u32(unicorn, menu_manager + index * 4);
        let view = read_u32(unicorn, widget + 0x28);
        let draw_context = read_u32(unicorn, view + 0x40);
        if widget != 0 && view != 0 && draw_context != 0 {
            let _ = write_u8(unicorn, draw_context + 0x28, 0);
            if read_u32(unicorn, draw_context + 0xb8) == 0 {
                let _ = write_u32(unicorn, draw_context + 0xb8, 1);
            }
        }
    }

    let forced = HMI_VIEW_DIRTY_FORCE_COUNT.fetch_add(1, Ordering::Relaxed);
    if forced < 10 {
        log::info!("PROCHMI: forced HMI view draw regions count={}", forced);
    }
}

fn trace_hmi_view_draw_contexts(unicorn: &mut Unicorn<'_, Context>) {
    force_hmi_view_draw_regions(unicorn);
    let base = PROCHMI_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    let instance_slot = read_u32(unicorn, base + GOT_GUI_MENU_MANAGER_INSTANCE);
    let menu_manager = read_u32(unicorn, instance_slot);
    let logged = HMI_VIEW_CLEAN_DIAG_COUNT.load(Ordering::Relaxed);
    if logged < 5 {
        log::info!(
            "PROCHMI: MenuManager instance_slot={:#x} menu_manager={:#x}",
            instance_slot,
            menu_manager
        );
    }
    if menu_manager == 0 {
        return;
    }
    for index in 0..4u32 {
        let widget = read_u32(unicorn, menu_manager + index * 4);
        let view = read_u32(unicorn, widget + 0x28);
        let draw_context = read_u32(unicorn, view + 0x40);
        if logged < 5 {
            log::info!(
                "PROCHMI: HMI view {} widget={:#x} view={:#x} draw_context={:#x} vptr={:#x} vtable_initialize={:#x} region_count={}",
                index,
                widget,
                view,
                draw_context,
                read_u32(unicorn, draw_context),
                read_u32(unicorn, read_u32(unicorn, draw_context) + GUI_GL_CONTEXT_INITIALIZE),
                read_u32(unicorn, draw_context + 0xb8)
            );
        }
    }
    HMI_VIEW_CLEAN_DIAG_COUNT.fetch_add(1, Ordering::Relaxed);
}

fn install_gl_layer_copy_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    for (offset, name) in [
        (
            GUI_GL_LAYER_SYNC_GET_LAYERS,
            "GUI_GL_LayerSync::getLayers",
        ),
        (GUI_GL_LAYER_SYNC_COPY_LAYER, "GUI_GL_LayerSync::copyLayer"),
        (
            GUI_GL_LAYER_SYNC_S_INITIALIZE,
            "GUI_GL_LayerSync::s_initialize",
        ),
        (
            GUI_GL_LAYER_SYNC_ON_SET_LAYER_NAMES,
            "GUI_GL_LayerSync::onSetLayerNames",
        ),
        (
            GUI_GL_LAYER_SYNC_APPLY_PENDING,
            "GUI_GL_LayerSync::applyPendingLayerSettings",
        ),
        (
            GUI_GL_LAYER_SYNC_REQUEST_VIEW_STATUS,
            "GUI_GL_LayerSync::requestViewStatus",
        ),
        (
            GUI_GL_LAYER_SYNC_ON_VIEW_STATUS_CHANGED,
            "GUI_GL_LayerSync::onViewStatusChanged",
        ),
        (
            GUI_GL_LAYER_SYNC_GET_VIEW_STATUS,
            "GUI_GL_LayerSync::getViewStatus",
        ),
        (
            GUI_DM_EA_MANAGER_REQUEST_EA_SHOW,
            "GUI_DM_EAManager::requestEAShow",
        ),
        (
            GUI_DM_EA_MANAGER_REQUEST_EA_STATE_CHANGE,
            "GUI_DM_EAManager::requestEAStateChange",
        ),
        (
            GUI_DM_EA_MANAGER_ON_VIEW_STATUS_CHANGED,
            "GUI_DM_EAManager::onViewStatusChanged",
        ),
        (
            GUI_DM_EA_MANAGER_REQUEST_EA_HIDE,
            "GUI_DM_EAManager::requestEAHide",
        ),
        (GUI_DM_EA_MANAGER_HIDE, "GUI_DM_EAManager::updateEAHide"),
        (
            GUI_DM_EA_MANAGER_REQUEST_DISPLAY_MODE_CHANGE,
            "GUI_DM_EAManager::requestDisplayModeChange",
        ),
        (
            GUI_DM_EA_MANAGER_SHOW,
            "GUI_DM_EAManager::updateEAShow",
        ),
        (GUI_DM_EA_MANAGER_UPDATE, "GUI_DM_EAManager::update"),
        (GUI_GL_LAYER_COPY_COPY, "GUI_GL_LayerCopy::copy"),
        (
            GUI_GL_LAYER_COPY_PERFORM_COPY,
            "GUI_GL_LayerCopy::performCopy",
        ),
        (GUI_GL_TEXTURE_CONSTRUCTOR, "GUI_GL_Texture::GUI_GL_Texture"),
        (GUI_GL_OPENGL_MIX_LAYERS, "GUI_GL_OpenGL::mixLayers"),
        (
            GUI_DM_DISPLAY_MANAGER_UPDATE,
            "GUI_DM_DisplayManager::update",
        ),
        (
            GUI_DISPLAY_MANAGER_THREAD,
            "GUI_DM_DisplayManager thread entry",
        ),
    ] {
        let addr = base_address + offset;
        unicorn
            .            add_code_hook(addr as u64, addr as u64, move |uc, address, _| {
                if name == "GUI_DM_DisplayManager thread entry" {
                    force_hmi_mainloop_display_update(uc);
                }
                if name == "GUI_DM_DisplayManager::update" {
                    trace_hmi_view_draw_contexts(uc);
                    if LSYNC_MAP_STAGE.load(Ordering::Relaxed) >= 10 {
                        let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                        post_lsync_map_announcement(uc, lr);
                    }
                }
                if name == "GUI_DM_EAManager::updateEAShow" {
                    LSYNC_EA_MANAGER.store(uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32, Ordering::Relaxed);
                    LSYNC_EA_ENTRY.store(uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32, Ordering::Relaxed);
                }
                if name == "GUI_GL_OpenGL::mixLayers"
                    && LSYNC_MAP_STAGE.load(Ordering::Relaxed) == 99
                {
                    // The LayerCopy map snapshot texture is taken once when
                    // the map layer hides and never refreshed; keep it in sync
                    // with procmap's live surface so the map animates behind
                    // the HMI.
                    let frames = LSYNC_REFRESH_FRAMES.fetch_add(1, Ordering::Relaxed);
                    if frames % 30 == 0 {
                        let manager = LSYNC_EA_MANAGER.load(Ordering::Relaxed);
                        let layer_copy = read_u32(uc, manager + 0x130);
                        let texture = read_u32(uc, layer_copy);
                        let tex_name = read_u32(uc, texture);
                        crate::gpu::refresh_texture_from_map_surface(
                            tex_name,
                            SVG_MAP_WIDTH as i32,
                            SVG_MAP_HEIGHT as i32,
                        );
                    }
                }
                if name == "GUI_GL_LayerSync::requestViewStatus"
                    && uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 == 0
                    && LSYNC_MAP_STAGE
                        .compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                {
                    log::info!(
                        "PROCHMI: prochmi asked EA0 view status; scheduling emulated procmap LayerSync reply"
                    );
                }

                let count = GL_LAYER_COPY_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 400 {
                    log::info!(
                        "PROCHMI: GL composition {} count={} r0={:#x} r1={:#x} r2={:#x} r3={:#x} lr={:#x}",
                        name,
                        count,
                        uc.reg_read(RegisterARM::R0).unwrap_or(0),
                        uc.reg_read(RegisterARM::R1).unwrap_or(0),
                        uc.reg_read(RegisterARM::R2).unwrap_or(0),
                        uc.reg_read(RegisterARM::R3).unwrap_or(0),
                        uc.reg_read(RegisterARM::LR)
                            .unwrap_or(0)
                            .wrapping_sub(base_address as u64),
                    );
                }
            })
            .unwrap();
    }

    let trace_start = base_address + GUI_MAINLOOP_TRACE_START;
    let trace_end = base_address + GUI_MAINLOOP_TRACE_END;
    unicorn
        .add_code_hook(trace_start as u64, trace_end as u64, move |uc, address, _| {
            if !HMI_MAINLOOP_TRACE_ARMED.load(Ordering::Relaxed) {
                return;
            }
            let count = HMI_MAINLOOP_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if address as u32 == trace_end - 0x70 {
                let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                log::info!(
                    "PROCHMI: mainloop epilogue count={} sp={:#x} s0={:#x} s1={:#x} s2={:#x} s3={:#x} s4={:#x} s5={:#x} s6={:#x} s7={:#x} s8={:#x}",
                    count,
                    sp,
                    read_u32(uc, sp),
                    read_u32(uc, sp + 4),
                    read_u32(uc, sp + 8),
                    read_u32(uc, sp + 0xc),
                    read_u32(uc, sp + 0x10),
                    read_u32(uc, sp + 0x14),
                    read_u32(uc, sp + 0x18),
                    read_u32(uc, sp + 0x1c),
                    read_u32(uc, sp + 0x20)
                );
            }
            if count < 3000 {
                log::info!(
                    "PROCHMI: mainloop trace {} pc={:#x} r0={:#x} r1={:#x} r2={:#x} r3={:#x} lr={:#x} sp={:#x}",
                    count,
                    address,
                    uc.reg_read(RegisterARM::R0).unwrap_or(0),
                    uc.reg_read(RegisterARM::R1).unwrap_or(0),
                    uc.reg_read(RegisterARM::R2).unwrap_or(0),
                    uc.reg_read(RegisterARM::R3).unwrap_or(0),
                    uc.reg_read(RegisterARM::LR).unwrap_or(0),
                    uc.reg_read(RegisterARM::SP).unwrap_or(0),
                );
            }
        })
        .unwrap();
}

fn write_svg_layer_status(unicorn: &mut Unicorn<'_, Context>, status: u32) {
    if status == 0 {
        return;
    }
    let mut data = vec![0u8; 0x40];
    write_u16_at(&mut data, 0x10, 0);
    write_u16_at(&mut data, 0x12, 0);
    write_u16_at(&mut data, 0x14, SVG_MAP_WIDTH);
    write_u16_at(&mut data, 0x16, SVG_MAP_HEIGHT);
    write_u16_at(&mut data, 0x18, 0);
    write_u16_at(&mut data, 0x1a, 0);
    write_u32_at(&mut data, 0x1c, SVG_MAP_SURFACE_HANDLE);
    let _ = unicorn.mem_write(status as u64, &data);
}

fn write_svg_surface_status(unicorn: &mut Unicorn<'_, Context>, status: u32) {
    if status == 0 {
        return;
    }
    let Some(base) = crate::gpu::write_map_surface_to_guest(unicorn) else {
        return;
    };
    {
        let mut probe = [0u8; 16];
        let _ = unicorn.mem_read((base + 200 * SVG_MAP_PITCH as u32 + 400 * 4) as u64, &mut probe);
        let mut nonblack = 0usize;
        let mut max_alpha = 0u8;
        let mut row = vec![0u8; (SVG_MAP_WIDTH as usize) * 4];
        if unicorn
            .mem_read((base + 240 * SVG_MAP_PITCH as u32) as u64, &mut row)
            .is_ok()
        {
            for px in row.chunks(4) {
                if px[0] | px[1] | px[2] != 0 {
                    nonblack += 1;
                }
                max_alpha = max_alpha.max(px[3]);
            }
        }
        log::info!(
            "PROCHMI: svg map surface probe base={:#x} midpx={:02x?} row240_nonblack={} max_alpha={:#x}",
            base,
            &probe[..4],
            nonblack,
            max_alpha
        );
    }
    // SVGSurfaceStatus layout recovered from GUI_GL_LayerCopy::performCopy:
    // +0x00 pixel base pointer, +0x08 u16 row pitch in bytes (used as
    // y_start * pitch + base), +0x0c format enum (1 = RGBA, 5 = RGB565),
    // +0x10 u16 row pitch again - performCopy divides it by 4 to obtain the
    // texture width in pixels.
    let mut data = vec![0u8; 0x40];
    write_u32_at(&mut data, 0x00, base);
    write_u32_at(&mut data, 0x04, 0);
    write_u16_at(&mut data, 0x08, SVG_MAP_PITCH);
    write_u16_at(&mut data, 0x0a, 4);
    write_u32_at(&mut data, 0x0c, 1);
    write_u16_at(&mut data, 0x10, SVG_MAP_PITCH);
    write_u32_at(&mut data, 0x34, SVG_MAP_SURFACE_HANDLE);
    let _ = unicorn.mem_write(status as u64, &data);
}

fn write_u16_at(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32_at(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
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
