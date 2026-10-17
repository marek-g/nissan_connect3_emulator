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
/// `GUI_FISystemMsg_Receiver::vOnServiceAvailable` - sets the receiver's ready byte
/// at `+0x1c` and runs the Lua callback that hides the "Starting navigation." popup.
const GUI_NAV_V_ON_SERVICE_AVAILABLE: u32 = 0x023a_9d28 - ORIGINAL_BASE;
/// `GUI_State::vSetProperties` - creates the MapService receiver for PROCNAV's
/// navigation service (0x0027).
const GUI_NAV_V_SET_PROPERTIES: u32 = 0x023a_9890 - ORIGINAL_BASE;
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
const SVG_ENABLE_LAYER: u32 = 0x00fb_6e84 - ORIGINAL_BASE;
const SVG_DISABLE_LAYER: u32 = 0x00fb_6a1c - ORIGINAL_BASE;
const SVG_SET_LAYER_WINDOW_DATA: u32 = 0x00fb_64dc - ORIGINAL_BASE;
const SVG_SET_LAYER_ORDER: u32 = 0x00fb_6e30 - ORIGINAL_BASE;
const SVG_SET_LAYER_BACKGROUND_COLOR: u32 = 0x00fb_6404 - ORIGINAL_BASE;
const SVG_SCALE_LAYER: u32 = 0x00fb_6548 - ORIGINAL_BASE;
const SVG_ENABLE_LAYER_SETTINGS: u32 = 0x00fb_671c - ORIGINAL_BASE;
const SVG_DISABLE_LAYER_SETTINGS: u32 = 0x00fb_65c0 - ORIGINAL_BASE;
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
// Inputs of GUI_GL_OpenGL::mixLayers that decide whether the HMI layer is
// cleared opaque (bit 0) or with a premultiplied alpha (bit 1: an external
// application layer copy is active).  0xffff = not observed yet.
static MIX_LAYERS_STATE: AtomicU32 = AtomicU32::new(0xffff);
static MIX_LAYERS_STATE_HITS: AtomicU32 = AtomicU32::new(0);
// The emulator is the display engine for the svg layer API: it has to remember
// which external layers the HMI enabled and where, the same way the hardware
// layer compositor would.  Only the map layer (SVG_MAP_LAYER_HANDLE) is fed to
// the compositor so far.
static SVG_LAYER_ENABLED: AtomicBool = AtomicBool::new(false);
static SVG_LAYER_X: AtomicU32 = AtomicU32::new(0);
static SVG_LAYER_Y: AtomicU32 = AtomicU32::new(0);
static SVG_LAYER_W: AtomicU32 = AtomicU32::new(0);
static SVG_LAYER_H: AtomicU32 = AtomicU32::new(0);
static SVG_LAYER_CONTROL_HITS: AtomicU32 = AtomicU32::new(0);

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

// r757: the startup internal events used to flush the moment they were armed
// (t=3.18s) - BEFORE the widget engine had painted the "Starting navigation."
// popup (first string read t~3.4s).  The 0x8d transition then left the popup's
// state (`0x1e5` -> `0x1e4`) while the widget did not exist yet, so the
// state's exit-hide found nothing and the later-painted popup stayed forever.
// The flush now waits for the popup's string to be read once (paint marker,
// set by the popup-string read hook, installed unconditionally) plus a settle
// delay; a fallback releases the events on runs where the popup never paints.
static POPUP_PAINT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn popup_paint_marker() {
    if POPUP_PAINT_MS
        .compare_exchange(
            0,
            crate::libs::dapi::emu_t0_elapsed_ms() as u64,
            Ordering::Relaxed,
            Ordering::Relaxed,
        )
        .is_ok()
    {
        log::info!(
            "PROCHMI: popup paint marker set at t={}ms (startup events release at paint+750ms)",
            crate::libs::dapi::emu_t0_elapsed_ms()
        );
    }
}

fn startup_events_release_now() -> bool {
    let paint = POPUP_PAINT_MS.load(Ordering::Relaxed);
    let now = crate::libs::dapi::emu_t0_elapsed_ms() as u64;
    if paint != 0 {
        now >= paint + 750
    } else {
        now >= 12_000
    }
}
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

/// Startup GUI internal events driving the HMI lock state machine
/// (LOCKED -> CHECKING -> ON).  r755: firing 0xcb SYSTEM_ALLOW_USER_ACTION
/// after these was tried and changes nothing - the "Starting navigation."
/// popup is not user-action gated; the hide path goes through the HSI
/// MsgBoxSystem component (see docs/issues/correctness.md).
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
    // r755: dedicated wide cap - prepareCall runs for every Lua-driven
    // widget/event and the shared 128 limit stopped the trace before boot
    // finished.
    LUA_CALL_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 4000
}

fn gui_event_name_trace_allowed() -> bool {
    if std::env::var_os("EMU_PROCHMI_GUI_EVENT_TRACE").is_none() {
        return false;
    }
    // r755: the registry lookup runs for every fired/dispatched event, so
    // the shared 128-call cap hid most of the Lua event name registry.  The
    // log line itself is deduplicated by id, so a wide cap stays quiet.
    LUA_CALL_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 6000
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
        crate::os::code_stub::trap_site(unicorn, addr, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:422", move |uc: &mut Unicorn<'_, Context>| {
                let result = if offset == SVG_INIT_RESOURCE {
                    0
                } else {
                    svg_fake_handle(uc)
                };
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                if !SVG_BYPASS_LOGGED.swap(true, Ordering::Relaxed) {
                    log::info!("PROCHMI: bypassing SVG resource functions");
                }
                uc.reg_write(RegisterARM::R0, result as u64).unwrap();
                lr
            });
    }

    install_svg_map_surface_hooks(unicorn, base_address);
    install_svg_layer_control_hooks(unicorn, base_address);

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
        
crate::os::code_stub::trap_site(unicorn, addr, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:464", move |_: &mut Unicorn<'_, Context>| {
                log::info!("PROCHMI: hit {}", name);
            
                0
            });
    }

    for (offset, store) in [
        (CL_HMI_MNGR_C1, "hmi_mngr"),
        (CL_HMI_MNGR_C2, "hmi_mngr"),
        (CL_GUI_WIDGET_C1, "gui_widget"),
        (CL_GUI_WIDGET_C2, "gui_widget"),
    ] {
        let addr = base_address + offset;
        
crate::os::code_stub::trap_site(unicorn, addr, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:478", move |uc: &mut Unicorn<'_, Context>| {
                let ptr = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                if ptr != 0 {
                    if store == "hmi_mngr" {
                        HMI_MNGR_POINTER.store(ptr, Ordering::Relaxed);
                    } else {
                        HMI_GUI_POINTER.store(ptr, Ordering::Relaxed);
                    }
                    log::debug!("PROCHMI: recorded {} object {:#x}", store, ptr);
                }
            
                0
            });
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
        
crate::os::code_stub::trap_site(unicorn, addr, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:505", move |uc: &mut Unicorn<'_, Context>| {
                let handler = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let force = matches!(
                    label,
                    "clHmiNavServerHandler vHMIInitialization"
                        | "clHmiNavServerHandler vHandleInitMapScreen"
                        | "clHmiNavServerHandler vForceMapInitialization"
                        | "clHmiNavServerHandler vHandleOnRequestCreateRenderView"
                );
                log_hmi_nav_handler_state(uc, handler, label, force || nav_trace_enabled());
            
                0
            });
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
        
crate::os::code_stub::trap_site(unicorn, addr, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:539", move |uc: &mut Unicorn<'_, Context>| {
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
            
                0
            });
    }

    let lua_do_file = base_address + GUI_LUA_DOFILE;
    
crate::os::code_stub::trap_site(unicorn, lua_do_file, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:569", move |uc: &mut Unicorn<'_, Context>| {
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
        
            0
        });

    let lua_prepare_call = base_address + GUI_LUA_PREPARE_CALL;
    
crate::os::code_stub::trap_site(unicorn, lua_prepare_call, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:585", move |uc: &mut Unicorn<'_, Context>| {
            if !lua_call_trace_allowed() {
                return 0;
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
        
            0
        });

    let gui_mainloop = base_address + CL_GUI_MAINLOOP;
    crate::os::code_stub::trap_site(unicorn, gui_mainloop, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:606", move |uc: &mut Unicorn<'_, Context>| {
            let address = gui_mainloop;
            // r751: merged with the old same-address narrow hook
            // (`GUI_DM_DisplayManager thread entry`): legacy hooks stacked,
            // trap sites do not, so the thread-entry logic runs here after
            // the mainloop body - on both paths, exactly like the second
            // stacked hook fired regardless of the first one's early return.
            let forced = if maybe_force_hmi_fi_init(uc, address) {
                true
            } else if maybe_inject_hsi_power_state(uc, address) {
                true
            } else {
                false
            };

            if !forced {
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
            }
            force_hmi_mainloop_display_update(uc);
            0
        });

    let check_msgbox = base_address + CL_GUI_CHECK_MSGBOX;
    
crate::os::code_stub::trap_site(unicorn, check_msgbox, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:653", move |uc: &mut Unicorn<'_, Context>| {
            let address = check_msgbox as u64;
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
            // r755-diag: (removed vtable peek: singleton was null at first
            // tick; the live dispatch is traced at the parseMessage sites
            // themselves instead.)
            // Stages >=10 mutate registers via guest calls; they must not run
            // from this hook because the stub return re-enters bCheckMsgBox's
            // own code hook. They run from the DisplayManager::update hook
            // with original_pc = caller LR.
            if LSYNC_MAP_STAGE.load(Ordering::Relaxed) < 10 {
                post_lsync_map_announcement(uc, address as u32);
            }
        
            0
        });

    // r755-diag: both message-box dispatchers (GUI + System variants) log
    // every queue message they consume, so the "Starting navigation." popup
    // create opcode becomes visible and the matching hide opcode can be
    // fabricated through the same GUI_UTIL_Queue the LayerSync announcements
    // already ride.  parseMessage(this, opcode): slot = 0x4*(opcode+1).
    for (offset, label) in [
        (0x0236_880cu32, "GUI_MessageBoxGUI::parseMessage"),
        (0x0236_9210u32, "GUI_MessageBoxSystem::parseMessage"),
    ] {
        let address = base_address + (offset - ORIGINAL_BASE);
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:msgbox-parse", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = HITS.fetch_add(1, Ordering::Relaxed);
            if n >= 48 {
                return 0;
            }
            let obj = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let opcode = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let vt = read_u32(uc, obj);
            let slot = if vt != 0 && (1..=7).contains(&opcode) {
                read_u32(uc, vt + (opcode + 1) * 4)
            } else {
                0
            };
            log::info!(
                "PROCHMI msgbox-parse {label} t={}ms thread=[{}] n={} obj={:#x} opcode={} vt={:#x} handler={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                n,
                obj,
                opcode,
                vt,
                slot,
            );
            0
        });
    }

    for offset in [
        GUI_UTIL_QUEUE_READER_C1,
        GUI_UTIL_QUEUE_READER_C2,
        GUI_UTIL_QUEUE_READER_BEGIN,
    ] {
        let addr = base_address + offset;
        
crate::os::code_stub::trap_site(unicorn, addr, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:684", move |uc: &mut Unicorn<'_, Context>| {
                if GUI_UTIL_MSGBOX_QUEUE.load(Ordering::Relaxed) != 0 {
                    return 0;
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
            
                0
            });
    }

    let msg_on_message = base_address + GUI_MSG_ON_MESSAGE;
    
crate::os::code_stub::trap_site(unicorn, msg_on_message, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:708", move |uc: &mut Unicorn<'_, Context>| {
            let messaging = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            if messaging != 0 {
                GUI_MESSAGING_POINTER.store(messaging, Ordering::Relaxed);
            }
            if GUI_INTERNAL_POST_PENDING.load(Ordering::Relaxed) != 0
                && messaging != 0
                && GUI_INTERNAL_EVENT_SENT_COUNT.load(Ordering::Relaxed) < GUI_INTERNAL_EVENT_COUNT
                && startup_events_release_now()
            {
                post_gui_internal_events(uc);
            }
            // r755-diag: enumerate the receiver lists bOnMessage walks
            // (`this[4]`/`this[5]` = +0x14/+0x18, receiver node event id at
            // +0xa, fwl_List-style next at +4) to learn every GUI event the
            // Lua side is waiting on - the unnamed ones are where the
            // "Starting navigation." hide trigger must live.
            {
                static DUMPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let d = DUMPS.fetch_add(1, Ordering::Relaxed);
                if messaging != 0
                    && matches!(d, 2 | 9 | 40 | 150 | 600 | 2400 | 9600)
                {
                    let mut raw: Vec<String> = Vec::new();
                    for i in 0..24u32 {
                        raw.push(format!("{:#x}", read_u32(uc, messaging + i * 4)));
                    }
                    log::info!("PROCHMI: GUI_Messaging obj raw n={d} {raw:?}");
                    for (base, tag) in [(0x10u32, "idle"), (0x14u32, "frozen")] {
                        let mut ids: Vec<String> = Vec::new();
                        let mut node = read_u32(uc, messaging + base);
                        for _ in 0..48 {
                            if node == 0 || node > 0xf000_0000 {
                                break;
                            }
                            ids.push(format!("{:#x}", read_u16(uc, node + 0xa)));
                            node = read_u32(uc, node + 4);
                        }
                        log::info!(
                            "PROCHMI: GUI_Messaging receiver list[{tag}] n={d} ids={ids:?}"
                        );
                    }
                }
            }
        
            0
        });

    // Which services the HMI is told about.  NOTE (r755): symtab proved both
    // offsets are MISATTRIBUTED - 0x023a9d28 lives inside
    // `HSA_System_TestMode__blBTDevConnectedServices` and 0x023a9890 inside
    // `..._ulwGetUsbDeviceSrvStatus`, so these traps watch Bluetooth/USB test
    // getters, not the nav receiver, and correctly see zero hits.  Kept as
    // cheap canaries; the real path is re-derived from the popup string below.
    for (offset, label) in [
        (GUI_NAV_V_ON_SERVICE_AVAILABLE, "vOnServiceAvailable"),
        (GUI_NAV_V_SET_PROPERTIES, "GUI_State::vSetProperties"),
    ] {
        let address = base_address + offset;
        
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:733", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: [std::sync::atomic::AtomicU32; 2] = [
                    std::sync::atomic::AtomicU32::new(0),
                    std::sync::atomic::AtomicU32::new(0),
                ];
                let index = if offset == GUI_NAV_V_SET_PROPERTIES { 1 } else { 0 };
                let hits = HITS[index].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if hits >= 24 {
                    return 0;
                }
                let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let message = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let content = read_u32(uc, message + 0x04);
                log::info!(
                    "PROCHMI nav-service trace {label} t={}ms thread=[{}] this={:#x} r1={:#x} ready={} svc={:#x} opcode={:#x} caller={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    this,
                    message,
                    if this != 0 { read_u8(uc, this + 0x1c) } else { 0xff },
                    if content != 0 { read_u16(uc, content + 0x14) } else { 0xffff },
                    if content != 0 { read_u8(uc, content + 0x1a) } else { 0xff },
                    uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                );
            
                0
            });
    }

    // r755/r757: find the popup's presenter and hider from the string itself.
    // "Starting navigation." sits at VA 0x025186fc (file offset 0x25106fc in
    // the LOAD segment based at 0x8000) and IS presented during boot, so a
    // MEM_READ hook over the string reports every pc that touches it - the
    // first hits are the presenter, any later distinct pc is the hider or a
    // re-paint.  Since r757 the hook is installed unconditionally: any read of
    // the string marks the popup as painted (`popup_paint_marker`), which is
    // the release signal for the startup internal events.  The (cheap on this
    // cold page) trace logging stays env-gated.
    {
        use unicorn_engine::unicorn_const::HookType;
        fn popup_string_trace_enabled() -> bool {
            static ENABLED: OnceLock<bool> = OnceLock::new();
            *ENABLED.get_or_init(|| std::env::var_os("EMU_PROCHMI_POPUP_TRACE").is_some())
        }
        // "Starting navigation." VA 0x025186fc (file 0x25106fc),
        // "STARTING NAVIGATION" VA 0x02452d6c (file 0x244ad6c) - both + 0x8000.
        for (lo, hi, tag) in [
            (0x0251_86f0u64, 0x0251_8720u64, "Starting navigation."),
            (0x0245_2d50u64, 0x0245_2d90u64, "STARTING NAVIGATION"),
        ] {
            let _tag = tag;
            let res = unicorn.add_mem_hook(
                HookType::MEM_READ,
                lo,
                hi,
                move |_uc, _t, address, size, _value| {
                    popup_paint_marker();
                    if !popup_string_trace_enabled() {
                        return true;
                    }
                    static SEEN: std::sync::LazyLock<
                        std::sync::Mutex<std::collections::HashMap<(u32, u64), u32>>,
                    > = std::sync::LazyLock::new(|| {
                        std::sync::Mutex::new(std::collections::HashMap::new())
                    });
                    static TEL: std::sync::atomic::AtomicU32 =
                        std::sync::atomic::AtomicU32::new(0);
                    let pc = _uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                    let elf = _uc.get_data().elf_path.clone();
                    let tid = _uc.get_data().inner.thread_id();
                    let count = {
                        let mut s = match SEEN.lock() {
                            Ok(s) => s,
                            Err(_) => return true,
                        };
                        if s.len() > 4096 {
                            return true;
                        }
                        let e = s.entry((pc, address)).or_insert(0);
                        *e += 1;
                        *e
                    };
                    // New (pc, addr): always. Repeats: log the 20th and then
                    // every 256th, so a steady-state re-paint is visible
                    // without flooding and the popup-hide pass is not missed.
                    let tel = TEL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if count > 1 && count != 20 && count % 256 != 20 {
                        return true;
                    }
                    let sp = _uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    let mut chain: Vec<u32> = Vec::new();
                    if sp > 0x1000 && sp < 0xf000_0000 {
                        for i in 0..192u32 {
                            let w = read_u32(_uc, sp + i * 4);
                            if (w >= 0x8000 && w < 0x0286_5294)
                                || (w >= 0x47d0_0000 && w < 0x4860_0000)
                            {
                                if !chain.contains(&w) {
                                    chain.push(w);
                                    if chain.len() >= 12 {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    log::warn!(
                        "PROCHMI popup-string READ({tag}) {} [{}] addr={:#x} size={} n={} tel={tel} pc={:#x} chain={:?}",
                        elf,
                        tid,
                        address,
                        size,
                        count,
                        pc,
                        chain.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>(),
                    );
                    true
                },
            );
            log::warn!("PROCHMI popup-string read-hook({tag}) installed: {}", res.is_ok());
        }
    }

    // r755: real service-state pipeline (symtab-verified, unlike the old
    // misattributed GUI_NAV_* canaries).  prochmi's HMI-CCA framework is the
    // same `ail_tclInternalDispatch`/`hmicca_tclApp` family procmapengine
    // uses.  These observers answer one question per hit: does the nav
    // (svc 0x0027) receiver registration / status path ever run at all, and
    // with what arguments.
    for (offset, label) in [
        (0x00fc_9348 - ORIGINAL_BASE, "hmicca_tclApp::bRegisterForService"),
        (0x00fc_80c8 - ORIGINAL_BASE, "hmicca_tclApp::vOnServiceState"),
        (0x00fe_01b0 - ORIGINAL_BASE, "ail_bHandleMsgServiceStatus"),
        (0x00fe_03c0 - ORIGINAL_BASE, "ail_bHandleMsgServiceRegisterConf"),
        (0x00fe_08b0 - ORIGINAL_BASE, "ail_bHandleMsgServiceRegister"),
        (0x00fd_ef38 - ORIGINAL_BASE, "ail_vOnServerApplicationStatus"),
        (0x00fe_8a94 - ORIGINAL_BASE, "ail_bIpcMessageWait"),
        (0x00fe_27cc - ORIGINAL_BASE, "ail_bDispatchCCAMessages"),
        (0x00fd_f8d8 - ORIGINAL_BASE, "ail_bHandleMsgAppInfoMessage"),
        (0x00fd_2950 - ORIGINAL_BASE, "ail_bServerApplicationListAvailable"),
        (0x00fd_4d88 - ORIGINAL_BASE, "ail_Restricted::bInitInstance"),
        (0x00fd_3f9c - ORIGINAL_BASE, "ail_Restricted::vDeinitInstance"),
        (0x00fd_7a88 - ORIGINAL_BASE, "ail_OneThreadAppInterface::bRegisterAsync"),
        (0x00fd_6730 - ORIGINAL_BASE, "ail::bSendCCASrvSupplierMsg"),
        (0x00fd_60b0 - ORIGINAL_BASE, "ail::bSendAppInfoMsg"),
        (0x00fe_4338 - ORIGINAL_BASE, "ail_bTakeListAccess"),
    ] {
        let address = base_address + offset;
        
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:ail-svc", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: std::sync::LazyLock<
                    std::sync::Mutex<std::collections::HashMap<&'static str, u32>>,
                > = std::sync::LazyLock::new(|| std::sync::Mutex::new(Default::default()));
                let n = {
                    let mut m = match HITS.lock() {
                        Ok(m) => m,
                        Err(_) => return 0,
                    };
                    let e = m.entry(label).or_insert(0);
                    *e += 1;
                    *e
                };
                if n > 32 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                // For the AppInfo dispatch (r0 = ail_tclInternalDispatch*)
                // dump the server-application list behind *r0+0x4c: decides
                // whether the registration gate fails for having no list at
                // all.  List nodes: {+0x10 u16 instance, +0x12 u16 appid,
                // +0x14 u8 state} (from bRegisterAsync's iterator walk).
                let mut extra = String::new();
                if label.ends_with("vOnServerApplicationStatus") {
                    let dispatch = read_u32(uc, r0);
                    let list_head = if dispatch != 0 {
                        read_u32(uc, dispatch + 0x4c)
                    } else {
                        0
                    };
                    let mut items: Vec<String> = Vec::new();
                    if list_head != 0 && list_head < 0xf000_0000 {
                        let mut node = read_u32(uc, list_head + 4);
                        for _ in 0..8 {
                            if node == 0 || node > 0xf000_0000 {
                                break;
                            }
                            let inst = read_u16(uc, node + 0x10);
                            let app = read_u16(uc, node + 0x12);
                            let st = read_u8(uc, node + 0x14);
                            items.push(format!("{{inst:{inst:#x} app:{app:#x} state:{st}}}"));
                            node = read_u32(uc, node + 4);
                        }
                    }
                    extra = format!(" list_head={list_head:#x} items={items:?}");
                }
                if label.ends_with("ail_bServerApplicationListAvailable") {
                    // Gate: *(this+0x4c) != 0 - the same fwl_List that
                    // bRegisterAsync walks as this[0x13] (int* -> byte 0x4c).
                    // Nodes: next=[head+4], payload at oItem(); dump node words
                    // raw to locate {inst, appid, state}.
                    let list_obj = read_u32(uc, r0 + 0x4c);
                    let mut items: Vec<String> = Vec::new();
                    if list_obj != 0 && list_obj < 0xf000_0000 {
                        let mut node = read_u32(uc, list_obj + 4);
                        for _ in 0..6 {
                            if node == 0 || node > 0xf000_0000 || node == list_obj {
                                break;
                            }
                            let w0 = read_u32(uc, node + 8);
                            let w1 = read_u32(uc, node + 0xc);
                            let w2 = read_u32(uc, node + 0x10);
                            let w3 = read_u32(uc, node + 0x14);
                            items.push(format!("{{+8:{w0:#x} +c:{w1:#x} +10:{w2:#x} +14:{w3:#x}}}"));
                            node = read_u32(uc, node + 4);
                        }
                    }
                    extra = format!(" list={list_obj:#x} nodes={items:?}");
                }
                if label.ends_with("bRegisterAsync") {
                    // (this, param_2..param_4 in r1..r3, param_5/param_6 on the
                    // stack): param_6 == 0xffff selects the srv-supplier path,
                    // otherwise the app-info path.
                    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    let p5 = if sp > 0x1000 && sp < 0xf000_0000 { read_u32(uc, sp) } else { 0 };
                    let p6 = if sp > 0x1000 && sp < 0xf000_0000 { read_u32(uc, sp + 4) } else { 0 };
                    extra = format!(" p5={p5:#x} p6={p6:#x}");
                }
                if label.ends_with("bDispatchCCAMessages") && r1 > 0x1000 && r1 < 0xf000_0000 {
                    // (dispatch, msg-ref*, size, flags): the amt wrapper reads
                    // its wire fields from the referenced content ([r1+4]).
                    // Dump the first body bytes to see what the pump received.
                    let content = read_u32(uc, r1 + 4);
                    let mut bytes: Vec<String> = Vec::new();
                    if content > 0x1000 && content < 0xf000_0000 {
                        for i in 0..5u32 {
                            let mut w = [0u8; 4];
                            if uc.mem_read((content + i * 4) as u64, &mut w).is_err() {
                                break;
                            }
                            bytes.push(format!("{:08x}", u32::from_le_bytes(w)));
                        }
                    }
                    extra = format!(" content={content:#x} body={bytes:?}");
                }
                log::info!(
                    "PROCHMI ail-svc {label} t={}ms thread=[{}] n={n} r0={:#x} r1={:#x} r2={:#x} r3={:#x} lr={:#x}{}{}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    r0,
                    r1,
                    r2,
                    r3,
                    lr,
                    extra,
                    if label.ends_with("bRegisterForService") && n <= 3 {
                        // hmicca_tclApp::bRegisterForService forwards to
                        // vtable+0x58: name the real implementation once, and
                        // peek the client object behind it.
                        let vt = read_u32(uc, r0);
                        let impl58 = if vt != 0 { read_u32(uc, vt + 0x58) } else { 0 };
                        format!(" vt={vt:#x} vtable58={impl58:#x}")
                    } else {
                        String::new()
                    },
                );
            
                0
            });
    }

    // r755: `hmicca_tclApp::vOnServiceState` forwards the event through
    // clHMIMngr: `piVar1 = *(mngr_ifc + 8); (*piVar1->vtable[0x18])(piVar1,
    // svc, serverApp, version, state, subid)`.  At 0xfc8180 (right before
    // `ldr pc, [ip, #0x18]`) r0 is the receiving object, R10 the state byte
    // and R9 the sub-id; resolve the dispatch target so the nav (0x27)
    // event's real GUI consumer gets a name.
    let nav_state_dispatch = base_address + (0x00fc_8180u32 - ORIGINAL_BASE);
    crate::os::code_stub::trap_site(unicorn, nav_state_dispatch, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:nav-state-dispatch", move |uc: &mut Unicorn<'_, Context>| {
        static HITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = HITS.fetch_add(1, Ordering::Relaxed);
        if n >= 40 {
            return 0;
        }
        let obj = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
        let svc = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
        let server = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
        let version = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
        let subid = uc.reg_read(RegisterARM::R9).unwrap_or(0) as u32;
        let state = uc.reg_read(RegisterARM::R10).unwrap_or(0) as u32;
        let vt = read_u32(uc, obj);
        let target = if vt != 0 { read_u32(uc, vt + 0x18) } else { 0 };
        log::info!(
            "PROCHMI nav-state-dispatch t={}ms thread=[{}] n={} obj={:#x} svc={:#x} server={:#x} version={:#x} state={} subid={:#x} vt={:#x} target={:#x}",
            crate::libs::dapi::emu_t0_elapsed_ms(),
            uc.get_data().inner.thread_id(),
            n,
            obj,
            svc,
            server,
            version,
            state,
            subid,
            vt,
            target,
        );
        0
    });

    // r757: enavi-server bring-up chain probes (popup chain in
    // docs/issues/correctness.md).  `vStartupEnaviServer` is the ONLY writer of
    // the FI interface fields (+0x7dc IEnavi, +0x814 IDAPI, +0x15c clFi_Navi)
    // that make `vHandleEnaviServerStatus` get past its early-return guards.
    // It has two live call sites: `clFi_Navi::vNewAppState` (needs previous fw
    // state 4 AND byte +0x34 == 1) and a branch inside
    // `clFi_Navi::ReceiveEventsFromEvtEn` (VA 0x14be3fc: sets +0x34 = 1 and
    // calls it) selected by event id 0x80108031 (switch base 0x80108030, ARM
    // jump table at 0x14bcdf8).  The constructor zeroes +0x34, so that event
    // branch is the only way the gate ever opens.  `poGetIEnaviServer` is the
    // lazy singleton behind +0x7dc.  One EMU_PROCHMI_NAV_TRACE=1 run names the
    // dead link.
    for (offset, label) in [
        (0x015e_0f74u32 - ORIGINAL_BASE, "enavi-startup"),
        (0x015e_0bacu32 - ORIGINAL_BASE, "enavi-shutdown"),
        (0x015e_2070u32 - ORIGINAL_BASE, "enavi-status"),
        (0x014b_9148u32 - ORIGINAL_BASE, "fi-nav-instance"),
        (0x014b_9aacu32 - ORIGINAL_BASE, "fi-nav-appstate"),
        (0x014b_b270u32 - ORIGINAL_BASE, "fi-nav-svcstate"),
        (0x014b_cd74u32 - ORIGINAL_BASE, "fi-nav-events"),
        (0x0135_28a0u32 - ORIGINAL_BASE, "hsi-fievent-router"),
        (0x0180_a95cu32 - ORIGINAL_BASE, "hsi-cmd"),
        (0x0180_a8bcu32 - ORIGINAL_BASE, "sys-event"),
        (0x0182_b7d4u32 - ORIGINAL_BASE, "cmnavi-msg"),
        (0x0182_7b04u32 - ORIGINAL_BASE, "cm-mngr-msg"),
        (0x00fc_80c8u32 - ORIGINAL_BASE, "hmicca-svcstate"),
        (0x00fc_68e4u32 - ORIGINAL_BASE, "fi-dispatch"),
        (0x00fc_9ae8u32 - ORIGINAL_BASE, "cca-getnext"),
        (0x013c_9a70u32 - ORIGINAL_BASE, "gw-regclient"),
        (0x0135_2d8cu32 - ORIGINAL_BASE, "deliver-fi"),
        (0x013c_9314u32 - ORIGINAL_BASE, "gw-lookup"),
        (0x013c_931cu32 - ORIGINAL_BASE, "gw-route"),
        (0x00fc_9af4u32 - ORIGINAL_BASE, "cca-del"),
        (0x00fd_0428u32 - ORIGINAL_BASE, "cpm-update"),
        (0x0135_2df4u32 - ORIGINAL_BASE, "hsi-mngr-appstate"),
        (0x0161_f754u32 - ORIGINAL_BASE, "ienvi-singleton"),
        (0x0134_ec20u32 - ORIGINAL_BASE, "hmi-appstate-set"),
        (0x00fc_9788u32 - ORIGINAL_BASE, "appstates-entry-off"),
        (0x00fc_9888u32 - ORIGINAL_BASE, "appstates-entry-pause"),
        (0x00fc_95d4u32 - ORIGINAL_BASE, "appstates-entry-x5d4"),
        (0x00fc_96ccu32 - ORIGINAL_BASE, "appstates-entry-normal"),
        (0x00fc_6574u32 - ORIGINAL_BASE, "appstates-onnew"),
        // r758: the HMI-side server object.  `vStartupEnaviServer` is a method
        // of clHmiNavServerHandler, and the component's own bring-up reads
        // FUNCTIONAL_COMPONENT\HMINAVSERVER (fix105: prochmi never opens those
        // keys, so the chain presumably dies before the registry read).  The
        // constructor tells whether the handler object is even instantiated;
        // vOpenPathHmiNavServerRegistry / u32GetHmiNavServerThreadPriority are
        // its first two registry-touching steps; bIsNavServerReady answers the
        // question the wait-popup widget asks.
        (0x015e_871cu32 - ORIGINAL_BASE, "navsrv-ctor"),
        (0x0169_dba8u32 - ORIGINAL_BASE, "navsrv-reg-open"),
        (0x00fc_4660u32 - ORIGINAL_BASE, "reg-bopen"),
        (0x013c_a4c8u32 - ORIGINAL_BASE, "svcstate-srv"),
        (0x013c_a534u32 - ORIGINAL_BASE, "svcstate-find"),
        (0x013c_a560u32 - ORIGINAL_BASE, "svcstate-sink"),
        (0x0169_e5f8u32 - ORIGINAL_BASE, "navsrv-prio"),
        (0x015f_4828u32 - ORIGINAL_BASE, "navsrv-avail"),
        (0x0161_fca8u32 - ORIGINAL_BASE, "ienvi-srvready"),
    ] {
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let label: &'static str = label;
        crate::os::code_stub::trap_site(unicorn, base_address + offset, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:enavi-chain", move |uc: &mut Unicorn<'_, Context>| {
            if !nav_trace_enabled() {
                return 0;
            }
            let n = hits.fetch_add(1, Ordering::Relaxed);
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            // The wait-widget's release event must survive the probe budget,
            // so it is exempt from the per-site cap.  The FI-event router is
            // uncapped: its full event spectrum names the feeder of the
            // enavi-start event 0x80108031.
            if n >= 12
                && !((label == "fi-nav-events" || label == "hsi-fievent-router")
                    && (r1 == 0x8010_8031 || r1 == 0x8100_8031))
                && label != "hsi-fievent-router"
                && label != "sys-event"
                && label != "hsi-cmd"
                && label != "cmnavi-msg"
                && label != "cm-mngr-msg"
                && label != "hmicca-svcstate"
                && label != "fi-dispatch"
                && label != "cca-getnext"
                && label != "gw-regclient"
                && label != "deliver-fi"
                && label != "gw-lookup"
                && label != "gw-route"
                && label != "cca-del"
                && label != "cpm-update"
                && label != "svcstate-srv"
                && label != "svcstate-find"
                && label != "svcstate-sink"
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            let extra = match label {
                "enavi-startup" | "enavi-status" => format!(
                    " this={r0:#x} navsrv={:#x} ienavi={:#x} idapi={:#x} gate830={:#x} state148={:#x}",
                    read_u32(uc, r0 + 0x15c),
                    read_u32(uc, r0 + 0x7dc),
                    read_u32(uc, r0 + 0x814),
                    read_u32(uc, r0 + 0x830),
                    read_u32(uc, r0 + 0x148),
                ),
                "hsi-mngr-appstate" => {
                    let state = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    // clHSIMngr lives at clHMIMngr+0x4e4, so the framework
                    // state byte (+0x670) sits at hsi+0x18c: dump it beside the
                    // broadcast parameter to see whether the byte already
                    // changed or the value was lost between the two functions.
                    let byte = read_u8(uc, r0.wrapping_add(0x18c));
                    format!(" state={state:#x} hsi={r0:#x} byte670={byte}")
                }
                "fi-nav-appstate" => {
                    let state = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    format!(
                        " this={r0:#x} state={state:#x} handler={:#x} gate34={} laststate={}",
                        read_u32(uc, r0 + 0x30),
                        read_u8(uc, r0 + 0x34),
                        read_u32(uc, 0x02a7_23e0),
                    )
                }
                "fi-nav-svcstate" => format!(" this={r0:#x} hsi={:#x}", read_u32(uc, r0 + 8)),
                // r758: `vOpenPathHmiNavServerRegistry` runs (t~2.35 s) but no
                // OSAL_IOOpen follows: dump the static reg_tclRegKey object's
                // vtable (0x0597e990, BSS), the path pointer global
                // (0x054e4bf0) and what it points at, plus the "opened" flag
                // (0x0597edb0), to see which precondition drops the open.
                "navsrv-reg-open" => {
                    let vptr = read_u32(uc, 0x0597_e990);
                    let pathp = read_u32(uc, 0x054e_4bf0);
                    let first = if pathp != 0 { read_u8(uc, pathp) } else { 0 };
                    let flag = read_u32(uc, 0x0597_edb0);
                    format!(" vptr={vptr:#x} pathp={pathp:#x} byte0={first} openedflag={flag}")
                }
                "reg-bopen" => {
                    let vptr = if r0 != 0 { read_u32(uc, r0) } else { 0 };
                    let head = if r1 > 0x1000 {
                        (read_u8(uc, r1), read_u8(uc, r1.wrapping_add(1)))
                    } else {
                        (0, 0)
                    };
                    format!(" key={r0:#x} path={r1:#x} head=[{:02x} {:02x}] vptr={vptr:#x}", head.0, head.1)
                }
                // r758: clCCAGateway(+0x18) service-state method (dispatched
                // for every vOnServiceState): entry args and the client-port
                // lookup result.  svc lives in r6 (callee-saved across the
                // lookup call), server in r7, state in sl([sp+0x50]); only
                // interesting for the NAVI service.
                "svcstate-srv" => {
                    let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                    if r6 != 0x27 {
                        return 0;
                    }
                    let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let r10 = uc.reg_read(RegisterARM::R10).unwrap_or(0) as u32;
                    // +0x24 selects between the direct notify loop and the
                    // client-port lookup path; +0x1c is the port manager the
                    // lookup runs against.
                    let gate = read_u8(uc, r0.wrapping_add(0x24));
                    let mgr = read_u32(uc, r0.wrapping_add(0x1c));
                    format!(
                        " this={r0:#x} svc={r6:#x} server={r7:#x} version={r2:#x} state={r10:#x} gate24={gate} portmgr={mgr:#x}"
                    )
                }
                "svcstate-find" => {
                    let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                    if r6 != 0x27 {
                        return 0;
                    }
                    // stack packet handed to the lookup: svc/server/version
                    // u16, state u8, subid u16 at sp+0x1e..0x27.
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let svc = read_u32(uc, sp.wrapping_add(0x1e)) & 0xffff;
                    let server = (read_u32(uc, sp.wrapping_add(0x1e)) >> 16) & 0xffff;
                    let ver_state = read_u32(uc, sp.wrapping_add(0x22));
                    format!(
                        " find_result={r0:#x} svc={svc:#x} server={server:#x} pkt={ver_state:#010x}"
                    )
                }
                // no client port -> indirect call on the fallback singleton's
                // vtable+8 (the two instructions before the site hold the
                // object in r0 and its vtable in r3)
                "svcstate-sink" => {
                    let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                    if r6 != 0x27 {
                        return 0;
                    }
                    let vt = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                    let target = if vt != 0 { read_u32(uc, vt + 8) } else { 0 };
                    // The sink is clHMIMngr; vtable+8 is a thunk that returns
                    // immediately while clHMIMngr+0x650 is null, so dump that
                    // field too: it names whether the notification dies at
                    // the guard.
                    let field = read_u32(uc, r0.wrapping_add(0x650));
                    format!(
                        " sink={r0:#x} vt={vt:#x} target={target:#x} mgr650={field:#x}"
                    )
                }
                "gw-regclient" => format!(" gw={r0:#x} fidata={r1:#x}"),
                "gw-lookup" => {
                    // clCCAGateway::bReadSharedMemory routing call site
                    // (mov lr,pc before ldr pc,[ip,#16]): r0 = clICCAGW_HSI
                    // handler, r1=svc, r2=src, r3=subid; its client vector is
                    // at +0xc..+0x10.  Counting clients tells us whether the
                    // nav svc 0x27 route entry was ever created.
                    let begin = read_u32(uc, r0.wrapping_add(0xc));
                    let end = read_u32(uc, r0.wrapping_add(0x10));
                    let k2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let k3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                    let n = if end > begin && (end - begin) % 4 == 0 {
                        (end - begin) / 4
                    } else {
                        u32::MAX
                    };
                    // Dump each clFiData route entry: svc@+8,
                    // sourceSubId@+10, registerId@+12, serverAppId@+18.
                    // pvGetHandler matches (svc, sourceSubId==src,
                    // registerId==wire subid@+0xe).
                    let mut dump = String::new();
                    if n != u32::MAX && n <= 64 {
                        for i in 0..n {
                            let e = read_u32(uc, begin + i * 4);
                            dump.push_str(&format!(
                                " [{i}]{e:#x}:{:#x}/{:#x}/{:#x}/{:#x}",
                                read_u16(uc, e + 8),
                                read_u16(uc, e + 10),
                                read_u16(uc, e + 12),
                                read_u16(uc, e + 18),
                            ));
                        }
                    }
                    format!(
                        " gw={r0:#x} svc={r1:#x} src={k2:#x} subid={k3:#x} clients={n}{dump}"
                    )
                }
                "deliver-fi" => {
                    // clHSIMngr::bInitFIs "DELIVER FI'S": the per-FI virtual
                    // into clHSIMngr+8 (the CCA-gateway registrar adapter).
                    // r1 = the FI object; FI 8 = clFi_Navi at 0x38f9970.
                    format!(" registrar={r0:#x} fi={r1:#x}")
                }
                "hsi-cmd" => {
                    let mut stack_refs: Vec<String> = Vec::new();
                    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    if sp > 0x1000 {
                        for i in 0..32u32 {
                            let w = read_u32(uc, sp + i * 4);
                            if (0x8000..0x0286_5294).contains(&w) && !stack_refs.contains(&format!("{w:#x}")) {
                                stack_refs.push(format!("{w:#x}"));
                                if stack_refs.len() >= 6 {
                                    break;
                                }
                            }
                        }
                    }
                    format!(" this={r0:#x} cmd={r1:#x} stack={stack_refs:?}")
                }
                "sys-event" => format!(" this={r0:#x} event={r1:#x}"),
                "fi-dispatch" => {
                    // hmicca_tclApp::vOnNewMessage's dispatch tail
                    // (`mov lr,pc; ldr pc,[r3,#8]`): r1 = the service-data
                    // object created from the wire message; its +4 points at
                    // the amt wire body.  Correlating this with the
                    // `clHSI_CMMngr::bExecuteMessage` sMsg.type names the
                    // wire->type conversion that must yield 0x80108031.
                    let sd = r1;
                    let vt = read_u32(uc, r0);
                    let tgt = read_u32(uc, vt.wrapping_add(8));
                    let w = read_u32(uc, sd.wrapping_add(4));
                    if (0x9000_0000..0x9200_0000).contains(&w) || (0x300_0000..0x500_0000).contains(&w) {
                        let mut hex = String::new();
                        for i in 0..0x28u32 {
                            hex.push_str(&format!("{:02x} ", read_u8(uc, w + i)));
                        }
                        format!(
                            " this={r0:#x} vt={vt:#x} target={tgt:#x} sd={sd:#x} wire={w:#x} bytes[{hex}]",
                        )
                    } else {
                        format!(" this={r0:#x} sd={sd:#x} wire={w:#x}")
                    }
                }
                "hmicca-svcstate" => {
                    // hmicca_tclApp::vOnServiceState(ushort svc, ushort sub,
                    // ushort id, byte state, ushort regid) - CCA tells the HMI
                    // a service changed state.  A READY report for the NAV
                    // service (0x27) is what should wake the HSI-NAV client and
                    // ultimately emit FI event 0x81008031.
                    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    let v2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let v3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                    // 6-arg member call: this,svc,sub,id in r0..r3; the byte
                    // `state` and the trailing `ushort regid` are passed on the
                    // stack (AAPCS promotes both to word slots).
                    let state = read_u32(uc, sp) & 0xff;
                    let regid = read_u32(uc, sp + 4) & 0xffff;
                    format!(
                        " this={r0:#x} svc={r1:#x} sub={v2:#x} id={v3:#x} state={state:#x} regid={regid:#x}"
                    )
                }
                "cpm-update" => {
                    // ahl_tclClientPortMngr::enUpdate(const amt_tclServiceData*)
                    // - client-port state propagation into the HMI app.
                    format!(
                        " this={r0:#x} data={r1:#x} d0={:#x} d1={:#x} d2={:#x} d3={:#x}",
                        read_u32(uc, r1),
                        read_u32(uc, r1 + 4),
                        read_u32(uc, r1 + 8),
                        read_u32(uc, r1 + 0xc),
                    )
                }
                "cm-mngr-msg" => {
                    // clHSI_CMMngr::bExecuteMessage(sMsg*) fans the message
                    // out to every clHSI_CM* client (CMNAVI included).  The
                    // backtrace names the queue pump that feeds it.
                    let mut stack_refs: Vec<String> = Vec::new();
                    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    if sp > 0x1000 {
                        for i in 0..40u32 {
                            let w = read_u32(uc, sp + i * 4);
                            if (0x8000..0x0286_5294).contains(&w) && !stack_refs.contains(&format!("{w:#x}")) {
                                stack_refs.push(format!("{w:#x}"));
                                if stack_refs.len() >= 8 {
                                    break;
                                }
                            }
                        }
                    }
                    // Verify the sMsg.type <-> wire linkage: dump the payload
                    // pointer dwords and, when one looks like a pointer into
                    // the materialised pool, the wire msgid@+0x10 it points at.
                    let d0 = read_u32(uc, r1);
                    let ty = read_u32(uc, r1 + 4);
                    let d2 = read_u32(uc, r1 + 8);
                    let p10 = read_u32(uc, r1 + 0x10);
                    let p14 = read_u32(uc, r1 + 0x14);
                    let wire = if (0x9000_0000..0x9100_0000).contains(&p10) {
                        read_u32(uc, p10 + 0x10)
                    } else if (0x9000_0000..0x9100_0000).contains(&p14) {
                        read_u32(uc, p14 + 0x10)
                    } else {
                        0
                    };
                    format!(
                        " msg={r1:#x} type={ty:#x} d0={d0:#x} d2={d2:#x} p10={p10:#x} p14={p14:#x} wiremsgid={wire:#x} stack={stack_refs:?}"
                    )
                }
                "cmnavi-msg" => {
                    // clHSI_CMNAVI::bExecuteMessage(sMsg*): sMsg+0x04 is a
                    // scalar message type the function dispatches on (values
                    // ~0x29xx); dump the header dwords.
                    let m = r1;
                    format!(
                        " this={r0:#x} msg={m:#x} d0={:#x} type={:#x} d2={:#x} d3={:#x}",
                        read_u32(uc, m),
                        read_u32(uc, m + 4),
                        read_u32(uc, m + 8),
                        read_u32(uc, m + 0xc),
                    )
                }
                "hsi-fievent-router" => {
                    // clHSIMngr (unnamed, 0x13528a0): routes event r1 to FI
                    // slot (r1 >> 12) & 0xff through that FI's vtable+0x3c.
                    // Backtrace names the producers of 0x810080xx (and should
                    // one day name the producer of the enavi-start event
                    // 0x80108031).
                    let mut stack_refs: Vec<String> = Vec::new();
                    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    if sp > 0x1000 {
                        for i in 0..24u32 {
                            let w = read_u32(uc, sp + i * 4);
                            if (0x8000..0x0286_5294).contains(&w) && !stack_refs.contains(&format!("{w:#x}")) {
                                stack_refs.push(format!("{w:#x}"));
                                if stack_refs.len() >= 6 {
                                    break;
                                }
                            }
                        }
                    }
                    format!(
                        " event={r1:#x} fi={:#x} hsi={r0:#x} stack={stack_refs:?}",
                        (r1 >> 12) & 0xff
                    )
                }
                "fi-nav-events" => {
                    format!(" this={r0:#x} event={r1:#x} gate34={}", read_u8(uc, r0 + 0x34))
                }
                "appstates-onnew" => {
                    let cur = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let new = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    format!(" app={r0:#x} cur={cur} new={new}")
                }
                "hmi-appstate-set" => {
                    let state = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let mut stack_refs: Vec<String> = Vec::new();
                    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    if sp > 0x1000 {
                        for i in 0..24u32 {
                            let w = read_u32(uc, sp + i * 4);
                            if (0x8000..0x0286_5294).contains(&w) && !stack_refs.contains(&format!("{w:#x}")) {
                                stack_refs.push(format!("{w:#x}"));
                                if stack_refs.len() >= 6 {
                                    break;
                                }
                            }
                        }
                    }
                    format!(" obj={r0:#x} state={state} prev={} stack={stack_refs:?}", read_u8(uc, r0 + 0x670))
                }
                _ => format!(" this={r0:#x}"),
            };
            log::info!(
                "PROCHMI enavi-chain {label} t={}ms thread=[{}] n={} caller={lr:#x}{extra}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                n,
            );
            0
        });
    }

    if nav_trace_allowed() {
        // r757: watch the clHMIMngr framework state byte (+0x670) and its
        // pending flag (+0x671) from t=0.  The object sits at 0x38eee60
        // (heap layout has been deterministic across runs fix77..fix80).
        // Installing the watch from the *load path* matters: a watch
        // installed during the first setter call missed every writer whose
        // translation block predates it - the code that pre-seeded the
        // initial NORMAL pending silently rewrote OFF(4) back to 3 with no
        // event (fix79/80: setter `str value=0x4` confirmed, yet
        // `vPerformNewAppState` read back 3).
        let watch_res = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            0x38ee_e60u64 + 0x670,
            0x38ee_e60u64 + 0x678,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                log::info!(
                    "PROCHMI appstate-byte WRITE t={}ms thread=[{}] addr={:#x} size={} value={:#x} pc={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    address,
                    size,
                    value,
                    uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32
                );
                true
            },
        );
        log::info!(
            "PROCHMI appstate-byte load-time write-watch installed: {}",
            watch_res.is_ok()
        );
    }

    let hmi_perform_new_app_state = base_address + CL_HMI_MNGR_PERFORM_NEW_APP_STATE;

    // r755: `ail_bHandleMsgServiceData` forwards a matched server data
    // response through the app-interface vtable slot +0x20.  At 0xfe2268
    // (right before `ldr pc, [r3, #0x20]`) r2 is the app interface, r1 the
    // amt message; resolve the receiver target and dump the wire body so
    // the nav (0x27) data path gains a named consumer.
    let svc_data_forward = base_address + (0x00fe_2268u32 - ORIGINAL_BASE);
    crate::os::code_stub::trap_site(unicorn, svc_data_forward, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:svc-data-forward", move |uc: &mut Unicorn<'_, Context>| {
        static HITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = HITS.fetch_add(1, Ordering::Relaxed);
        if n >= 32 {
            return 0;
        }
        let appif = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
        let msg = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
        let vt = read_u32(uc, appif);
        let target = if vt != 0 { read_u32(uc, vt + 0x20) } else { 0 };
        let mut bytes: Vec<String> = Vec::new();
        let content = read_u32(uc, msg + 4);
        if content > 0x1000 && content < 0xf000_0000 {
            for i in 0..8u32 {
                let mut w = [0u8; 4];
                if uc.mem_read((content + i * 4) as u64, &mut w).is_err() {
                    break;
                }
                bytes.push(format!("{:08x}", u32::from_le_bytes(w)));
            }
        }
        log::info!(
            "PROCHMI svc-data-forward t={}ms thread=[{}] n={} appif={:#x} vt={:#x} target={:#x} content={:#x} body={:?}",
            crate::libs::dapi::emu_t0_elapsed_ms(),
            uc.get_data().inner.thread_id(),
            n,
            appif,
            vt,
            target,
            content,
            bytes,
        );
        0
    });
    
crate::os::code_stub::trap_site(unicorn, hmi_perform_new_app_state, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:763", move |uc: &mut Unicorn<'_, Context>| {
                if !hmi_app_state_trace_allowed() {
                    return 0;
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
            
            0
        });

    let hsi_send_system_event = base_address + HSI_CM_BASE_SEND_SYSTEM_EVENT;
    
crate::os::code_stub::trap_site(unicorn, hsi_send_system_event, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:795", move |uc: &mut Unicorn<'_, Context>| {
            if !hsi_cm_event_trace_allowed() {
                return 0;
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
        
            0
        });

    let cm_startup_update_status = base_address + CL_HSI_CM_STARTUP_UPDATE_STATUS;
    
crate::os::code_stub::trap_site(unicorn, cm_startup_update_status, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:819", move |uc: &mut Unicorn<'_, Context>| {
                if !hsi_cm_event_trace_allowed() {
                    return 0;
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
            
            0
        });

    let cm_startup_check_status = base_address + CL_HSI_CM_STARTUP_CHECK_STATUS;
    
crate::os::code_stub::trap_site(unicorn, cm_startup_check_status, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:846", move |uc: &mut Unicorn<'_, Context>| {
                if !hsi_cm_event_trace_allowed() {
                    return 0;
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
            
            0
        });

    let gui_msg_event_name = base_address + GUI_MSG_EVENT_NAME_RETURN;
    
crate::os::code_stub::trap_site(unicorn, gui_msg_event_name, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:874", move |uc: &mut Unicorn<'_, Context>| {
            if !gui_event_name_trace_allowed() {
                return 0;
            }
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let event = if sp != 0 {
                read_u16(uc, sp + 0x4c)
            } else {
                0
            };
            let name_ptr = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let name = read_cstr(uc, name_ptr, 160);
            {
                use std::sync::Mutex;
                static SEEN: std::sync::LazyLock<Mutex<std::collections::HashSet<u16>>> =
                    std::sync::LazyLock::new(|| Mutex::new(Default::default()));
                if let Ok(mut s) = SEEN.lock() {
                    if !s.insert(event as u16) {
                        return 0;
                    }
                }
            }
            log::info!(
                "PROCHMI: GUI_Messaging event id={:#x} name={} name_ptr={:#x}",
                event,
                name,
                name_ptr
            );
        
            0
        });

    let gui_fire_internal_event = base_address + GUI_MSG_FIRE_INTERNAL_EVENT;
    
crate::os::code_stub::trap_site(unicorn, gui_fire_internal_event, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:897", move |uc: &mut Unicorn<'_, Context>| {
                if !gui_fire_event_trace_allowed() {
                    return 0;
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
            
            0
        });

    let hsi_factory_post_assign = base_address + HSI_FACTORY_CREATE_POST_ASSIGN;
    
crate::os::code_stub::trap_site(unicorn, hsi_factory_post_assign, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:931", move |uc: &mut Unicorn<'_, Context>| {
                let object = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                let component_id = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                let factory = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                if component_id != HSI_CM_STARTUP_ID || object == 0 {
                    return 0;
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
            
            0
        });

    let state_on_message = base_address + GUI_STATEMACHINE_ON_MESSAGE;
    
crate::os::code_stub::trap_site(unicorn, state_on_message, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:970", move |uc: &mut Unicorn<'_, Context>| {
            if !nav_trace_enabled() {
                return 0;
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
                return 0;
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
                    "PROCHMI: nav startup bOnMessage t={}ms sm={:#x} leaf={:#x} event={:#x} msg={:#x} caller={:#x} [0x{:08x},0x{:08x},0x{:08x},0x{:08x}]",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
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
        
            0
        });

    let state_perform_transition = base_address + GUI_STATEMACHINE_PERFORM_TRANSITION;
    
crate::os::code_stub::trap_site(unicorn, state_perform_transition, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:1018", move |uc: &mut Unicorn<'_, Context>| {
                if !nav_trace_enabled() {
                    return 0;
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
                    return 0;
                }
                let event = read_u16(uc, transition + GUI_TRANSITION_EVENT_ID);
                let source = read_u16(uc, transition + GUI_TRANSITION_SOURCE_STATE);
                let target = read_u16(uc, transition + GUI_TRANSITION_TARGET_STATE);
                let kind = read_u16(uc, transition + GUI_TRANSITION_TYPE);
                if nav_trace_allowed() {
                    log::info!(
                        "PROCHMI: GUI_StateMachine performTransition t={}ms sm={:#x} leaf={:#x} state={:#x} trans={:#x} event={:#x} src={:#x} tgt={:#x} kind={:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
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
            
            0
        });

    let state_do_transition = base_address + GUI_STATEMACHINE_DO_TRANSITION;
    
crate::os::code_stub::trap_site(unicorn, state_do_transition, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:1063", move |uc: &mut Unicorn<'_, Context>| {
            if !nav_trace_enabled() {
                return 0;
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
                return 0;
            }
            if nav_trace_allowed() {
                log::info!(
                    "PROCHMI: nav startup doTransition t={}ms sm={:#x} leaf={:#x} transition_id={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    sm,
                    leaf,
                    transition
                );
            }
        
            0
        });

    let state_add_transition = base_address + GUI_STATE_ADD_TRANSITION;
    
crate::os::code_stub::trap_site(unicorn, state_add_transition, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:1095", move |uc: &mut Unicorn<'_, Context>| {
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
            
            0
        });

    for (offset, store) in [
        (CL_HMI_MNGR_B_EXECUTE, "hmi_mngr"),
        (CL_GUI_WIDGET_B_EXECUTE, "gui_widget"),
    ] {
        let addr = base_address + offset;
        
crate::os::code_stub::trap_site(unicorn, addr, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:1145", move |uc: &mut Unicorn<'_, Context>| {
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
            
                0
            });
    }

    let hmi_main_entry = base_address + HMI_MAIN_ENTRY;
    
crate::os::code_stub::trap_site(unicorn, hmi_main_entry, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:1164", move |uc: &mut Unicorn<'_, Context>| {
            let tid = uc.get_data().thread_id();
            HMI_MAIN_THREAD_ID.store(tid, Ordering::Relaxed);
            log::info!("PROCHMI: recorded HMI_MAIN thread id {}", tid);
        
            0
        });
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
        // Nothing else is driven by hand: the hide/snapshot flow used to be
        // forced here (poking the EAWStatus command byte and the manager dirty
        // flag), but that made the HMI hide the very layer we want to see and
        // switched it into snapshot-on-hide mode.  The natural main loop keeps
        // pumping EAManager::update, and the svg layer control hooks now record
        // what the HMI asks the display engine to do.
        10 => {
            let manager = LSYNC_EA_MANAGER.load(Ordering::Relaxed);
            let entry = LSYNC_EA_ENTRY.load(Ordering::Relaxed);
            LSYNC_MAP_STAGE.store(99, Ordering::Relaxed);
            log::info!(
                "PROCHMI: map layer show flow complete, leaving EA state machine alone manager={:#x} entry={:#x} state={}",
                manager,
                entry,
                if entry == 0 {
                    0
                } else {
                    read_u32(unicorn, entry + 0x24)
                }
            );
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
    // TEMP r757 diagnostic: sample the framework state byte every scheduler
    // pass.  Runs fix79-81 proved +0x670 changes (0xff -> 3, 4 -> 3) with no
    // MEM_WRITE event even when the watch was live from t=0, so the writer is
    // not a guest instruction on this unicorn (host mem_write or a cross-VM
    // alias).  The sampler brackets the change in time to correlate it with
    // emulator-side activity.
    if nav_trace_allowed() && unicorn.get_data().elf_path.contains("prochmi") {
        static LAST_SAMPLE: std::sync::atomic::AtomicU32 =
            std::sync::atomic::AtomicU32::new(0xffff_ffff);
        let byte = read_u8(unicorn, 0x38ee_e60 + 0x670) as u32;
        let pending = read_u8(unicorn, 0x38ee_e60 + 0x671) as u32;
        let sample = byte | (pending << 8);
        let last = LAST_SAMPLE.swap(sample, Ordering::Relaxed);
        if last != sample {
            log::info!(
                "PROCHMI appstate-byte SAMPLE t={}ms {:#x} -> {:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                last,
                sample
            );
        }
    }
    // r757: release the PWR proxy's paced HMI messages (NORMAL after OFF) as
    // plain queue posts once their virtual-time deadline passes; the HMI
    // dispatcher never re-enters its Wait, so the Wait-serve channel is dead
    // for them (run_fix77).
    crate::libs::libosal_linux::message::post_due_power_messages(unicorn);
    if GUI_INTERNAL_POST_PENDING.load(Ordering::Relaxed) != 0
        && GUI_MESSAGING_POINTER.load(Ordering::Relaxed) != 0
        && GUI_INTERNAL_EVENT_SENT_COUNT.load(Ordering::Relaxed) < GUI_INTERNAL_EVENT_COUNT
        && startup_events_release_now()
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
        crate::os::code_stub::trap_site(unicorn, addr, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:1810", move |uc: &mut Unicorn<'_, Context>| {
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
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                lr
            });
    }
}

/// The svg layer entry points are stubs that jump into libsvg; on the unit they
/// program the hardware layer compositor.  Under emulation the emulator *is*
/// that compositor, so the layer control calls have to be intercepted: the map
/// layer's enabled state and window are exactly what a display engine needs to
/// know to composite `MAP_SURFACE_BYTES` below the HMI layer.  Answering with
/// "no error" (r0 = 0) is also what `LayerCopy::copy` and `setLayersVisible`
/// check before they go on.
fn install_svg_layer_control_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    for (offset, kind) in [
        (SVG_ENABLE_LAYER, "enable_layer"),
        (SVG_DISABLE_LAYER, "disable_layer"),
        (SVG_SET_LAYER_WINDOW_DATA, "set_window_data"),
        (SVG_SET_LAYER_ORDER, "set_order"),
        (SVG_SET_LAYER_BACKGROUND_COLOR, "set_bg_color"),
        (SVG_SCALE_LAYER, "scale_layer"),
        (SVG_ENABLE_LAYER_SETTINGS, "enable_settings"),
        (SVG_DISABLE_LAYER_SETTINGS, "disable_settings"),
    ] {
        let addr = base_address + offset;
        crate::os::code_stub::trap_site(unicorn, addr, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:1880", move |uc: &mut Unicorn<'_, Context>| {
                let handle = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                let stack0 = read_u32(uc, sp);
                if handle == SVG_MAP_LAYER_HANDLE {
                    match kind {
                        "enable_layer" => SVG_LAYER_ENABLED.store(true, Ordering::Relaxed),
                        "disable_layer" => SVG_LAYER_ENABLED.store(false, Ordering::Relaxed),
                        // svgSetLayerWindowData(handle, x, y, w, h): four
                        // one-per-register u32 arguments, the height in [sp]
                        // (measured: r1=0 r2=0 r3=800 stack0=480).
                        "set_window_data" => {
                            SVG_LAYER_X.store(r1, Ordering::Relaxed);
                            SVG_LAYER_Y.store(r2, Ordering::Relaxed);
                            SVG_LAYER_W.store(r3, Ordering::Relaxed);
                            SVG_LAYER_H.store(stack0, Ordering::Relaxed);
                        }
                        _ => {}
                    }
                    publish_svg_map_layer_window();
                }
                let hits = SVG_LAYER_CONTROL_HITS.fetch_add(1, Ordering::Relaxed);
                if hits < 40 {
                    log::info!(
                        "PROCHMI: SVG layer control {} handle={:#x} map_layer={} r1={:#x} r2={:#x} r3={:#x} stack0={:#x} enabled={}",
                        kind,
                        handle,
                        handle == SVG_MAP_LAYER_HANDLE,
                        r1,
                        r2,
                        r3,
                        stack0,
                        SVG_LAYER_ENABLED.load(Ordering::Relaxed)
                    );
                }
                // The layer functions return void on this ABI; callers only
                // look at svgGetLayerError(), which the status hooks keep at 0.
                uc.reg_write(RegisterARM::R0, 0).unwrap();
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                lr
            });
    }
}

/// Hand the display-engine state of the map layer (window requested by the HMI
/// through `LayerSync::setLayersVisible`, plus the enable/disable calls) to the
/// GPU compositor.
fn publish_svg_map_layer_window() {
    crate::gpu::set_map_layer_window(
        SVG_LAYER_ENABLED.load(Ordering::Relaxed),
        SVG_LAYER_X.load(Ordering::Relaxed),
        SVG_LAYER_Y.load(Ordering::Relaxed),
        SVG_LAYER_W.load(Ordering::Relaxed),
        SVG_LAYER_H.load(Ordering::Relaxed),
    );
}

fn force_ea_manager_background(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let addr = base_address + GUI_DM_EA_MANAGER_GET_BACKGROUND;
    crate::os::code_stub::trap_site(unicorn, addr, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:1942", move |uc: &mut Unicorn<'_, Context>| {
            let ea = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let layer_copy = ea + 0x130;
            let texture = read_u32(uc, layer_copy);
            if texture == 0 {
                return 0;
            }
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
            let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
            lr
        });
}





fn force_display_manager_dirty(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let addr = base_address + GUI_DM_DISPLAY_MANAGER_IS_DIRTY;
    crate::os::code_stub::trap_site(unicorn, addr, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:1971", move |uc: &mut Unicorn<'_, Context>| {
            uc.reg_write(RegisterARM::R0, 1).unwrap();
            let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
            lr
        });
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
        // Gate chain for the map (external application) layer.  prochni is the
        // only component that can make the display engine show procmap's layer:
        // `EAManager::getBackground` hands the LayerCopy to `mixLayers`, which
        // then calls `LayerCopy::draw`, and `setLayersVisible` is the one place
        // that enables the foreign svg layer at all.
        // (getBackground 0x0133f528 itself carries the functional forced-EA
        // background trap at `add_code_hooks`:1941 - registered only once.)
        (0x0133_ede4 - ORIGINAL_BASE, "GUI_DM_EAManager::isBlocked"),
        (0x0134_36e4 - ORIGINAL_BASE, "GUI_GL_LayerSync::setLayersVisible"),
        (0x0134_2b24 - ORIGINAL_BASE, "GUI_GL_LayerCopy::free"),
        (0x0134_2a28 - ORIGINAL_BASE, "GUI_GL_LayerCopy::draw"),
        (
            0x0133_b7b8 - ORIGINAL_BASE,
            "clGUIWidgetEngine::onGuiLSyncViewStatusChanged",
        ),
        (
            GUI_DM_DISPLAY_MANAGER_UPDATE,
            "GUI_DM_DisplayManager::update",
        ),
        // r751: `GUI_DM_DisplayManager thread entry` (0x0133be70) and
        // `GUI_DM_EAManager::getBackground` (0x0133f528) are dropped from the
        // observer list: both addresses carry functional trap sites already
        // (`bGUIMainloop` at 606 - the old narrow-hook overlap is merged into
        // it there - and the forced-EA-background trap at 1941), and trap
        // sites are unique per address, so a duplicate registration would
        // only demote the functional trap to a legacy fallback hook.
    ] {
        let addr = base_address + offset;
        crate::os::code_stub::trap_site(unicorn, addr, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:2157", move |uc: &mut Unicorn<'_, Context>| {
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
                        crate::gpu::refresh_texture_from_map_surface(
                            crate::gpu::map_layer_texture(),
                            SVG_MAP_WIDTH as i32,
                            SVG_MAP_HEIGHT as i32,
                        );
                    }
                }
                if name == "GUI_GL_OpenGL::mixLayers" {
                    // mixLayers(gl, layer_copy, &transparent): the opaque-black
                    // clear only happens when gl+0x140 is set, and the
                    // alpha-0 (premultiplied) clear only when the derived
                    // blend flag is false, i.e. when a layer copy is active or
                    // the background view (view 0) is absent.  Both inputs are
                    // logged here so the black map area can be attributed.
                    let gl = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let layer_copy = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let opaque_bg = read_u32(uc, gl + 0x140);
                    let signature = (opaque_bg != 0) as u32 | ((layer_copy != 0) as u32) << 1;
                    let last = MIX_LAYERS_STATE.swap(signature, Ordering::Relaxed);
                    let hits = MIX_LAYERS_STATE_HITS.fetch_add(1, Ordering::Relaxed);
                    if last != signature || hits < 4 {
                        log::info!(
                            "PROCHMI: mixLayers gl={:#x} layer_copy={:#x} opaque_bg={:#x} texture={:#x} count={}",
                            gl,
                            layer_copy,
                            opaque_bg,
                            if layer_copy == 0 {
                                0
                            } else {
                                read_u32(uc, layer_copy)
                            },
                            hits
                        );
                    }
                }
                if name == "GUI_GL_LayerSync::setLayersVisible" {
                    // setLayersVisible(layer_sync, ea, visible, rect).  The rect
                    // is what the HMI tells the display engine the external
                    // layer covers, so it is the region the emulator has to
                    // composite the map surface into.  Log both plausible
                    // layouts (4 x u32 and 4 x u16 GUI_Rect) until the field
                    // width is confirmed by the values themselves.
                    let ea = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let visible = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let rect = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                    if ea == 0 {
                        let words = [
                            read_u32(uc, rect),
                            read_u32(uc, rect + 4),
                            read_u32(uc, rect + 8),
                            read_u32(uc, rect + 12),
                        ];
                        log::info!(
                            "PROCHMI: setLayersVisible ea=0 visible={} rect={:#x} words=[{:#x},{:#x},{:#x},{:#x}] halves=[{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x}]",
                            visible,
                            rect,
                            words[0],
                            words[1],
                            words[2],
                            words[3],
                            words[0] & 0xffff,
                            words[0] >> 16,
                            words[1] & 0xffff,
                            words[1] >> 16,
                            words[2] & 0xffff,
                            words[2] >> 16,
                            words[3] & 0xffff,
                            words[3] >> 16
                        );
                        if visible != 0 {
                            SVG_LAYER_X.store(words[0], Ordering::Relaxed);
                            SVG_LAYER_Y.store(words[1], Ordering::Relaxed);
                            SVG_LAYER_W.store(words[2], Ordering::Relaxed);
                            SVG_LAYER_H.store(words[3], Ordering::Relaxed);
                        }
                        publish_svg_map_layer_window();
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
            0
        });
    }

    let trace_start = base_address + GUI_MAINLOOP_TRACE_START;
    let trace_end = base_address + GUI_MAINLOOP_TRACE_END;
    // r751: per-instruction range tracer over bGUIMainloop - pure
    // observation, and a 64 KiB bounded-hook range forces the per-instruction
    // dispatch check on every TB of the hot GUI loop (r743: the emulator's
    // main hook cost).  No trap-site counterpart exists for a fetch-sequence
    // trace, so it registers only under the prochmi trace env flags now; bare
    // runs pay zero range-hook cost.
    if nav_trace_enabled() || lua_call_trace_enabled() {
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
