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
// Exported widget-engine touch entry points (GUI_TouchPad_vPressed/vMoved/
// vReleased, both u16 args): the exact functions
// clHSI_CMKeyboard::bExecuteMessage drives from real touch-controller
// samples (sMsg+0x08/+0x0c).  Their timers are created by
// GUI_TouchPad_vInit (bl 0x136e5d4 from the widget-engine init), which runs
// during normal startup, so calling them once the GUI engine exists is safe.
const TOUCH_PAD_V_PRESSED: u32 = 0x0136_e418 - ORIGINAL_BASE;
const TOUCH_PAD_V_MOVED: u32 = 0x0136_e328 - ORIGINAL_BASE;
const TOUCH_PAD_V_RELEASED: u32 = 0x0136_e278 - ORIGINAL_BASE;
// GUI_KeyInjector's exported hardwired-key API (`.dynsym`): the exact
// functions `clHSI_CMKeyboard::bExecuteMessage` calls for real
// `midw_kbdfi_tclMsgKeyInfo` samples (47/46 bl sites inside it), so the
// widget engine + Lua HK_* state machines see a stock input path (guest code
// untouched).  `vKeyPressed` takes the id of its 51-entry key table (<= 50),
// rejects others; `vKeyReleased` only fires when the id matches the active
// key (state VA 0x02cb9574, active id at +8); the two encoder entries take a
// signed direction and fire |dir| events each.
const KEY_INJECTOR_V_KEY_PRESSED: u32 = 0x0136_8138 - ORIGINAL_BASE;
const KEY_INJECTOR_V_KEY_RELEASED: u32 = 0x0136_8028 - ORIGINAL_BASE;
const KEY_INJECTOR_V_ENCODER_MOVED: u32 = 0x0136_7f84 - ORIGINAL_BASE;
const KEY_INJECTOR_V_VOLUME_ENCODER_MOVED: u32 = 0x0136_7f6c - ORIGINAL_BASE;
/// `GUI_ContentString::ms_apcStrings`: content id -> C-string name (the key
/// table stores content ids, so the semantic name of every hardwired key is
/// resolved at runtime instead of guessing row numbers).
const GUI_CONTENT_STRINGS: u32 = 0x02a4_3b28 - ORIGINAL_BASE;
/// `GUI_KeyInjector`'s static 51 x 24-byte key table (id = row index):
/// +0x0c u16 press content id, +0x12 u16 short-release content id,
/// +0x14 u16 long-release content id.  Base pinned from vKeyPressed's
/// pc-relative literal pool (`0x2a12818 + 0xffb6a044` at the `add r5,pc,r5`
/// anchor 0x0136814c, so +0xc past the symbol-style guess 0x0257c854);
/// `vKeyReleased` confirms +0x12 (fires as type 4) and +0x14 (type 5).
/// Contents are filled by `vInit` at runtime, so the resolver must read
/// guest memory, not the file image.
const KEY_INJECTOR_KEY_TABLE: u32 = 0x0257_c85c - ORIGINAL_BASE;
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
static TOUCH_EVENTS_DELIVERED: AtomicU32 = AtomicU32::new(0);
static TOUCH_POST_DUMP: AtomicU32 = AtomicU32::new(0);
// GUI_GL_OpenGL::drawImage entry counter (page-hook in install_lib hooks):
// the paint funnel of the HMI; a page switch that paints widgets moves this.
pub(crate) static DRAW_IMAGE_CALLS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
// GUI_Widget::vDraw entry counter (see r779 page hook): widget paint passes.
static VDRAW_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
// HMI glDrawArrays volume across the switch + a delayed second sample:
// a page that creates widgets but draws nothing shows a flat draw rate.
static TOUCH_DRAW_SNAP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static TOUCH_DI_SNAP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static TOUCH_POST_COUNTDOWN: AtomicU32 = AtomicU32::new(0);
// r779 page-switch paint pump: after a view-create the freshly built widget
// tree never enters the damage/composite pass (idle stays 2 background
// paints/frame).  Milestone-1 proved the working recipe is vInvalidate over
// each view's draw context (ctx->[0x14](1)) + one internal-event dispatcher
// nudge + forced present; run it a beat after the view switches, one guest
// call per mainloop iteration through guest_call_via_lr.
static VIEW_PAINT_STATE: AtomicU32 = AtomicU32::new(0);
// r784: how many invalidate rounds the page-switch pump has run for the
// current page switch (see the pump in the mainloop hook).
static VIEW_PAINT_ROUND: AtomicU32 = AtomicU32::new(0);
static VIEW_PAINT_CD: AtomicU32 = AtomicU32::new(0);
static VIEW_PAINT_I: AtomicU32 = AtomicU32::new(0);
static VIEW_PAINT_FNS: [AtomicU32; 4] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
static VIEW_PAINT_CTXS: [AtomicU32; 4] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
// `guest_call_via_lr` clobbers r0-r3/lr and, via `sp_slack`, r13: a touch
// call returning onto the bGUIMainloop trap site re-enters its handler with
// those entry values gone.  r1-r3/r12 must come back too - bGUIMainloop's
// prologue spills them and later code (fault at 0x133bf04) reloads them.
static TOUCH_RETURN_PENDING: AtomicU32 = AtomicU32::new(0);
static TOUCH_RETURN_REGS: [AtomicU32; 7] = [
    AtomicU32::new(0), // r0
    AtomicU32::new(0), // r1
    AtomicU32::new(0), // r2
    AtomicU32::new(0), // r3
    AtomicU32::new(0), // r12
    AtomicU32::new(0), // r13
    AtomicU32::new(0), // r14
];
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

/// r767: gate for the enavi-fab RUNNING write alone.  fix241 proved the
/// flush's internal events (12.16 s view-change included!) run from the
/// shared release gate, while the popup paints LAST in that same serialized
/// flush (~12.45 s).  Writing RUNNING at release (12.09) therefore made the
/// organic view-change destroy the popup's view ~285 ms BEFORE the paint -
/// the factory can no longer erase a destroyed view, so every hide channel
/// whiffs on the orphaned pixels.  On the real device the server needs
/// seconds to boot and always reports RUNNING after the paint; mirror that
/// by firing the fab only paint+750 ms (fallback: the release flush had 3
/// s to paint by 15 s).
fn enavi_fab_fire_now() -> bool {
    let paint = POPUP_PAINT_MS.load(Ordering::Relaxed);
    let now = crate::libs::dapi::emu_t0_elapsed_ms() as u64;
    if paint != 0 {
        now >= paint + 750
    } else {
        now >= 15_000
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

/// r761: set when the HMI framework *visibly* stores a pre-NORMAL state
/// (INITIALIZED(2), OFF(4), PAUSE(5), DIAGNOSIS(6) in HSI coding) into
/// `clHMIMngr+0x670`; cleared by a visible NORMAL store (the framework's own
/// promotion) or after consumption.  A ghost writer invisible to `MEM_WRITE`
/// hooks reverts the byte to NORMAL before the framework's periodic apply
/// pass reads it (the r79/80 collapse mystery, reproduced fix121..fix124),
/// so `clFi_Navi` never sees the OFF dwell its `vNewAppState` startup guard
/// waits for.  fix126..133 proved that *broadcasting* the OFF (rewriting the
/// byte at the consumption point) is harmful: dispatcher case 4 zeroes
/// `clHSIMngr+0x14` and the HMI's CCA client-port registration burst at
/// t~6 s never happens.  So the flag is consumed instead by the trap in
/// `prochmi_add_code_hooks`, which only reproduces what the OFF dwell
/// *leaves behind* on the device (static prev-state = OFF plus the fabricated
/// enavi start-request) right before the first real NORMAL broadcast.
pub static OFF_SEEN: AtomicU32 = AtomicU32::new(0);

/// The prev-state value the device's OFF dwell would have left in the static
/// `clFi_Navi` previous-state global when the first post-OFF NORMAL broadcast
/// runs; 0 = nothing to reproduce.
pub static OFF_PREV_TO_RESTORE: AtomicU32 = AtomicU32::new(0);

/// r761: state word of the enavi popup-release rescue machine, values 0..4
/// documented on maybe_repost_evt8001.  Background: the release event
/// 0x81008001 is queued organically into the clEventEngine ring (fix140:
/// ring[0]=0x81008001), but `clEventEngine::vNewAppState` - called by
/// `clHMIMngr::vPerformNewAppState` on *every* ~20 ms framework cycle - runs
/// `clEventQueueManager::bResetQueue` for NORMAL(3) *and* OFF(4), always
/// after that cycle's event producers; the organic pump (`clHMIMngr::
/// bExecute` -> `clEventEngine::bExecute`, 0x134ece8) never runs because its
/// host-side wait (idle semaphore) never completes here, so the entry rots
/// and the "Starting navigation." popup is never released.  On the device
/// the enavi server posts the release asynchronously between cycles and the
/// next pump consumes it; maybe_repost_evt8001 restores exactly that
/// ordering with guest calls into the app's own queue/pump functions.
pub static EVT8001_REPOST: AtomicU32 = AtomicU32::new(0);

/// r761: fabricate-enavi-server-RUNNING state (0 idle, 1 armed, 2 fired).
/// See the "bstartup-reg"/"deliver-fi" arms of the enavi-chain trap.
static ENAVI_FAB_STATE: AtomicU32 = AtomicU32::new(0);
static ENAVI_FAB_DEADLINE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// thread that ran the injected setter call; its next hit on the same trap
/// site is the setter's return (see the enavi-fab arms).
static ENAVI_FAB_TID: AtomicU32 = AtomicU32::new(0xffff_ffff);
/// SP at the moment of the enavi-fab injection (see the sp_slack there).
static ENAVI_FAB_SP: AtomicU32 = AtomicU32::new(0);
/// LR at the moment of the enavi-fab injection.  The router trap site is
/// the function's entry `push {r4-r7,lr}`: when the injected call lands
/// back on the site the cave replays that push, and without the original
/// LR the frame stored the callees' leftover lr (bWriteQueue's bl-return
/// 0x135a3c8 - the fix187..195 fatal pop, see the dframe watch forensics).
static ENAVI_FAB_LR: AtomicU32 = AtomicU32::new(0);
/// SP at the moment of the evt8001 engine-pump injection (same sp_slack
/// story: the dispatch trap site is a function entry push).
static EVT8001_PUMP_SP: AtomicU32 = AtomicU32::new(0);
/// LR counterpart for the pump injection (the dispatch site is an entry
/// push too - see ENAVI_FAB_LR).
static EVT8001_PUMP_LR: AtomicU32 = AtomicU32::new(0);
/// r763: set once the enavi-fab release handler has completed, asking the
/// bOnMessage trap to re-post the visual-hide GUI transition 0x8d (see the
/// there comment).
/// r763: the enavi-fab release completed - the post-wipe trap re-posts the
/// popup exit-hide GUI event 0x8d paced once per app-state transition (see
/// the there comment); REMAIN counts the remaining posts.
static FAB_HIDE_EVENT_REMAIN: AtomicU32 = AtomicU32::new(0);
/// r764: direct drive of the popup exit transition.  The SM-page trace
/// (fix214/215/218) shows the state machine PASSES THROUGH state 0x1e5
/// (SYSDLG nav-starting-up) within ~20 ms during the startup cascade, while
/// the Lua side only paints the widget ~350 ms later (paint marker): on
/// every run the organic exit-hide fires BEFORE the paint and the popup is
/// orphaned - exactly the r757 marker-trap seen from the other side.  After
/// the release, re-posting event 0x8d is useless (the leaf long left the
/// 0x1e5 region, fix218).  So replay the recorded transition itself:
/// vDoTransition(0x1371170, sm, id) just stores the record pointer
/// (table 0x25a9568 + id*0x10) at [sm+0xc] with NO source validation, and
/// performTransition(0x1371390, sm) executes the pending record (exit chain
/// of src -> entry chain of tgt, Lua actions included) and clears pending.
/// Calling both with the real exit transition 0x2061 (0x1e5 -> 0x1e4, kind
/// 4, event 0x8d) runs the app's own hide machinery after the orphaned
/// paint.  SM_HIDE_SM is captured from the SM-page hook whenever a
/// doTransition touches a 0x1eX sysdlg state, so only the real SM instance
/// of that region is driven.
static SM_HIDE_STATE: AtomicU32 = AtomicU32::new(0);
static SM_HIDE_SM: AtomicU32 = AtomicU32::new(0);
/// r766: live GUI_StateView record of state 0x1e5 (the wait popup's state),
/// captured from the SM-page hook.  GUI_StateView::vLeave (0x1375be8) runs
/// the app's own dismiss path for it (GUI_MenuManager::vLeaveView +
/// vSetView(mm, 0) + composite flag clear) - the organic counterpart of the
/// vEnter that showed the view on the startup cascade.
static SM_HIDE_RECORD: AtomicU32 = AtomicU32::new(0);
/// r767: live GUI_MenuManager singleton ([GUI_GLOBAL_BASE+0xb0], read at
/// injection time) - vSetView/vPerformViewChange run on it.
static SM_HIDE_MM: AtomicU32 = AtomicU32::new(0);
/// r767: window flag - while set, the vFocusGained entry trap (0x1371b44)
/// returns immediately.  fix246/248: perform's path-A tail calls
/// GUI_StateMachine::vFocusGained (up to 4x in its slot loop, bl at
/// 0x1364aec), which broadcasts internal event 0x59 with pReceiver == 0;
/// no receiver is registered for 0x59 in our emulated unit, so the OSAL
/// assert "msg.pReceiver != 0" fires and the dispatcher then faults loading
/// the null receiver's vtable (READ_UNMAPPED 0x1365924, thread death).  The
/// real device has listeners for 0x59; skipping the broadcast only during
/// the injected rebuild keeps every other focus path intact.
static SM_HIDE_FOCUS_SKIP: AtomicU32 = AtomicU32::new(0);
/// r767: cid of the last main-block view creation (viewfac probe, lr ==
/// main-create site 0x136474c) - the live main view as the FACTORY knows
/// it.  fix250: the mm+0x10 "drawn" u16 held a stale non-factory value
/// (0x82d8) at injection time, so feeding it to vSetView armed
/// vPerformViewChange with a cid pCreateView cannot instantiate (returned
/// NULL -> MenuManager root object 0 -> zombie-panic consumer faulted).
/// The organic 12 s switch logs the true main cid (247).
static SM_HIDE_MAIN_CID: AtomicU32 = AtomicU32::new(0);
static SM_HIDE_REGS: [AtomicU32; 4] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
static SM_HIDE_SP: AtomicU32 = AtomicU32::new(0);
static SM_HIDE_LR: AtomicU32 = AtomicU32::new(0);
static SM_HIDE_TID: AtomicU32 = AtomicU32::new(0);
/// r768: the popup's painted script objects (GUI_Label vtable 0x2872150 /
/// secondary 0x28721a4, GUI_BGImg vtable 0x28721c8) and their impl widgets
/// ([obj+0x1c]), captured at paint time by the text-builder POPUPCELLS
/// NODE probe.  The widget-teardown channel (sm_hide_fire_or_restore) runs
/// the app's own GUI_Scripted dismiss pair over PU_OBJS (switches the
/// +0x68 paint gate off) and then GUI_Widget::vInvalidate over PU_IMPS to
/// post the damage rect, so the compositor repaints the region from the
/// surviving main-view tree.
static PU_OBJS: [AtomicU32; 8] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
static PU_IMPS: [AtomicU32; 8] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
static PU_COUNT: AtomicU32 = AtomicU32::new(0);
static PU_I: AtomicU32 = AtomicU32::new(0);
/// r768: the popup GUI_Widget as the popup manager knows it - [singleton
/// +0x14] of [[0x2a13a00]], captured live by the release-wfind/release-pop
/// probes (r762 forensics: that slot held 0x3a0ecf8 across the organic
/// release).  Making this widget's +0x3c visibility ContentId resolve to
/// false removes the whole painted popup subtree from GUI_Widget::vDraw.
static PU_WIDGET: AtomicU32 = AtomicU32::new(0);
/// r768: `this` of the clGUIWidgetEngine/clHMI bExecute site that hosted
/// the invalidate loop - re-stored into r0 when the injected
/// GUI_Widget::vInvalidate returns onto the site (the prologue must see
/// the real engine pointer, fix251 lesson).
static PU_THIS: AtomicU32 = AtomicU32::new(0);
/// r768: live draw context ([[popup widget +0x28]+0x40], fix259:
/// 0x38f7b28-class) used for the whole-context `ctx->[0x14](ctx, 1)`
/// redraw trigger.
static PU_CTX: AtomicU32 = AtomicU32::new(0);
/// r768: pGetView(3) - the LIVE main view (kids=50, the FM1 tree; fix264:
/// slots 0/1 are empty shells, the drawn world hangs off slot 3) and its
/// draw context [v3+0x40].  Markers 0xF1/0xF2/0xF3 replay update()'s own
/// view-render triple on them (ctx->[8] drawBegin, callDraw(v3,0,0),
/// ctx->[0xc] drawEnd) - the organic path the 12.1 s view-change used.
static PU_V3: AtomicU32 = AtomicU32::new(0);
static PU_V3CTX: AtomicU32 = AtomicU32::new(0);
/// r768: overlay widget root (popup container's +0x28 ctx-holder), the
/// second render triple's tree.
static PU_OVR: AtomicU32 = AtomicU32::new(0);
/// r0-r3 of the interrupted `deliver-fi` call, restored when the injected
/// setter returns onto the trap site (see the enavi-fab arms above).
static ENAVI_FAB_REGS: [AtomicU32; 4] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

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

// r784: on-screen keyboard (clFi_KEYBRD / clHSI_CMKeyboard) path probes.
fn keyb_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("EMU_PROCHMI_KEYB_TRACE").is_some())
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

/// r798b: last RegisterConf transaction ordinal per svc-0x06 CL entry
/// (recorded by the conf-store page watch).  The state-6 member probe
/// (g-m18) re-fires the completion push for entries whose first push lost
/// the mbx_289 receive race to tid 84's enConnect Wait (f244: mem[5]/[6]
/// starved at state 1 forever although READY only needs [+4]==2).
static SVC6_TXN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<u32, u16>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

pub fn prochmi_add_code_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    PROCHMI_BASE.store(base_address, Ordering::Relaxed);
    // r789d one-shot diagnostic (EMU_PROCHMI_XREF_SCAN=1).  prochmi is
    // prelinked: callers of the exported navif/enavi glue (enActivate,
    // IFMediator::enActivate, bInitialise, bNavIFCreate) bind through
    // GOT-style slots and Ghidra carries no xrefs for them - the navif
    // bring-up caller hunt on the engine service gate is statically blind.
    // Walk every mapped RW region for words equal to the runtime VAs (and
    // the vtable constants, stored relocated as base + 8) and dump the
    // relocated vtable heads, so the pointer owners become identifiable.
    if std::env::var("EMU_PROCHMI_XREF_SCAN").is_ok() {
        const TARGETS: [(&str, u32); 8] = [
            ("enavi_enActivate", 0x017c_1bf8),
            ("ifmed_enActivate", 0x01c7_d3c8),
            ("navif_bInitialise", 0x01c7_b44c),
            ("navif_enNavIFCreate", 0x01c7_b4bc),
            ("ennavi_bNavIFCreate", 0x017c_1e80),
            ("fi_navi_vSetRegisterID", 0x014b_b224),
            ("enavi_navif_vtable", 0x0523_8558),
            ("navif_svcman_vtable", 0x0533_65a8),
        ];
        let mut needles: Vec<(&str, u32)> = Vec::new();
        for (name, va) in TARGETS {
            needles.push((name, base_address + va - ORIGINAL_BASE));
            if va >= ORIGINAL_BASE {
                // vtable constants are stored as `vtable + 8` and get
                // relocated the same base delta as code.
                needles.push((name, base_address + va + 8 - ORIGINAL_BASE));
            }
        }
        let regions: Vec<(u32, u32)> = match unicorn.mem_regions() {
            Ok(rs) => rs.iter().map(|r| (r.begin as u32, r.end as u32)).collect(),
            Err(_) => Vec::new(),
        };
        let mut buf = vec![0u8; 0x10000];
        let mut total_hits = 0usize;
        for (b, e) in &regions {
            let size = e.wrapping_sub(*b);
            if size > 0x600_0000 || size == 0 {
                continue;
            }
            let mut off = 0u32;
            while off < size && total_hits < 200 {
                let len = std::cmp::min(0x10000, size - off) as usize;
                if unicorn.mem_read((b + off) as u64, &mut buf[..len]).is_err() {
                    off += 0x10000;
                    continue;
                }
                for i in 0..len / 4 {
                    let w = i * 4;
                    let word = u32::from_le_bytes([buf[w], buf[w + 1], buf[w + 2], buf[w + 3]]);
                    for (name, needle) in &needles {
                        if word == *needle {
                            total_hits += 1;
                            log::warn!(
                                "PROCHMI xref-scan {name}: slot={:#x} region={b:#x}+{off:#x}+{w:#x} value={word:#x}",
                                b + off + w as u32,
                            );
                        }
                    }
                }
                off += 0x10000;
            }
        }
        log::warn!("PROCHMI xref-scan complete: {total_hits} hits");
        for (name, va) in [
            ("enavi_navif_vtable", 0x0523_8558u32),
            ("navif_svcman_vtable", 0x0533_65a8u32),
        ] {
            let rt = base_address + va - ORIGINAL_BASE;
            let mut dump = String::new();
            for i in 0..14u32 {
                let mut w = [0u8; 4];
                if unicorn.mem_read((rt + i * 4) as u64, &mut w).is_ok() {
                    let v = u32::from_le_bytes(w);
                    dump.push_str(&format!(" {:x}", v.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE)));
                }
            }
            log::warn!("PROCHMI xref-scan {name}@{rt:#x}:{dump}");
        }
    }
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
            // A `drain_gui_touch_event` guest call just returned here: put
            // back the entry register state before anything reads it again.
            if TOUCH_RETURN_PENDING.swap(0, Ordering::Relaxed) == 1 {
                for (index, register) in [
                    RegisterARM::R0,
                    RegisterARM::R1,
                    RegisterARM::R2,
                    RegisterARM::R3,
                    RegisterARM::R12,
                    RegisterARM::R13,
                    RegisterARM::R14,
                ]
                .into_iter()
                .enumerate()
                {
                    let saved = TOUCH_RETURN_REGS[index].load(Ordering::Relaxed);
                    let _ = uc.reg_write(register, saved as u64);
                }
            }
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
            drain_gui_touch_event(uc, address);
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
                    // A raw stack scan (the r757 probe) showed stale frames
                    // from earlier GUI traffic, so only accept words that are
                    // real ARM `bl` return points: the word before them must
                    // carry the bl encoding (top byte 0xeb/0x9b-0xdb family).
                    let mut chain: Vec<u32> = Vec::new();
                    if sp > 0x1000 && sp < 0xf000_0000 {
                        for i in 0..256u32 {
                            let w = read_u32(_uc, sp + i * 4) & !1u32;
                            if !((0x0000_8004..0x0286_5294).contains(&w)
                                || (0x47d0_0004..0x4860_0000).contains(&w))
                            {
                                continue;
                            }
                            let caller_insn = read_u32(_uc, w.wrapping_sub(4));
                            let top = (caller_insn >> 24) & 0xff;
                            // ARM bl/blx: bits[27:25] = 101, L bit24 -> the
                            // top byte's low nibble is 0xb (bl) or 0xa (blx).
                            if !matches!(top & 0x0f, 0x0a | 0x0b) {
                                continue;
                            }
                            if !chain.contains(&w) {
                                chain.push(w);
                                if chain.len() >= 10 {
                                    break;
                                }
                            }
                        }
                    }
                    let _ = sp;
                    log::warn!(
                        "PROCHMI popup-string READ({tag}) {} [{}] addr={:#x} size={} n={} tel={tel} pc={:#x} lr={:#x} regs={:?}",
                        elf,
                        tid,
                        address,
                        size,
                        count,
                        pc,
                        _uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                        [
                            RegisterARM::R0,
                            RegisterARM::R1,
                            RegisterARM::R2,
                            RegisterARM::R3,
                            RegisterARM::R4,
                            RegisterARM::R5,
                            RegisterARM::R6,
                            RegisterARM::R7,
                            RegisterARM::R8,
                            RegisterARM::R9,
                            RegisterARM::R10,
                            RegisterARM::R11,
                        ]
                        .iter()
                        .map(|r| format!("{:#x}", _uc.reg_read(*r).unwrap_or(0)))
                        .collect::<Vec<_>>(),
                    );
                    log::warn!(
                        "PROCHMI popup-string CALLCHAIN({tag}) tel={tel} chain={:?}",
                        chain.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>(),
                    );
                    if count == 1 {
                        let words: Vec<u32> =
                            (0..48u32).map(|i| read_u32(_uc, sp + i * 4)).collect();
                        log::warn!(
                            "PROCHMI popup-string STACK({tag}) tel={tel} sp={sp:#x}: {:?}",
                            words.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>(),
                        );
                    }
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
        // r758: the gateway's client-delivery indirect call (bReadSharedMemory
        // tail): handler in r9, amt descriptor in r7, target = vtable+0x10.
        // Dumping handler/vtable/target names the client class that consumes
        // nav records - i.e. where the wire bytes meet the CM layer.
        (0x013c_9340u32 - ORIGINAL_BASE, "gw-deliver"),
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
        (0x015f_48a4u32 - ORIGINAL_BASE, "enavi-srvstate"),
        // post point inside vOnUpdateEnaviServerState: the ldr that loads the
        // command-port vtable immediately before (**vtable)(port, 0x81008001)
        (0x015f_4914u32 - ORIGINAL_BASE, "srvstate-post"),
        (0x0161_fca8u32 - ORIGINAL_BASE, "ienvi-srvready"),
        // r761: startup-chain progress probes.  fix134: PREV4+FLAG1 fires
        // vStartupEnaviServer organically at t~3.1 s and the whole CCA
        // registration burst is back, but no sink ever receives
        // vOnUpdateEnaviServerState (0x15f48a4 = the sender of the popup
        // release 0x81008001).  These sites map how far the startup body
        // gets: sink registration, the server-side broadcast itself, the
        // state getter, and the enavi action-thread / ProcEnaviStartup
        // handshake (DAPI dataset availability) that drives it.
        (0x0161_d5f8u32 - ORIGINAL_BASE, "sink-register"),
        (0x0161_d3d4u32 - ORIGINAL_BASE, "srvstate-bcast"),
        (0x0161_ce68u32 - ORIGINAL_BASE, "srvstate-get"),
        (0x015e_1954u32 - ORIGINAL_BASE, "navsrv-svcready"),
        (0x017c_2b58u32 - ORIGINAL_BASE, "procstartup-ctor"),
        (0x017c_2760u32 - ORIGINAL_BASE, "props-before"),
        (0x017c_73c0u32 - ORIGINAL_BASE, "run-when-running"),
        // r761: bStartup tail instrumentation (fix164: server state word
        // stalls at 1; the setter with 2 lives in the 0x17c73c0 FSM handler
        // which never runs).  These three sites show whether bStartup even
        // reaches the action-thread constructor and the two registration
        // calls that hand it to the tcl pump.
        (0x017b_f6acu32 - ORIGINAL_BASE, "bstartup-kick"),
        (0x017b_f6d0u32 - ORIGINAL_BASE, "bstartup-reg"),
        (0x017c_45b4u32 - ORIGINAL_BASE, "actthread-ctor"),
        // r761: unwind probes for the enavi-fab crash (fix167/169: stack
        // corrupts during the setter->bcast->sink->post->bWriteQueue return
        // path).  Dump the return slot just before each epilogue pops it.
        (0x0161_d478u32 - ORIGINAL_BASE, "bcast-sinkloop-end"),
        (0x0161_d498u32 - ORIGINAL_BASE, "bcast-epilogue"),
        // r761: probes along the state==2 popup-release branch of
        // vHandleEnaviServerStatus (fix177: it is reached with the fused
        // RUNNING status but dies with a fault inside nQueueSpaceLeft,
        // 0x135a1cc, before any further trap).
        (0x015e_2238u32 - ORIGINAL_BASE, "release-branch"),
        (0x0170_6710u32 - ORIGINAL_BASE, "release-query"),
        (0x015e_0418u32 - ORIGINAL_BASE, "release-hidefn"),
        (0x015e_226cu32 - ORIGINAL_BASE, "release-postvtable"),
        // branch-decision probe: state148 vs {5, 2&&r7==1} gate inputs
        (0x015e_21a4u32 - ORIGINAL_BASE, "release-gate"),
        // fix184: the release branch now runs and posts real events; fault
        // moved deeper - probe the two widget calls of the branch.
        (0x015e_229cu32 - ORIGINAL_BASE, "release-afterw1"),
        (0x0162_1a24u32 - ORIGINAL_BASE, "release-widget1"),
        (0x0162_59d8u32 - ORIGINAL_BASE, "release-widget2"),
        // fix185: fault is past the widget pair; probe the three handler
        // calls the branch makes right after them.
        (0x015d_fce4u32 - ORIGINAL_BASE, "release-call-a"),
        (0x015d_fb3cu32 - ORIGINAL_BASE, "release-call-b"),
        (0x015e_1a7cu32 - ORIGINAL_BASE, "release-call-c"),
        // epilogue site: fires only if the vtable+56 virtual call at
        // 0x15e2214 returns (i.e. handler completes).
        (0x015e_221cu32 - ORIGINAL_BASE, "release-pop"),
        // fix201 forensics: the 0x15e23bc close path calls 0x1650478 with
        // the popup widget in r0 ([handler+0x7e0]); probe the widget's
        // first words before (here) and after (release-pop) the whole
        // handler to see whether the close touches it at all.
        (0x0165_0478u32 - ORIGINAL_BASE, "release-close"),
        // fix208: the decision point inside 0x1650478 (`beq` at 0x16504e8,
        // taken when r5==0 or r0=[GUI singleton +0x14]==0 -> r4:=0 and the
        // close silently no-ops) - log both operands to see whether the
        // close actually found a registered popup to remove.
        (0x0165_04e8u32 - ORIGINAL_BASE, "release-wfind"),
        // fix187 forensics: nSpaceLeft entry gate.  Every legitimate
        // caller passes q in 0..7; a bigger r1 means a mis-wired caller or
        // a stale translation block.  Dumps lr + a stack backtrace so the
        // culprit is named even when the fault follows within the same TB.
        (0x0135_a1c0u32 - ORIGINAL_BASE, "spaceleft-gate"),
        // fix189 forensics: same anomaly one level up.  The only legal
        // entry to bWriteQueue is the wrapper at 0x1359d4c (which forces
        // r1=0) or a `b` from 0x1359d54; these gates name where r1 turns
        // into a stack address.  Quiet unless the index is implausible.
        (0x0135_9d4cu32 - ORIGINAL_BASE, "wrappost-gate"),
        (0x0135_a310u32 - ORIGINAL_BASE, "qwqueue-gate"),
        // fix190: fatal spaceleft had the frame of the fab's 6438 ms entry
        // - bracket the post-store region of bWriteQueue to see whether the
        // fatal bl #3 is a legitimate continuation or a mid-function jump.
        (0x0135_a374u32 - ORIGINAL_BASE, "qwbody-gate1"),
        (0x0135_a3c8u32 - ORIGINAL_BASE, "qwbody-gate2"),
        // r761: vOnUpdateEnaviServerState delivers 0x81008001 through
        // clEventEngine::bEvent2Engine (NOT the HSI-FI router): bWriteEvent
        // (guarded by engine+0x484 != 4 and 1 <= id <= 0xfffffffd) then
        // vtable+8 on the clHMIMngr engine-iface singleton (_DAT_05972998).
        // These sites name the link that drops the release event.
        (0x0135_9d4cu32 - ORIGINAL_BASE, "event-write"),
        (0x0135_9ca0u32 - ORIGINAL_BASE, "event-exec"),
        (0x0135_9c90u32 - ORIGINAL_BASE, "event-read"),
        // inside bExecute right after bl bReadEvent: r0 = queue-manager,
        // [sp+0xc] = event id just dequeued, [r4+0x480] = dispatcher object
        (0x0135_9cf4u32 - ORIGINAL_BASE, "event-read2"),
        // clEventQueueManager::bWriteQueue entry + bConfigure entry: tell
        // whether the queue-0 ring buffer is live when the release event is
        // posted at t~3.1 s (drop path) versus t~6.7 s (successful events).
        // (0x0135_a310u32 - ORIGINAL_BASE, "event-qwrite"),  // fix189: removed -
        // suspected of feeding the fatal mis-parameterised bWriteQueue entry
        // (svc-cave literal address observed as r1 in the fault, fix187/188).
        // The evtqm mem-write watch and the event-write wrapper trap already
        // cover everything this observer reported.
        (0x0135_9decu32 - ORIGINAL_BASE, "event-conf"),
    ] {
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let label: &'static str = label;
        let site = base_address + offset;
        crate::os::code_stub::trap_site(unicorn, base_address + offset, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:enavi-chain", move |uc: &mut Unicorn<'_, Context>| {
            // r761: fabricated enavi server bring-up completion (always-on).
            // The real device's enavi server answers the client's startup
            // action with a RUNNING report; the client's action FSM (handler
            // 0x17c73c0) never advances past STARTING(1) here because its
            // pump - the FSM case that issues and awaits that report - needs
            // the whole ProcEnaviStartup/PROCNAV handshake (run-fix164/165:
            // fsm byte stays 0xff, handler never runs, state word
            // [[IEnavi]+0x4c]+0xe0 stalls at 1, wait-popup never released).
            // Simulate the server answer at the RTOS boundary (same class of
            // fabrication as the CCA-directory answers): write the status
            // word and queue the release event, both shallow; the pump, the
            // router delivery and the popup-release branch in
            // vHandleEnaviServerStatus then all run organically.  Fire site
            // is the HSI-FI router trap: unlike the app-state sites (only
            // reached on transitions) it keeps ticking while the HMI idles.
            if label == "hsi-fievent-router" {
                enavi_fab_fire_or_restore(uc, site);
                // r764: two-stage popup exit-transition injection (see the
                // sm_hide_fire_or_restore doc - the 0x8d re-posts of r763
                // only replayed the startup cascade because the leaf long
                // left the sysdlg region by release time).
                sm_hide_fire_or_restore(uc, site);
            }
            match label {
                "bstartup-reg" => {
                    if EVT8001_REPOST.load(Ordering::Relaxed) < 8 && ENAVI_FAB_STATE.load(Ordering::Relaxed) == 0 {
                        ENAVI_FAB_DEADLINE.store(
                            crate::libs::dapi::emu_t0_elapsed_ms() as u64 + 800,
                            Ordering::Relaxed,
                        );
                        ENAVI_FAB_STATE.store(1, Ordering::Relaxed);
                        log::info!(
                            "PROCHMI enavi-fab armed (server-RUNNING report in 800 ms) t={}ms",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                        );
                    }
                }
                _ => {}
            }
            // The nSpaceLeft entry gate is on the hot post path: never
            // trace it normally, only warn about implausible queue indices
            // (a legit caller passes q in 0..7; anything else names a
            // mis-wired caller or stale TB by lr + stack backtrace).
            if label == "spaceleft-gate" {
                let r1v = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                if r1v > 7 {
                    let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    log::warn!(
                        "PROCHMI spaceleft-gate: q={r1v:#x} r0={:#x} lr={lr:#x} sp={sp:#x} bt={:#?}",
                        uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                        (0..12)
                            .map(|i| read_u32(uc, sp + 4 * i))
                            .filter(|v| (0x0001_0000..0x0400_0000).contains(v))
                            .collect::<Vec<_>>(),
                    );
                }
                return 0;
            }
            // The two post-path gates log every hit (all are rare) so the
            // fatal mis-parameterised bWriteQueue entry is bracketed.
            if label == "wrappost-gate"
                || label == "qwqueue-gate"
                || label == "qwbody-gate1"
                || label == "qwbody-gate2"
            {
                static GATE_LOGS: AtomicU32 = AtomicU32::new(0);
                if GATE_LOGS.fetch_add(1, Ordering::Relaxed) < 40 {
                    let t = crate::libs::dapi::emu_t0_elapsed_ms();
                    if label == "qwbody-gate2" {
                        let hexes = |base: u32| -> String {
                            (0..8)
                                .map(|i| format!("{:x}", read_u32(uc, base + 4 * i)))
                                .collect::<Vec<_>>()
                                .join(" ")
                        };
                        log::info!(
                            "PROCHMI qwbody-gate2: t={t}ms r0={:#x} r6={:#x} lr={:#x} sp={:#x} hdr=[{}] head=[{}] cnt=[{}] cap0={}",
                            uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                            uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32,
                            uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                            uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32,
                            hexes(0x38ee_ec8),
                            hexes(0x38ee_ec8 + 0x28),
                            hexes(0x38ee_ec8 + 0x50),
                            read_u32(uc, 0x38ee_f2c + 0x3c),
                        );
                    } else {
                        log::info!(
                            "PROCHMI {label}: t={t}ms r0={:#x} r1={:#x} r2={:#x} r4={:#x} r6={:#x} lr={:#x} sp={:#x}",
                            uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                            uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                            uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32,
                            uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32,
                            uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32,
                            uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                            uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32,
                        );
                    }
                }
                return 0;
            }
            // r769: the pump consuming the 0x8001 release event finalises
            // the evt8001 repost machinery (EVT8001_REPOST -> 0), which the
            // enavi-fab fire gate and the hide arm both require.  The
            // label-match below is trace-gated, so run just this reset on
            // the always-on path (event-read2 is the engine event pump).
            if label == "event-read2" && !nav_trace_enabled() {
                let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                if read_u32(uc, sp.wrapping_add(0xc)) == 0x8100_8001 {
                    EVT8001_REPOST.store(0, Ordering::SeqCst);
                }
                return 0;
            }
            // r769: the release-* label arms below are hide-channel data
            // path (cryptcard answer, session word, PU_WIDGET capture,
            // SM_HIDE_STATE arming) and must run in default no-env runs -
            // they are rare sites, so letting them through the trace gate
            // costs nothing.  Everything else stays trace-gated.
            if !nav_trace_enabled()
                && !matches!(
                    label,
                    "release-gate" | "release-pop" | "release-close" | "release-wfind"
                )
            {
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
                && label != "gw-deliver"
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
                // bl 0x17c2a08 in bStartup: r0 = freshly constructed action
                // object ([server+0xd8]); 0x17c2a08 only kicks it when its FSM
                // byte-field +0x30 has reached 8.
                "bstartup-kick" => format!(
                    " obj={r0:#x} fsm30={:#x} vtab={:#x}",
                    read_u32(uc, r0.wrapping_add(0x30)),
                    if r0 != 0 { read_u32(uc, r0) } else { 0 },
                ),
                // bl 0x17bf324 in bStartup: r0 = server object, r2 = literal
                // pool entry (name/vtable arg).
                "bstartup-reg" => {
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    format!(" srv={r0:#x} r2={r2:#x} obj_d8={:#x}", read_u32(uc, r0 + 0xd8))
                }
                "actthread-ctor" => format!(" obj={r0:#x} owner={r1:#x}"),
                // pop {r4,r5,r6,r7,r8,sl,pc}: the pc slot sits at sp+24.
                "bcast-sinkloop-end" | "bcast-epilogue" => {
                    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    format!(
                        " this={r0:#x} sp={sp:#x} slot_sp24={:#x} lr={:#x} r4={:#x} r5={:#x} r6={:#x}",
                        read_u32(uc, sp.wrapping_add(24)),
                        lr,
                        uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32,
                        uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32,
                        read_u32(uc, r0 + 0x58),
                    )
                }
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
                "gw-deliver" => {
                    let r9 = uc.reg_read(RegisterARM::R9).unwrap_or(0) as u32;
                    let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                    let vt = if r9 != 0 { read_u32(uc, r9) } else { 0 };
                    let target = if vt != 0 { read_u32(uc, vt + 0x10) } else { 0 };
                    // amt message body = *(descriptor + 4)
                    let body = if r7 != 0 { read_u32(uc, r7 + 4) } else { 0 };
                    let (b0, t14, cls) = if body != 0 {
                        (
                            read_u32(uc, body) & 0xffff,
                            (read_u32(uc, body.wrapping_add(0x14)) & 0xffff) as u16,
                            read_u8(uc, body.wrapping_add(0xb)),
                        )
                    } else {
                        (0, 0, 0)
                    };
                    format!(
                        " handler={r9:#x} vt={vt:#x} target={target:#x} amt={r7:#x} body={body:#x} key={b0:#x} t14={t14:#x} class={cls:#x}"
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
                "sink-register" => format!(" this={r0:#x} sink={r1:#x}"),
                "srvstate-bcast" => format!(" this={r0:#x} state={r1:#x}"),
                "enavi-srvstate" => format!(
                    " sink={r0:#x} state={r1:#x} dat={:#x} laststate+4={:#x}",
                    read_u32(uc, 0x0597_3de8),
                    read_u32(uc, r0 + 4),
                ),
                "srvstate-post" => {
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let vt = if r2 != 0 { read_u32(uc, r2) } else { 0 };
                    format!(
                        " port={r2:#x} vtable={vt:#x} hmimngr_dat={:#x}",
                        read_u32(uc, 0x0597_2998),
                    )
                }
                "event-write" => {
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    format!(
                        " this={r0:#x} state484={:#x} id={r2:#x}",
                        read_u32(uc, r0.wrapping_add(0x484)),
                    )
                }
                "event-exec" | "event-read" => {
                    format!(" this={r0:#x} state484={:#x}", read_u32(uc, r0.wrapping_add(0x484)))
                }
                "event-read2" => {
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    let ev = read_u32(uc, sp.wrapping_add(0xc));
                    if ev == 0x8100_8001 {
                        // The pump consumed the release event: the repost
                        // machinery is done (see EVT8001_REPOST).
                        EVT8001_REPOST.store(0, Ordering::SeqCst);
                    }
                    format!(
                        " engine={r4:#x} id={ev:#x} disp={:#x}",
                        read_u32(uc, r4.wrapping_add(0x480)),
                    )
                }
                "event-qwrite" => {
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    // clEventQueueManager layout: buf[q] at qm+q*4, head[q]
                    // at qm+(q+10)*4, count[q] at qm+(q+0x14)*4
                    let buf = read_u32(uc, r0.wrapping_add(r1 * 4));
                    format!(
                        " qm={r0:#x} q={r1:#x} id={r2:#x} buf={buf:#x} cnt={:#x} head={:#x} ring0={:#x}",
                        read_u32(uc, r0.wrapping_add((r1 + 0x14) * 4)),
                        read_u32(uc, r0.wrapping_add((r1 + 10) * 4)),
                        if buf != 0 { read_u32(uc, buf) } else { 0 },
                    )
                }
                "event-conf" => format!(" this={r0:#x} trace={r1:#x}"),
                // r795b: vSvcReady (0x15e1954) has four null-guards
                // (+0x15c, FINAVI singleton at global 0x2a1ce78, +0x7dc,
                // +0x160) - any zero aborts the READY propagation that the
                // state-6 gate waits for ([0x3a23f5c]==2 via 0x166c0d0).
                // Dump all four to find the missing wire.
                "navsrv-svcready" => {
                    let finavi_ptr = read_u32(uc, 0x02a1_be8c);
                    let finavi = read_u32(uc, finavi_ptr);
                    format!(
                        " this={r0:#x} +15c={:#x} fsvc={finavi_ptr:#x} finavi={finavi:#x} +7dc={:#x} +160={:#x}",
                        read_u32(uc, r0 + 0x15c),
                        read_u32(uc, r0 + 0x7dc),
                        read_u32(uc, r0 + 0x160),
                    )
                }
                "srvstate-get" => {
                    let v = read_u32(uc, r0 + 0x140);
                    let proxy = read_u32(uc, r0 + 0x4c);
                    // r796: dynamic write-watch on the navsrv server-state
                    // word [proxy+0xe0] (1 -> 2 without sink broadcast; the
                    // gate object's cached copy never follows).  Heap moves
                    // per build, so pin it at first observation.
                    if proxy != 0 {
                        static SW: std::sync::atomic::AtomicBool =
                            std::sync::atomic::AtomicBool::new(false);
                        if !SW.swap(true, Ordering::Relaxed) {
                            let want = (proxy + 0xe0) as u64;
                            log::warn!(
                                "PROCHMI navsrv-state watch installed t={}ms word={:#x}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                want,
                            );
                            let _ = uc.add_mem_hook(
                                unicorn_engine::unicorn_const::HookType::MEM_WRITE,
                                want,
                                want + 3,
                                move |uc: &mut Unicorn<'_, Context>,
                                      _t,
                                      address,
                                      size,
                                      value| {
                                    static WHS: AtomicU32 = AtomicU32::new(0);
                                    if WHS.fetch_add(1, Ordering::Relaxed) < 12 {
                                        let lr = uc
                                            .reg_read(RegisterARM::R14)
                                            .unwrap_or(0) as u32;
                                        let pc = uc
                                            .reg_read(RegisterARM::R15)
                                            .unwrap_or(0) as u32;
                                        log::warn!(
                                            "PROCHMI navsrv-state WRITE t={}ms addr={address:#x} size={size} value={value:#x} pc={pc:#x} lr={lr:#x}",
                                            crate::libs::dapi::emu_t0_elapsed_ms(),
                                        );
                                    }
                                    true
                                },
                            );
                        }
                    }
                    format!(
                        " this={r0:#x} +140={v:#x} proxy4c={proxy:#x} e0={:#x}",
                        if proxy != 0 {
                            read_u32(uc, proxy + 0xe0)
                        } else {
                            0
                        },
                    )
                }
                "procstartup-ctor" => format!(" this={r0:#x} thread={r1:#x}"),
                "release-gate" => {
                    // Last moment before the two idapi queries run: make
                    // sure the "/dev/cryptcard" lookup can resolve (the
                    // release branch needs its 1 answer, fix179).
                    fake_cryptcard_into_idapi(uc);
                    let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                    // A RUNNING server report implies an established HMI-
                    // server session: the embedded session object at
                    // handler+0x19d8 carries its state in u16 +0x98 (getter
                    // vtable+0x40).  Real hardware shows the popup behind a
                    // live session, so release takes the close path
                    // 0x15e23bc (bl 0x1650478 closes widget [handler+0x7e0]);
                    // state 0 there drops to the 0x15e0418 fallback, which
                    // leaves the widget painted (fix196..199: hide request
                    // event 0x81008132 delivered, popup still rendered).
                    // Same provider-signal class as the RUNNING word itself.
                    if uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32 == 2 {
                        let w98 = r5 + 0x19d8 + 0x98;
                        if read_u32(uc, w98) & 0xffff == 0 {
                            let _ = uc.mem_write(w98 as u64, &1u16.to_le_bytes());
                            log::info!(
                                "PROCHMI enavi-fab: session word {w98:#x} := 1 (RUNNING implies connected) t={}ms",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                            );
                        }
                    }
                    let w7e0 = read_u32(uc, r5 + 0x7e0);
                    let w98 = read_u32(uc, r5 + 0x19d8 + 0x98) & 0xffff;
                    let prox = read_u32(uc, r5 + 0x19d8);
                    let pvt = if prox != 0 { read_u32(uc, prox) } else { 0 };
                    format!(
                        " h={r5:#x} state148_r3={:#x} r7={:#x} f160={:#x} f15c={:#x} f7e4={:#x} f7e0={w7e0:#x} w98={w98:#x} f19d8={prox:#x} pvt40={:#x} b79={} gate830={:#x}",
                        uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32,
                        uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32,
                        read_u32(uc, r5 + 0x160),
                        read_u32(uc, r5 + 0x15c),
                        read_u32(uc, r5 + 0x7e4),
                        if pvt != 0 {
                            read_u32(uc, pvt + 0x40)
                        } else {
                            0
                        },
                        read_u8(uc, r5 + 0x79),
                        read_u32(uc, r5 + 0x830),
                    )
                }
                "release-branch" | "release-query" | "release-hidefn" | "release-postvtable" => {
                    // r4/r5 are callee-saved here: name the objects whose
                    // virtual calls run on the popup-release path.
                    let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    let navvt = read_u32(uc, 0x38f_9970);
                    format!(
                        " this={r0:#x} r4={r4:#x} r5={:#x} r4vt={:#x} navvt={navvt:#x} nav56={:#x}",
                        uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32,
                        if r4 != 0 { read_u32(uc, r4) } else { 0 },
                        if navvt != 0 {
                            read_u32(uc, navvt + 0x38)
                        } else {
                            0
                        },
                    )
                }
                "release-close" | "release-pop" => {
                    // fix212: slot14 reads identical pre/post - install a
                    // one-shot write-watch on the singleton slot from the
                    // close entry (hook applies from the next TB onward,
                    // i.e. it sees THIS close's stores: writes happen after
                    // the entry trap) to catch a clear-then-reregister wave.
                    if label == "release-close" {
                        static SLOTPEND: AtomicU32 = AtomicU32::new(0);
                        SLOTPEND.fetch_add(1, Ordering::Relaxed);
                        if SLOTPEND.load(Ordering::Relaxed) <= 3 {
                            let holder = read_u32(uc, 0x02a1_3a00);
                            let singl_addr = if holder != 0 {
                                read_u32(uc, holder)
                            } else {
                                0
                            };
                            if singl_addr != 0 {
                                let res = uc.add_mem_hook(
                                    unicorn_engine::unicorn_const::HookType::MEM_WRITE,
                                    (singl_addr + 0x14) as u64,
                                    (singl_addr + 0x18) as u64,
                                    move |uc: &mut Unicorn<'_, Context>,
                                          _t,
                                          address,
                                          size,
                                          value| {
                                        let lr =
                                            uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                                        let pc =
                                            uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32;
                                        log::warn!(
                                            "PROCHMI popupslot WRITE t={}ms thread=[{}] addr={:#x} size={} value={:#x} pc={:#x} lr={lr:#x}",
                                            crate::libs::dapi::emu_t0_elapsed_ms(),
                                            uc.get_data().inner.thread_id(),
                                            address,
                                            size,
                                            value,
                                            pc,
                                        );
                                        true
                                    },
                                );
                                log::info!(
                                    "PROCHMI popupslot watch on {:#x}: {}",
                                    singl_addr + 0x14,
                                    res.is_ok()
                                );
                            }
                        }
                    }
                    // Diff the popup widget's head words around the close:
                    // release-close sees r0 = widget from 0x1650478's own
                    // entry; release-pop reads [handler+0x7e0] again after
                    // the handler finished (r5 is callee-saved = handler).
                    let (w, tag) = if label == "release-close" {
                        (r0, "pre")
                    } else {
                        let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                        if read_u32(uc, r5 + 0x148) == 2 {
                            // r764 (DISABLED, fix219): the two-stage
                            // vDoTransition+performTransition injection
                            // crashed the thread - performTransition's
                            // state-object resolution READ_UNMAPPEDs
                            // outside the real event-dispatch context.
                            // r766: armed instead for the STATEVIEW HIDE:
                            // GUI_StateView::vLeave (0x1375be8) on the live
                            // record of state 0x1e5 runs the app's own
                            // view-dismiss path (GUI_MenuManager::vLeaveView
                            // + vSetView(mm, 0)) after the orphaned paint.
                            let _ = SM_HIDE_STATE.compare_exchange(
                                0,
                                1,
                                Ordering::Relaxed,
                                Ordering::Relaxed,
                            );
                        }
                        (read_u32(uc, r5 + 0x7e0), "post")
                    };
                    // The GUI singleton instance used by 0x1650478 is the
                    // getter's live return (release-wfind shows r0=
                    // [singl+0x14] = the registered popup widget there);
                    // my earlier static guesses (0x2a13a08, 0x2551720) were
                    // wrong (0x2551720=[0x2cbe398] is a vtable, its +0x14
                    // is text 0x6172b3).  Read the same slot wfind saw by
                    // re-walking the getter chain live.
                    let holder = read_u32(uc, 0x02a1_3a00);
                    let singl = if holder != 0 { read_u32(uc, holder) } else { 0 };
                    let slot14 = if singl != 0 {
                        read_u32(uc, singl + 0x14)
                    } else {
                        0
                    };
                    let singl_addr = singl;
                    if slot14 != 0 {
                        // r768: the popup singleton's registered widget -
                        // the widget the hide channel makes invisible.
                        PU_WIDGET.store(slot14, Ordering::Relaxed);
                    }
                    let mut words = Vec::new();
                    if w != 0 {
                        for off in (0..0x30u32).step_by(4) {
                            words.push(format!("{:#x}", read_u32(uc, w + off)));
                        }
                    }
                    format!(
                        " {tag} w={w:#x} vt={:#x} w28={:#x} w98={:#x} singl={singl_addr:#x} slot14={slot14:#x} head=[{}]",
                        if w != 0 { read_u32(uc, w) } else { 0 },
                        if w != 0 { read_u32(uc, w + 0x28) } else { 0 },
                        if w != 0 { read_u32(uc, w + 0x98) & 0xffff } else { 0 },
                        words.join(" "),
                    )
                }
                "release-wfind" => {
                    let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                    if (0x0380_0000..0x0800_0000).contains(&r0) {
                        PU_WIDGET.store(r0, Ordering::Relaxed);
                    }
                    format!(
                        " slot14={r0:#x} proxy={r5:#x} noop={}",
                        r5 == 0 || r0 == 0,
                    )
                }
                _ => format!(" this={r0:#x}"),
            };
            log::info!(
                "PROCHMI enavi-chain {label} t={}ms thread=[{}] n={} caller={lr:#x} r1={r1:#x}{extra}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                n,
            );
            0
        });
    }

    if nav_trace_allowed() {
        // r788: write-watch on the enavi_tclRegistry scratch page that holds
        // the hminavserver registry path (0x54e4bf0) and the 16
        // USE_CCA_SRV_* name pointers (0x54e4c00..).  The page sits beyond
        // every LOAD segment and no relocation targets it, so someone must
        // copy it in at runtime; if nobody does, the path is all zeros,
        // vOpenPathHmiNavServerRegistry fails and all 16 services fall back
        // to the zero default - no FI client registration, enavi sinks
        // FAILED(2).  Log every writer (PC) to find who initializes it - or
        // prove nobody does.
        let watch_res_enavi = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            0x054e_4000u64,
            0x054e_4fffu64,
            move |_uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) < 80 {
                    log::warn!(
                        "PROCHMI enavi-scratch WRITE addr={address:#x} size={size} value={value:#x}"
                    );
                }
                true
            },
        );
        log::info!(
            "PROCHMI enavi-scratch write-watch installed: {}",
            watch_res_enavi.is_ok()
        );
        // r788b: cldp_Mngr singleton (0x38eee88, heap-deterministic like the
        // other 0x38e/0x39x singletons) keeps its mounted datapool domains in
        // the vector [+0x20 begin, +0x24 end); get_podp() resolves an element
        // through it, and the dest-entry writes die silently because the
        // domains owning 0x8007025/0x8007023/0x801a0c3/0x801a0c4/0x8076000
        // are never mounted.  Watch both slots: every push names a mount
        // event, so the boot census shows exactly who mounts which domain
        // and whether any mount follows the tap3 dest-entry request.
        let watch_res_dpmnt = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            0x0038_eee88u64 + 0x20,
            0x0038_eee88u64 + 0x27,
            move |_uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) < 120 {
                    log::warn!(
                        "PROCHMI dpmount WRITE t={}ms addr={address:#x} size={size} value={value:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                    );
                }
                true
            },
        );
        log::info!(
            "PROCHMI dpmount write-watch installed: {}",
            watch_res_dpmnt.is_ok()
        );
        // r757: watch the clHMIMngr framework state byte (+0x670) and its
        // pending flag (+0x671) from t=0.  The object sits at 0x38eee60
        // (heap layout has been deterministic across runs fix77..fix80).
        // Installing the watch from the *load path* matters: a watch
        // installed during the first setter call missed every writer whose
        // translation block predates it - the code that pre-seeded the
        // initial NORMAL pending silently rewrote OFF(4) back to 3 with no
        // event (r79/80: setter `str value=0x4` confirmed, yet
        // `vPerformNewAppState` read back 3; the always-on repair for this
        // ghost collapse lives further down, keyed on PENDING_FW_STATE).
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
        // r761: clEventEngine::bEventDelivery routes 0x81-type events (the
        // popup release 0x81008001 is one) to *(engine+0x474)'s vtable+0 -
        // the HSI-FI router port.  fix138: that slot is 0 for the whole run,
        // so the bWriteQueue ring was never the problem either; find who is
        // supposed to fill the engine's delivery ports (+0x474 0x81-type,
        // +0x478 0x85-type, +0x47c 0x87-type, +0x480 unhandled-tracer).
        let watch_res_eng = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            0x38ee_ebcu64 + 0x474,
            0x38ee_ebcu64 + 0x480,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::info!(
                    "PROCHMI evteng-port WRITE t={}ms thread=[{}] addr={:#x} size={} value={:#x} pc={:#x} lr={lr:#x}",
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
            "PROCHMI evteng-port write-watch installed: {}",
            watch_res_eng.is_ok()
        );
        // r761: the release event 0x81008001 IS written into the engine ring
        // (fix140: ring[0]@0x38eef2c), but every clEventEngine::bExecute in
        // the next 3.5 s sees count[q]==head[q] and the ring entry rots.
        // Watch the whole clEventQueueManager control block to catch who
        // re-zeroes count/head after a successful bWriteQueue.
        let watch_res_qm = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            0x38ee_ec8u64,
            0x38ee_f50u64,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::info!(
                    "PROCHMI evtqm WRITE t={}ms thread=[{}] addr={:#x} size={} value={:#x} pc={:#x} lr={lr:#x}",
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
            "PROCHMI evtqm write-watch installed: {}",
            watch_res_qm.is_ok()
        );
        // r761 forensics (fix187..194): the fatal crash pops {r5=qm,
        // r6=&frame, pc=0x135a3c8} off thread 52's mainloop stack around
        // 0x90610d00.  Watch that word range and log every store of those
        // smoking-gun values (any push reaching these slots included) to
        // name the frame that poisoned them.
        let watch_res_dframe = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            0x9061_0cd0u64,
            0x9061_0d24u64,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                let v = value as u32;
                if v == 0x135a_3c8
                    || v == 0x135a_3e0
                    || v == 0x135a_310
                    || v == 0x135a_1c0
                    || v == 0x9061_0d1c
                {
                    let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                    log::warn!(
                        "PROCHMI dframe WRITE t={}ms thread=[{}] addr={:#x} size={} value={:#x} pc={:#x} lr={lr:#x} sp={:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        address,
                        size,
                        v,
                        uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32,
                        uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32,
                    );
                }
                true
            },
        );
        log::info!(
            "PROCHMI dframe write-watch installed: {}",
            watch_res_dframe.is_ok()
        );
        // r761: the release-branch query 0x1706710 is getter vtable+0x40 =
        // u16[widget+0x98] on the embedded widget handler+0x19d8; its value
        // decides the 0x15e23bc close path vs the 0x15e0418 fallback.  Watch
        // the state word to learn who sets it (real device: the popup show
        // path) and whether our run ever sets it non-zero.
        let watch_res_w98 = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            (0x38f_99b0 + 0x19d8 + 0x94) as u64,
            (0x38f_99b0 + 0x19d8 + 0x9f) as u64,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI waitw98 WRITE t={}ms thread=[{}] addr={:#x} size={} value={:#x} pc={:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    address,
                    size,
                    value,
                    uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32,
                );
                true
            },
        );
        log::info!(
            "PROCHMI waitw98 write-watch installed: {}",
            watch_res_w98.is_ok()
        );
        // fix202: when does the popup widget pointer enter/leave
        // handler+0x7e0?  A write here is the show (store of the widget) or
        // the real close (store of 0).  Comparing that timestamp with the
        // enavi-fab release tells whether we release BEFORE the popup was
        // ever shown (which is exactly the r757 dangling-paint trap).
        let watch_res_show = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            (0x38f_99b0 + 0x7e0) as u64,
            (0x38f_99b0 + 0x7e8) as u64,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI popup7e0 WRITE t={}ms thread=[{}] addr={:#x} size={} value={:#x} pc={:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    address,
                    size,
                    value,
                    uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32,
                );
                true
            },
        );
        log::info!(
            "PROCHMI popup7e0 write-watch installed: {}",
            watch_res_show.is_ok()
        );
        // r761: the enavi server startup state word [[IEnavi 0x3a241c8
        // +0x4c]proxy 0x3a31b40]+0xe0 stalls at 1 (STARTING) - the
        // popup-release branch of vHandleEnaviServerStatus needs 2.  Watch
        // the word to see who writes 1 and whether anything ever advances it.
        let watch_res_srv = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            0x3a31_c20u64,
            0x3a31_c28u64,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::info!(
                    "PROCHMI enavi-srv WRITE t={}ms thread=[{}] addr={:#x} size={} value={:#x} pc={:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    address,
                    size,
                    value,
                    uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32,
                );
                true
            },
        );
        log::info!(
            "PROCHMI enavi-srv write-watch installed: {}",
            watch_res_srv.is_ok()
        );
        // r761: fix176 caught the popup handler's getter seeing a different
        // proxy than the one we wrote - watch IEnavi+0x4c for repointing.
        let watch_res_pp = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            0x3a24_214u64,
            0x3a24_218u64,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::info!(
                    "PROCHMI ienavi-proxy WRITE t={}ms thread=[{}] addr={:#x} size={} value={:#x} pc={:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    address,
                    size,
                    value,
                    uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32,
                );
                true
            },
        );
        log::info!(
            "PROCHMI ienavi-proxy write-watch installed: {}",
            watch_res_pp.is_ok()
        );
        // r759: dual-mapping hypothesis - if libosal's heap region is mapped
        // a second time under the 0x4000_0000 libosal window, a store through
        // the alias lands on the same host bytes but escapes the watch above.
        // Watch the byte at the identical intra-page offset (0x4d0) in the
        // 0x4009_4000 page to catch such alias stores.
        let watch_res3 = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            0x4009_4d0u64,
            0x4009_4d8u64,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                log::info!(
                    "PROCHMI alias-watch WRITE t={}ms thread=[{}] addr={:#x} size={} value={:#x} pc={:#x}",
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
            "PROCHMI alias-watch load-time write-watch installed: {}",
            watch_res3.is_ok()
        );
        // r759: clFi_Navi::vNewAppState (0x14b9aac) calls vStartupEnaviServer
        // *directly* - no FI wire event needed - when the static "previous
        // framework state" at 0x2a723e0 reads 4 (the HSI-coded OFF the
        // framework stores when the app enters `vEntryOff`) at the moment a
        // state-3/5 callback arrives and the nav-server handler pointer
        // (clFi_Navi+0x30) is non-null (0x14b9ba4 cmp #4 -> 0x14b9d64 ->
        // bl 0x15e0f74).  Watch it from load time to name the writer, and
        // trap the guarded entry blocks so every callback's argument and the
        // global's value at that instant are logged.
        let watch_res2 = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            0x02a7_23e0u64,
            0x02a7_23e4u64,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::info!(
                    "PROCHMI navprev-global WRITE t={}ms thread=[{}] addr={:#x} size={} value={:#x} pc={:#x} lr={lr:#x}",
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
            "PROCHMI navprev-global load-time write-watch installed: {}",
            watch_res2.is_ok()
        );
        for (off, label) in [
            (0x014b_9ba0u32 - ORIGINAL_BASE, "finavi-newstate-35"),
            (0x014b_9c14u32 - ORIGINAL_BASE, "finavi-newstate-4"),
        ] {
            let site = base_address + off;
            crate::os::code_stub::trap_site(unicorn, site, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:finavi-newstate", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let n = HITS.fetch_add(1, Ordering::Relaxed);
                if n >= 24 {
                    return 0;
                }
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                let state = read_u32(uc, (uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32).wrapping_add(0x14));
                let prev = read_u32(uc, 0x02a7_23e0);
                let navsrv = read_u32(uc, 0x38f_9970 + 0x30);
                let flag34 = read_u8(uc, 0x38f_9970 + 0x34);
                log::info!(
                    "PROCHMI enavi-chain {label} t={}ms thread=[{}] n={} caller={lr:#x} state={state:#x} prev={prev:#x} navsrv+30={navsrv:#x} flag34={flag34:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    n,
                );
                0
            });
        }
    }

    // r761: always-on enavi start fabrication (runs regardless of the trace
    // env).
    //
    // The LPM proxy hands the HMI framework its boot app states and the
    // framework setter visibly stores them at `clHMIMngr+0x670` (strb pair
    // 0x134ec64/68, hook-confirmed) with the pending flag at +0x671, but a
    // writer invisible to load-time MEM_WRITE hooks (fix121..fix124: the
    // apply pass at 0x134ee18 always finds the byte back at NORMAL(3)
    // ~20 ms later) reverts it before `clHMIMngr::vPerformNewAppState`
    // (0x134e938) reads it - so `clFi_Navi::vNewAppState` never sees the
    // INITIALIZED/OFF states whose broadcast the device's slow LPM guarantees
    // it.  That broadcast is the organic startup trigger: vNewAppState
    // (0x14b9ba0) calls `clHmiNavServerHandler::vStartupEnaviServer` when
    // NORMAL/PAUSE arrives while its static previous-state (0x2a723e0) reads
    // OFF(4), the handler (object+0x30) exists and the start-request flag
    // (object+0x34) is 1 (0x14b9d70).
    //
    // fix126..133 lesson: re-asserting the byte at the consumption point
    // makes the framework *broadcast* OFF, and dispatcher case 4 zeroes
    // clHSIMngr+0x14 - the HMI then never registers its CCA client ports at
    // t~6 s, including the NAVI service 0x27 port whose availability sink
    // (vOnUpdateNavServerServiceAvailability, 0x15f48a4) is the organic
    // sender of the 0x81008001 popup-release event.  So instead of faking the
    // dwell, reproduce only its end state: the watch records visible
    // pre-NORMAL stores; at the first real NORMAL broadcast through the
    // dispatcher (trap at 0x1352df4 entry, before the FI fan-out) the static
    // prev-global is left at the recorded value (OFF here - what the device
    // dwell would leave in it) and the start-request flag is set.  The flag's
    // device-side setter is the phone FI event 0x80108031 (0x14be400; the id
    // occurs nowhere in any firmware/SD file, scanned fix117..fix126) whose
    // producer, the smartphone enavi app, does not exist here - same
    // sanctioned fabrication class as the NAVI FI service answers.
    // Everything downstream of the guard is real prochmi code.
    if unicorn.get_data().elf_path.contains("prochmi") {
        let _ = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            0x38ee_e60u64 + 0x670,
            0x38ee_e60u64 + 0x672,
            |_uc: &mut Unicorn<'_, Context>, _t, address, _size, value| {
                if address == 0x38ee_e60u64 + 0x670 {
                    match value as u32 {
                        2 | 4 | 5 | 6 => {
                            OFF_PREV_TO_RESTORE.store(value as u32, Ordering::SeqCst);
                            OFF_SEEN.store(1, Ordering::SeqCst);
                        }
                        3 => {
                            OFF_SEEN.store(0, Ordering::SeqCst);
                            OFF_PREV_TO_RESTORE.store(0, Ordering::SeqCst);
                        }
                        _ => {}
                    }
                }
                true
            },
        );
        for (off, label) in [
            (0x0134_ee18u32 - ORIGINAL_BASE, "hmimngr-apply"),
            (0x0135_2df4u32 - ORIGINAL_BASE, "hsimngr-dispatch"),
        ] {
            let site = base_address + off;
            crate::os::code_stub::trap_site(unicorn, site, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:state-repair", move |uc: &mut Unicorn<'_, Context>| {
                static LOGS: AtomicU32 = AtomicU32::new(0);
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let byte = read_u8(uc, 0x38ee_e60u32 + 0x670);
                let pending = read_u8(uc, 0x38ee_e60u32 + 0x671);
                let want = OFF_SEEN.load(Ordering::SeqCst);
                let prev = OFF_PREV_TO_RESTORE.load(Ordering::SeqCst);
                let mut repair = "";
                if label == "hsimngr-dispatch" && r1 == 3 && want != 0 {
                    // A NORMAL broadcast is being delivered and the LPM proxy
                    // genuinely handed the HMI an OFF state a few seconds ago
                    // (visible store, hook-confirmed).  On the device that
                    // OFF dwells for seconds - the FI clients record it as
                    // their previous state and clFi_Navi's static prev-state
                    // ends at OFF(4).  Here the ghost writer collapses every
                    // non-NORMAL state within one 20 ms cycle, so reproduce
                    // only what the dwell *leaves behind*, not the broadcast:
                    // broadcasting OFF itself zeroes clHSIMngr+0x14 (dispatcher
                    // case 4) which kills the HMI's CCA client-port
                    // registration burst at t~6 s (fix126..133: zero
                    // `cca server answered ServiceRegister` lines, zero
                    // svc=0x27 gateway hits - the very client the
                    // vOnUpdateNavServerServiceAvailability sink needs for
                    // the organic 0x81008001 popup release).  Two stores
                    // here, three guest instructions before the guard reads
                    // them (0x14b9ba4 prev, 0x14b9d70 flag), serialized
                    // emulator => deterministic: prev-global := OFF and the
                    // fabricated start-request flag (counterpart of the
                    // phone-side 0x80108031, producer absent here).
                    let _ = write_u32(uc, 0x02a7_23e0u32, prev);
                    write_u8(uc, 0x38f_9970 + 0x34, 1);
                    OFF_SEEN.store(0, Ordering::SeqCst);
                    repair = " PREV4+FLAG1";
                }
                if nav_trace_enabled() && LOGS.fetch_add(1, Ordering::Relaxed) < 96 {
                    log::info!(
                        "PROCHMI enavi-chain {label} t={}ms thread=[{}] caller={lr:#x} r1={r1:#x} byte670={byte:#x} pending671={pending:#x} want={want:#x} offprev={prev:#x}{repair}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                }
                // The engine-pump rescue state machine needs a firing that
                // runs every framework cycle; the clGUI_MainLoop trap it was
                // hooked to in fix146/148 turns out to be entered only
                // sparsely (idle-semaphore blocked, like clHMIMngr::bExecute).
                // The dispatch site sees the post-pump confirmation; the
                // post-wipe site (trap below) does the re-write.
                if label == "hsimngr-dispatch" {
                    maybe_repost_evt8001(uc, site, false);
                }
                0
            });
        }
        // r761: no traps on the queue machinery itself - the repost/pump
        // state machine (maybe_repost_evt8001, run from the proven
        // clGUI_MainLoop trap) observes the ring control fields directly and
        // arms itself on an unconsumed organic 0x81008001 entry.  (fix144/
        // 145: traps at bWriteQueue's entry made the guest-call re-enter the
        // same site on return, the code_stub site ring dropped the callback
        // and the thread faulted on a garbage PC.)
        // r761: post-wipe site of the evt8001 rescue state machine (see
        // maybe_repost_evt8001).  0x134ead8 is the instruction right after
        // the bl that ran clEventEngine::vNewAppState -> bResetQueue in the
        // NORMAL branch of clHMIMngr::vPerformNewAppState, executed every
        // framework cycle.
        let site_pw = base_address + (0x0134_ead8u32 - ORIGINAL_BASE);
        crate::os::code_stub::trap_site(unicorn, site_pw, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:evt8001-pump", move |uc: &mut Unicorn<'_, Context>| {
            maybe_repost_evt8001(uc, site_pw, true);
            0
        });
        // Once-only enavi start-request flag (clFi_Navi+0x34), see above.
        // fix127: after the ctor-time write the byte read back 0 at the first
        // vNewAppState call (2.96 s), so watch the byte to name the clearer.
        let flag_watch = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            0x38f_9970u64 + 0x34,
            0x38f_9970u64 + 0x34,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::info!(
                    "PROCHMI enavi-flag WRITE t={}ms thread=[{}] addr={:#x} size={} value={:#x} pc={:#x} lr={lr:#x}",
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
            "PROCHMI enavi-flag write-watch installed: {}",
            flag_watch.is_ok()
        );
        let ctor = base_address + (0x015e_871cu32 - ORIGINAL_BASE);
        crate::os::code_stub::trap_site(unicorn, ctor, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:enavi-startflag", move |uc: &mut Unicorn<'_, Context>| {
            static DONE: AtomicBool = AtomicBool::new(false);
            if !DONE.swap(true, Ordering::SeqCst) {
                let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let ok = write_u8(uc, 0x38f_9970 + 0x34, 1);
                log::info!(
                    "PROCHMI fabricated enavi start-request flag (clFi_Navi+0x34=1) ok={ok} at handler ctor this={this:#x}"
                );
            }
            0
        });
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

    // r763 forensics: the narrow trap sites on GUI_StateMachine
    // vDoTransition (0x1371170) and performTransition (0x1371390) show zero
    // hits across fix204..213 while the popup demonstrably hid once - a
    // zero-hit probe proves nothing until re-checked with a page-range hook
    // (AGENTS.md, unicorn HOOK_EXISTS_BOUNDED quirk).  Both targets share
    // page 0x1371000; dispatch on the exact fetch address there and log any
    // other traffic on the page as a liveness witness.
    if nav_trace_enabled() {
        let page = base_address + 0x0137_1000u32 - ORIGINAL_BASE;
        let do_t = base_address + GUI_STATEMACHINE_DO_TRANSITION;
        let perf_t = base_address + GUI_STATEMACHINE_PERFORM_TRANSITION;
        // r786: bOnMessage entry witness - which SM instance receives which
        // message id while in which state (the transition-matching input).
        let onmsg_t = base_address + GUI_STATEMACHINE_ON_MESSAGE;
        // r779: decision points inside vPerformTransition for the post-click
        // autopsies: which gate (from/to path, LCA, [sm+0x18] byte, dynamic
        // stack) aborts a pending transition.  Armed whenever performTransition
        // runs with pending != 0 on that same sm.
        let dec_points: [(u32, &'static str); 7] = [
            (base_address + 0x0137_152cu32 - ORIGINAL_BASE, "from-path"),
            (base_address + 0x0137_1544u32 - ORIGINAL_BASE, "to-path"),
            (base_address + 0x0137_15a8u32 - ORIGINAL_BASE, "lca"),
            (base_address + 0x0137_1674u32 - ORIGINAL_BASE, "lca-check"),
            (base_address + 0x0137_170cu32 - ORIGINAL_BASE, "flag18"),
            (base_address + 0x0137_1724u32 - ORIGINAL_BASE, "dynstack-empty"),
            (base_address + 0x0137_177cu32 - ORIGINAL_BASE, "dynsm-active"),
        ];
        let res = unicorn.add_code_hook(
            page as u64,
            (page + 0xfff) as u64,
            move |uc: &mut Unicorn<'_, Context>, address, _size| {
                let addr = address as u32;
                let sm = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                static DEC_SM: AtomicU32 = AtomicU32::new(0);
                static DEC_WIN: AtomicU32 = AtomicU32::new(0);
                if addr == do_t {
                    let transition =
                        uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xffff;
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
                    // r764: remember which SM instance owns the sysdlg
                    // state family (0x1e4..0x1ef) - that is the popup's
                    // state machine the SM-hide injection must be driven on.
                    if (0x1e4..=0x1ef).contains(&state_id) {
                        SM_HIDE_SM.store(sm, Ordering::Relaxed);
                    }
                    // r766: capture the live 0x1e5 view record for the
                    // vLeave hide injection, and once dump the GUI single
                    // globals the vLeave path dereferences (0x5972ae0 =
                    // GUI_MenuManager singleton, Ghidram uRam05972ae0).
                    if state_id == 0x1e5 && leaf != 0 {
                        SM_HIDE_RECORD.store(leaf, Ordering::Relaxed);
                        static ONCE: AtomicU32 = AtomicU32::new(0);
                        if ONCE.fetch_add(1, Ordering::Relaxed) == 0 {
                            let mmv = read_u32(uc, 0x02a1_3f50);
                            log::warn!(
                                "PROCHMI sm-hide: captured 0x1e5 record={leaf:#x} view={:#x} globals: mm={mmv:#x} dm={:#x} sm4={:#x} mm_main={:#x}/{:#x} mm_ovl={:#x}/{:#x} sm21c={:#x}",
                                read_u16(uc, leaf + 0x30),
                                read_u32(uc, 0x02a1_2710),
                                read_u32(uc, 0x02a1_3b58),
                                read_u16(uc, mmv + 0x10),
                                read_u16(uc, mmv + 0x18),
                                read_u16(uc, mmv + 0x12),
                                read_u16(uc, mmv + 0x1a),
                                read_u32(uc, sm + 0x21c),
                            );
                        }
                    }
                    // r765: dump each sysdlg state object once - the field
                    // that matches a gui-interp r2 list pointer identifies
                    // where the entry/exit action lists live (replay key).
                    if (0x1e3..=0x1ef).contains(&state_id) && leaf != 0 {
                        static SEEN: AtomicU32 = AtomicU32::new(0);
                        let bit = 1u32 << (state_id - 0x1e3);
                        let old = SEEN.fetch_or(bit, Ordering::Relaxed);
                        if old & bit == 0 {
                            let dump: Vec<u32> = (0..64u32)
                                .map(|i| read_u32(uc, leaf + i * 4))
                                .collect();
                            log::warn!(
                                "PROCHMI SM-PAGE STATE-OBJ t={}ms state={state_id:#x} leaf={leaf:#x}: {:?}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                dump.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>(),
                            );
                        }
                    }
                    log::warn!(
                        "PROCHMI SM-PAGE doTransition t={}ms sm={sm:#x} leaf={leaf:#x} state={state_id:#x} trans={transition:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                    );
                } else if addr == perf_t {
                    let transition = if sm != 0 {
                        read_u32(uc, sm + GUI_STATE_SM_PENDING_TRANSITION)
                    } else {
                        0
                    };
                    if nav_trace_enabled() {
                        if transition != 0 {
                            DEC_SM.store(sm, Ordering::Relaxed);
                            DEC_WIN.store(3, Ordering::Relaxed);
                        } else if DEC_WIN.load(Ordering::Relaxed) > 0
                            && sm == DEC_SM.load(Ordering::Relaxed)
                        {
                            DEC_WIN.fetch_sub(1, Ordering::Relaxed);
                        }
                    }
                    if transition != 0 {
                        // Which state machine is stuck: dump its header
                        // (vtable + candidate name/descriptor pointers) once
                        // per pending pointer so the strings resolve offline.
                        // r784: cap per pending pointer, not globally - the
                        // boot-time transitions used to consume all 40 slots
                        // so post-menu stuck machines (cid 653 destination
                        // entry churn) never got decoded at all.
                        static STUCK: std::sync::Mutex<Vec<(u32, u32)>> =
                            std::sync::Mutex::new(Vec::new());
                        let seen = {
                            let mut v = STUCK.lock().unwrap_or_else(|e| e.into_inner());
                            let slot = v.iter_mut().find(|(p, _)| *p == transition);
                            match slot {
                                Some(e) => {
                                    e.1 += 1;
                                    e.1
                                }
                                None => {
                                    if v.len() < 64 {
                                        v.push((transition, 1));
                                        1
                                    } else {
                                        u32::MAX
                                    }
                                }
                            }
                        };
                        if seen <= 6 {
                            let words: Vec<u32> =
                                (0..12u32).map(|i| read_u32(uc, sm + i * 4)).collect();
                            // Record layout (GUI_StateMachine::vPerformTransition):
                            // +4 flags, +8 from-state id (>>16), +0xc low = to-state
                            // id, +0xc >>16 = trigger kind (5/9 = silent cancel).
                            let rec4 = read_u32(uc, transition + 4);
                            let rec8 = read_u32(uc, transition + 8);
                            let recc = read_u32(uc, transition + 0xc);
                            // Resolve the from/to state names the way
                            // vPerformTransition does: global state table
                            // ([iRam05973d70] + id*4) then the state's owner
                            // object ([iRam05972a30] + family*4); its word
                            // pointing at rodata holds the trace name.
                            // State table load as done by vPerformTransition
                            // (ARM, GOT base fp = 0x2a12818): table =
                            // [*[*0x2a14178] + 4], owner array = *0x2a13300,
                            // owner = owners[[state+8 >> 28]].
                            let got_table = read_u32(
                                uc,
                                PROCHMI_BASE.load(Ordering::Relaxed) + 0x02a1_4178u32 - ORIGINAL_BASE,
                            );
                            let g_table = read_u32(uc, read_u32(uc, got_table) + 4);
                            let owners_base = read_u32(
                                uc,
                                PROCHMI_BASE.load(Ordering::Relaxed) + 0x02a1_3300u32 - ORIGINAL_BASE,
                            );
                            let gsm = 0u32;
                            let _ = gsm;
                            let mut names = [String::new(), String::new()];
                            for (slot, id) in [
                                (0usize, (rec8 >> 16) as usize),
                                (1usize, (recc & 0xffff) as usize),
                            ] {
                                let st = read_u32(uc, g_table + (id as u32) * 4);
                                if st == 0 {
                                    names[slot] = "NULL".into();
                                    continue;
                                }
                                let family = (read_u32(uc, st + 8) >> 28) as usize;
                                let owner = read_u32(uc, owners_base + (family as u32) * 4);
                                let mut found = String::new();
                                for src in [st, owner] {
                                    if src == 0 || !found.is_empty() {
                                        continue;
                                    }
                                    for w in 0..24u32 {
                                        let s = read_sm_name(uc, read_u32(uc, src + w * 4));
                                        if !s.is_empty() {
                                            found = s;
                                            break;
                                        }
                                    }
                                }
                                // Simulate bGetStatePath: parent chain via
                                // state+0xc until NULL; failure iff the chain
                                // tops at a state whose vtable+0x10 returns 3.
                                let mut chain = Vec::new();
                                let mut cur = st;
                                while cur != 0 && chain.len() < 30 {
                                    chain.push(cur);
                                    cur = read_u32(uc, cur + 0xc);
                                }
                                let root = *chain.last().unwrap_or(&0);
                                let root_vt = if root != 0 { read_u32(uc, root) } else { 0 };
                                let root_typefn =
                                    if root_vt != 0 { read_u32(uc, root_vt + 0x10) } else { 0 };
                                names[slot] = format!(
                                    "st{st:#x} f{family}/{found} d{} root={root:#x} rt10={root_typefn:#x}",
                                    chain.len()
                                );
                            }
                            let n98 = read_u32(uc, sm + 0x98);
                            let n118 = read_u32(uc, sm + 0x118);
                            log::warn!(
                                "PROCHMI SM-PAGE stuck t={}ms sm={sm:#x} pending={transition:#x} from={:#x}=>{} to={:#x}=>{} trig={:#x} n98={n98:#x} n118={n118:#x}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                rec8 >> 16,
                                names[0],
                                recc & 0xffff,
                                names[1],
                                (recc >> 16) & 0xff,
                            );
                            for (i, w) in words.iter().enumerate() {
                                let s = read_sm_name(uc, *w);
                                if !s.is_empty() {
                                    log::warn!("PROCHMI SM-PAGE stuck sm+{:#x} -> \"{s}\"", i * 4);
                                }
                            }
                        }
                    }
                    log::warn!(
                        "PROCHMI SM-PAGE performTransition t={}ms sm={sm:#x} pending={transition:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                    );
                } else if addr == onmsg_t {
                    // r786: every message entering a state machine, with the
                    // machine's current state - the transition-matching input.
                    if nav_trace_enabled() {
                        static ON_HITS: AtomicU32 = AtomicU32::new(0);
                        if ON_HITS.fetch_add(1, Ordering::Relaxed) < 3000 {
                            let msg = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                            let msgid = read_u32(uc, msg + 4);
                            let mtype = read_u16(uc, msg + 0xc);
                            let state = read_u32(uc, sm + 8);
                            let dyn18 = read_u32(uc, sm + 0x18) & 0xff;
                            let leaf =
                                if sm != 0 { read_u32(uc, sm + GUI_STATE_SM_CURRENT_LEAF) } else { 0 };
                            let state_id =
                                if leaf != 0 { read_u16(uc, leaf + GUI_STATE_STATE_ID) } else { 0 };
                            log::warn!(
                                "PROCHMI SM-PAGE onMessage t={}ms sm={sm:#x} msgid={msgid:#x} type={mtype:#x} state={state:#x} dyn={dyn18:#x} sid={state_id:#x}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                            );
                        }
                    }
                } else if nav_trace_enabled() {
                    for (dec_addr, dec_name) in dec_points.iter() {
                        if addr != *dec_addr {
                            continue;
                        }
                        let sm7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                        if DEC_WIN.load(Ordering::Relaxed) > 0
                            && sm7 == DEC_SM.load(Ordering::Relaxed)
                        {
                            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                            let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                            let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                            let r9 = uc.reg_read(RegisterARM::R9).unwrap_or(0) as u32;
                            let n98 = read_u32(uc, sm7 + 0x98).min(20);
                            let n118 = read_u32(uc, sm7 + 0x118).min(20);
                            let a1 = read_u32(uc, sm7 + 0x94);
                            let a2 = read_u32(uc, sm7 + 0x114);
                            let ids = |arr: u32, cnt: u32| -> Vec<u32> {
                                (0..=cnt)
                                    .map(|i| {
                                        let st = read_u32(uc, arr + i * 4);
                                        if st == 0 {
                                            0xffff
                                        } else {
                                            read_u16(uc, st + GUI_STATE_STATE_ID) as u32
                                        }
                                    })
                                    .collect()
                            };
                            log::warn!(
                                "PROCHMI SM-DEC {dec_name} t={} sm={sm7:#x} r0={r0:#x} r3={r3:#x} r4={r4:#x} r9={r9:#x} p1={:?} p2={:?}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                ids(a1, n98),
                                ids(a2, n118),
                            );
                        }
                        break;
                    }
                } else {
                    static OTHERS: AtomicU32 = AtomicU32::new(0);
                    let n = OTHERS.fetch_add(1, Ordering::Relaxed);
                    if n < 4 {
                        log::info!(
                            "PROCHMI SM-PAGE other-fetch t={}ms addr={addr:#x} lr={:#x}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                        );
                    }
                }
            },
        );
        log::info!(
            "PROCHMI SM page-hook 0x{page:x} installed: {}",
            res.is_ok()
        );
    }

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

    // r765: the popup-string read chain ends in the UTF-16/8 builder pair
    // 0x1362cb0/0x1362d34 (fix224: strlen inside them reading the string).
    // Trap the builder entry - its own prologue pushes the caller's return,
    // so the raw stack [5] and LR name the true presenter.  Cold site: only
    // GUI text touches fire it.
    {
        for (offset, label) in [
            (0x0136_2cb0u32 - ORIGINAL_BASE, "text-builder"),
            (0x0136_2d34u32 - ORIGINAL_BASE, "text-builder2"),
            (0x0136_bc40u32 - ORIGINAL_BASE, "gui-interp"),
        ] {
            let address = base_address + offset;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:textbuilder", move |uc: &mut Unicorn<'_, Context>| {
                // r769: the script-cell capture below is the hide
                // channel's data path (PU_OBJS/PU_IMPS) and must run in
                // default no-env runs; only the forensic dumps stay
                // behind EMU_PROCHMI_NAV_TRACE.  Once both painted cells
                // are captured a trace-less hit is a cheap early return.
                let tracing = nav_trace_enabled();
                if !tracing && PU_COUNT.load(Ordering::Relaxed) >= 2 {
                    return 0;
                }
                static HITS: AtomicU32 = AtomicU32::new(0);
                let now = crate::libs::dapi::emu_t0_elapsed_ms();
                if HITS.fetch_add(1, Ordering::Relaxed) >= 4000 || now < 10_000 {
                    return 0;
                }
                let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                let words: Vec<u32> =
                    (0..10u32).map(|i| read_u32(uc, sp + i * 4)).collect();
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r2v = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                // r766: on the popup-string paint, dump the interpreter's
                // materialized command cells at r0 and resolve every heap
                // node's string fields - the names identify the widget
                // object being painted (resource SYSDLG_NAV_STARTING_UP_
                // SYSIND), the handle the hide must be driven on.
                let r1v = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    if label == "text-builder" && r1v == 0x0251_86fc {
                        static DONE: AtomicU32 = AtomicU32::new(0);
                        if DONE.fetch_add(1, Ordering::Relaxed) < 2 {
                            let cells: Vec<u32> =
                                (0..40u32).map(|i| read_u32(uc, r0 + i * 4)).collect();
                            if tracing {
                                log::warn!(
                                    "PROCHMI {label} POPUPCELLS r0={r0:#x}: {:?}",
                                    cells.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>(),
                                );
                            }
                        let mut targets: Vec<u32> = cells
                            .iter()
                            .copied()
                            .filter(|w| (0x0390_0000..0x03d0_0000).contains(w))
                            .collect();
                        let level1 = targets.clone();
                        for cell in &level1 {
                            for i in [2u32, 4] {
                                let w = read_u32(uc, cell + i * 4);
                                if (0x0390_0000..0x03d0_0000).contains(&w)
                                    && !targets.contains(&w)
                                {
                                    targets.push(w);
                                }
                            }
                        }
                        for cell in targets {
                            let nf: Vec<u32> =
                                (0..12u32).map(|i| read_u32(uc, cell + i * 4)).collect();
                            // r768: capture the painted popup script objects -
                            // GUI_Label (primary vtable 0x2872150, secondary
                            // 0x28721a4) and GUI_BGImg (0x28721c8).  Their
                            // vDraw runs the Lua paint only while obj+0x68 is
                            // set, so the widget-teardown channel can switch
                            // them off without touching any MenuManager slot.
                            const SCRIPT_VT: [u32; 3] =
                                [0x0287_2150, 0x0287_21a4, 0x0287_21c8];
                            if SCRIPT_VT.contains(&nf[0]) {
                                let n = PU_COUNT.load(Ordering::Relaxed) as usize;
                                let already = (0..n)
                                    .any(|k| PU_OBJS[k].load(Ordering::Relaxed) == cell);
                                if n < PU_OBJS.len() && !already {
                                    PU_OBJS[n].store(cell, Ordering::Relaxed);
                                    // the cell's owning widget (f6, +0x18)
                                    PU_IMPS[n].store(read_u32(uc, cell + 0x18), Ordering::Relaxed);
                                    PU_COUNT.store(n as u32 + 1, Ordering::Relaxed);
                                    log::warn!(
                                        "PROCHMI popup-uninit: captured cell {cell:#x} vt={:#x} owner={:#x}",
                                        nf[0],
                                        read_u32(uc, cell + 0x18),
                                    );
                                }
                            }
                            // C++ class name: GUI classes keep the mangled
                            // name pointer near vtable+0x2c (r765 forensics
                            // on GUI_StateView); resolve [cell] vtables too.
                            // (pure forensics - skipped without tracing, the
                            // PU_* capture above already ran.)
                            if !tracing {
                                continue;
                            }
                            let mut classes: Vec<String> = Vec::new();
                            for fld in [nf[0], nf[2], nf[4]] {
                                if !(0x0286_5294..0x02d0_0000).contains(&fld) {
                                    continue;
                                }
                                for vo in [0x1cu32, 0x20, 0x24, 0x28, 0x2c, 0x30] {
                                    let np = read_u32(uc, fld + vo);
                                    if !(0x0000_8000..0x0286_5294).contains(&np) {
                                        continue;
                                    }
                                    let mut rb = [0u8; 96];
                                    if uc.mem_read(np as u64, &mut rb).is_err() {
                                        continue;
                                    }
                                    if let Some(end) = rb.iter().position(|&b| b == 0) {
                                        if let Ok(s) = std::str::from_utf8(&rb[..end]) {
                                            if s.len() >= 6
                                                && s.bytes().all(|b| b.is_ascii_graphic())
                                                && !classes.iter().any(|c| c == s)
                                            {
                                                classes.push(s.to_string());
                                            }
                                        }
                                    }
                                }
                            }
                            let names: Vec<String> = nf
                                .iter()
                                .filter_map(|&w| {
                                    if !(0x0000_8000..0x0286_5294).contains(&w) {
                                        return None;
                                    }
                                    let mut rb = [0u8; 96];
                                    if uc.mem_read(w as u64, &mut rb).is_err() {
                                        return None;
                                    }
                                    let end = rb.iter().position(|&b| b == 0)?;
                                    let s = std::str::from_utf8(&rb[..end]).ok()?;
                                    if s.len() >= 5 && s.bytes().all(|b| b.is_ascii_graphic()) {
                                        Some(s.to_string())
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            log::warn!(
                                "PROCHMI {label} NODE {cell:#x}: f={:?} cls={:?} strs={:?}",
                                nf.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>(),
                                classes,
                                names,
                            );
                        }
                    }
                }
                // r765: for the interpreter, also dump the head of the
                // action list it is about to run (r2) - the record format
                // is what the r766 replay needs (entry list of state 0x1e4
                // = the organic hide action sequence).
                if tracing && label == "gui-interp" {
                    // r765: dump printable strings inside the script object
                    // referenced by the list header (list[2], list[4]) -
                    // the VM's string constants name the popup's actions
                    // (and its hide counterpart).
                    let hdr2 = read_u32(uc, r2v + 8);
                    let r1v = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    static SEEN2: std::sync::LazyLock<
                        std::sync::Mutex<std::collections::HashSet<u32>>,
                    > = std::sync::LazyLock::new(|| {
                        std::sync::Mutex::new(std::collections::HashSet::new())
                    });
                    let fresh = match SEEN2.lock() {
                        Ok(mut s) => s.len() < 6 && r1v != 0 && s.insert(r1v),
                        Err(_) => false,
                    };
                    if fresh {
                        for obj in [hdr2, r1v] {
                        let mut strs: Vec<String> = Vec::new();
                        let mut cur = String::new();
                        for i in 0..4096u32 {
                            let mut bb = [0u8; 1];
                            let b = if uc.mem_read((obj + i) as u64, &mut bb).is_ok() {
                                bb[0]
                            } else {
                                break;
                            };
                            if b.is_ascii_alphanumeric()
                                || matches!(b, b'_' | b'.' | b'-' | b'(' | b')' | b' ')
                            {
                                cur.push(b as char);
                            } else {
                                if cur.len() >= 5 && !strs.contains(&cur) {
                                    strs.push(std::mem::take(&mut cur));
                                }
                                cur.clear();
                            }
                        }
                        log::warn!(
                            "PROCHMI gui-interp STRINGS t={}ms obj={obj:#x}: {:?}",
                            now,
                            strs,
                        );
                        }
                    }
                }
                if label == "gui-interp" {
                    // One dump per distinct action list (r2): the lists are
                    // long-lived heap compile products, re-run every frame.
                    static SEEN: std::sync::LazyLock<
                        std::sync::Mutex<std::collections::HashSet<u32>>,
                    > = std::sync::LazyLock::new(|| {
                        std::sync::Mutex::new(std::collections::HashSet::new())
                    });
                    let fresh = match SEEN.lock() {
                        Ok(mut s) => s.len() < 24 && s.insert(r2v),
                        Err(_) => false,
                    };
                    if fresh {
                        let listdump: Vec<u32> =
                            (0..32u32).map(|i| read_u32(uc, r2v + i * 4)).collect();
                        log::warn!(
                            "PROCHMI gui-interp LIST t={}ms r0={:#x} r1={:#x} r2={r2v:#x} list={:?}",
                            now,
                            r0,
                            uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                            listdump.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>(),
                        );
                    }
                }
                log::warn!(
                    "PROCHMI {label} t={}ms thread=[{}] r0={:#x} r1={:#x} r2={:#x} r3={:#x} lr={:#x} sp={sp:#x} st={:?}",
                    now,
                    uc.get_data().inner.thread_id(),
                    r0,
                    uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                    words.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>(),
                );
                0
            });
        }
    }

    // r778: GUI_Messaging::vFireInternalEvent - which internal GUI message
    // (id/type words) does a widget click put on the bus, and who is the
    // receiver?  Trace-only probe to attribute page-switch chains.
    {
        let address = base_address + 0x0136_5868u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:msg-fire", move |uc: &mut Unicorn<'_, Context>| {
            if !nav_trace_enabled() {
                return 0;
            }
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 4000 {
                return 0;
            }
            let msg = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let receiver = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let w0 = read_u32(uc, msg);
            let w1 = read_u32(uc, msg + 4);
            let w2 = read_u32(uc, msg + 8);
            // r786: GUI_Message::vFire dispatch key is the type short at +0xc
            // (GUI_SuperStateMachine::bOnMessage switches on it), the id word
            // alone is per-class and not the routing key.
            let mtype = read_u16(uc, msg + 0xc);
            // r786: receiver class fingerprint.  The vptr is a static rodata
            // address (Ghidra VAs == runtime VAs for this image), so the vtable
            // dump identifies the receiver's C++ class without symbols.
            let vptr = read_u32(uc, receiver);
            let vtab: Vec<u32> = if (ORIGINAL_BASE..0x3000_0000).contains(&vptr) {
                (0..6u32)
                    .map(|i| read_u32(uc, vptr.wrapping_add(i * 4)))
                    .collect()
            } else {
                Vec::new()
            };
            // r786: type 0x49 = GUI_ContentPopupOper action token.  The
            // action table (GUI_ContentPopupOper::s_vPerformAction_now:
            // entry = u16 op, u16 a, u16 c, u8 b; table ptr @0x02a1769c,
            // SuperSM global @[[0x02a14178]] - both slots of the GUI global
            // table at 0x02a12818) decides which dynamic-stack operation a
            // button click runs.  Dump the decoded entry so tap actions
            // resolve offline.
            let action = if mtype == 0x49 && (w1 & 0xf000_0000) == 0xc000_0000 {
                let idx = w1 & 0x07ff_ffff;
                let table = read_u32(uc, 0x02a1_769c);
                if idx < 0x300 && table != 0 {
                    let e = table + idx * 8;
                    let op = read_u16(uc, e);
                    let a = read_u16(uc, e + 2);
                    let c = read_u16(uc, e + 4);
                    let b = (read_u32(uc, e + 4) >> 16) as u16 & 0xff;
                    let smg = read_u32(uc, read_u32(uc, 0x02a1_4178));
                    Some(format!("idx={idx:#x} op={op:#x} a={a:#x} b={b:#x} c={c:#x} sm={smg:#x}"))
                } else {
                    Some(format!("idx={idx:#x} table={table:#x}"))
                }
            } else {
                None
            };
            log::warn!(
                "PROCHMI msg-fire t={}ms msg={msg:#x} w=[{w0:#x},{w1:#x},{w2:#x}] type={mtype:#x} receiver={receiver:#x} vptr={vptr:#x} vt={:?} action={:?} lr={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                vtab.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>(),
                action,
                read_u32(uc, (uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32).wrapping_sub(8)),
            );
            0
        });
    }

    // r786: GUI_SuperStateMachine::vPerformDynamicStackOperation (0x1377370)
    // - the decoded action every content-popup button executes (r1 = op/state
    // index, r2 = popup id, r3 = operation kind 0..7, [sp] = priority).  This
    // is the level where "New Address" either pushes the keyboard popup onto
    // the dynamic stack or silently drops it.
    {
        let want = base_address + 0x0137_7370u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, want, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:dynstack-op", move |uc: &mut Unicorn<'_, Context>| {
            if !nav_trace_enabled() {
                return 0;
            }
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 4000 {
                return 0;
            }
            // The post-tap2 heartbeat re-fires REMOVE popup 0x622 at 15 Hz;
            // cap each distinct (op, popup, kind) at 3 log lines so the
            // budget covers the whole run.
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
            {
                static SEEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<(u32, u32, u32), u32>>> =
                    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
                let mut show = true;
                if let Ok(mut m) = SEEN.lock() {
                    let c = m.entry((r1, r2, r3)).or_insert(0);
                    *c += 1;
                    show = *c <= 3;
                }
                if !show {
                    return 0;
                }
            }
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let prio = read_u16(uc, sp);
            // Top of the live dynamic stack after previous ops: begin/end
            // cursors at sm+0x30/+0x34, records 0x18B with the key u16 pair
            // at record+0x4/+0x6 (seen as end-0x14/end-0x12 in the switch).
            let begin = read_u32(uc, r0 + 0x30);
            let end = read_u32(uc, r0 + 0x34);
            let top = if end >= begin + 0x18 {
                let t = end - 0x18;
                format!("{:#x}/{:#x}", read_u16(uc, t + 4), read_u16(uc, t + 6))
            } else {
                "empty".into()
            };
            // r786: on INSERT, dump the state-table slots the operation
            // resolves (GUI_StateBase slots at *(sm+4)[op] and *(sm+4)[popup])
            // and the popup state object's head - the slot whose +0x30 u16 is
            // the view cid the companion block later instantiates.
            let slots = if true {
                let tb = read_u32(uc, r0 + 4);
                let sop = read_u32(uc, tb.wrapping_add(r1 * 4));
                let spo = read_u32(uc, tb.wrapping_add(r2 * 4));
                let obj = if spo != 0 && spo < 0x0f00_0000 {
                    let w: Vec<u32> =
                        (0..10u32).map(|i| read_u32(uc, spo + i * 4)).collect();
                    // The popup's GUI_StateView child sits at record+0x50
                    // (observed: waypoint 0x39f8a68 -> 0x39f8ab8 view 0x25f).
                    let child = spo + 0x50;
                    let cview = read_u16(uc, child + 0x30);
                    let cidval = read_u16(uc, spo + 0x30);
                    format!(
                        " cid={cidval:#x} vt={:#x} child{{vt={:#x} id={:#x} view={cview:#x}}} w={:?}",
                        w[0],
                        read_u32(uc, child),
                        read_u32(uc, child + 8),
                        w.iter()
                            .skip(1)
                            .map(|v| format!("{v:#x}"))
                            .collect::<Vec<_>>()
                    )
                } else {
                    String::new()
                };
                format!(" slot[{r1:#x}]={sop:#x} slot[{r2:#x}]={spo:#x}{obj}")
            } else {
                String::new()
            };
            log::warn!(
                "PROCHMI dynstack-op t={}ms sm={r0:#x} op={r1:#x} popup={r2:#x} kind={r3:#x} prio={prio:#x} top={top} lr={lr:#x} slots={slots}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
            );
            0
        });
    }

    // r786: who writes the pending companion cid of the overlay view record
    // (0x0391ab50: main pair {cur,next} u16 at +0x10/+0x12, overlay pair at
    // +0x18/+0x1a)?  "New Address" switches with next cid 0 - the writer of
    // that 0 is the requester whose CCA binding failed; the waypoint writer
    // (cid 653) is the working reference.
    {
        use unicorn_engine::unicorn_const::HookType;
        let ovl = 0x0391_ab50u32;
        let res = unicorn.add_mem_hook(
            HookType::MEM_WRITE,
            (ovl + 0x10) as u64,
            (ovl + 0x1b) as u64,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                if !nav_trace_enabled() {
                    return true;
                }
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 300 {
                    return true;
                }
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI ovl-cid t={}ms ovl+{:#x} <- {:#x} (size {}) lr={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    address as u32 - ovl,
                    value & 0xffff_ffff,
                    size,
                    lr,
                );
                true
            },
        );
        log::debug!("PROCHMI ovl-cid watch installed: {}", res.is_ok());
    }

    // r786: vPerformViewChange companion-block loads: 0x136491c loads the
    // outgoing cid from record+0x12, 0x136492c the incoming one from
    // record+0x1a (r5 = the view-change record).  Dump r5 and the record on
    // every companion switch so the requester that queued cid 0 (popup 0x636)
    // can be identified and its record memory watched.
    {
        for (offset, label) in [
            (0x0136_491cu32 - ORIGINAL_BASE, "ovl-out"),
            (0x0136_492cu32 - ORIGINAL_BASE, "ovl-in"),
        ] {
            let address = base_address + offset;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:viewchg-ovl", move |uc: &mut Unicorn<'_, Context>| {
                if !nav_trace_enabled() {
                    return 0;
                }
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 60 {
                    return 0;
                }
                let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                let rec: Vec<u32> =
                    (0..10u32).map(|i| read_u32(uc, r5.wrapping_add(i * 4))).collect();
                log::warn!(
                    "PROCHMI viewchg {label} t={}ms r5={r5:#x} rec={:?}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    rec.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>(),
                );
                0
            });
        }
    }

    // r786: GUI_StateView::vEnter (0x1375a4c) is the point where a popup's
    // compiled view cid (this+0x30) is requested of the MenuManager, gated by
    // ([parent state]+0x18 || SuperSM+0x18).  Log both gate inputs and the
    // cid for every enter/leave so the cid-0 dialog request is witnessed.
    {
        for (offset, label) in [
            (0x0137_5a4cu32 - ORIGINAL_BASE, "enter"),
            (0x0137_5be8u32 - ORIGINAL_BASE, "leave"),
        ] {
            let address = base_address + offset;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:stateview", move |uc: &mut Unicorn<'_, Context>| {
                if !nav_trace_enabled() {
                    return 0;
                }
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 400 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let rec = read_u32(uc, r0 + 0xc);
                let parent = read_u32(uc, rec + 4);
                log::warn!(
                    "PROCHMI stateview {label} t={}ms this={r0:#x} id={:#x} view={:#x} pgate={:#x} dynsm={:#x} layer={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    read_u32(uc, r0 + 8),
                    read_u16(uc, r0 + 0x30),
                    read_u32(uc, parent + 0x18) & 0xff,
                    read_u32(uc, 0x039e_1cb0 + 0x18) & 0xff,
                    read_u32(uc, parent + 0x21c),
                );
                0
            });
        }
    }

    // r786: GUI_StateMachine::vEnter (0x1371a1c) / vLeave (0x1371b9c) - the
    // sub-SM pointers vShowDynamicState drives per dynamic-stack item
    // (item+8).  For working popups this is the family's dynsm; witness what
    // gets entered for popup 0x636 (null?  dynsm without a matching
    // transition?).
    {
        for offset in [0x0137_1a1cu32 - ORIGINAL_BASE, 0x0137_1b9cu32 - ORIGINAL_BASE] {
            let address = base_address + offset;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:sm-enterleave", move |uc: &mut Unicorn<'_, Context>| {
                if !nav_trace_enabled() {
                    return 0;
                }
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 400 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let label = if offset == 0x0137_1a1cu32 - ORIGINAL_BASE {
                    "enter"
                } else {
                    "leave"
                };
                log::warn!(
                    "PROCHMI sm-{} t={}ms sm={r0:#x} vt={:#x} dyn={:#x} state={:#x}",
                    label,
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    read_u32(uc, r0),
                    read_u32(uc, r0 + 0x18) & 0xff,
                    read_u32(uc, r0 + 8),
                );
                0
            });
        }
    }

    // r788: navif_tclIFMediator (the NAVIF AIL service client that feeds the
    // GUI mediators, incl. kind-9 0x90000206) - enumerate which service ids
    // this interface registers/awaits and whether answers arrive.
    {
        for (offset, name) in [
            (0x01c7_cf6cu32, "svc-register"),
            (0x01c7_d058u32, "svc-available"),
            (0x01c7_d068u32, "svc-change"),
            (0x01c7_d248u32, "exec-cmd"),
        ] {
            let address = base_address + offset - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:navif-mediator", move |uc: &mut Unicorn<'_, Context>| {
                if !nav_trace_enabled() {
                    return 0;
                }
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 600 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI navif-{name} t={}ms this={r0:#x} r1={r1:#x} r2={r2:#x} r3={r3:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                );
                0
            });
        }
    }

    // r788: OSAL_tclThread::vEntryHelper (0x1c83d70) - the entry of every
    // CL (communication layer) thread prochmi spawns (HMINAVSRV among
    // them).  Logs tid + the vfn+0x28 thread body so the thread can be
    // followed in the scheduler/bridge logs (which wait it parks on).
    {
        let address = base_address + 0x01c8_3d70u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:tcl-thread", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) < 40 {
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let vt = read_u32(uc, r0);
                log::warn!(
                    "PROCHMI tcl-thread tid=[{}] this={r0:#x} vt={vt:#x} body={:#x}",
                    uc.get_data().inner.thread_id(),
                    read_u32(uc, vt.wrapping_add(0x28)),
                );
            }
            0
        });
    }

    // r787: GUI_StatePseudoMediatorSync::vStart (0x13756ec) - every compiled
    // mediator-sync state logs its mediator descriptor here when its machine
    // starts.  The descriptor identifies the CL data source (mediator content
    // id + registry kind) a dialog waits on; which ones never report
    // data-ready is the milestone-C blocker list.
    {
        let address = base_address + 0x0137_56ecu32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:mediator-sync", move |uc: &mut Unicorn<'_, Context>| {
            if !nav_trace_enabled() {
                return 0;
            }
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 400 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            let id = read_u32(uc, r0 + 8);
            let desc = read_u32(uc, r0 + 0x20);
            let (d8, dc, d10) = if desc != 0 && desc < 0x0f00_0000 {
                (read_u32(uc, desc + 8), read_u32(uc, desc + 0xc), read_u16(uc, desc + 0x10))
            } else {
                (0, 0, 0)
            };
            log::warn!(
                "PROCHMI mediator-sync t={}ms this={r0:#x} id={id:#x} flags={:#x} desc={desc:#x} d+8={d8:#x} d+c={dc:#x} d+10={d10:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                read_u32(uc, r0 + 0x28),
            );
            // One-shot dump of the kind-indexed content registry the state
            // consults.  vStart: r4 = 0x1375708 + 0x169d110 = the .got base
            // 0x02a12818, table = [r4 + 0xae8] = [0x02a13300] (GUI_ContentId
            // singleton, prelinked at 0x02cb9518).
            static DUMPED: AtomicU32 = AtomicU32::new(0);
            if DUMPED.fetch_add(1, Ordering::Relaxed) < 3 {
                let table = read_u32(uc, 0x02a1_3300u32);
                let mut line = format!(" table={table:#x}");
                for kind in 0..10u32 {
                    let h = read_u32(uc, table.wrapping_add(kind * 4));
                    let hv = read_u32(uc, h);
                    line.push_str(&format!(" k{kind}={h:#x}/{hv:#x}"));
                }
                log::warn!("PROCHMI cid-reg-dump{line}");
                // The wait's actual data source: GUI_ContentFunction entry
                // 0x206 (mediator 0x90000206).  s_pGetData: table =
                // [.got 0x02a12818 + 0x3ea4] (heap, allocated by
                // GUI_ContentId::s_vInitialize), 8-byte {fnptr, raw, type}
                // rows; fnptr names the producer class feeding the dialog.
                let table = read_u32(uc, 0x02a1_66bcu32);
                let entry = table.wrapping_add(0x206 * 8);
                let fnp = read_u32(uc, entry);
                let raw = read_u32(uc, entry + 4);
                log::warn!(
                    "PROCHMI cid-fn206 table={table:#x} entry={entry:#x} getter={fnp:#x} raw={raw:#x} type={}",
                    read_u32(uc, entry + 6) & 0xff,
                );
            }
            0
        });
    }

    // r788: which framework function registers HSA devices with CCA (the
    // KEYBRD device gets vSetRegisterID(0x180,0xe7) at t=6 s but
    // clHSA_Navigation never does) - stack backtrace at the keybrd call
    // names the registration driver; its per-device gate is the exact point
    // where the NAVI device drops out.
    {
        let address = base_address + 0x0138_be84u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:hsa-reg-bt", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 40 {
                return 0;
            }
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let mut bt = String::new();
            for i in 0..14u32 {
                let w = read_u32(uc, sp.wrapping_add(i * 4));
                if (0x00fb_7080..0x023b_eb1b).contains(&w) {
                    bt.push_str(&format!(" {w:#x}"));
                }
            }
            log::warn!(
                "PROCHMI hsa-reg-bt t={}ms dev={r0:#x} svc={r1:#x} regid={r2:#x} bt:{bt}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
            );
            0
        });
    }

    // r788: enavi client registry gate.  bIsCCAServiceRequired() consults
    // the hminavserver APP_CONFIG key opened by vOpenPathHmiNavServerRegistry
    // (reg key object 0x597e990, path ptr 0x54e4bf0, open-ok flag
    // 0x597edb0); on a failed open all 16 USE_CCA_SRV_* fall back to the
    // default byte table at 0x2635e28 - which is all zeros - so
    // oGetRequiredServiceIDs returns an EMPTY vector and the enavi client
    // never registers a single FI service: every dest-entry sink answers
    // FAILED(2).  The path string lives in runtime .bss, so log it (and the
    // open flag) from the live guest memory.
    {
        let address = base_address + 0x0169_dba8u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:enavi-reg", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            let n = HITS.fetch_add(1, Ordering::Relaxed);
            if n >= 8 {
                return 0;
            }
            let path_ptr = read_u32(uc, 0x054e_4bf0);
            let mut path = String::new();
            if path_ptr != 0 {
                for i in 0..96u32 {
                    let b = read_u32(uc, path_ptr + i) & 0xff;
                    if b == 0 {
                        break;
                    }
                    path.push(char::from(b as u8));
                }
            }
            // Dump the 16 USE_CCA_SRV_* key-name strings (pointer table in
            // .bss at 0x54e4c00) so the required registry keys are known.
            let mut names = String::new();
            for k in 0..16u32 {
                let np = read_u32(uc, 0x054e_4c00 + k * 4);
                if np == 0 {
                    names.push_str("<null> ");
                    continue;
                }
                let mut t = String::new();
                for i in 0..48u32 {
                    let b = read_u32(uc, np + i) & 0xff;
                    if b == 0 {
                        break;
                    }
                    t.push(char::from(b as u8));
                }
                names.push_str(&t);
                names.push(' ');
            }
            // Prelink-baked slot contents live in .rel.ro (0x2a1c8b0 = path
            // slot, 0x2a18978 = name-table slot); vOpenPath dereferences
            // slot -> pointer -> string, so the RUNTIME slot value decides
            // what path the guest really passes.  Log it alongside.
            let path_slot = read_u32(uc, 0x02a1_c8b0);
            let name_slot = read_u32(uc, 0x02a1_8978);
            log::warn!(
                "PROCHMI enavi-reg t={}ms tid=[{}] path={path:?} openflag={} pathslot={path_slot:#x} nameslot={name_slot:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                read_u32(uc, 0x0597_edb0) & 0xff,
            );
            log::warn!("PROCHMI enavi-reg keys: {names}");
            0
        });
        // r788b: `cmp r0,#0` right after `bl bOpen` inside the same function
        // (0x169dbc0 `ldr r1,[r2]` produced the char* argument): log the real
        // path pointer/string, the intermediate dereference r2 and bOpen's
        // return, so a runtime fix-up of the slot (or an unmapped scratch
        // read) becomes visible.
        let address2 = base_address + 0x0169_dbccu32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address2, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:enavi-reg2", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            let n = HITS.fetch_add(1, Ordering::Relaxed);
            if n >= 8 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let mut path = String::new();
            if r1 > 0x100 {
                for i in 0..128u32 {
                    let b = read_u32(uc, r1.wrapping_add(i)) & 0xff;
                    if b == 0 {
                        break;
                    }
                    path.push(char::from(b as u8));
                }
            }
            log::warn!(
                "PROCHMI enavi-reg2 t={}ms tid=[{}] bOpen ret={r0:#x} path_ptr={r1:#x} deref={r2:#x} path={path:?}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }

    // r788b: clHSA_Navigation bring-up census.  vInit + the Init family are
    // what turn the constructed singleton into a live client (register FI
    // handlers, seed datapool listeners, arm the dest-entry speller).  The
    // constructor ran at t=2.2 s (inst-hsa-nav-constr); if vInit/vDestEntry*
    // never fire, the bring-up chain breaks between construction and use,
    // and the last executed Init names the missing precondition.
    {
        for (offset, name) in [
            (0x0187_9794u32, "hsanav-vinit"),
            (0x0187_ce5cu32, "hsanav-destentry-init"),
            (0x0187_dac4u32, "hsanav-destentry-country"),
            (0x0187_a2a8u32, "hsanav-homedest-init"),
            (0x018a_2fa4u32, "hsanav-routecriteria-init"),
            (0x0161_fd1cu32, "enavi-printstatus"),
        ] {
            let address = base_address + offset - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:hsanav-init", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 60 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI {name} t={}ms tid=[{}] this={r0:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
                0
            });
        }
    }

    // r788b: the proxy dispatch inside vDestEntryInitWithCountry: 0x187db38
    // `ldr pc,[r3,#4]` jumps to getCMNavi-proxy vtable slot +4 with the
    // {0,0x2955,0,0} request on stack.  Log the proxy, its vtable and slots
    // so the genuine destination-entry request transport becomes visible -
    // on the unit this call must survive until the address database is
    // loaded (popup-gated), so its target is the gate to the keyboard page.
    {
        let address = base_address + 0x0187_db38u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:cmnavi-call", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            let n = HITS.fetch_add(1, Ordering::Relaxed);
            if n >= 20 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let w0 = read_u32(uc, sp.wrapping_add(0x14));
            let w1 = read_u32(uc, sp.wrapping_add(0x18));
            let slots: Vec<String> = (0..6u32)
                .map(|i| format!("{:#x}", read_u32(uc, r3.wrapping_add(i * 4))))
                .collect();
            // clHSI_CMNAVI internals: [obj+0xc] = datapool write port (its
            // vfn+0x1c writes an element), base vfn+0x24 forwards to the GUI.
            // Dump both vtables to get next-run hook targets for the real
            // transport of the dest-entry request.
            let port = read_u32(uc, r0.wrapping_add(0xc));
            let port_vt = read_u32(uc, port);
            let port_slots: Vec<String> = (0..12u32)
                .map(|i| format!("{:#x}", read_u32(uc, port_vt.wrapping_add(i * 4))))
                .collect();
            log::warn!(
                "PROCHMI cmnavi-call t={}ms tid=[{}] proxy={r0:#x} vptr={r3:#x} req=[{w0:#x},{w1:#x}] vt={slots:?} port={port:#x} port_vt={port_vt:#x} pvt={port_slots:?}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }

    // r788b: cldp_datapool::u32dp_set element filter.  The dest-entry chain
    // writes control elements 0x8007025/0x8007023/0x801a0c3/0x801a0c4 (and
    // the mediator's 0x8076000 must move too); log every write to the
    // 0x8007xxxx/0x801a0xxx/0x807xxxxx bands with the caller chain to see
    // whether the tap actually moved any datapool state and who consumed it.
    {
        let address = base_address + 0x0138_7c30u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:dpset-filter", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            // dpset-cal calibration: r0=pool, r1=element, r2=value ptr, r3=size.
            // f56-f65 band bug: the dest-entry elements are 0x08xxxxxx
            // (domain byte 0x08 - the log's hex display hid the leading zero),
            // not 0x80xxxxxx.
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let band = (r1 & 0xff00_0000) == 0x0800_0000
                && matches!(r1 & 0x00f0_0000, 0x0000_0000 | 0x0010_0000 | 0x0070_0000);
            if !band {
                return 0;
            }
            if HITS.fetch_add(1, Ordering::Relaxed) >= 80 {
                return 0;
            }
            let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let mut bt = String::new();
            for i in 0..10u32 {
                let w = read_u32(uc, sp.wrapping_add(i * 4));
                if (0x00fb_7080..0x023b_eb1b).contains(&w) {
                    bt.push_str(&format!(" {w:#x}"));
                }
            }
            log::warn!(
                "PROCHMI dpset-filter t={}ms tid=[{}] elem={r1:#x} size={r3} v0={} lr={lr:#x} bt:{bt}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                read_u32(uc, r2) & 0xff,
            );
            0
        });
    }

    {
        let address = base_address + 0x0138_7c30u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:dpset-cal", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 12 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI dpset-cal t={}ms tid=[{}] r0={r0:#x} r1={r1:#x} r2={r2:#x} r3={r3:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }

    // r788b: Thumb entry of clHSI_CMNAVI::vDestEntryInitWithCountry - the
    // bExecuteMessage(0x2955) branch target that performs the four dp
    // element writes and posts 0x291f.  Confirms whether the HSI-side init
    // runs at all (its guard chain is obj[3]!=0 && obj[1]!=0).
    {
        let address = base_address + 0x0183_4c44u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Thumb, "libs/prochmi.rs:hsi-deinit", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            let n = HITS.fetch_add(1, Ordering::Relaxed);
            if n >= 20 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let port = read_u32(uc, r0.wrapping_add(0xc));
            let trace = read_u32(uc, r0.wrapping_add(4));
            let state = read_u32(uc, r0.wrapping_add(0x98));
            log::warn!(
                "PROCHMI hsi-deinit t={}ms tid=[{}] obj={r0:#x} port={port:#x} trace={trace:#x} state98={state:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }

    // r788b: census of the enavi required-service computation itself: if
    // bIsCCAServiceRequired never runs, nobody ever consults the USE_CCA
    // switches and the FI client list is never built; if it runs but all
    // return 0, the KDS tree lacks the keys.  Entry log per service id.
    {
        for (offset, name) in [
            (0x0161_d6dcu32, "enavi-req-ids"),
            (0x0169_e308u32, "enavi-req-check"),
        ] {
            let address = base_address + offset - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:enavi-gate", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 60 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI {name} t={}ms tid=[{}] r0={r0:#x} r1={r1:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
                0
            });
        }
    }

    // r788b: cldp_datapool::add_element - census of every datapool element
    // ever created.  The dest-entry writes die in cldp_Mngr::u32dp_set's
    // get_podp()==0 branch because control elements 0x8007025/0x8007023/
    // 0x801a0c3/0x801a0c4 and mediator element 0x8076000 were never added;
    // this census names the creator call sites that DO run so we can see
    // which provisioning/bridging step is missing.
    {
        let address = base_address + 0x0138_8180u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:dp-add", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            let n = HITS.fetch_add(1, Ordering::Relaxed);
            if n >= 1200 {
                return 0;
            }
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI dp-add t={}ms tid=[{}] #{n} elem={r1:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }

    // r788b: return sites of the three bIsCCAServiceRequired callers - the
    // bool in r0 tells whether the KDS cache hit or the default table was
    // used (svc 0x32 defaults to 0, so 0 here means "route-info optional,
    // registration legitimately skipped" vs a masked registry miss).
    {
        for (offset, name) in [
            (0x016b_8f0cu32, "enavi-req-ret-a"),
            (0x016b_8244u32, "enavi-req-ret-b"),
            (0x016c_cf90u32, "enavi-req-ret-c"),
        ] {
            let address = base_address + offset - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:enavi-gate-ret", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 40 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI {name} t={}ms tid=[{}] ret={r0:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
                0
            });
        }
    }

    // r788b: page-granular watch over the cldp_Mngr dp-access functions
    // (get_podp 0x1382a98, u32dp_set 0x1382cf0, its get_podp==0 branch
    // 0x1382d34): the narrow probes on these mid-TB addresses are exactly
    // the unicorn 2.1.5 false-negative class (AGENTS.md), and a zero-hit
    // dpset-miss proves nothing until re-confirmed with a page hook.  The
    // callback dispatches on the exact fetch address.
    {
        let page = base_address + 0x0138_2000u32 - ORIGINAL_BASE;
        let result = unicorn.add_code_hook(
            page as u64,
            page as u64 + 0x1000,
            move |uc: &mut Unicorn<'_, Context>, addr, _| {
                static GETPODP: AtomicU32 = AtomicU32::new(0);
                static SETIN: AtomicU32 = AtomicU32::new(0);
                static MISS: AtomicU32 = AtomicU32::new(0);
                static DUMPED: AtomicU32 = AtomicU32::new(0);
                let hit = addr as u32;
                let in_band = |e: u32| {
                    (e & 0xff00_0000) == 0x0800_0000
                        && matches!(e & 0x00f0_0000, 0x0000_0000 | 0x0010_0000 | 0x0070_0000)
                };
                if hit == page + 0x0a98u32 {
                    // get_podp(this r0, elem r1): one-shot mounted-domain
                    // census after boot, plus band-filtered lookups.
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    if crate::libs::dapi::emu_t0_elapsed_ms() > 2600
                        && DUMPED.swap(1, Ordering::Relaxed) == 0
                    {
                        let begin = read_u32(uc, r0.wrapping_add(0x20));
                        let end = read_u32(uc, r0.wrapping_add(0x24));
                        let cap = read_u32(uc, r0.wrapping_add(0x18));
                        log::warn!(
                            "PROCHMI dp-domains t={}ms mgr={r0:#x} begin={begin:#x} end={end:#x} size={} cap={cap:#x} slot[0x80]={:#x}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            (end.wrapping_sub(begin)) >> 2,
                            read_u32(uc, begin.wrapping_add(0x80 * 4)),
                        );
                    }
                    if in_band(r1) && GETPODP.fetch_add(1, Ordering::Relaxed) < 40 {
                        let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                        log::warn!(
                            "PROCHMI dp-getpodp t={}ms tid=[{}] elem={r1:#x} lr={lr:#x}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            uc.get_data().inner.thread_id(),
                        );
                    }
                } else if hit == page + 0x0cf0u32 {
                    // cldp_Mngr::u32dp_set entry.  f61/f62 band-filtered
                    // logging missed the tap calls; log every post-tap entry
                    // with r1 so even a wrong r1 hypothesis is testable.
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    if r1 == 0x1801_8000 {
                        return; // 50ms UI timer spam ate the whole budget (f64)
                    }
                    if crate::libs::dapi::emu_t0_elapsed_ms() > 30_000
                        && SETIN.fetch_add(1, Ordering::Relaxed) < 40
                    {
                        let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                        log::warn!(
                            "PROCHMI dp-setin t={}ms tid=[{}] elem={r1:#x} lr={lr:#x}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            uc.get_data().inner.thread_id(),
                        );
                    } else if in_band(r1) && SETIN.fetch_add(1, Ordering::Relaxed) < 40 {
                        let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                        log::warn!(
                            "PROCHMI dp-setin-band t={}ms tid=[{}] elem={r1:#x} lr={lr:#x}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            uc.get_data().inner.thread_id(),
                        );
                    }
                } else if hit == page + 0x0d34u32 {
                    // get_podp()==0 branch: elem lives in [sp, #12].
                    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    let elem = read_u32(uc, sp.wrapping_add(12));
                    if in_band(elem) && MISS.fetch_add(1, Ordering::Relaxed) < 40 {
                        let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                        log::warn!(
                            "PROCHMI dpset-miss t={}ms tid=[{}] elem={elem:#x} lr={lr:#x}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            uc.get_data().inner.thread_id(),
                        );
                    }
                }
            },
        );
        log::info!(
            "PROCHMI dp page-watch installed at {:#x}: {}",
            page,
            result.is_ok()
        );
    }

    // r788b: micro-step trace inside clHSI_CMNAVI::vDestEntryInitWithCountry
    // after the tap: page-watch on 0x1834000 logging every fetch in the
    // function body window (t>30s only).  This settles whether the four
    // dp-write call sites execute or whether the branch skips them (Ghidra
    // decompile claims they precede the 0x291f self-post, yet the page hook
    // at 0x1382cf0 stays silent).
    {
        let page = base_address + 0x0183_4000u32 - ORIGINAL_BASE;
        let result = unicorn.add_code_hook(
            page as u64,
            page as u64 + 0x1000,
            move |uc: &mut Unicorn<'_, Context>, addr, _| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                let hit = addr as u32;
                if hit < page + 0x0c44u32 || hit > page + 0x0e20u32 {
                    return;
                }
                if crate::libs::dapi::emu_t0_elapsed_ms() < 30_000 {
                    return;
                }
                if HITS.fetch_add(1, Ordering::Relaxed) >= 160 {
                    return;
                }
                let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                // The write's indirect target is resolved from the manager's
                // live vptr: dump vt and slot +0x1c at every step to catch a
                // runtime vtable swap.
                let vt = read_u32(uc, 0x0038_eee88);
                let slot = read_u32(uc, vt.wrapping_add(0x1c));
                log::warn!(
                    "PROCHMI hsi-step t={}ms {hit:#x} r3={r3:#x} mgrvt={vt:#x} s1c={slot:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                );
            },
        );
        log::info!(
            "PROCHMI hsi-step trace installed at {:#x}: {}",
            page,
            result.is_ok()
        );
    }

    // r788b: cldp_Mngr::bConfigure(cfg,count,trace,adapter) builds the
    // mounted-domain vector - one cldp_datapool per 12-byte tagrdp_cfg entry
    // {u16 domainId; u16; u32 elemBase; u8}, and get_podp() indexes that
    // vector by elem>>24, so an element's domain must be <= the boot count.
    // Dump the whole list (f59: count=52, cap=0x34 - domain 0x80 of the
    // dest-entry elements cannot exist) plus the caller chain: whoever
    // assembles this list decides which FI-service domains get mounted.
    {
        let address = base_address + 0x0138_2eccu32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:dp-configure", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 4 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let mut bt = String::new();
            for i in 0..12u32 {
                let w = read_u32(uc, sp.wrapping_add(i * 4));
                if (0x00fb_7080..0x023b_eb1b).contains(&w) {
                    bt.push_str(&format!(" {w:#x}"));
                }
            }
            let count = r2.min(255);
            let mut doms = String::new();
            for i in 0..count {
                let e = r1.wrapping_add(i * 12);
                let d = read_u32(uc, e) & 0xffff;
                let base = read_u32(uc, e.wrapping_add(4));
                doms.push_str(&format!(" {d:#x}:{base:#x}"));
            }
            log::warn!(
                "PROCHMI dp-configure t={}ms tid=[{}] mgr={r0:#x} cfg={r1:#x} count={r2} lr={lr:#x} bt:{bt} domains:{doms}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }

    // r788b: clHSIMngr::bEvent2App decision site (0x1352930: cmp r4,#0 after
    // clFiFactory::GetFi).  Event 0x81008089 (dest-entry init) extracts
    // fi-index 0x8; if the factory has no FI instance 8 the router silently
    // drops the event (beq -> trace "no fi" -> return 1), which would end
    // the keyboard-page chain right after the (now working) dp writes.
    {
        let address = base_address + 0x0135_2930u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:event2app-decision", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 60 {
                return 0;
            }
            let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI event2app t={}ms tid=[{}] event={r7:#x} fiobj={r4:#x} {} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                if r4 == 0 { "DROP(no fi)" } else { "dispatch" },
            );
            0
        });
    }

    {
        let address = base_address + 0x0135_2944u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:event2app-target", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 60 {
                return 0;
            }
            let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
            // probe fires before `ldr r3,[r4]` executes - load vptr ourselves
            let vptr = read_u32(uc, r4);
            let target = read_u32(uc, vptr.wrapping_add(0x3c));
            log::warn!(
                "PROCHMI event2app-go t={}ms tid=[{}] event={r7:#x} fi={r4:#x} vptr={vptr:#x} vfn3c={target:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }

    // r788b: clFi_Navi::ReceiveEventsFromEvtEng entry (0x14bcd74).  Its
    // guard is [fi+0x2c]!=0 && [fi+0x30]!=0 && [fi+8]!=0; if the hminavserver
    // FI app never got past its own bring-up the dest-entry event 0x81008089
    // (dispatched at f67 t=33019 to fi=0x38f9970 vfn+0x3c) dies here.
    // Dump the three guard fields plus the post-guard path decision.
    {
        let address = base_address + 0x014b_cd74u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:fi-recv-events", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 60 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let f8 = read_u32(uc, r0.wrapping_add(8));
            let f2c = read_u32(uc, r0.wrapping_add(0x2c));
            let f30 = read_u32(uc, r0.wrapping_add(0x30));
            log::warn!(
                "PROCHMI fi-recv t={}ms tid=[{}] fi={r0:#x} event={r1:#x} trace={f8:#x} f2c={f2c:#x} f30={f30:#x} {}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                if f8 == 0 || f2c == 0 || f30 == 0 { "GUARD-FAIL" } else { "pass" },
            );
            0
        });
    }

    // r788b: page-watches over the ReceiveEventsFromEvtEng slot-table jump
    // (slot for 0x81008089 = 0x14bcf60) and the GCC shared case tail on page
    // 0x14c2000 (body 0x14c2b2c, tail bl 0x138bbd0 @0x14c2b68): the narrow
    // probes there are false-negative suspects (f68: fi-recv guard passed,
    // case probes zero - mid-TB legacy hooks, AGENTS.md).
    {
        let slot_page = base_address + 0x014b_c000u32 - ORIGINAL_BASE;
        let slot_target = base_address + 0x014b_cf60u32 - ORIGINAL_BASE;
        let body_page = base_address + 0x014c_2000u32 - ORIGINAL_BASE;
        for (page, span, lo, hi, tag) in [
            (slot_page, 0x1000u32, slot_target, slot_target + 4u32, "slot89"),
            (body_page, 0x1000u32, base_address + 0x014c_2b0cu32 - ORIGINAL_BASE, base_address + 0x014c_2b84u32 - ORIGINAL_BASE, "case89"),
        ] {
            let result = unicorn.add_code_hook(
                page as u64,
                page as u64 + span as u64,
                move |uc: &mut Unicorn<'_, Context>, addr, _| {
                    static HITS: AtomicU32 = AtomicU32::new(0);
                    let hit = addr as u32;
                    if hit < lo || hit >= hi {
                        return;
                    }
                    if HITS.fetch_add(1, Ordering::Relaxed) >= 80 {
                        return;
                    }
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    // for the slot page: dump the runtime instruction word and
                    // decode the ARM `b` target (0xea......) to verify the
                    // static-vs-runtime jump table agree.
                    let word = read_u32(uc, hit);
                    let tgt = if word >> 26 == 0x3a || word >> 26 == 0x2a || word >> 26 == 0x3b || word >> 26 == 0x39 || word >> 26 == 0x12 {
                        let imm = (word & 0x00ff_ffff) << 2;
                        let imm = if word & 0x0080_0000 != 0 { imm | !0x03ff_ffff } else { imm & 0x03ff_ffff };
                        hit.wrapping_add(8).wrapping_add(imm)
                    } else {
                        0
                    };
                    log::warn!(
                        "PROCHMI {tag} t={}ms {hit:#x} r0={r0:#x} word={word:#x} b->{tgt:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                    );
                },
            );
            log::info!(
                "PROCHMI fi {tag} page-watch {:#x}: {}",
                page,
                result.is_ok()
            );
        }
    }

    // r788b: clHmiNavServerHandler::vInitDestinationEntry(mode) entry - the
    // terminal handler of dest-entry init (event 0x81008089 -> case 6).
    // Silent-return guards: [this+0x160] trace and [this+0x7e8]
    // enavi_tclILocationInput proxy.  Dump both plus the speller char-mode
    // byte [+0x75], the dataset-index [+0x4c] and the state field [+0x168].
    {
        let address = base_address + 0x015a_7228u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:deinit-entry", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 20 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let f160 = read_u32(uc, r0.wrapping_add(0x160));
            let f7e8 = read_u32(uc, r0.wrapping_add(0x7e8));
            let f75 = read_u32(uc, r0.wrapping_add(0x75)) & 0xff;
            let f4c = read_u32(uc, r0.wrapping_add(0x4c)) & 0xff;
            let f168 = read_u32(uc, r0.wrapping_add(0x168));
            log::warn!(
                "PROCHMI deinit-entry t={}ms tid=[{}] hnd={r0:#x} mode={r1} trace={f160:#x} locinput={f7e8:#x} li_mode={f75} ds={f4c} state168={f168} {}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                if f160 == 0 || f7e8 == 0 { "GUARD-FAIL" } else { "pass" },
            );
            0
        });
    }

    // r788b: enavi LI request layer census.  After vInitDestinationEntry
    // passes, the keyboard page needs (a) the NAVI-server service-ready
    // verdict (bIsNavServerServiceReady - the same gate as the unit's popup)
    // and (b) actual CL requests out to the loc-input server
    // (vEnableElementListRequest 4-arg + 3-arg, bRequestInit).  Entry logs
    // show which of them HMI even attempts after the tap.
    {
        for (offset, name) in [
            (0x0162_f570u32, "li-svcready"),
            (0x0163_0480u32, "li-elemlist4"),
            (0x0163_6b54u32, "li-elemlist3"),
            (0x0163_3e48u32, "li-reqinit"),
            (0x0157_8d88u32, "hnd-li-elemlist"),
        ] {
            let address = base_address + offset - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:li-req", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 40 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI {name} t={}ms tid=[{}] this={r0:#x} a1={r1:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
                0
            });
        }
    }

    // r788b: the enavi request engine.  vEnableElementListRequest only sets
    // the enable flag; the actual CL request runs inside enavi_tclEnaviThread
    // processes (enavi_tclProcLIRefineLocationDescription).  Census: is the
    // enavi thread's pump alive (bBlockWhileProcessing), and what server
    // state transitions happen (vSetEnaviServerState / vOnUpdate... - the
    // unit's startup popup tracks exactly this state machine).
    {
        for (offset, name) in [
            (0x017b_e4e8u32, "enavi-pump"),
            (0x017b_e7d0u32, "enavi-setstate"),
            (0x0161_d3d4u32, "enavi-updstate"),
            (0x0161_ce68u32, "enavi-getstate"),
            (0x0161_d918u32, "enavi-chgstate"),
        ] {
            let address = base_address + offset - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:enavi-engine", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 50 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let tid = uc.get_data().inner.thread_id().to_string();
                log::warn!(
                    "PROCHMI {name} t={}ms tid=[{tid}] this={r0:#x} a1={r1:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                );
                0
            });
        }
    }

    // r788b: enavi_tclEnaviThread::vEntry (the HMINAVSRV OSAL thread body,
    // created t=3.1s but gone from the scheduler within milliseconds - tid 53
    // exited(0)).  vEntry runs its pump loop only while [this+0x64]==0; log
    // the byte at entry and MEM_WATCH the exact object byte (thread object
    // 0x3a31ab0 is heap-deterministic) to catch who pre-arms the shutdown
    // flag in the serialized emulator (real unit's thread races ahead of the
    // flag setter).
    {
        let address = base_address + 0x017b_ebecu32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:enavi-ventry", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 10 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let f64 = read_u32(uc, r0.wrapping_add(0x64)) & 0xff;
            log::warn!(
                "PROCHMI enavi-ventry t={}ms tid=[{}] this={r0:#x} flag64={f64}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    {
        let result = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            0x03a3_1b14u64,
            0x03a3_1b17u64,
            move |_uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) < 40 {
                    log::warn!(
                        "PROCHMI enavi-flag64 WRITE t={}ms addr={address:#x} size={size} value={value:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                    );
                }
                true
            },
        );
        log::info!(
            "PROCHMI enavi-flag64 write-watch installed: {}",
            result.is_ok()
        );
    }

    // r789b: FI-8 bring-up census.  The framework hands a CCA register-id to
    // an FI proxy (clFi_Navi::vSetRegisterID, navifi-setregid probe, zero
    // hits) only if the FI is registered with the CCA app layer -
    // hmicca_tclApp::bRegisterForFID(fiId) is that registration, and
    // clFiFactory::CreateFi(fiId) is the creation.  Log both id censuses:
    // if 8 is created but never registered-for-FID, the framework config
    // that enumerates FIs is the missing precondition (registry-provisioned,
    // fixable in data/registry_compat.reg).
    {
        for (offset, name) in [
            (0x00fc_92b8u32, "reg-for-fid"),
            (0x013d_15e4u32, "create-fi"),
        ] {
            let address = base_address + offset - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:fi-census", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 60 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI fi-census {name} t={}ms tid=[{}] r0={r0:#x} r1={r1:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
                0
            });
        }
    }
    {
        // bDefaultSvcDataHandler matched-entry dispatch (blx ip at
        // 0x1e82440 preceded by the pFindMsgEntry result test): the gw feeds
        // svc 0x27 ServiceData(0x45) to a clDispSet_CCAhandler work object
        // (0x38feab0, f78 gw-deliver); this site says whether its msg map
        // (0x521d610) has a 0x27 handler and what the handler address is.
        let address = base_address + 0x01e8_2418u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:svcmgr-dispatch", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 40 {
                return 0;
            }
            let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
            let entry = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            // svcData wrapper r6 -> body -> svc u16 @ +0x18
            let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
            let body = read_u32(uc, r6.wrapping_add(4));
            let svc = (read_u32(uc, body.wrapping_add(0x18)) & 0xffff) as u16;
            let (handler, off) = if entry != 0 {
                (read_u32(uc, entry), read_u32(uc, entry.wrapping_add(4)))
            } else {
                (0, 0)
            };
            log::warn!(
                "PROCHMI svcmgr-dispatch t={}ms tid=[{}] work={r5:#x} svc={svc:#06x} entry={entry:#x} handler={handler:#x} off={off:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }

    // r789c: STARTUP case-4/5 chain instrumentation.  f77 shows state 4 at
    // t=20.1 s and a retry to state 1 at t=35.6 s; the navif/routecca probes
    // cannot tell whether bStartupNavIF/bNavIFCreate ran at all (enActivate
    // is downstream of them).  Log the state machine entry state, the
    // bStartupNavIF result (beq site after bl 0x17be644), bNavIFCreate entry,
    // the NAVIF_APPID registry read and the enNavIFCreate result.
    {
        let address = base_address + 0x017c_73c0u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:startup-step", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 20_000 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            // The FSM state word is at +0x30 (switched by `cmp r3,[obj+0x30];
            // addls pc,pc,r3<<2` at 0x17c73dc).  +0xc is a self back-pointer,
            // which is why earlier runs printed a constant pointer value.
            let state = read_u32(uc, r0.wrapping_add(0x30));
            // r797b: state 6 opened (f217) - the 30-hit budget hid everything
            // past it; log only state changes now.
            static LAST: AtomicU32 = AtomicU32::new(0xffff_ffff);
            if LAST.swap(state, Ordering::Relaxed) == state {
                return 0;
            }
            // r798b: publish the FSM state for the mbx_7 sensorfi responder
            // (message.rs): its regid-3 svc-6 answers belong to the state-8
            // readiness entry, and answering the same request during the
            // member era collides with the members' pending transactions.
            crate::libs::libosal_linux::message::note_hmi_fsm_state(state);
            log::warn!(
                "PROCHMI startup-step t={}ms tid=[{}] proc={r0:#x} state={state}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    for (offset, label) in [
        (0x017c_75a8u32, "startup-navif-result"),
        (0x017c_1e80u32, "navif-create"),
        (0x017c_1ef0u32, "navif-appid"),
        (0x017c_1f1cu32, "ennavif-create-result"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:navif-chain", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 20 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI navif-chain {label} t={}ms tid=[{}] r0={r0:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r795: does the guest ever execute address 0?  `bl 0xfb65b4`'s vector
    // slot reads 0 in the probe; if the fetch to 0 lands in a mapped (zero)
    // page, these andeq-NOPs run.  Page-range hook (reliable per AGENTS.md),
    // fires only if page 0 is actually fetched.
    {
        let lo = 0u32;
        let hi = 0x1000u32;
        unicorn
            .add_code_hook(lo as u64, hi as u64, move |uc: &mut Unicorn<'_, Context>, address: u64, _: u32| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 5 {
                    log::error!(
                        "PROCHMI page0-exec t={}ms tid=[{}] addr={address:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                }
            })
            .unwrap();
    }
    // r795: f151 shows bNavIFCreate's `bl enNavIFCreate` returning 0, so the
    // navif object never exists, enActivate never runs, no CCA route feeds
    // the service table, case 5 never passes and the keyboard page (via the
    // LI mediator) is dead.  Map which of enNavIFCreate's branches fails.
    // Static layout (0x1c7b4bc, ARM): singleton {+0 navif, +4 handler,
    // +8 handler2} at anchor 0x2a12818 + 0x2b1c08 = 0x2cc4420; args on the
    // stack: +0x20 u16 msg-type (must equal 0x80010001 half-pair), +0x24
    // extra, +0x28 appid.  Decision points probed below.
    {
        let singleton = base_address + 0x02cc_4420u32 - ORIGINAL_BASE;
        for (offset, label) in [
            (0x01c7_b4bcu32, "c1-entry"),
            (0x01c7_b4f8u32, "c2-handler2-alive"),
            (0x01c7_b4e4u32, "c3-no-handler2"),
            (0x01c7_b47cu32, "c4-appid-check-res"),
            (0x01c7_b584u32, "c5-msgtype-test"),
            (0x01c7_b5ecu32, "c6-navif-ctor-res"),
            (0x01c7_b558u32, "c7-activate-res"),
            (0x01c7_b50cu32, "c8-ret"),
            (0x01c7_f6d0u32, "d0-entry"),
            (0x01c7_f704u32, "d1-fail-path"),
            (0x01c7_f6ecu32, "d2-x3c-path"),
            (0x01c7_f700u32, "d3-x3c-check"),
            (0x01c7_f730u32, "d4-x34-res"),
            (0x01c7_f758u32, "d5-mutex-res"),
            // Failure tail of `0x1c7f79c`'s caller (the CCA subscriber
            // bring-up that produced c7-activate-res): every exit site.
            (0x01c7_f500u32, "e0-entry"),
            (0x01c7_f548u32, "e1-x34-res"),
            (0x01c7_f5b4u32, "e2"),
            (0x01c7_f5e8u32, "e3"),
            (0x01c7_f5fcu32, "e4"),
            (0x01c7_f610u32, "e5"),
            (0x01c7_f650u32, "e6"),
            // `0x1c7f670` short-circuits to the success id 0x80010001 when
            // this OSAL vector veneer (ldr pc,[0x1b0125bc+0x328]) returns
            // non-zero; f155 shows it returning 0, forcing the long init
            // path that ends failing in enActivate.  Identify the target
            // (offset into libosal) to learn what it actually checks.
            (0x00fb_65b4u32, "c9-osal-vec"),
            // Return site of `bl 0xfb65b4` in `0x1c7f670`: proves whether the
            // veneer came back and with what r0 (the whole long-path failure
            // hinges on this predicate).
            (0x01c7_f6a0u32, "c10-vec-ret"),
        ] {
            let address = base_address + offset - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:navif-deep", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 30 {
                    return 0;
                }
                let r = |reg| uc.reg_read(reg).unwrap_or(0) as u32;
                let (r0, r4, sp) = (
                    r(RegisterARM::R0),
                    r(RegisterARM::R4),
                    r(RegisterARM::R13),
                );
                // Every d*/e* probe also dumps the queue the probed body is
                // about to post to (its msg-ref header, pinned live at r795:
                // u32@+0x4 is the target-queue name's NUL-terminated pointer)
                // and the OSAL queue handle it uses (+0x14): the CCA bring-up
                // request stalls for ~6.5 s and returns 0 - find out where it
                // is sent and which answer it waits for.
                if matches!(
                    label,
                    "d4-x34-res" | "d5-mutex-res" | "e0-entry" | "e1-x34-res"
                ) {
                    let obj = r(RegisterARM::R6);
                    let ptr34 = read_u32(uc, obj.wrapping_add(0x34));
                    let vt = read_u32(uc, ptr34);
                    log::warn!(
                        "PROCHMI navif-deep {label}-vfn t={}ms obj={obj:#x} +0x34={ptr34:#x} vt={vt:#x} vfn0={:#x} vfn8={:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        read_u32(uc, vt),
                        read_u32(uc, vt.wrapping_add(8)),
                    );
                    let m28 = read_u32(uc, obj.wrapping_add(0x28));
                    let name = read_cstr(uc, m28, 12);
                    let m14 = read_u32(uc, obj.wrapping_add(0x14));
                    log::warn!(
                        "PROCHMI navif-deep {label}-msg t={}ms tid=[{}] obj={obj:#x} +0x28={m28:#x} name={name:?} +0x14={m14:#x} +0x40={:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        read_u32(uc, obj.wrapping_add(0x40)),
                    );
                }
                let sl = r(RegisterARM::R10);
                let words = match label {
                    "c1-entry" => {
                        let msgtype = (read_u32(uc, sp.wrapping_add(0x20)) & 0xffff) as u32;
                        let extra = read_u32(uc, sp.wrapping_add(0x24));
                        let appid = read_u32(uc, sp.wrapping_add(0x28));
                        format!(
                            " r0={r0:#x} msgtype={msgtype:#010x} extra={extra:#x} appid={appid:#x} sing=[{:#x},{:#x},{:#x}]",
                            read_u32(uc, singleton),
                            read_u32(uc, singleton + 4),
                            read_u32(uc, singleton + 8),
                        )
                    }
                    "c6-navif-ctor-res" => format!(
                        " r0={r0:#x} navif@{r4:#x}=[{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x}]",
                        read_u32(uc, r4),
                        read_u32(uc, r4 + 4),
                        read_u32(uc, r4 + 8),
                        read_u32(uc, r4 + 0xc),
                        read_u32(uc, r4 + 0x10),
                        read_u32(uc, r4 + 0x14),
                        read_u32(uc, r4.wrapping_add(0x4).wrapping_add(0x10)),
                    ),
                    "c5-msgtype-test" => {
                        format!(" sl={sl:#x} r3={:#x}", r(RegisterARM::R3))
                    }
                    "c9-osal-vec" => {
                        let base = 0x02a1_25bcu32;
                        let mut b4 = [0u8; 4];
                        let mapped = uc.mem_read((base + 0x328) as u64, &mut b4).is_ok();
                        format!(
                            " entry={:#x} mapped={mapped} prev={:#x} next={:#x}",
                            read_u32(uc, base + 0x328),
                            read_u32(uc, base + 0x320),
                            read_u32(uc, base + 0x330),
                        )
                    }
                    "c10-vec-ret" => {
                        // r795: slot computation fixed - `add ip,pc,#imm`
                        // imm is ROR (2*rot), so base = pc + 0x1a00000 +
                        // 0x5c000 lands in the app's own .bss (0x2a125xx,
                        // next to the 0x2a12818 navif singleton anchor),
                        // *not* a fixed-mapped 0x1b012xxx page.  Dump the
                        // live table window to see what filled each slot.
                        let lo = 0x02a1_28d0u32 + base_address - ORIGINAL_BASE;
                        let words: Vec<String> =
                            (0..8).map(|i| format!("{:#x}", read_u32(uc, lo + 4 * i))).collect();
                        format!(
                            " r0={r0:#x} table[{:#x}..]=({})",
                            lo,
                            words.join(" ")
                        )
                    }
                    "d0-entry" | "d1-fail-path" | "d2-x3c-path" | "d3-x3c-check"
                    | "d4-x34-res" | "d5-mutex-res" => {
                        let obj = r(RegisterARM::R6);
                        format!(
                            " r0={r0:#x} obj={obj:#x} +0x28={:#x} +0x34={:#x} +0x3c={:#x} +0x45={}",
                            read_u32(uc, obj.wrapping_add(0x28)),
                            read_u32(uc, obj.wrapping_add(0x34)),
                            read_u32(uc, obj.wrapping_add(0x3c)),
                            read_u32(uc, obj.wrapping_add(0x45)) & 0xff,
                        )
                    }
                    _ => format!(" r0={r0:#x} sl={sl:#x}"),
                };
                log::warn!(
                    "PROCHMI navif-deep {label} t={}ms tid=[{}]{}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    words,
                );
                0
            });
        }
    }
    // r790b: after the pool fix bNavIFCreate succeeds but bStartupNavIF still
    // fails - the CCACommandSubscriber construction path is the suspect.
    // Log enActivate args + navif fields, the subscriber ctor args, and every
    // poNavIFGetControl request.
    for (offset, label) in [
        (0x017b_e644u32, "startup-navif-entry"),
        (0x017c_1bf8u32, "navif-activate"),
        (0x017b_9954u32, "cmdsub-ctor"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:navif-subs", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 20 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            let (obj, f8, fc) = if label == "cmdsub-ctor" {
                (r1, read_u32(uc, r1.wrapping_add(8)), read_u32(uc, r1.wrapping_add(0xc)))
            } else {
                (r0, read_u32(uc, r0.wrapping_add(8)), read_u32(uc, r0.wrapping_add(0xc)))
            };
            log::warn!(
                "PROCHMI navif-subs {label} t={}ms tid=[{}] r0={r0:#x} r1={r1:#x} obj={obj:#x} +8={f8:#x} +0xc={fc:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    {
        // enavi_tclNavIF::poNavIFGetControl: r0=navif, r1=control type.
        let address = base_address + 0x017c_1ce0u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:getcontrol", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 30 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            let f8 = read_u32(uc, r0.wrapping_add(8));
            log::warn!(
                "PROCHMI getcontrol t={}ms tid=[{}] navif={r0:#x} type={r1:#x} +8={f8:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r790c: CmdSubscriber::C1 decision points (0x17b9714): the control
    // attach vfn+0xc (result site 0x17b980c), the bIsCCAServiceRequired
    // filter (0x169e308, result site 0x17b9840, loop index over the 16-entry
    // table {0a,2c,1f,1e,18,0d,06,152,3c,07,37,32,300,56,7e,7d}) and the
    // per-svc registration vfn+0x14 (result site 0x17b9878).
    {
        let address = base_address + 0x017b_97c8u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:cmdsub-step", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 10 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let vt = read_u32(uc, r0);
            log::warn!(
                "PROCHMI cmdsub-step t={}ms tid=[{}] control={r0:#x} vt={vt:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r790g: write-watch on the scd dynamic-mode flag (0x02ab44b9): log every
    // guest store to it with the program counter, to learn who turns the
    // client's SCD lookup into queue mode (which the monitor then refuses).
    {
        let watch_res_scdflag = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            (base_address + 0x02ab_44b9u32 - ORIGINAL_BASE) as u64,
            (base_address + 0x02ab_44b9u32 - ORIGINAL_BASE) as u64 + 3,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) < 10 {
                    let pc = uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32;
                    let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                    log::warn!(
                        "PROCHMI scd-flag WRITE t={}ms tid=[{}] addr={address:#x} size={size} value={value:#x} pc={pc:#x} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                }
                true
            },
        );
        log::info!(
            "PROCHMI scd-flag write-watch installed: {}",
            watch_res_scdflag.is_ok()
        );
    }
    // r791p: write-watch on the dangling media-action object and the IDAPI
    // vector's element slot, from t=0, to learn who first installs the
    // action's vtable word and who later overwrites it.
    for (addr, what) in [
        (0x3a2a990u32, "idapi+18"),
        (0x3a2a994u32, "idapi+1c"),
        (0x907bf020u32, "action+00"),
        (0x907bf100u32, "vec[0]"),
    ] {
        let res = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            addr as u64,
            (addr + 3) as u64,
            Box::new(move |uc: &mut Unicorn<'_, Context>, access, address, size, value| {
                let _ = access;
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 80 {
                    return true;
                }
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                let pc = uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI actionmem WRITE {what} t={}ms tid=[{tid}] addr={address:#x} size={size} value={value:#x} pc={pc:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    tid = uc.get_data().inner.thread_id(),
                );
                true
            }),
        );
        let _ = (res, what);
    }
    // r791o: who writes the enavi server's IDAPI slot and the IDAPI
    // action-vector fields.  The singleton addresses are stable across runs
    // (server 0x3a23eb8, IDAPI 0x3a2a978 in r791n runs); watch the three
    // fields the svc-6 media branch reads.
    for (addr, what) in [
        (0x3a23edcu32, "server+24"),
        (0x3a2a990u32, "idapi+18"),
        (0x3a2a994u32, "idapi+1c"),
    ] {
        let res = unicorn.add_mem_hook(
            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
            addr as u64,
            (addr + 3) as u64,
            Box::new(move |uc: &mut Unicorn<'_, Context>, access, address, size, value| {
                let _ = access;
                if crate::libs::dapi::emu_t0_elapsed_ms() < 10_000 {
                    return true;
                }
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                let pc = uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI mediafield WRITE {what} t={}ms tid=[{tid}] addr={address:#x} size={size} value={value:#x} pc={pc:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    tid = uc.get_data().inner.thread_id(),
                );
                true
            }),
        );
        let _ = (res, what);
    }
    // r791r: the media chain continues into MarkUnknown's tail helper
    // 0x1614f14 and then enavi_tclRegistry::vSetMediaDescription, whose
    // string argument carried a bad vtable (READ_UNMAPPED at 0x169dcf8,
    // run f119).  Log both entries to identify the broken object.
    {
        let address = base_address + 0x01614f14u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:media-tail", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 15_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 4
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let vt = read_u32(uc, r0);
            log::warn!(
                "PROCHMI media-tail t={}ms tid=[{}] r0={r0:#x} vt0={vt:#x} r1={r1:#x} r2={r2:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    {
        let address = base_address + 0x0169dbf4u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:setmediadesc", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 15_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 4
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let vt = read_u32(uc, r1);
            let s = read_u32(uc, r1.wrapping_add(4));
            log::warn!(
                "PROCHMI setmediadesc t={}ms tid=[{}] idx={r0:#x} str={r1:#x} vt={vt:#x} chars={s:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r792: clFi_Navi::ReceiveEventsFromEvtEngine indirect dispatch (0x14bcdf4
    // ldrcc pc,[..]).  Page-range hook (r792 lesson: the narrow legacy hook
    // on the un-relocatable ldrcc pc never fired).  Logs the resolved table
    // entry per FI event so we can tell which handler consumed the
    // 0x81008115 event that swapped the "Starting navigation" popup (0x5da)
    // for the "No SD card" popup (0x5e2).
    {
        let page = (base_address + 0x014b_c000u32 - ORIGINAL_BASE) as u64;
        let site = (base_address + 0x014b_cdf4u32 - ORIGINAL_BASE) as u64;
        let base = base_address;
        let result = unicorn.add_code_hook(page, page + 0x1000, move |uc: &mut Unicorn<'_, Context>, addr, _| {
            if addr != site {
                return;
            }
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 20_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 40
            {
                return;
            }
            let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let slot = base + 0x014b_cdfcu32 - ORIGINAL_BASE + r3.wrapping_mul(4);
            let w = {
                let mut b = [0u8; 4];
                if uc.mem_read(slot as u64, &mut b).is_ok() {
                    u32::from_le_bytes(b)
                } else {
                    0
                }
            };
            log::warn!(
                "PROCHMI fidispatch t={}ms tid=[{}] ev={r1:#x} idx={r3:#x} slot={:#x} word={w:#x} btarget={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                slot,
                if w >> 26 == 0b1010 {
                    slot.wrapping_add(8).wrapping_add(((w & 0x03ff_ffff) << 2) as u32)
                } else {
                    0
                },
            );
        });
        match result {
            Ok(_) => log::info!("PROCHMI: fidispatch page-watch installed"),
            Err(e) => log::warn!("PROCHMI: fidispatch page-hook failed: {e}"),
        }
    }

    // r791t: media-availability decision fields.  Both deciders read only
    // this+{0x80,0x88,0x98,0x9c}, so the entry dump plus the decompiled
    // switch fully determines the returned state (3 = ok, 4 = unknown,
    // 5 = not-inserted-ish, 0/1/2/6 = other).  Distinguishes the fabricated
    // cryptcard from the guest's real media action per evaluation.
    for (offset, label) in [
        (0x0171_fd7cu32, "chkmedium"),
        (0x0171_fcf4u32, "medavail"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:mediadec", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let f80 = read_u32(uc, r0.wrapping_add(0x80));
            let f88 = read_u32(uc, r0.wrapping_add(0x88));
            let b98 = read_u32(uc, r0.wrapping_add(0x98));
            let b9c = read_u32(uc, r0.wrapping_add(0x9c));
            // r799e (moved here from the 0x1614dac wrapper in f302: the
            // state-8 legs call the deciders directly): the enavi
            // media-device entry carries +0x80 = +0x88 = 0 and an EMPTY
            // record vector, because on the real unit those fields are set
            // by the TE-side media-state answers the emulator has no
            // source for.  Give the entry the READY shape the fabricated
            // cryptcard carries (and the popup-release leg accepts):
            // +0x80 = 2, +0x88 = 3 (state switch case 3 -> READY record
            // scan -> 2), one 0x18 record (+4 nonzero, +0x0c = 2).  Same
            // provider-signal class as fake_cryptcard_into_idapi;
            // EMU_FAKE_ENAVI_MEDIA=0 disables, state-8 gate as for the
            // SENSORFI responder (early READY ran the release branch
            // against a half-initialised app - f300).
            {
                let enabled =
                    std::env::var_os("EMU_FAKE_ENAVI_MEDIA").map_or(true, |v| v != "0");
                let fsm = crate::libs::libosal_linux::message::hmi_fsm_state();
                if enabled && r0 > 0x8000 && r0 < 0x4000_0000 && (8..0xffff_ffff).contains(&fsm)
                    && f80 <= 2 && f88 == 0 && b98 == b9c
                {
                    static REC: AtomicU32 = AtomicU32::new(0);
                    let mut rec = REC.load(Ordering::Relaxed);
                    if rec == 0 {
                        let mmu_arc = uc.get_data().mmu.clone();
                        rec = mmu_arc.lock().unwrap().heap_alloc(
                            uc,
                            0x100,
                            Prot::READ | Prot::WRITE,
                            "[prochmi-enavi-media-fab]",
                        );
                        REC.store(rec, Ordering::Relaxed);
                        if rec != 0 {
                            let words = [r0, 1, 0, 2, 0, 0];
                            for (i, w) in words.iter().enumerate() {
                                let _ = write_u32(uc, rec + (i * 4) as u32, *w);
                            }
                        }
                    }
                    if rec != 0 {
                        let ok = write_u32(uc, r0 + 0x80, 2)
                            && write_u32(uc, r0 + 0x88, 3)
                            && write_u32(uc, r0 + 0x98, rec)
                            && write_u32(uc, r0 + 0x9c, rec + 0x18);
                        static ONCE: AtomicU32 = AtomicU32::new(0);
                        if ok && ONCE.fetch_add(1, Ordering::Relaxed) < 3 {
                            log::warn!(
                                "PROCHMI enavi-media-fab t={}ms: media READY shape (+0x80=2 +0x88=3, record {rec:#x}) on device entry {r0:#x} at {label} (media-state provider signal; EMU_FAKE_ENAVI_MEDIA=0 disables)",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                            );
                        }
                        return 0;
                    }
                }
            }
            if crate::libs::dapi::emu_t0_elapsed_ms() < 15_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 30
            {
                return 0;
            }
            log::warn!(
                "PROCHMI {label} t={}ms tid=[{}] act={r0:#x} +80={f80:#x} +88={f88:#x} recs={b98:#x}..{b9c:#x} n={}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                b9c.wrapping_sub(b98) / 24,
            );
            0
        });
    }
    // r791n: the svc-6 media-state branch that killed the SM pump with a
    // null vtable slot.  Log the enavi_tclIDAPI object at the virtual entry
    // and the action object + vtable slot +0x38 inside the action method.
    {
        let address = base_address + 0x01615ea0u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:idapi-unk", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 4 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let vt = read_u32(uc, r0);
            let f18 = read_u32(uc, r0.wrapping_add(0x18));
            let f1c = read_u32(uc, r0.wrapping_add(0x1c));
            let e0 = read_u32(uc, f18);
            log::warn!(
                "PROCHMI idapi-unk t={}ms tid=[{}] idapi={r0:#x} vt={vt:#x} begin={f18:#x} end={f1c:#x} elem0={e0:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    {
        let address = base_address + 0x01720458u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:media-unk", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 4 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let vt = read_u32(uc, r0);
            let s38 = read_u32(uc, vt.wrapping_add(0x38));
            let s34 = read_u32(uc, vt.wrapping_add(0x34));
            log::warn!(
                "PROCHMI media-unk t={}ms tid=[{}] action={r0:#x} vt={vt:#x} vfn34={s34:#x} vfn38={s38:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r791m: the bRunWhenRunning service-availability chain outcomes.  The
    // chain (0x17c7604 case body) checks bIsServiceAvailable for
    // {0x0a,0x2c,0x1f,0x1e,0x06,0x300,0x3c,0x37,0x56}; an unavailable
    // *required* service exits at 0x17c745c, 0x56 exits at 0x17c7cec, and
    // passing all of them falls through to 0x17c7774 (the state advance).
    for (offset, label) in [
        (0x017c_7774u32, "case5-pass"),
        (0x017c_745cu32, "case5-fail"),
        (0x017c_7cecu32, "case5-56fail"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:case5", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 15_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 250
            {
                return 0;
            }
            log::warn!(
                "PROCHMI {label} t={}ms tid=[{}]",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r795b: the state-6 (0x17c7604) service-availability case in detail.
    // Its two null guards silently fall back to state 1 (`beq 0x17c7410`):
    // [server]=obj+0x34 and [server+0xec] (the transfer-msg handler the
    // navif brings up).  The nine `bl bIsServiceAvailable; cmp r0,#1` sites
    // gate {0x0a,0x2c,0x1f,0x1e,0x06,lit,0x3c,0x37,0x56}; non-available
    // services fall to `0x169e308(svc)` (required-check) which decides
    // between 0x17c745c (abort) and skipping - log each verdict + svc.
    for (offset, label) in [
        (0x017c_7604u32, "c6-entry"),
        (0x017c_760cu32, "c6-guardsrv"),
        (0x017c_7618u32, "c6-guardec"),
        (0x017c_7624u32, "c6-svc0a"),
        (0x017c_764cu32, "c6-svc2c"),
        (0x017c_7674u32, "c6-svc1f"),
        (0x017c_769cu32, "c6-svc1e"),
        (0x017c_76c4u32, "c6-svc06"),
        (0x017c_76ecu32, "c6-svcLIT"),
        (0x017c_7714u32, "c6-svc3c"),
        (0x017c_773cu32, "c6-svc37"),
        (0x017c_7764u32, "c6-svc56"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:svcgate", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 8 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI svcgate {label} t={}ms tid=[{}] r0={r0:#x} r1={r1:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r795b: the state-6 -> 7 gate `0x17c2760(sl,&ready)` (bIsHmiReady-
    // style poll): 0x169e308(0x0a) says service 0x0a required -> path
    // 0x17c289c wants 0x161f754()!=0 and its [+4] (the iENVI/IDAPI
    // interface) non-null; NOT-required path iterates the [this+0x58]
    // sink vector via vfn+0x24 then vfn+0x18.  Log which verdict and
    // which object is missing.
    for (offset, label) in [
        (0x017c_277cu32, "p0-req0a"),
        (0x017c_27f4u32, "p-false"),
        (0x017c_2814u32, "p-ret"),
        (0x017c_28a0u32, "p1-hmiobj"),
        (0x017c_28b0u32, "p2-iface"),
        (0x017c_27a4u32, "p3-req06"),
        (0x017c_27b4u32, "p4-req3c"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:readygate", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 15_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 10
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI readygate {label} t={}ms tid=[{}] r0={r0:#x} r4={r4:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r795b: dissect bIsCCAServiceRequired (0x169e308).  For svc 0x0a it
    // returns 1, which drives 0x17c2760 down the branch whose *out byte is
    // never set (only the req(0x0a)!=1 fall-through writes *out=1 at
    // 0x17c2808) - so the state-6 -> 7 advance is impossible.  The verdict
    // comes from either a registry value read (0xfc4780, key name from the
    // table at anchor+0x2acc08) or the static default byte at anchor+0x98+idx.
    // Log which source fired and with what value + key name.
    for (offset, label) in [
        (0x0169_e3e0u32, "rq-default"),
        (0x0169_e3bcu32, "rq-cached"),
        (0x0169_e408u32, "rq-regread"),
        (0x0169_e40cu32, "rq-regres"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:reqcheck", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 40 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            let key = if label == "rq-regread" {
                read_cstr(uc, r1, 48)
            } else {
                String::new()
            };
            // On the registry-return site also dump the out-param [sp+4].
            let out4 = if label == "rq-regres" {
                let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                read_u32(uc, sp + 4)
            } else {
                0
            };
            // At rq-default the `ldrb r0,[r3,#0x98]` has not executed yet;
            // read the static default byte ourselves (r3 = anchor + idx).
            let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
            let dflt = if label == "rq-default" {
                let mut b = [0u8; 1];
                if uc.mem_read((r3 + 0x98) as u64, &mut b).is_ok() {
                    b[0]
                } else {
                    0xff
                }
            } else {
                0
            };
            log::warn!(
                "PROCHMI reqcheck {label} t={}ms tid=[{}] r0={r0:#x} idx={r4} out4={out4:#x} dflt={dflt} key={key}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r795b: THE decisive site.  The state-6 timeout/second branch
    // (0x17c7b64) calls vfn+0x24 (0x1719458) on the sub-object at
    // [iface+0x78] and if it returns != 0 jumps to 0x17c7c18 which sets
    // state = 9 (bypassing the 0x17c2760 byte gate entirely).  vfn+0x24
    // dispatches on [obj+4]: state 0 -> lock, vfn+0x38, vfn+0x48 -> r7
    // (return value!), vfn+0x18; state 1 -> vfn+0x44; state 2 -> r7=1...
    // Probe the whole decision chain with one page hook (unicorn narrow
    // hooks are unreliable mid-TB - page dispatch per XP_TARGETS design).
    for (offset, label) in [
        (0x017c_7ba4u32, "s9-vfn24res"),
        (0x0171_9458u32, "v24-entry"),
        (0x0171_94d4u32, "v24-v48res"),
        (0x0171_94ecu32, "v24-v18res"),
        (0x0171_9500u32, "v24-v14res"),
        (0x0171_955cu32, "v24-ret"),
        (0x0161_4ce4u32, "bcast-a"),
        (0x0161_8fa0u32, "bcast-b"),
        (0x0161_d364u32, "bcast-sinkvfn"),
        (0x0161_d264u32, "bcast18-sinkvfn"),
        // r796: the vSvcReady vfn+0x1c event post (to the static FINAVI
        // object) and the navsrv sink-vector scan decision: 0x161d32c
        // skips the whole scan when the vector is empty (r4==r2).
        (0x015e_1a30u32, "svc-post"),
        (0x015e_1a54u32, "svc-post2"),
        // r796: the SERVICE_STATE_CHANGED consumer on the CL client object:
        // r4+0x30 = connected-service handler; null -> event dropped.
        (0x0138_2cf0u32, "cl-entry"),
        (0x0138_2d14u32, "cl-have"),
        (0x0138_2d38u32, "cl-nosvc"),
        (0x0161_d328u32, "sinkscan-vec"),
        // r796: navsrv-side notify posts that should wake the CL entries:
        // both are function entries (safe for narrow traps).
        (0x0157_8d88u32, "notify-svc"),
        (0x0157_5cdu32 & 0xffff_fffcu32, "notify-gen"),
        // r796: the tick's READY decision: 0x1719ac8 = all-sinks-connected
        // (0x17ba0a0 svc-status AND 0x17ba318 sink-state); its result gates
        // the store of 2 to the gate word at 0x1719cec-0x1719cf4.
        (0x0171_9af4u32, "conn-a"),
        (0x0171_9b0cu32, "conn-b"),
        (0x0171_9cf8u32, "conn-all"),
        (0x0171_9d04u32, "conn-jump2"),
        // per-entry conf verdict in the cmdsub step: r0 from 0x17b94b8;
        // r4 = [entry+0x28] (outstanding-conf counter), obj = the entry.
        (0x0171_9cecu32, "conf-res"),
        // r796: replaced dead narrow hooks - page-watch dispatch below.
        (0x0171_9cf0u32, "conf-store"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:s9gate", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 4000 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
            let obj = if label == "v24-entry" { r0 } else { r5 };
            let st = read_u32(uc, obj.wrapping_add(4));
            let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
            let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
            let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            // r796: the FINAVI vSvcReady event lands on r0's vfn+0x1c;
            // dump the callee class so the consumer can be disassembled.
            let spost = if label.starts_with("svc-post") {
                let vt = read_u32(uc, r0);
                format!(
                    " vt={vt:#x} v0={:#x} v18={:#x} v1c={:#x} v20={:#x} byte0={:#x} byte1={:#x}",
                    read_u32(uc, vt),
                    read_u32(uc, vt.wrapping_add(0x18)),
                    read_u32(uc, vt.wrapping_add(0x1c)),
                    read_u32(uc, vt.wrapping_add(0x20)),
                    {
                        let mut b = [0u8; 2];
                        let _ = uc.mem_read(r0 as u64, &mut b);
                        b[0]
                    },
                    {
                        let mut b = [0u8; 2];
                        let _ = uc.mem_read(r0 as u64, &mut b);
                        b[1]
                    },
                )
            } else if label.starts_with("cl-") {
                let base = if label == "cl-entry" { r0 } else { r4 };
                format!(
                    " clvt={:#x} handler={:#x}",
                    read_u32(uc, base),
                    read_u32(uc, base.wrapping_add(0x30)),
                )
            } else {
                String::new()
            };
            log::warn!(
                "PROCHMI s9gate {label} t={}ms tid=[{}] r0={r0:#x} r1={r1:#x} r2={r2:#x} obj={obj:#x} obj+4={st} r7={r7:#x} r3={r3:#x} r4={r4:#x}{spost}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r796: page-watch over the cmdsub step + tick tail on page 0x1719000:
    // the narrow probes at conf-res/conn-* are mid-TB and false-negative
    // (conf-store fired via the mem-watch while its conf-res probe stayed
    // at zero hits - AGENTS.md unicorn quirk).  Dispatch per fetch address.
    {
        let page = base_address + 0x0171_9000u32 - ORIGINAL_BASE;
        let sites: [(u32, &str); 14] = [
            (base_address + 0x0171_9af4u32 - ORIGINAL_BASE, "conn-a"),
            (base_address + 0x0171_9b0cu32 - ORIGINAL_BASE, "conn-b"),
            (base_address + 0x0171_9cecu32 - ORIGINAL_BASE, "conf-res"),
            (base_address + 0x0171_9cf0u32 - ORIGINAL_BASE, "conf-store"),
            (base_address + 0x0171_9cf8u32 - ORIGINAL_BASE, "conf-open"),
            (base_address + 0x0171_9d04u32 - ORIGINAL_BASE, "conn-jump2"),
            // r796: the SERVICE_STATUS event handler (entry vfn+0x1c):
            // entry, [entry+4] conn ptr, conn-ready check, the two branch
            // decisions, and the state-2 store itself.
            (base_address + 0x0171_9b74u32 - ORIGINAL_BASE, "ev-entry"),
            (base_address + 0x0171_9b88u32 - ORIGINAL_BASE, "ev-conn"),
            (base_address + 0x0171_9bd8u32 - ORIGINAL_BASE, "ev-connready"),
            (base_address + 0x0171_9c04u32 - ORIGINAL_BASE, "ev-v38"),
            (base_address + 0x0171_9e58u32 - ORIGINAL_BASE, "ev-store2"),
            (base_address + 0x0171_9c60u32 - ORIGINAL_BASE, "ev-v2c"),
            // r796b: the helper result at the step prologue: zero branches
            // straight to the state-2 store (0x1719e58), non-zero registers
            // the entry in the pump list and continues to the send.
            (base_address + 0x0171_9ca4u32 - ORIGINAL_BASE, "step-branch"),
            // r797: the conf-result store's branch target.  With no CCA
            // directory in the emulator the gate's RegisterConf is consumed
            // by the enConnect wait BEFORE this send runs (the burst raced
            // ahead at t=26.4), so 0x17b94b8 returns "no conf" and the step
            // falls into the service-resolution legs, where the gate's
            // vfn+0x38 is NULL (nothing resolves FI services for client
            // 0x0121) and the state-2 stores at 0x1719d74/0x1719ea4 are
            // unreachable.  The probe below re-stores the value the unit's
            // transaction layer would have returned.
            (base_address + 0x0171_9cf4u32 - ORIGINAL_BASE, "conf-tail"),
        ];
        let _ = unicorn.add_code_hook(
            page as u64,
            page as u64 + 0x1000,
            move |uc: &mut Unicorn<'_, Context>, addr, _| {
                static HP: AtomicU32 = AtomicU32::new(0);
                let hit = addr as u32;
                let Some((_, label)) = sites.iter().find(|(a, _)| *a == hit) else {
                    return;
                };
                let logged = HP.fetch_add(1, Ordering::Relaxed) < 4000;
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                let entry = if *label == "ev-entry" { r0 } else { r5 };
                let svc = read_u32(uc, entry.wrapping_add(0x20)) & 0xffff;
                let st = read_u32(uc, entry.wrapping_add(4));
                let conn = read_u32(uc, entry.wrapping_add(8));
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                // r796b: list-selection global (helper 0x1719b4c) live value
                let g = read_u32(uc, 0x02a1_39f8);
                let gv = read_u32(uc, g);
                // r797: live dump of the entry's vtable and the fields the
                // conf-failure legs evaluate (the runtime vtable lives in
                // .bss and must be read live, never offline).
                let vt = read_u32(uc, entry);
                let (s1c, s30, s34, s38) = (
                    read_u32(uc, vt.wrapping_add(0x1c)),
                    read_u32(uc, vt.wrapping_add(0x30)),
                    read_u32(uc, vt.wrapping_add(0x34)),
                    read_u32(uc, vt.wrapping_add(0x38)),
                );
                let (f0c, f18, f1c) = (
                    read_u32(uc, entry.wrapping_add(0xc)),
                    read_u32(uc, entry.wrapping_add(0x18)),
                    read_u32(uc, entry.wrapping_add(0x1c)),
                );
                if logged {
                    log::warn!(
                    "PROCHMI stepprobe {label} t={}ms tid=[{}] r0={r0:#x} entry={entry:#x} state={st} svc={svc:#x} c28={r4:#x} conn={conn:#x} lsv={gv:#x} lr={lr:#x} vt={vt:#x} v1c={s1c:#x} v30={s30:#x} v34={s34:#x} v38={s38:#x} f0c={f0c:#x} f18={f18:#x} f1c={f1c:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
                }
                // r797: the gate entry (svc 0x000a) has now stored its
                // REGISTERED(1) state - its event pump only picks up
                // statuses that arrive *after* this point (f202: earlier
                // ones were eaten by the startup connector's Wait).  The
                // late status makes the keyed dispatch reach the entry,
                // which drives vfn+0x4c -> store READY(2) -> popup 0x5da.
                // r797b (f217): the gate OPENED this way.  The keyed
                // completer (0x1c80adc) only fires for class-0x45 answers
                // and matches the entry purely by the transaction ordinal
                // echoed at content +0x1c (the dispatcher accepted even
                // event=0x0), and r1 at this very site IS that ordinal
                // (f217: 1,2 = the svc-0x06 entries, 3 = the gate).  The
                // state-7+ gates are the burst-registered FI services
                // (0x1e/0x18/0x1f/0x2c/0x0d/0x300), so answer every
                // unhosted conf the same way: 0x0a/0x1e ride the genuine
                // SENSORFI responder (real GpsDataStatus/NavPositionStatus
                // payloads), the rest get a minimal ServiceData push.
                // All of it is logged; EMU_FAKE_SENSORFI=0 / 
                // EMU_FAKE_FI_READY_PUSH=0 disable the legs.
                let ordinal =
                    (uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xffff) as u16;
                if svc == 6 && (st == 0 || st == 1) {
                    // Record the live transaction ordinal per entry so the
                    // g-m18 starvation leg can re-fire the completion push.
                    if let Ok(mut m) = SVC6_TXN.lock() {
                        m.insert(entry, ordinal);
                    }
                }
                // r798b: answer re-registrations too.  A CL entry whose
                // first answer got stolen (or raced) re-sends RegisterConf
                // with a NEW ordinal via the vfn+0x24 state-1 leg, whose
                // 0x1719cf0 store sees st==1; answering only st==0 left
                // seven of the eight state-6 members READY-starved forever
                // (f239 g-list-mem: only mem[1] ever won).  One push per
                // (service, ordinal) still bounds the traffic, and a READY
                // entry stops re-driving (the state-2 leg never re-sends).
                if *label == "conf-store" && (st == 0 || st == 1) {
                    // r798b: ordinal 0 stays answerable: two svc-0x06
                    // members register with it (f240) and only a key-0 push
                    // can complete them; a wrong-key match is a no-op in the
                    // completer (f232: keys 0x11..0x1e all inert).
                    if svc == 0x000a {
                        static DEFERRED: AtomicU32 = AtomicU32::new(0);
                        if DEFERRED.fetch_add(1, Ordering::Relaxed) == 0 {
                            log::warn!(
                                "PROCHMI gate: conf-store for svc 0xa done (ordinal {ordinal}), injecting the late SENSORFI ServiceStatus"
                            );
                            crate::libs::libosal_linux::message::cca_post_deferred_status(
                                uc, 0x0015, 0x0121, 0x000a,
                            );
                            crate::libs::libosal_linux::message::fabricate_fi_data_request(
                                uc, 0x000a, ordinal,
                            );
                            // r798b: the request leg depends on the parked
                            // tid-84 reader of mbx_21; also post the server
                            // answer directly (pump queue first, see
                            // message.rs) so the gate completion (and the
                            // FI-event kick it doubles as) does not depend
                            // on it (f235: request never consumed, state 6
                            // forever).
                            let route =
                                crate::libs::libosal_linux::message::push_gps_answer(
                                    uc, ordinal,
                                );
                            log::info!(
                                "[LIBOSAL-MBX] gate svc 0x000a txn {ordinal}: direct GpsDataStatus answer route {route}"
                            );
                        }
                    } else if matches!(svc, 0x0006 | 0x001e | 0x0018 | 0x001f | 0x002c | 0x000d | 0x0300) {
                        // one push per (service, transaction ordinal); a
                        // re-registration gets a new ordinal and is answered
                        // anew, repeats of the same transaction are not.
                        // r798: DAPI's GENUINE svc-0x06 (DAP_RESINF) answers
                        // do reach the completer and dispatch (f232: key=1
                        // at 26926ms) but carry events 0x80000215/218/00e,
                        // which route to the app data receiver, never to the
                        // entry-completion leg; only event=0 (what the
                        // pump's 0xfb8f50 conversion yields for the minimal
                        // push shape) drove the gate's READY store.  The
                        // startup entry therefore needs its own event-0
                        // completion like the other unhosted confs.
                        static ANSWERED: std::sync::OnceLock<
                            std::sync::Mutex<std::collections::HashSet<(u16, u16, u32)>>,
                        > = std::sync::OnceLock::new();
                        let set = ANSWERED.get_or_init(Default::default);
                        // r798b: keyed by ENTRY too - f240 saw two svc-0x06
                        // members register with transaction 0, and a
                        // (svc, ordinal) dedup left one of them starved.
                        let fresh = set
                            .lock()
                            .map(|mut s| s.insert((svc as u16, ordinal, entry)))
                            .unwrap_or(true);
                        if fresh {
                            log::warn!(
                                "PROCHMI fi-ready: fabricating ServiceData completion for svc {svc:#06x} transaction {ordinal} (EMU_FAKE_SENSORFI=0 / EMU_FAKE_FI_READY_PUSH=0 disable)"
                            );
                            if svc == 0x001e {
                                crate::libs::libosal_linux::message::fabricate_fi_data_request(
                                    uc, svc as u16, ordinal,
                                );
                            } else {
                                crate::libs::libosal_linux::message::fabricate_fi_ready_push(
                                    uc, svc as u16, ordinal,
                                );
                            }
                        }
                    }
                }
            },
        );
    }
    // r796c: the real HMINavServerLib pump (libosal caller offsets are
    // lr-0x8000, so "caller=+0x17c36xx" lines are runtime VA 0x17cb6xx):
    // 0x17cb574 receives one message from [obj+8], special-cases class 0x45
    // (extracts content+0x1b/+0x1c, converts via 0xfb8f50); 0x17cb668 posts
    // the 8-byte internal event. All three are function entries (safe for
    // narrow traps).
    {
        for (off, label, budget) in [
            // r798b: budgets raised across the board - f266 showed the old
            // 30-hit caps were exhausted by the 27 s member burst, hiding
            // every post-27 s completer/dispatch event (the f74 window).
            (0x017c_b574u32, "rx-pump", 400u32),
            (0x000f_b8f0u32, "rx-45", 200),
            (0x017c_b668u32, "post8", 300),
            (0x017c_b6b0u32, "notify", 200),
            (0x017c_cb88u32, "list-add", 200),
            (0x017b_93fcu32, "dispatch", 400),
            (0x017c_ce38u32, "re-entry", 200),
            // r797b: the CCA answer-completer (liboscbcu-in-prochmi): called
            // per consumed answer with r0=connector, r1=event id,
            // ldrh [sp+0x50]=svc, ldrh [sp+0x54]=transaction key,
            // [sp+0x58]=msg object.  It produced dispatch(key=1/2) for
            // DAPI's genuine svc-0x06 pair and NOTHING for our 0x0a answers
            // (f204-f209) - log its arguments to see whether our message
            // ever reaches it and which field disqualifies it.
            (0x01c8_0adcu32, "completer", 400),
            // r797b: between keyed dispatch and completer there are two
            // bail guards (f214 disasm): the receiver's vfn+0x10 (0x1c7c768)
            // must return != 0, then [receiver+0xc] (0x1c7c778) must be non-
            // NULL and 0x1c7c150 (before 0x1c7c794) must return != 0.  Our
            // GpsDataStatus 0x45 died on one of them - find which.
            (0x01c7_c768u32, "vfn10", 300),
            (0x01c7_c778u32, "guard-c", 120),
            (0x01c7_c794u32, "guard-150", 120),
            // r799d: the pending-entry table lookup the pump-event handler
            // performs (0x1c7ed50 -> 0x1c7d110(r0=conn+0x38, r1=svc u16,
            // r2=key u16, r3=func u16(content+0x18), [sp]=optype
            // (content+0x1a), [sp+4]=msgobj).  Log the live parameter shape
            // so the golden completions' func/optype can be compared with
            // our fabricated answers' (func 0, optype 9).
            (0x01c7_d110u32, "lookup", 300),
            // r799d: right after `bl #0x1c7ddac` (the service->entry map
            // find inside the lookup): r0 = entry (0 = the misses we keep
            // getting), else dump its fields: +0x00 u16, +0x02 u16,
            // +0x06 u16, +0x08 u16, +0x0a STATE byte (3 = pending, 4 =
            // answered leg) and +0x0c u16 (transaction id).
            (0x01c7_d14cu32, "found", 300),
            // r799d: the completer's OWN pending-map find
            // (0x1c80adc -> bl 0x1f8d018(r0,r1,r2) -> bl 0x1f6e098): log the
            // key triple at entry and the found/NULL result right after.
            (0x01f8_d018u32, "clook-e", 200),
            (0x01f8_d020u32, "clook-r", 200),
            // r799d: return of the completer (vfn+0x1c) inside the guard
            // chain (0x1c7c7bc `ldr pc,[ip,#0x1c]`, r4 = result at 0x1c7c7ec)
            // - f292: the answer FINDS the entry yet tid-62 still exits(127)
            // moments later, so the poison is returned deeper down.
            (0x01c7_c7ecu32, "compres", 300),
            // r799d: return of the pump-event handler `bl #0x1c80298` in
            // tid-62's drain loop (0x17c1ca8, r4 = result at 0x17c1cac).
            // The loop fed a non-zero result into its thread-body exit
            // (exit(127), f287-f293) - read the per-message code.
            (0x017c_1cacu32, "hres", 300),
            // r799d: tid-62 dies through glibc abort() (raise SIGABRT with
            // no stderr message = uncaught C++ exception / bare abort) right
            // after the f74 found-leg handler returns (f294: 27667ms).
            // Capture WHICH host calls abort/throw.
            (0x00fb_6ca4u32, "abort", 40),
            (0x00fb_6b60u32, "throw", 40),
            (0x00fb_6a64u32, "assert", 40),
            // f296: tid-62 dies via exit_group(127) with lr inside the
            // static glibc leaf region - glibc abort() raises SIGABRT twice
            // and ends `_exit(127)`.  The abort() body itself never hit its
            // dynsym site (f295), so watch `raise` as the sequence witness.
            (0x00fb_6b78u32, "raise", 40),
            // r799e: 0x1614dac(idapi, key, &out) = vector search + the
            // 0x171fcf4 record scan (medavail).  The state-8 f74 consumer
            // leg ends on entry 0x3a31f40 with an EMPTY record vector
            // (f294/f297) - log which key asks for it, and who asks.
            (0x0161_4dacu32, "idq", 120),
            (0x0161_4db8u32, "idq-r", 120),
            (0x0161_4d94u32, "ids-r", 120),
        ] {
            let address = base_address + off - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:pumptrace", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: std::sync::OnceLock<
                    std::sync::Mutex<std::collections::HashMap<&'static str, u32>>,
                > = std::sync::OnceLock::new();
                let map = HITS.get_or_init(Default::default);
                let mut map = match map.lock() {
                    Ok(m) => m,
                    Err(p) => p.into_inner(),
                };
                let n = map.entry(label).or_insert(0);
                if *n >= budget {
                    return 0;
                }
                *n += 1;
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                if label == "rx-pump" {
                    // r3 = message ref, content = [r3+4] (see 0x17cb5e0)
                    let content = read_u32(uc, r3.wrapping_add(4));
                    let cls = read_u32(uc, content.wrapping_add(0x08)) >> 24;
                    let svc = read_u32(uc, content.wrapping_add(0x14)) & 0xffff;
                    // r798b: every received message logged class 0 - dump the
                    // raw frames once to pin the actual queue format.
                    let (m0, m4, m8, mc) = (
                        read_u32(uc, r3),
                        read_u32(uc, r3.wrapping_add(4)),
                        read_u32(uc, r3.wrapping_add(8)),
                        read_u32(uc, r3.wrapping_add(0xc)),
                    );
                    static RAW: AtomicU32 = AtomicU32::new(0);
                    let raw = if RAW.fetch_add(1, Ordering::Relaxed) < 6 {
                        let (c0, c4, c8, cc, c14) = (
                            read_u32(uc, content),
                            read_u32(uc, content.wrapping_add(4)),
                            read_u32(uc, content.wrapping_add(8)),
                            read_u32(uc, content.wrapping_add(0xc)),
                            read_u32(uc, content.wrapping_add(0x14)),
                        );
                        format!(" msg=[{m0:#x},{m4:#x},{m8:#x},{mc:#x}] blob=[{c0:#x},{c4:#x},{c8:#x},{cc:#x},{c14:#x}]")
                    } else {
                        String::new()
                    };
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] obj={r0:#x} msg={r3:#x} content={content:#x} class={cls:#x} svc={svc:#x}{raw} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "post8" {
                    let q = read_u32(uc, r0.wrapping_add(8));
                    let w1 = read_u32(uc, r1.wrapping_add(4));
                    let w2 = read_u32(uc, r1.wrapping_add(8));
                    let w3 = read_u32(uc, r1.wrapping_add(0xc));
                    // r798b: keep the live [1, slot, w2, w3] shape so host
                    // completion pushes can be byte-identical (message.rs).
                    crate::libs::libosal_linux::message::note_pump_post_shape(w2, w3);
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] queue={q:#x} w1={w1:#x} w3={w3:#x} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "dispatch" {
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let a4 = read_u32(uc, sp.wrapping_add(0x20));
                    let a5 = read_u32(uc, sp.wrapping_add(0x24));
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] obj={r0:#x} event={r1:#x} key={r2:#x} a4={a4:#x} a5={a5:#x} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "completer" {
                    // r797b: trap fires BEFORE the entry's
                    // `push {r4..lr}, sub sp,#0x2c` (0x50 bytes total), so
                    // the function's `[sp+0x50]/[sp+0x54]/[sp+0x58]` stack
                    // args (svc u16, transaction key u16, msg object) sit
                    // at OUR sp+0x00 / +0x04 / +0x08.
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let svc = read_u32(uc, sp) & 0xffff;
                    let key = read_u32(uc, sp.wrapping_add(4)) & 0xffff;
                    let msgobj = read_u32(uc, sp.wrapping_add(8));
                    let data = if msgobj != 0 {
                        read_u32(uc, msgobj.wrapping_add(4))
                    } else {
                        0
                    };
                    let (mut w, mut cls, mut src, mut regid) =
                        (0u32, 0u32, 0u32, 0u32);
                    if data != 0 {
                        w = read_u32(uc, data.wrapping_add(8));
                        cls = (w >> 24) & 0xff; // content byte +0x0b
                        src = read_u32(uc, data) & 0xffff;
                        regid = (read_u32(uc, data.wrapping_add(0x14)) >> 16)
                            & 0xffff;
                    }
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] conn={r0:#x} event={r1:#x} svc={svc:#x} key={key:#x} msgobj={msgobj:#x} data={data:#x} class={cls:#x} src={src:#x} regid={regid:#x} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "vfn10" {
                    let vt = read_u32(uc, r0);
                    let f10 = read_u32(uc, vt.wrapping_add(0x10));
                    let f1c = read_u32(uc, vt.wrapping_add(0x1c));
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] recv={r0:#x} vt={vt:#x} f10={f10:#x} f1c={f1c:#x} r1={r1:#x} r2={r2:#x} r3={r3:#x} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "guard-c" {
                    let fc = read_u32(uc, r0.wrapping_add(0xc));
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] recv={r0:#x} +0xc={fc:#x} r1={r1:#x} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if matches!(label, "idq-r" | "ids-r") {
                    // r799e: the enavi media-device entry resolved for the
                    // popup-release / f74-readiness legs (f294/f297/f298:
                    // 0x3a31f40, f299: 0x3a332a8) carries +0x80 = +0x88 = 0
                    // and an EMPTY record vector, because on the real unit
                    // those fields are set by the TE-side media-state
                    // answers the emulator has no source for.  The
                    // deciders (0x171fd7c state switch on +0x88,
                    // 0x171fcf4 record leg) then report EMPTY(0).  Give the
                    // entry the byte-identical READY shape the fabricated
                    // cryptcard carries and that the popup-release leg
                    // accepts: +0x80 = 2, +0x88 = 3 (switch case 3 -> READY
                    // record scan -> 2), one 0x18 record (+4 nonzero,
                    // +0x0c = 2).  Same provider-signal fabrication class
                    // as fake_cryptcard_into_idapi; EMU_FAKE_ENAVI_MEDIA=0
                    // disables.
                    let f80 = read_u32(uc, r0.wrapping_add(0x80));
                    let f88 = read_u32(uc, r0.wrapping_add(0x88));
                    let b98 = read_u32(uc, r0.wrapping_add(0x98));
                    let b9c = read_u32(uc, r0.wrapping_add(0x9c));
                    let enabled =
                        std::env::var_os("EMU_FAKE_ENAVI_MEDIA").map_or(true, |v| v != "0");
                    // Same state-8 gate policy as the SENSORFI responder:
                    // answering the popup leg before startup reaches state 8
                    // runs the release branch against a half-initialised app
                    // (f300: fab at t=3.5s, abort on tid 62 at 26.7s).
                    let fsm = crate::libs::libosal_linux::message::hmi_fsm_state();
                    if enabled && r0 != 0 && r0 < 0xf000_0000 && (8..0xffff_ffff).contains(&fsm) && f80 <= 2 && f88 == 0 && b98 == b9c {
                        static REC: AtomicU32 = AtomicU32::new(0);
                        let mut rec = REC.load(Ordering::Relaxed);
                        if rec == 0 {
                            let mmu_arc = uc.get_data().mmu.clone();
                            rec = mmu_arc.lock().unwrap().heap_alloc(
                                uc,
                                0x100,
                                Prot::READ | Prot::WRITE,
                                "[prochmi-enavi-media-fab]",
                            );
                            REC.store(rec, Ordering::Relaxed);
                            if rec != 0 {
                                // one 0x18-byte status record: device back-
                                // pointer, +4 nonzero (valid), +0x0c = 2
                                // (READY), remaining words 0.
                                let words = [r0, 1, 0, 2, 0, 0];
                                for (i, w) in words.iter().enumerate() {
                                    let _ = write_u32(uc, rec + (i * 4) as u32, *w);
                                }
                            }
                        }
                        if rec != 0 {
                            let ok = write_u32(uc, r0 + 0x80, 2)
                                && write_u32(uc, r0 + 0x88, 3)
                                && write_u32(uc, r0 + 0x98, rec)
                                && write_u32(uc, r0 + 0x9c, rec + 0x18);
                            static ONCE: AtomicU32 = AtomicU32::new(0);
                            if ok && ONCE.fetch_add(1, Ordering::Relaxed) < 3 {
                                log::warn!(
                                    "PROCHMI enavi-media-fab t={}ms: media READY shape (+0x80=2 +0x88=3, record {rec:#x}) on device entry {r0:#x} (media-state provider signal; EMU_FAKE_ENAVI_MEDIA=0 disables)",
                                    crate::libs::dapi::emu_t0_elapsed_ms(),
                                );
                            }
                        }
                    }
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] entry={r0:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "idq" {
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let (k0, k1) = if r1 > 0x8000 && r1 < 0xf000_0000 {
                        (read_u32(uc, r1), read_u32(uc, r1.wrapping_add(4)))
                    } else {
                        (0, 0)
                    };
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] idapi={r0:#x} key={r1:#x} kw=[{k0:#x},{k1:#x}] out={r2:#x} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if matches!(label, "abort" | "throw" | "assert" | "raise") {
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let mut chain: Vec<u32> = Vec::new();
                    for i in 0..64u32 {
                        let mut w = [0u8; 4];
                        if uc.mem_read((sp + i * 4) as u64, &mut w).is_err() {
                            break;
                        }
                        let w = u32::from_le_bytes(w);
                        if (0x8000..0x0300_0000).contains(&w) && w & 3 == 0 {
                            if !chain.contains(&w) {
                                chain.push(w);
                                if chain.len() >= 10 {
                                    break;
                                }
                            }
                        }
                    }
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] lr={lr:#x} sp={sp:#x} chain={chain:#x?}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "hres" {
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] handler_res={r0:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "compres" {
                    let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] completer_res={r4:#x} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "clook-e" {
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] a0={r0:#x} a1={r1:#x} a2={r2:#x} r3={r3:#x} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "clook-r" {
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] res={r0:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "found" {
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32 & 0xffff;
                    let ordinal = read_u32(uc, sp + 0x14) & 0xffff;
                    let (w0, w2, w6, w8, wa, wc) = if r0 != 0 && r0 < 0xf000_0000 {
                        (
                            read_u32(uc, r0) & 0xffff,
                            (read_u32(uc, r0.wrapping_add(2)) >> 16) & 0xffff,
                            read_u32(uc, r0.wrapping_add(6)) & 0xffff,
                            (read_u32(uc, r0.wrapping_add(8)) >> 16) & 0xffff,
                            (read_u32(uc, r0.wrapping_add(8)) >> 8) & 0xff,
                            read_u32(uc, r0.wrapping_add(0xc)) & 0xffff,
                        )
                    } else {
                        (0xffff, 0xffff, 0xffff, 0xffff, 0xff, 0xffff)
                    };
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] entry={r0:#x} service={r7:#x} want_ord={ordinal:#x} w0={w0:#x} w2={w2:#x} w6={w6:#x} w8={w8:#x} state={wa:#x} w12={wc:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "lookup" {
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let optype = read_u32(uc, sp) & 0xff;
                    let msgobj = read_u32(uc, sp.wrapping_add(4));
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] tbl={r0:#x} svc={:#x} key={:#x} func={:#x} optype={optype:#x} msgobj={msgobj:#x} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        r1 & 0xffff,
                        r2 & 0xffff,
                        r3 & 0xffff,
                    );
                } else if label == "waitret" {
                    // r0 = message-ref pointer (0 on timeout).  Dump the ref
                    // and the 8-byte queue frame the Wait wrote at its
                    // output buffer (fixed tid-62 stack VA 0x907bdd2c, same
                    // every call; the completer's msgobj == 0x907bdd20).
                    let (w0, w4, w8, wc) = if r0 != 0 && r0 < 0xf000_0000 {
                        (
                            read_u32(uc, r0),
                            read_u32(uc, r0.wrapping_add(4)),
                            read_u32(uc, r0.wrapping_add(8)),
                            read_u32(uc, r0.wrapping_add(0xc)),
                        )
                    } else {
                        (0, 0, 0, 0)
                    };
                    let buf = 0x907b_dd2cu32;
                    let (b0, b4) = (read_u32(uc, buf), read_u32(uc, buf + 4));
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] r0={r0:#x} ref=[{w0:#x},{w4:#x},{w8:#x},{wc:#x}] buf=[{b0:#x},{b4:#x}] lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "guard-150" {
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] res={r0:#x} r1={r1:#x} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else if label == "list-add" {
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] anchor={r0:#x} node={r1:#x} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                    // r796d: watch the vector header (data ptr @+4, count
                    // @+0xc) from the first registration on: whoever READS
                    // these fields is the walker that dispatches to the
                    // entries - catching its pc/lr pins the event path the
                    // gate entry (svc 0x000a) is missing.
                    static WALKED: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    if !WALKED.swap(true, Ordering::Relaxed) && r0 != 0 {
                        let lo = r0.wrapping_sub(4) as u64;
                        let hi = r0.wrapping_add(0x20) as u64;
                        let _ = uc.add_mem_hook(
                            unicorn_engine::unicorn_const::HookType::MEM_READ,
                            lo,
                            hi,
                            move |uc: &mut Unicorn<'_, Context>, _t, address, size, _| {
                                static RP: AtomicU32 = AtomicU32::new(0);
                                if RP.fetch_add(1, Ordering::Relaxed) >= 40 {
                                    return true;
                                }
                                let pc = uc
                                    .reg_read(RegisterARM::PC)
                                    .unwrap_or(0) as u32;
                                let lr = uc
                                    .reg_read(RegisterARM::LR)
                                    .unwrap_or(0) as u32;
                                log::warn!(
                                    "PROCHMI vecwalk t={}ms tid=[{}] addr={address:#x} size={size} pc={pc:#x} lr={lr:#x}",
                                    crate::libs::dapi::emu_t0_elapsed_ms(),
                                    uc.get_data().inner.thread_id(),
                                );
                                true
                            },
                        );
                    }
                } else if label == "notify" {
                    let w1 = read_u32(uc, r1.wrapping_add(4));
                    let w2 = read_u32(uc, r1.wrapping_add(8));
                    let w3 = read_u32(uc, r1.wrapping_add(0xc));
                    let p0 = read_u32(uc, w3);
                    let p4 = read_u32(uc, w3.wrapping_add(4));
                    let p8 = read_u32(uc, w3.wrapping_add(8));
                    let pc = read_u32(uc, w3.wrapping_add(0xc));
                    let p10 = read_u32(uc, w3.wrapping_add(0x10));
                    let p14 = read_u32(uc, w3.wrapping_add(0x14));
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] w1={w1:#x} w2={w2:#x} payload={w3:#x} [{p0:#x} {p4:#x} {p8:#x} {pc:#x} {p10:#x} {p14:#x}] lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                } else {
                    let a0 = read_u32(uc, r0);
                    let a1 = read_u32(uc, r0.wrapping_add(4));
                    let a2 = read_u32(uc, r0.wrapping_add(8));
                    let a3 = read_u32(uc, r0.wrapping_add(0xc));
                    log::warn!(
                        "PROCHMI pumptrace {label} t={}ms tid=[{}] r0={r0:#x} [r0]={a0:#x} [r0+4]={a1:#x} [r0+8]={a2:#x} [r0+0xc]={a3:#x} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                    );
                }
                0
            });
        }
    }
    // r797b: the rx-pump's content/class decision (f215 disasm): 0x17cb5e0
    // loads content=[r7+4] (content==0 jumps to 0x17cb5f8), 0x17cb5ec tests
    // byte content[0xb] == 0x45 (else fall-through reject 0x17cb5f8), the
    // accept leg is 0x17cb60c, receive-empty is 0x17cb600.  Page-watch the
    // whole page and dispatch on the fetch address (mid-TB narrow traps can
    // register dead, r797 lesson).
    {
        let page = (base_address + 0x017c_b000u32 - ORIGINAL_BASE) as u64;
        let rej = (base_address + 0x017c_b5f8u32 - ORIGINAL_BASE) as u64;
        let acc = (base_address + 0x017c_b60cu32 - ORIGINAL_BASE) as u64;
        let empty = (base_address + 0x017c_b600u32 - ORIGINAL_BASE) as u64;
        let result = unicorn.add_code_hook(page, page + 0x1000, move |uc: &mut Unicorn<'_, Context>, addr, _| {
            if addr != rej && addr != acc && addr != empty {
                return;
            }
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 24_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 40
            {
                return;
            }
            let tag = if addr == rej {
                "reject"
            } else if addr == acc {
                "accept"
            } else {
                "empty"
            };
            let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
            let mut word = |a: u64| -> u32 {
                let mut b = [0u8; 4];
                if uc.mem_read(a, &mut b).is_ok() {
                    u32::from_le_bytes(b)
                } else {
                    0
                }
            };
            let content = word(r7 as u64 + 4);
            let (w0, w4, w14, cls) = if content != 0 {
                (
                    word(content as u64),
                    word(content as u64 + 4),
                    word(content as u64 + 0x14),
                    word(content as u64 + 8) >> 24,
                )
            } else {
                (0, 0, 0, 0xff)
            };
            log::warn!(
                "PROCHMI pumppage {tag} t={}ms tid=[{}] msgobj={r7:#x} content={content:#x} cls={cls:#x} w0={w0:#x} w4={w4:#010x} w14={w14:#010x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
        });
        match result {
            Ok(_) => log::info!("PROCHMI: pumppage decision watch installed"),
            Err(e) => log::warn!("PROCHMI: pumppage watch failed: {e}"),
        }
    }
    // r799d: return of the HMINavServerLib_1 Wait in tid-62's drain (the api
    // intercept logs caller=+0x17c35c0: `bl #0x161f754` at 0x17c35bc, result
    // check at 0x17c35c4).  f278: the drain consumed its last rx at 27.26s
    // although 24 fabricated events were native-posted 27.3-28.9s, then the
    // loop gave up (the f263-era pump-death shape).  Did the Wait return 0
    // (the native queue never delivering our posts) or a filled ref?
    // Page-dispatched: the narrow trap_site at this mid-TB address is dead
    // in f279 (registered, zero hits, while the same-thread entry traps fire).
    {
        let page = (base_address + 0x017c_3000u32 - ORIGINAL_BASE) as u64;
        let sites = [(0x017c_35c4u32, "A"), (0x017c_3630u32, "B"), (0x017c_369cu32, "C")];
        let targets: Vec<u64> = sites
            .iter()
            .map(|(off, _)| (base_address + off - ORIGINAL_BASE) as u64)
            .collect();
        let result = unicorn.add_code_hook(page, page + 0x1000, move |uc: &mut Unicorn<'_, Context>, addr, _| {
            let Some(idx) = targets.iter().position(|t| *t == addr) else {
                return;
            };
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 240 {
                return;
            }
            let tid = uc.get_data().inner.thread_id();
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let (w0, w4) = if r0 != 0 && r0 < 0xf000_0000 {
                (read_u32(uc, r0), read_u32(uc, r0.wrapping_add(4)))
            } else {
                (0, 0)
            };
            let buf = 0x907b_dd2cu32;
            let (b0, b4) = (read_u32(uc, buf), read_u32(uc, buf + 4));
            log::warn!(
                "PROCHMI waitret{} t={}ms tid=[{tid}] r0={r0:#x} ref=[{w0:#x},{w4:#x}] buf=[{b0:#x},{b4:#x}]",
                sites[idx].1,
                crate::libs::dapi::emu_t0_elapsed_ms(),
            );
        });
        match result {
            Ok(_) => log::info!("PROCHMI: waitret page watch installed"),
            Err(e) => log::warn!("PROCHMI: waitret watch failed: {e}"),
        }
    }
    // r796: earliest reliable sighting of the gate object: 0x17c28a0 runs
    // right after `bl 0x161f754` (hmi singleton) inside 0x17c2760 - install
    // the gate-word write-watch HERE (no 15 s gate, no hit budget) so the
    // boot-time 0 -> 1 writer and any later writer are all captured.
    {
        let address = base_address + 0x017c_28a0u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:earlygate", move |uc: &mut Unicorn<'_, Context>| {
            static SEEN: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if SEEN.swap(true, Ordering::Relaxed) {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let holder = read_u32(uc, r0.wrapping_add(4));
            let obj = read_u32(uc, holder.wrapping_add(4));
            if obj == 0 {
                return 0;
            }
            let word = obj.wrapping_add(4);
            let cur = read_u32(uc, word);
            log::warn!(
                "PROCHMI earlygate t={}ms hmi={r0:#x} holder={holder:#x} obj={obj:#x} word={word:#x} cur={cur}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
            );
            let _ = uc.add_mem_hook(
                unicorn_engine::unicorn_const::HookType::MEM_WRITE,
                (word - 0x10) as u64,
                (word + 0x30) as u64, // r797: extended - also watch the
                // registration-timeout pair at +0x18/+0x1c that the sink walk's
                // vfn+0x14 (`0x1719258`) evaluates: it returns "not connected"
                // while `+0x1c == 0`, so whoever is supposed to store the
                // timeout window into the gate entry is on the critical path.
                move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                    static WE: AtomicU32 = AtomicU32::new(0);
                    if WE.fetch_add(1, Ordering::Relaxed) < 24 {
                        let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                        let pc = uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32;
                        log::warn!(
                            "PROCHMI earlygate WRITE t={}ms addr={address:#x} size={size} value={value:#x} pc={pc:#x} lr={lr:#x}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                        );
                    }
                    // r798: the state-6 body only re-evaluates the gate when
                    // an enavi-server status event re-ticks its handler
                    // (f232: the 6->7 advance ran ~4 s AFTER the READY store,
                    // kicked by a GpsDataStatus event2app; f233: the store
                    // landed 211 ms after the LAST burst event and the FSM
                    // sat in state 6 forever).  Every 1 -> READY(2) transition
                    // of the gate word therefore fires one fabricated
                    // SENSORFI request whose genuine GpsDataStatus answer
                    // supplies the kick.  A high dummy ordinal keeps it out
                    // of the completer (unmatched keys are no-ops, f232:
                    // 0x11..0x1e).  EMU_FAKE_SENSORFI=0 disables.
                    if address == word as u64 && value == 2 {
                        static KICK: AtomicU32 = AtomicU32::new(0);
                        if KICK.fetch_add(1, Ordering::Relaxed) < 3 {
                            // r798b: the fabricated REQUEST path is unreliable
                            // here - mbx_21's only reader is the same tid-84
                            // enConnect loop that is often parked (f235: the
                            // request was never consumed, the gate never
                            // opened).  Post the server-side answer directly:
                            // identical body shape to the responder's, so the
                            // pump converts it to the same 0x81008114 FI event
                            // whose tid-50 handler re-ticks the startup FSM.
                            // pump queue (see message.rs), so the kick does
                            // not depend on the mbx_289 receive race.
                            let route =
                                crate::libs::libosal_linux::message::push_gps_answer(
                                    uc, 0x5aa5,
                                );
                            if route != "none" {
                                log::warn!(
                                    "PROCHMI gate-kick: gate stored READY(2), posted a GpsDataStatus-shaped answer via {route} so its FI event re-ticks the enavi-status handler t={}ms (EMU_FAKE_SENSORFI=0 disables)",
                                    crate::libs::dapi::emu_t0_elapsed_ms(),
                                );
                            }
                        }
                    }
                    true
                },
            );
            // r797: who READS the gate entry at all?  Every dispatch of its
            // vfn+0x1c handler fetches the vtable from [entry+0], every walk
            // reads the svc field at +0x20 / the state at +4.  A read-watch
            // over the whole entry header proves whether the pump ever
            // touches this entry after its REGISTERED(1) store - the keyed
            // dispatch chain demonstrably never calls 0x1719b74 on it.
            let _ = uc.add_mem_hook(
                unicorn_engine::unicorn_const::HookType::MEM_READ,
                (obj - 4) as u64,
                (obj + 0x30) as u64,
                move |uc: &mut Unicorn<'_, Context>, _t, address, size, _| {
                    static RE: AtomicU32 = AtomicU32::new(0);
                    let t = crate::libs::dapi::emu_t0_elapsed_ms();
                    if t < 24000 {
                        return true;
                    }
                    if RE.fetch_add(1, Ordering::Relaxed) < 90 {
                        let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                        let pc = uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32;
                        log::warn!(
                            "PROCHMI earlygate READ t={t}ms addr={address:#x} size={size} pc={pc:#x} lr={lr:#x}",
                        );
                    }
                    true
                },
            );
            0
        });
    }
    // r795b: the state-6 gate's interface round-trip: iface = [[0x3a23eb8
    // +4]+4]; vfn+0x24 -> r8, vfn+0x18 -> gate pass, and the state-7 body
    // (0x17c7908) first calls vfn+0x1c (cmp #1).  Log the three results and
    // the iface vtable.
    for (offset, label) in [
        (0x017c_28ccu32, "g-vfn24"),
        (0x017c_28e4u32, "g-vfn18"),
        // r797b: reached only when the inner vfn+0x18 returned != 0; r8 =
        // the vfn+0x24 result that the helper turns into the ready byte.
        (0x017c_2910u32, "g-lic"),
        // r798b: helper 0x17c2760 has flag-query legs (0x169e308 = feature
        // membership in a lazily built 16-entry u16 list) and a second
        // object leg (0x17c2920) that reach the byte-0 exit 0x17c2814 even
        // when the gate itself is READY; f237 saw g-lic with r8=1 yet the
        // byte never became 1.  Log every leg result.
        (0x017c_277cu32, "g-fla"),
        (0x017c_27a4u32, "g-fl6"),
        (0x017c_27b4u32, "g-fl3c"),
        (0x017c_2800u32, "g-fl3cb"),
        (0x017c_27d4u32, "g-s30"),
        (0x017c_281cu32, "g-list"),
        (0x017c_2920u32, "g-leg920"),
        (0x017c_2960u32, "g-v18b"),
        (0x017c_2808u32, "g-byte1"),
        (0x017c_284cu32, "g-m24"),
        (0x017c_2880u32, "g-m18"),
        // r798b: the navsrv connector's mbx_289 receive (0x17cb574) silently
        // drops every message whose content is null or whose content[+0xb]
        // != 0x45 (r0=1 -> caller waits again: popped and lost).  Observe
        // the filter decisions to find where the completion pushes die.
        (0x017c_b5ecu32, "f-accept"),
        (0x017c_b5f8u32, "f-drop"),
        (0x0161_fb60u32, "g-limnull"),
        (0x0161_fb70u32, "g-lim-plusc"),
        (0x0161_fb8cu32, "g-lim-2f570"),
        (0x0162_09b4u32, "g-lim-sub"),
        (0x0162_f598u32, "g-lim-d3b0"),
        (0x0162_f590u32, "g-srv24"),
        (0x0162_f5a0u32, "g-srv-ok"),
        (0x0162_f5a4u32, "g-srv-fail"),
        (0x017c_7918u32, "g7-vfn1c"),
        (0x017c_7938u32, "g7-vfn18res"),
        (0x017c_7954u32, "g7-byte"),
        // r797b: state-8 body = per-entry scan of the list at [hmi+0x24]
        // (0x169e02c = iterator next; 0x1614d8c = per-entry state query,
        // result 2 == OK).  Log both results (r0 at these sites is exactly
        // the value consumed by the `mov r8/r5, r0` that follows).
        (0x017c_7830u32, "s8-next"),
        (0x017c_7840u32, "s8-item"),
        (0x0161_4d9cu32, "s8-found"),
        // r798: the *real* state-8 body is 0x17c78bc (jump table base
        // 0x17c73e8 + 8*4 = 0x17c7408 -> b 0x17c78bc); the scan body above
        // (0x17c77c0) is state 7 and its list-exhausted leg 0x17c7a6c
        // stores 8 only when [sl+0x50] != 1, else 9.  Body 8 polls the
        // readiness pointer [sl+0x4c] (byte != 0 -> 0x17c7b64) and falls
        // through to a [sl+0x34] check; body 9 (0x17c746c) calls 0x17c6330
        // and stores 0 (startup finished) when [sp+0x27] == 1.
        (0x017c_78ccu32, "s8b-4c"),
        (0x017c_78bcu32, "s8b-in"),
        (0x017c_7b64u32, "s8b-rdy"),
        (0x017c_747cu32, "s9-chk"),
        // r798: inside 0x17c2a64 (state-8 readiness-by-name query):
        // 0x17c2ab0 = right after `bl 0x1614dac` (found && 0x171fcf4 == 1),
        // 0x17c2ac4 = after `bl 0x1614d8c` (legacy query, 3 -> ready).
        // r4 = list, r5 = name string pointer.
        (0x017c_2ab0u32, "s8q-dac"),
        (0x017c_2ac4u32, "s8q-d8c"),
        // r798: the three virtual calls of the state-8 ready leg; r0 at
        // each is the value the following cmp consumes.  0x17c7c18 is the
        // `str #9` store itself.
        (0x017c_7ba4u32, "s8-v24"),
        (0x017c_7bf8u32, "s8-v24b"),
        (0x017c_7c10u32, "s8-v18"),
        (0x017c_7c18u32, "s8-to9"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:envigate", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 15_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 4000
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r8 = uc.reg_read(RegisterARM::R8).unwrap_or(0) as u32;
            let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            // 0x165a900 returns [[holder+4]]-deref chain: the CALLEE object
            // is [r4+4], its vtable is the word there - not [r4] (holder's
            // own vtable), which is what earlier runs printed.
            let obj = read_u32(uc, r4.wrapping_add(4));
            let vt = read_u32(uc, obj);
            // r795b: heap objects shift between builds, so pin the gate
            // word dynamically: on the first inner vfn+0x18 observation,
            // attach a write-watch to [obj+4] (the state byte 0x166c0d0
            // compares against 2) and log every writer with PC/LR.
            if label == "g-vfn18" && obj != 0 {
                static WATCHED: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if !WATCHED.swap(true, Ordering::Relaxed) {
                    let word = obj.wrapping_add(4);
                    let want = (word - 0x100) as u64;
                    let cur = read_u32(uc, word);
                    log::warn!(
                        "PROCHMI envigate gate-watch installed t={}ms word={:#x} cur={cur}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        word,
                    );
                    let _ = uc.add_mem_hook(
                        unicorn_engine::unicorn_const::HookType::MEM_WRITE,
                        want,
                        want + 0x200 - 1,
                        move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                            static WH: AtomicU32 = AtomicU32::new(0);
                            if WH.fetch_add(1, Ordering::Relaxed) < 40 {
                                let lr =
                                    uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                                let pc =
                                    uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32;
                                log::warn!(
                                    "PROCHMI envigate gate-watch WRITE t={}ms addr={address:#x} size={size} value={value:#x} pc={pc:#x} lr={lr:#x}",
                                    crate::libs::dapi::emu_t0_elapsed_ms(),
                                );
                            }
                            true
                        },
                    );
                }
            }
            // At the state-6 caller: the ready byte lives at [sp+0x27]
            // before `ldrb r3,[sp,#0x27]` executes.
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let rdy = if label == "g7-byte" {
                let mut b = [0u8; 1];
                if uc.mem_read((sp + 0x27) as u64, &mut b).is_ok() {
                    b[0]
                } else {
                    0xff
                }
            } else {
                0
            };
            if label.starts_with("s8-") {
                // r7 = sp+0x1c: the iterator's out slot ([r7] = current item
                // word, [r7+4] = iterator flag word), [sp+0x14] = list root.
                let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                let sp14 = read_u32(uc, sp.wrapping_add(0x14));
                let item = read_u32(uc, r7);
                log::warn!(
                    "PROCHMI envigate {label} t={}ms tid=[{}] res={r0:#x} item={item:#x} list={sp14:#x} r8={r8:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
            }
            if label == "f-accept" || label == "f-drop" {
                // f-accept: r3 = content pointer before the +0xb test.
                // f-drop: r3 = the class byte (class leg) or 0 (null-content
                // leg) on the r0=1 "popped and ignored" return.
                let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let (cls, svc, txn) = if r3 > 0x10000 {
                    (
                        read_u32(uc, r3.wrapping_add(0xb)) & 0xff,
                        read_u32(uc, r3.wrapping_add(0x14)) & 0xffff,
                        read_u32(uc, r3.wrapping_add(0x1c)) & 0xffff,
                    )
                } else {
                    (r3, 0xffff, 0xffff)
                };
                log::warn!(
                    "PROCHMI envigate {label} t={}ms tid=[{}] r3={r3:#x} cls={cls:#x} svc={svc:#06x} txn={txn}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
            }
            if label.starts_with("g-fl")
                || label == "g-s30"
                || label == "g-list"
                || label == "g-leg920"
                || label == "g-v18b"
                || label == "g-byte1"
                || label == "g-m24"
                || label == "g-m18"
            {
                // r6 = sl (the startup-FSM object, moved at 0x17c2770);
                // [sl+0x58..0x5c] is the member list of the 0x17c281c leg.
                let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                let l58 = read_u32(uc, r6.wrapping_add(0x58));
                let l5c = read_u32(uc, r6.wrapping_add(0x5c));
                let mem = if label == "g-m18" || label == "g-m24" {
                    let m = read_u32(uc, r4);
                    format!(
                        " m={m:#x} st4={} v18={:#x}",
                        read_u32(uc, m.wrapping_add(4)),
                        read_u32(uc, read_u32(uc, m).wrapping_add(0x18))
                    )
                } else {
                    String::new()
                };
                log::warn!(
                    "PROCHMI envigate {label} t={}ms tid=[{}] r0={r0:#x} r4={r4:#x} r5={r5:#x} r8={r8:#x} sl={r6:#x} l58={l58:#x} l5c={l5c:#x}{mem}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
                if label == "g-list" {
                    // per tick only the FIRST non-ready member's vfn+0x18
                    // aborts the leg, so name every member up front: entry,
                    // class vtable, state word, service, getter.
                    let mut p = l58;
                    let mut i = 0u32;
                    while p != l5c && i < 12 {
                        let w = read_u32(uc, p);
                        let vt = read_u32(uc, w);
                        log::warn!(
                            "PROCHMI envigate g-list-mem[{i}] t={}ms slot={p:#x} entry={w:#x} vt={vt:#x} st4={} svc={:#x} v18={:#x} v24={:#x}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            read_u32(uc, w.wrapping_add(4)),
                            read_u32(uc, w.wrapping_add(0x20)) & 0xffff,
                            read_u32(uc, vt.wrapping_add(0x18)),
                            read_u32(uc, vt.wrapping_add(0x24)),
                        );
                        i += 1;
                        p = p.wrapping_add(4);
                    }
                }
                if label == "g-m18" && r0 == 0 {
                    // Starved member: its getter says not-READY while the
                    // entry sits in state 1.  Re-fire the completion push
                    // with the recorded ordinal - each attempt is a fresh
                    // chance against tid 84's Wait on mbx_289, and a READY
                    // entry stops asking (getter != 0 from then on).
                    let m = read_u32(uc, r4);
                    if read_u32(uc, m.wrapping_add(4)) == 1 {
                        let ord = SVC6_TXN.lock().ok().and_then(|t| t.get(&m).copied());
                        static RP: AtomicU32 = AtomicU32::new(0);
                        if let Some(o) = ord {
                            if RP.fetch_add(1, Ordering::Relaxed) < 300 {
                                crate::libs::libosal_linux::message::fabricate_fi_ready_push(
                                    uc, 0x0006, o,
                                );
                            }
                        }
                    }
                }
            }
            if label == "s8b-in" || label == "s8b-4c" || label == "s8b-rdy"
                || label == "s9-chk"
                || label.starts_with("s8q-")
                || label.starts_with("s8-v")
                || label == "s8-to9"
            {
                // r798: state-8/9 body observation.  `sl` (R10) is the
                // startup-FSM object the switch at 0x17c73dc works on.
                let sl = uc.reg_read(RegisterARM::R10).unwrap_or(0) as u32;
                let state = read_u32(uc, sl.wrapping_add(0x30));
                let f34 = read_u32(uc, sl.wrapping_add(0x34));
                let p4c = read_u32(uc, sl.wrapping_add(0x4c));
                let b4c = if p4c != 0 {
                    let mut b = [0u8; 1];
                    if uc.mem_read(p4c as u64, &mut b).is_ok() {
                        b[0]
                    } else {
                        0xff
                    }
                } else {
                    0xfe
                };
                let mut b50 = [0u8; 1];
                if uc.mem_read(sl.wrapping_add(0x50) as u64, &mut b50).is_err() {
                    b50[0] = 0xff;
                }
                let b50b = b50[0];
                let rdy27 = if label == "s9-chk" {
                    let mut b = [0u8; 1];
                    if uc.mem_read((sp + 0x27) as u64, &mut b).is_ok() {
                        b[0]
                    } else {
                        0xff
                    }
                } else {
                    0xfe
                };
                // r798: the readiness name the state-8 query searches for
                // (r5 inside 0x17c2a64) plus the [sl+0x48] word.
                let mut nm = String::new();
                for p in [
                    uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32,
                    read_u32(uc, sl.wrapping_add(0x48)),
                    p4c,
                ] {
                    if p == 0 {
                        continue;
                    }
                    let mut buf = [0u8; 24];
                    if let Ok(()) = uc.mem_read(p as u64, &mut buf) {
                        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
                        if end >= 3 && buf[..end].iter().all(|c| c.is_ascii_graphic()) {
                            nm.push_str(&format!(
                                " \"{}\"",
                                String::from_utf8_lossy(&buf[..end])
                            ));
                        }
                    }
                }
                let w48 = read_u32(uc, sl.wrapping_add(0x48));
                // The ready-leg virtual calls target the object at
                // [sl+0x74] (0x17c2430) and its +0x78 sub-object.
                let f74 = read_u32(uc, sl.wrapping_add(0x74));
                let vt74 = read_u32(uc, f74);
                let s78 = read_u32(uc, f74.wrapping_add(0x78));
                let vt78 = read_u32(uc, s78);
                let v18t = read_u32(uc, vt74.wrapping_add(0x18));
                let v24t = read_u32(uc, vt74.wrapping_add(0x24));
                // r798: the entry words of the [sl+0x74] gate-class object:
                // +0x04 is the state the 0x166c0d0 getter compares to 2,
                // +0x08..+0x2c hold the registration identity.
                let mut ew = String::new();
                for i in (0..0x40u32).step_by(4) {
                    ew.push_str(&format!(
                        " +{i:#x}={:#x}",
                        read_u32(uc, f74.wrapping_add(i))
                    ));
                }
                log::warn!(
                    "PROCHMI envigate {label} t={}ms tid=[{}] sl={sl:#x} state={state} f34={f34:#x} p4c={p4c:#x} b4c={b4c} b50={b50b} w48={w48:#x} r0={r0:#x} rdy27={rdy27} f74={f74:#x} vt74={vt74:#x} s78={s78:#x} vt78={vt78:#x} v18t={v18t:#x} v24t={v24t:#x}{ew}{nm}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
                if label.starts_with("s8q-") {
                    // Inside 0x17c2a64: r4 = readiness list, r5 = the name
                    // descriptor from sl+0x48.  Dump both so the pending
                    // service name state 8 waits for is on the log.
                    let r4n = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    let r5n = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                    let mut nb = [0u8; 32];
                    let _ = uc.mem_read(r5n as u64, &mut nb);
                    let s: String = nb
                        .iter()
                        .take_while(|b| **b >= 0x20 && **b < 0x7f)
                        .map(|b| *b as char)
                        .collect();
                    log::warn!(
                        "PROCHMI envigate {label}-args t={}ms tid=[{}] list={r4n:#x} name={r5n:#x} text={s:?} w0={:02x?}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        &nb[..12],
                    );
                }
                if (label == "s8-v18" || label == "s8-v24") && r0 == 0 {
                    // State-8 starved: the FSM body re-registers its
                    // readiness entry ([sl+0x74]) every tick and the
                    // completion push for the latest ordinal keeps getting
                    // swallowed by tid 84's own enConnect Wait (the forwarder
                    // only sees a fraction - f261: 209 delivered, none of the
                    // f74 ordinals dispatched).  Same hammer as g-m18: with
                    // the ordinal the conf-store page watch just recorded,
                    // re-fire the completion push until one lands on the
                    // pump consumer side.
                    let f74 = read_u32(uc, sl.wrapping_add(0x74));
                    let ord = SVC6_TXN.lock().ok().and_then(|t| t.get(&f74).copied());
                    // r799d: re-drive REMOVED.  The responder now answers
                    // every mbx_7 regid-3 request 1:1 as a pump event, and
                    // this per-tick hammer (budget 400) duplicated that:
                    // f281 spawned guest-post threads tid 100..481, ~380
                    // events for a drain that consumes ~25 - the queue
                    // overflowed, tid-65's loop gave up and tid-84 exited.
                    // Ordinal presence stays observable via the log line.
                    let _ = ord;
                }
                // Whoever sets b4c/b50 organically is the producer we are
                // missing; watch the whole FSM object page for writes into
                // its header fields (offsets 0x2c..0x5c).
                if label == "s8b-in" && sl != 0 {
                    static SLWATCH: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    if !SLWATCH.swap(true, Ordering::Relaxed) {
                        let want = sl & !0xfff;
                        log::warn!(
                            "PROCHMI envigate fsm-watch installed t={}ms sl={sl:#x}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                        );
                        let _ = uc.add_mem_hook(
                            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
                            want as u64,
                            want as u64 + 0xfff,
                            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                                static WH: AtomicU32 = AtomicU32::new(0);
                                let off = address.wrapping_sub((sl & !0xfff) as u64) as u32;
                                if (0x2c..0x5c).contains(&off)
                                    && WH.fetch_add(1, Ordering::Relaxed) < 40
                                {
                                    let lr = uc
                                        .reg_read(RegisterARM::R14)
                                        .unwrap_or(0) as u32;
                                    let pc = uc
                                        .reg_read(RegisterARM::R15)
                                        .unwrap_or(0) as u32;
                                    log::warn!(
                                        "PROCHMI fsm-watch WRITE t={}ms sl+{off:#x} size={size} value={value:#x} pc={pc:#x} lr={lr:#x}",
                                        crate::libs::dapi::emu_t0_elapsed_ms(),
                                    );
                                }
                                true
                            },
                        );
                    }
                }
            }
            if label == "s8-found" && r0 != 0 {
                // right after `bl 0x1613ac4` in 0x1614d8c: r0 = the found
                // connection object whose +0x88 state 0x171fd7c switches on
                // (2 is served from +0x80 in states 4/7).  Dump its vtable,
                // the state words and the 3 entries of the +0x98/+0x9c
                // vector (8-byte elements), resolving any ASCII names.
                let vt = read_u32(uc, r0);
                let (b0, e0) = (read_u32(uc, r0.wrapping_add(0x98)), read_u32(uc, r0.wrapping_add(0x9c)));
                let mut entries = String::new();
                let mut a = b0;
                while a < e0 && a.wrapping_sub(b0) < 0x60 {
                    let w0 = read_u32(uc, a);
                    let w1 = read_u32(uc, a.wrapping_add(4));
                    let name = |p: u32| -> String {
                        if !(0x8000..0xf000_0000).contains(&p) {
                            return String::new();
                        }
                        let mut buf = [0u8; 24];
                        match uc.mem_read(p as u64, &mut buf) {
                            Ok(()) => {
                                let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
                                let s = &buf[..end];
                                if s.len() >= 3 && s.iter().all(|c| c.is_ascii_graphic()) {
                                    String::from_utf8_lossy(s).into_owned()
                                } else {
                                    String::new()
                                }
                            }
                            Err(_) => String::new(),
                        }
                    };
                    let n0 = name(w0);
                    let n1 = name(w1);
                    let s0 = if n0.is_empty() { String::new() } else { format!(" \"{n0}\"") };
                    let s1 = if n1.is_empty() { String::new() } else { format!(" \"{n1}\"") };
                    entries.push_str(&format!(
                        "[{a:#x}]={w0:#x}{s0}{w1:#x}{s1} "
                    ));
                    a += 8;
                }
                log::warn!(
                    "PROCHMI envigate {label} t={}ms tid=[{}] obj={r0:#x} vt={vt:#x} +0x80={:#x} +0x88={:#x} vec=[{b0:#x},{e0:#x}] {entries}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    read_u32(uc, r0.wrapping_add(0x80)),
                    read_u32(uc, r0.wrapping_add(0x88)),
                );
                // r797b: this is the "/dev/cryptcard" readiness object the
                // state-8 body polls; whoever advances +0x88 (0 -> 3 with
                // +0x80 = 1 makes 0x171fd7c return READY) unblocks the
                // startup FSM.  Watch both state words live.
                static CARDWATCH: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if !CARDWATCH.swap(true, Ordering::Relaxed) {
                    let want = r0.wrapping_add(0x80) & !0xff;
                    let _ = uc.add_mem_hook(
                        unicorn_engine::unicorn_const::HookType::MEM_WRITE,
                        want as u64,
                        want as u64 + 0x1ff,
                        move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                            static WH: AtomicU32 = AtomicU32::new(0);
                            if WH.fetch_add(1, Ordering::Relaxed) < 30 {
                                let lr =
                                    uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                                let pc =
                                    uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32;
                                log::warn!(
                                    "PROCHMI cryptcard-state WRITE t={}ms addr={address:#x} size={size} value={value:#x} pc={pc:#x} lr={lr:#x}",
                                    crate::libs::dapi::emu_t0_elapsed_ms(),
                                );
                            }
                            true
                        },
                    );
                }
            }
            // r796: LIM verdict chain - 0x162d3b0 = [[x+0xac]+0x4c],
            // 0x162d3a0 = [[x+0xac]+0x54].  At the g-srv/g-lim sites r0 is
            // the hmi singleton; dump its sub-object fields.
            let sub = read_u32(uc, r0.wrapping_add(0xac));
            log::warn!(
                "PROCHMI envigate {label} t={}ms tid=[{}] r0={r0:#x} r8={r8:#x} rdy={rdy} holder={r4:#x} obj={obj:#x} vt={vt:#x} v18={:#x} v1c={:#x} v24={:#x} hmi+0xc={:#x} hmi+0x24={:#x} sub={sub:#x} s4c={:#x} s54={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                read_u32(uc, vt.wrapping_add(0x18)),
                read_u32(uc, vt.wrapping_add(0x1c)),
                read_u32(uc, vt.wrapping_add(0x24)),
                read_u32(uc, r0.wrapping_add(0xc)),
                read_u32(uc, r0.wrapping_add(0x24)),
                read_u32(uc, sub.wrapping_add(0x4c)),
                read_u32(uc, sub.wrapping_add(0x54)),
            );
            0
        });
    }
    // r795b: the two stale fixed-address page watches on the gate word were
    // replaced by the dynamic gate-watch installed inside the envigate
    // probe above (heap addresses shift between builds, static addresses
    // went stale).
    // r796: trap vOnUpdateEnaviServerState (0x15f48a4: `str r1,[r0,#4]` +
    // singleton vfn+0 notify).  If the READY(2) call for the gate object
    // never appears here, the producer of that transition is elsewhere.
    {
        let address = base_address + 0x015f_48a4u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:srvsetter", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 60 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let mut bt = String::new();
            for i in 0..10u32 {
                let w = read_u32(uc, sp.wrapping_add(i * 4));
                if (0x00fb_7080..0x023b_eb1b).contains(&w) {
                    bt.push_str(&format!(" {w:#x}"));
                }
            }
            log::warn!(
                "PROCHMI srvsetter t={}ms tid=[{}] obj={r0:#x} state={r1:#x} lr={lr:#x} bt:{bt}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r795b: the FINAVI singleton globals.  0x15e1954 (vSvcReady, run 12x)
    // aborts at `finavi=[global]` null-guard - both singleton slots
    // (0x2a1ce78 via the 0x14b9148 getter, 0x2628d0c for 0x15e1954 itself)
    // are 0, so the server-READY propagation that sets the state-6 gate
    // word ([0x3a23f5c]==2) dies.  Watch both slots: whoever must store the
    // FINAVI object will show up with PC; zero hits = the creating path
    // never ran.
    {
        for (lo, hi, want, tag) in [
            (0x02a1_c000u64, 0x02a1_cfffu64, 0x02a1_ce78u32, "getter-slot"),
            (0x02a1_b000u64, 0x02a1_bfffu64, 0x02a1_be8cu32, "svc-slot"),
            // r796b: helper 0x1719b4c reads [[0x2a139f8]]; its null-ness at
            // entry-registration time picks the pump list (entry+0x14 vs
            // entry+0x28): only the +0x28-list entries get re-ticked by the
            // event pump.  The 0x0a entry landed in the +0x14 list, so this
            // global was non-null when it registered - catch its writer.
            (0x02a1_3000u64, 0x02a1_3fffu64, 0x02a1_39f8u32, "list-sel"),
        ] {
            unicorn
                .add_mem_hook(
                    unicorn_engine::unicorn_const::HookType::MEM_WRITE,
                    lo,
                    hi,
                    move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                        static HITS: AtomicU32 = AtomicU32::new(0);
                        if address as u32 == want
                            && HITS.fetch_add(1, Ordering::Relaxed) < 8
                        {
                            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                            log::warn!(
                                "PROCHMI finavi-global WRITE {tag} t={}ms value={value:#x} size={size} lr={lr:#x}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                            );
                        }
                        true
                    },
                )
                .unwrap();
        }
    }
    // r795b: trap the READY setter (0x166c104: `str #2,[r0,#4]`) - it lives
    // at vfn+0x34 of ~200 state-proxy classes.  Log every object that ever
    // reaches READY and the caller, to find which trigger is missing for
    // the inner gate object 0x3a23f58 (vfn+0x18 on it gates state 6 -> 7).
    {
        let address = base_address + 0x0166_c104u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:ready-setter", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 60 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            let vt = read_u32(uc, r0);
            log::warn!(
                "PROCHMI ready-setter t={}ms tid=[{}] obj={r0:#x} vt={vt:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r791k: vMainLoop's wait outcome.  0x1c7f34c: r0 = bMessageWait result
    // (1 = message delivered); 0x1c7f2f0: failure-path loop check;
    // 0x1c7f38c: the loop-exit tail (active or in-object gone).
    for (offset, label) in [
        (0x01c7_f34cu32, "wait-res"),
        (0x01c7_f2f0u32, "loopcheck"),
        (0x01c7_f38cu32, "loop-exit"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:ifm-loop", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 20_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 12
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            let active = read_u32(uc, r4.wrapping_add(0x44)) & 0xff;
            let inbox = read_u32(uc, r4.wrapping_add(0x3c));
            log::warn!(
                "PROCHMI ifm-{label} t={}ms tid=[{}] r0={r0:#x} sm={r4:#x} active={active:#x} inbox={inbox:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r791j: the ServiceManager pump's dispatch of our CL answers.
    // bDispatchMessage (0x1c7f0cc): r1 = base message, [r1+4] = content,
    // class byte at content+0xb.  Then the 0x43/0x44 handlers.
    {
        let address = base_address + 0x01c7_f0ccu32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:smdisp", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 30 {
                return 0;
            }
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let content = read_u32(uc, r1.wrapping_add(4));
            let cls = read_u32(uc, content.wrapping_add(0xb)) & 0xff;
            let src = read_u32(uc, content) & 0xffff;
            let svc = read_u32(uc, content.wrapping_add(0x14)) & 0xffff;
            log::warn!(
                "PROCHMI sm-dispatch t={}ms tid=[{}] msg={r1:#x} content={content:#x} class={cls:#x} src={src:#x} svc={svc:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    for (offset, label) in [
        (0x01c7_edacu32, "svc-regconf"),
        (0x01c7_ef34u32, "svc-status"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:smhandlers", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 30 {
                return 0;
            }
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let content = read_u32(uc, r1.wrapping_add(4));
            let svc = read_u32(uc, content.wrapping_add(0x14)) & 0xffff;
            log::warn!(
                "PROCHMI sm-{label} t={}ms tid=[{}] svc={svc:#x} content={content:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r791i: tclBaseThread::vMainLoopHelper (0x1c80674) dispatches the
    // thread's virtual vMainLoop (*obj+0xc).  Log the thread object and the
    // resolved main-loop target (the NAV_IFM pump body).
    {
        let address = base_address + 0x01c8_0698u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:ifm-main", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 4 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            let vt = read_u32(uc, r0);
            let target = read_u32(uc, vt.wrapping_add(0xc));
            log::warn!(
                "PROCHMI ifm-mainloop t={}ms tid=[{}] obj={r0:#x} vt={vt:#x} main={target:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r791h: ProxyManager::bSetState (0x1c82814 6-arg, 0x1c8292c 8-arg) and
    // ProxyOutCCA::bSetState (0x1c830c4) - fed by the ServiceStatus(0x44)
    // dispatch.  Log args to learn whether our answers reach the state
    // machine and with which state value.
    for (offset, label) in [
        (0x01c8_2814u32, "pm-setstate6"),
        (0x01c8_292cu32, "pm-setstate8"),
        (0x01c8_30c4u32, "cca-setstate"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:setstate", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 20_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 80
            {
                return 0;
            }
            let regs: Vec<u32> = [
                RegisterARM::R0,
                RegisterARM::R1,
                RegisterARM::R2,
                RegisterARM::R3,
            ]
            .iter()
            .map(|r| uc.reg_read(*r).unwrap_or(0) as u32)
            .collect();
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI {label} t={}ms tid=[{}] r0={:#x} r1={:#x} r2={:#x} r3={:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                regs[0],
                regs[1],
                regs[2],
                regs[3],
            );
            0
        });
    }
    // r791g2: did the map search find a proxy (bGetCurrentProxyInfo entry
    // 0x1c82ff4: r0 = proxy, [proxy+0x1c] = CS that gates the info copy)?
    {
        let address = base_address + 0x01c8_2ff4u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:curproxyinfo", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 25_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 12
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let cs = read_u32(uc, r0.wrapping_add(0x1c));
            let svc = read_u32(uc, r0.wrapping_add(8)) & 0xffff;
            log::warn!(
                "PROCHMI curproxyinfo t={}ms tid=[{}] proxy={r0:#x} svc={svc:#x} cs={cs:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r791f: the getService pointer chain hops.  0x1c7cf8c: r0 = the
    // navif_tclIFMediator, [r0+4] = the ServiceManager; 0x1c7ecc0: r0 = the
    // ServiceManager, [r0+0x30] = the ProxyManager; 0x1c82720: r0 = the
    // ProxyManager (reached only when no hop is null).
    for (offset, label, field) in [
        (0x01c7_cf8cu32, "hop-mediator", 4u32),
        (0x01c7_ecc0u32, "hop-smgr", 0x30u32),
        (0x01c8_2720u32, "hop-pmgr", 0u32),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:getsvc-chain", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 25_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 12
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let next = if field == 0 { 0 } else { read_u32(uc, r0.wrapping_add(field)) };
            let (map_head, cs) = if label == "hop-pmgr" {
                (read_u32(uc, r0.wrapping_add(8)), read_u32(uc, r0.wrapping_add(0x18)))
            } else {
                (0, 0)
            };
            log::warn!(
                "PROCHMI getsvc-{label} t={}ms tid=[{}] obj={r0:#x} next={next:#x} map={map_head:#x} cs={cs:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r791e: navif_tclCriticalSection::bEnter (0x1c7b6e0) - the CS gate used
    // by ProxyManager::bGetServiceInfo / bGetCurrentProxyInfo.  Logs the
    // semaphore handle (from [cs]) and the raw OSAL_s32SemaphoreWait return
    // (0 = entered; bEnter maps 1-x, >1 -> 0).
    for (offset, label) in [
        (0x01c7_b6ecu32, "benter-enter"),
        (0x01c7_b6f0u32, "benter-ret"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:benter", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 15_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 24
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI {label} t={}ms tid=[{}] r0={r0:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r791d: the post-registration availability gate.  bRunWhenRunning's
    // case-body walks every required service through
    // enavi_tclCCAStatusSubscriber::bIsServiceAvailable (0x17ba0a0:
    // ([sub+8] vtable +0x1c)(out_byte, svc)); a zero result for a required
    // service aborts before the state-5 transition.  Log the subscriber,
    // the CL status object behind it, its vtable and the +0x1c target.
    {
        let address = base_address + 0x017b_a0a0u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:isavail", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 15_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 24
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let cache = read_u32(uc, r0.wrapping_add(8));
            let vt = read_u32(uc, cache);
            let vfn = read_u32(uc, vt.wrapping_add(0x1c));
            log::warn!(
                "PROCHMI isavail-enter t={}ms tid=[{}] sub={r0:#x} svc={r1:#x} cache={cache:#x} vt={vt:#x} vfn14={vfn:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    {
        let address = base_address + 0x017b_a0dcu32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:isavail-ret", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 15_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 24
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
            let outb = read_u32(uc, sp.wrapping_add(0x10)) & 0xff;
            log::warn!(
                "PROCHMI isavail-ret t={}ms tid=[{}] r0={r0:#x} outbyte={outb:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r790f: ProxyOutCCA::enConnect service-resolution outcomes.  After
    // scd_bGetAppIdFromServiceId (call at 0x1c836d0, out1=[sp+0x34] count,
    // out2=[sp+0x20] server app id) and after scd_OpenQueue (call at
    // 0x1c835d0, r0 = queue handle): log what the client learned and which
    // queue handle it got, plus the app id it asked about ([r5+0xc]).
    for (offset, label) in [
        (0x01c8_36d4u32, "scd-lookup"),
        (0x01c8_35d4u32, "scd-openq"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:enconnect-scd", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 20_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 12
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
            let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
            let svc = read_u32(uc, r5.wrapping_add(0xc)) & 0xffff;
            let cnt = read_u32(uc, sp.wrapping_add(0x34));
            let app = read_u32(uc, sp.wrapping_add(0x20)) & 0xffff;
            // scd_bGetAppIdFromServiceId globals: static-mode flag (byte, 0 =
            // walk the registry map) and the SCD queue handle (-1 forces the
            // early-fail branch that skips the map entirely).
            let mode = read_u32(uc, 0x02ab_44b9) & 0xff;
            let qh = read_u32(uc, 0x02a1_e41c);
            log::warn!(
                "PROCHMI enconnect-{label} t={}ms tid=[{}] proxy={r5:#x} svc={svc:#x} r0={r0:#x} cnt={cnt:#x} app={app:#x} mode={mode:#x} scdq={qh:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r790e: amt_tclMappableMessage::bAllocateMessage (0xfbeaf4): the
    // ServiceRegister message allocation goes through prochmi import thunks
    // (MessageCreate via 0xfb64e8, ContentGet via 0xfb68fc).  Log both result
    // sites and the runtime-resolved GOT slots so we can see which step
    // fails (nonzero create return, or NULL content) and where the thunks
    // actually land.
    {
        let address = base_address + 0x00fb_eb18u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:allocmsg", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 20_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 12
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            let h = read_u32(uc, r4);
            let c = read_u32(uc, r4.wrapping_add(4));
            let slot = read_u32(uc, 0x0775_68a0);
            log::warn!(
                "PROCHMI allocmsg-create t={}ms tid=[{}] r0={r0:#x} handle={h:#x} content={c:#x} got={slot:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    {
        let address = base_address + 0x00fb_eb34u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:allocmsg", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if crate::libs::dapi::emu_t0_elapsed_ms() < 20_000
                || HITS.fetch_add(1, Ordering::Relaxed) >= 12
            {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let slot = read_u32(uc, 0x0776_9104);
            log::warn!(
                "PROCHMI allocmsg-content t={}ms tid=[{}] r0(content)={r0:#x} got={slot:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    // r790d: ProxyOutCCA::enConnect (0x1c83520) returns 0x8000000b either from
    // the entry guard ([proxy+0x1c] == 0, i.e. the critical section object was
    // never created -> pCreate/bEnter failure) or from bEnter != 1.  Log the
    // proxy fields at entry, plus which of the two exits is taken.
    {
        let address = base_address + 0x01c8_3520u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:enconnect", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 12 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            let cs = read_u32(uc, r0.wrapping_add(0x1c));
            let q = read_u32(uc, r0.wrapping_add(4));
            let svc = read_u32(uc, r0.wrapping_add(2)) & 0xffff;
            let app = read_u32(uc, r0.wrapping_add(0xa)) & 0xffff;
            log::warn!(
                "PROCHMI enconnect-entry t={}ms tid=[{}] proxy={r0:#x} svc={svc:#x} +0x1c(cs)={cs:#x} q={q:#x} app={app:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    for (offset, label) in [
        (0x01c8_356cu32, "enconnect-fail"),
        (0x01c8_357cu32, "enconnect-cont"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:enconnect-exit", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 24 {
                return 0;
            }
            let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
            let cs = read_u32(uc, r5.wrapping_add(0x1c));
            let handle = if cs != 0 && cs < 0x8000_0000 {
                read_u32(uc, cs)
            } else {
                0xdead_beef
            };
            log::warn!(
                "PROCHMI enconnect-{label} t={}ms tid=[{}] proxy={r5:#x} cs={cs:#x} semh={handle:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    for (offset, label) in [
        (0x017b_980cu32, "attach-vfn0c"),
        (0x017b_9840u32, "cca-required"),
        (0x017b_9878u32, "reg-vfn14"),
    ] {
        let address = base_address + offset - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:cmdsub-res", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 40 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
            let sl = uc.reg_read(RegisterARM::SL).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI cmdsub-res {label} t={}ms tid=[{}] r0={r0:#x} r4={r4:#x} svc={r5:#x} tgt={sl:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }

    // r788c: enavi NavIF CCA input path.  STARTUP case 5 (bRunWhenRunning)
    // gates the enavi server on enavi_tclCCAStatusSubscriber::bIsServiceAvailable
    // for the seven registry-required services (0x0a/0x2c/0x1f/0x1e/0x18/6/0x152);
    // their availability lands through enavi_tclRouteCCA: enActivate creates
    // the route (poRouteCCACreate(1)), bCheckCCAInput -> bMessageRead polls
    // the route's own queue ([route+8]) for class-'E' (0x45 ServiceData)
    // messages.  Log the bring-up (enActivate ran? route created? which queue
    // handle is polled, with what result) to see where the CCA feed is missing.
    {
        let address = base_address + 0x017c_1bf8u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:navif-activate", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 10 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI navif-activate t={}ms tid=[{}] navif={r0:#x} svcmgr={r1:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    {
        let address = base_address + 0x017c_b9f4u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:routecca-create", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 10 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI routecca-create t={}ms tid=[{}] route={r0:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }
    {
        // bMessageRead: r0=route; log the queue handle it polls and the
        // outcome (return site 0x17cb5f0: r0=result after the queue wait).
        let address = base_address + 0x017c_b574u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:routecca-read", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 30 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let q = read_u32(uc, r0.wrapping_add(8));
            log::warn!(
                "PROCHMI routecca-read t={}ms tid=[{}] route={r0:#x} queue={q:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }

    // r788: registration-census comparison: KEYBRD's CCA registration hooks
    // vs Navi's (probed above).  If KEYBRD got vSetRegisterID/
    // vServiceStateChanged but Navi never did, the FI-proxy registration
    // path works but the NAVI service config/gate is missing; if neither
    // did, no FI client registers at all in this emulator.  vNewAppState is
    // the fw state-machine edge that normally triggers FI registration.
    {
        for (offset, name) in [
            (0x013e_726cu32, "keybrd-setregid"),
            (0x013e_7124u32, "keybrd-svcstate"),
            (0x00fc_6574u32, "hmicca-newappstate"),
        ] {
            let address = base_address + offset - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:regcensus", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 300 {
                    return 0;
                }
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI {name} t={}ms tid=[{}] r1={r1:#x} r2={r2:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
                0
            });
        }
    }

    // r788: FI/CL singleton instantiation census.  pclGetInstance is the
    // only way these lazy singletons come to life; comparing Navi against
    // healthy siblings (KEYBRD, SPM) and the clHSA_* family (built per
    // T_EN_HSA_ID at component bring-up) shows whether the NAVI client
    // classes were ever even referenced.  First hit names the caller.
    {
        for (offset, name) in [
            (0x014b_9148u32, "inst-navi"),
            (0x013e_7000u32, "inst-keybrd"),
            (0x0146_9a08u32, "inst-spm"),
            (0x0188_44dcu32, "inst-hsa-nav-constr"),
            (0x0134_e1d8u32, "inst-hsa-evtadapter"),
        ] {
            let address = base_address + offset - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:inst", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                let n = HITS.fetch_add(1, Ordering::Relaxed);
                if n >= 60 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI {name} t={}ms tid=[{}] a0={r0:#x} hsaid={r1} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
                0
            });
        }
    }

    // r788: clFi_Navi - the HMI-side NAVI FI proxy (CCA-registered client
    // whose incoming FI messages become datapool writes -> observers -> the
    // dest-entry mediator).  If vSetRegisterID never runs, the proxy is not
    // even on the CCA directory, so PROCNAV answers can never be routed to
    // it; bExecuteMessage hits would prove FI traffic arriving.
    {
        for (offset, name) in [
            (0x014b_9598u32, "navifi-exec-smsg"),
            (0x014b_9fe0u32, "navifi-exec"),
            (0x014b_b224u32, "navifi-setregid"),
            (0x014b_b270u32, "navifi-svcstate"),
        ] {
            let address = base_address + offset - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:navifi", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 600 {
                    return 0;
                }
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI {name} t={}ms tid=[{}] r1={r1:#x} r2={r2:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
                0
            });
        }
    }

    // r788: timer arming/firing for the mediator-sync waits, plus the
    // HSA_Navigation wait-sync-state getter's state fields (the dialog's
    // count source: content function 0x206 -> GUI_FUNCTION_WaitSyncState_
    // 22081 -> clHSA_Navigation::ulwWaitSyncState; [obj+0x68] waiting flag,
    // [obj+0x80] wait-event mask, [obj+0x88] request mask, all zero = "no
    // pending wait" -> returns 1).  No timer fire AND no data update =>
    // bCheckState never re-runs and the keyboard transition can never fire.
    {
        for (offset, name) in [
            (0x0137_5670u32, "mediator-timerarm"),
            (0x0137_55d8u32, "mediator-timerfire"),
        ] {
            let address = base_address + offset - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:mediator-timer", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 300 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI {name} t={}ms this={r0:#x} flags={:#x} tmr={:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    read_u32(uc, r0 + 0x28),
                    read_u32(uc, r0 + 0x24),
                );
                0
            });
        }
        let address = base_address + 0x0189_28e0u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:hsa-waitstate", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 400 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI hsa-waitstate t={}ms hsa={r0:#x} mask={r1:#x} waiting={} waitmask={:#x} reqmask={:#x} proxy={:#x} pvt={:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                read_u32(uc, r0 + 0x68) & 0xff,
                read_u32(uc, r0 + 0x80),
                read_u32(uc, r0 + 0x88),
                {
                    let p = read_u32(uc, r0 + 0xc);
                    p
                },
                read_u32(uc, read_u32(uc, r0 + 0xc)),
            );
            0
        });
    }

    // r788: who arms/clrs the HSA_Navigation wait events.  [hsa+0x68]=1 was
    // already set when the dialog's getter ran - the setter's lr names the
    // code that issues the dest-entry request, and its request-sibling is
    // what must reach PROCNAV.  (vSetWaitEventId 0x18a3394, vSetWaitEv_
    // ReqPrepareDestEnvMap 0x1879af8, vClrWaitEv_ReqPrepareWeatherMap
    // 0x1879b20.)
    {
        for (offset, name) in [
            (0x018a_3394u32, "hsa-setevent"),
            (0x0187_9af8u32, "hsa-set-destenv"),
            (0x0187_9b88u32, "hsa-set-syncmap"),
        ] {
            let address = base_address + offset - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:hsa-setwait", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 300 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI {name} t={}ms hsa={r0:#x} a1={r1:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                );
                0
            });
        }
        // Who writes the HSA_Navigation waiting flag ([obj+0x68]) - it reads
        // 1 when the dialog evaluates, yet no setter function ran: some path
        // writes the field directly.  (Object 0x3908d98 = heap-deterministic
        // clHSA_Navigation instance.)
        {
            use unicorn_engine::unicorn_const::HookType;
            let res = unicorn.add_mem_hook(
                HookType::MEM_WRITE,
                0x0390_8d98u64 + 0x68,
                0x6b,
                move |_uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                    static HITS: AtomicU32 = AtomicU32::new(0);
                    if address >= 0x0390_8da0u64
                        && address < 0x0390_8da4u64
                        && HITS.fetch_add(1, Ordering::Relaxed) < 40
                    {
                        let pc = _uc.reg_read(RegisterARM::R15).unwrap_or(0) as u32;
                        let lr = _uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                        log::warn!(
                            "PROCHMI hsa-flag <- {value:#x} (size {size}) pc={pc:#x} lr={lr:#x}",
                        );
                    }
                    true
                },
            );
            if res.is_err() {
                log::debug!("prochmi: hsa-flag watch not installed");
            }
        }
    }

    // r788: datapool/GUI registration pairing.  Every mediator-sync state
    // that arms registers its receiver in GUI_ObserverList via vInitialize;
    // the DATA side registers a datapool listener (bdp_Register) for the
    // element.  Zero bdp_Register(0x8076000) => the CL pump for the
    // dest-entry element was never started anywhere in the system.
    {
        let address = base_address + 0x0137_5150u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:mediator-init", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 300 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI mediator-init t={}ms this={r0:#x} flags={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                read_u32(uc, r0 + 0x28),
            );
            0
        });
        let address = base_address + 0x0138_2c6cu32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:bdp-register", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 2000 {
                return 0;
            }
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::warn!(
                "PROCHMI bdp-register t={}ms id={r1:#x} lr={lr:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
            );
            0
        });
    }

    // r788: clHSAEventAdapter::vsendDataUpdateEvent (0x134dfe0) - the single
    // door through which CL HSA events become GUI observer notifications
    // (all 13 boot notifies ran through it).  A backtrace (stack scan for
    // return addresses inside prochmi text) names the CL message handler
    // that owns the HSA event dispatch; a nav-service event for the dest-
    // entry wait must eventually arrive on exactly this call path.
    {
        let address = base_address + 0x0134_dfe0u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:hsa-event", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 400 {
                return 0;
            }
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            let mut bt = String::new();
            for i in 0..10u32 {
                let w = read_u32(uc, sp.wrapping_add(i * 4));
                if (0x00fb_7080..0x023b_eb1b).contains(&w) {
                    bt.push_str(&format!(" {w:#x}"));
                }
            }
            log::warn!(
                "PROCHMI hsa-event t={}ms tid=[{}] ev={r1:#x} lr={lr:#x} bt:{bt}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
            );
            0
        });
    }

    // r788: GUI_ObserverList::vOnDataUpdate(contentId) (0x13662bc) - THE
    // notify entry that feeds every mediator-sync wait (the mediator waits
    // register their receiver here via vInitialize for the content id in
    // their ROM row).  Logging id + lr names the provider module that
    // pushes data for each mediator family; a missing 0x90000206 push is
    // the exact silence that keeps the keyboard page from opening.
    {
        let address = base_address + 0x0136_62bcu32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:obs-notify", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 4000 {
                return 0;
            }
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            static SEEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<(u32, u32), u32>>> =
                std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
            let mut show = true;
            if let Ok(mut m) = SEEN.lock() {
                let c = m.entry((r1, lr)).or_insert(0);
                *c += 1;
                show = *c <= 2;
            }
            if show {
                log::warn!(
                    "PROCHMI obs-notify t={}ms tid=[{}] id={r1:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
            }
            0
        });
    }

    // r788: GUI_StatePseudoMediatorSync::vCheckFire (0x13754b0) - the single
    // funnel of ALL re-evaluation triggers (vOnDataUpdate tail-calls here,
    // the timer callback tail-calls here, activate calls it).  The lr says
    // which source re-fired the wait; a data-update lr = the CL feed
    // arriving.  Dedup per (this, lr).
    {
        let address = base_address + 0x0137_54b0u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:mediator-fire", move |uc: &mut Unicorn<'_, Context>| {
            if !nav_trace_enabled() {
                return 0;
            }
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 4000 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            static SEEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<(u32, u32), u32>>> =
                std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
            let mut show = true;
            if let Ok(mut m) = SEEN.lock() {
                let c = m.entry((r0, lr)).or_insert(0);
                *c += 1;
                show = *c <= 3;
            }
            if show {
                log::warn!(
                    "PROCHMI mediator-fire t={}ms this={r0:#x} a1={r1:#x} flags={:#x} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    read_u32(uc, r0 + 0x28),
                );
            }
            0
        });
    }

    // r787: GUI_StatePseudoMediatorSync::bCheckState (0x137586c) - polled by
    // the state's own timer; the return value says whether the bound
    // mediator's data count matches the wait polarity.  Dedup per (this,
    // result) so the periodic poll does not flood.
    {
        let address = base_address + 0x0137_586cu32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:mediator-check", move |uc: &mut Unicorn<'_, Context>| {
            if !nav_trace_enabled() {
                return 0;
            }
            static HITS: AtomicU32 = AtomicU32::new(0);
            if HITS.fetch_add(1, Ordering::Relaxed) >= 4000 {
                return 0;
            }
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let desc = read_u32(uc, r0 + 0x20);
            let (dmc, handlers, vfn) = if desc != 0 && desc < 0x0f00_0000 {
                let dmc = read_u32(uc, desc + 0xc);
                // Same handler lookup bCheckState itself does: kind-indexed
                // mediator registry, handler vtable slot +4 = count query.
                let handlers = read_u32(
                    uc,
                    read_u32(uc, 0x02a1_3300u32)
                        .wrapping_add((dmc >> 28) * 4),
                );
                let vfn = read_u32(uc, read_u32(uc, handlers).wrapping_add(4));
                (dmc, handlers, vfn)
            } else {
                (0, 0, 0)
            };
            static SEEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<(u32, u32), u32>>> =
                std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
            let mut show = true;
            if let Ok(mut m) = SEEN.lock() {
                let c = m.entry((r0, vfn)).or_insert(0);
                *c += 1;
                show = *c <= 2;
            }
            if show {
                // Identify the kind-9 handler object (the mediator content
                // registry for this family): object head + its three traced
                // vtable targets' static module location is what we need.
                let hv = if handlers != 0 && handlers < 0x0f00_0000 {
                    read_u32(uc, handlers)
                } else {
                    0
                };
                log::warn!(
                    "PROCHMI mediator-check t={}ms this={r0:#x} flags={:#x} mediator={dmc:#x} handler={handlers:#x} vt={hv:#x} countfn={vfn:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    read_u32(uc, r0 + 0x28),
                );
            }
            0
        });
    }

    // r786: who flips bIsDynamicStateMachineActivated (SuperSM 0x39e1cb0
    // byte +0x18)?  vEnter's gate depends on it; at the New Address tap it is
    // 0 and stays 0, so no dynamic popup state can be entered.
    {
        use unicorn_engine::unicorn_const::HookType;
        let flag = 0x039e_1cb0u32 + 0x18;
        let res = unicorn.add_mem_hook(
            HookType::MEM_WRITE,
            flag as u64,
            (flag + 3) as u64,
            move |uc: &mut Unicorn<'_, Context>, _t, address, size, value| {
                if !nav_trace_enabled() {
                    return true;
                }
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 300 {
                    return true;
                }
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI dynsm-flag t={}ms <- {:#x} (size {}) lr={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    value & 0xff,
                    size,
                    lr,
                );
                true
            },
        );
        log::debug!("PROCHMI dynsm-flag watch installed: {}", res.is_ok());
    }

    // r786: GUI_Widget::vDraw (0x137af7c) - the per-view render-block entry
    // the PU fix266 triple calls for a repaint.  Counting it tells whether
    // DisplayManager::update() ran a widget paint pass for the new page at
    // all (startup: hundreds; after a menu switch without the gate open: 0).
    {
        let want = base_address + 0x0137_af7cu32 - ORIGINAL_BASE;
        let page = want & !0xfff;
        let res = unicorn.add_code_hook(
            page as u64,
            (page + 0xfff) as u64,
            move |_uc: &mut Unicorn<'_, Context>, address, _size| {
                if address as u32 == want {
                    VDRAW_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                // r780: GUI_Scripted::vDraw gate probe (same page): scripted
                // flag (+0x68) and the Lua refs (+0x6c ref, +0x70 enter-fn,
                // +0x74 leave-fn) decide whether the scripted paint runs.
                let scripted = base_address + 0x0137_a398u32 - ORIGINAL_BASE;
                if address as u32 == scripted {
                    static SC_WIN: std::sync::OnceLock<(u128, u128)> =
                        std::sync::OnceLock::new();
                    static SC_N: AtomicU32 = AtomicU32::new(0);
                    let (w0, w1) =
                        *SC_WIN.get_or_init(|| match std::env::var("EMU_VDRAW_LOG") {
                            Ok(s) => {
                                let mut it = s.split(':').filter_map(|p| p.parse().ok());
                                (
                                    it.next().unwrap_or(0),
                                    it.next().unwrap_or(u128::MAX),
                                )
                            }
                            Err(_) => (u128::MAX, 0),
                        });
                    let t_ms = crate::libs::dapi::emu_t0_elapsed_ms();
                    if t_ms < w0 || t_ms > w1 {
                        return;
                    }
                    let n = SC_N.fetch_add(1, Ordering::Relaxed);
                    if n < 24 {
                        let obj = _uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                        log::warn!(
                            "PROCHMI scriptDraw#{n} t={t_ms}ms obj={obj:#x} vt={:#x} flag68={} ref6c={:#x} fn70={:#x} fn74={:#x}",
                            read_u8(_uc, obj + 0x68),
                            read_u32(_uc, obj + 0x6c),
                            read_u32(_uc, obj + 0x70),
                            read_u32(_uc, obj + 0x74),
                            read_u32(_uc, obj),
                        );
                    }
                    return;
                }
            },
        );
        log::info!("PROCHMI vDraw page-hook 0x{page:x} installed: {}", res.is_ok());
    }

    // r780: GUI_Rect::intersects (0x1362f3c) entry probe - callDraw's clip
    // test is a mid-TB branch (unicorn range hooks dispatch at TB entries
    // only), so read the rects straight from the intersects entry (r0 = clip,
    // r1 = widget rect, both s32[4]).  EMU_VDRAW_LOG="a:b" ms window.
    {
        let want = base_address + 0x0136_2f3cu32 - ORIGINAL_BASE;
        let page = want & !0xfff;
        let res = unicorn.add_code_hook(
            page as u64,
            (page + 0xfff) as u64,
            move |_uc: &mut Unicorn<'_, Context>, address, _size| {
                if address as u32 != want {
                    return;
                }
                static IN_WIN: std::sync::OnceLock<(u128, u128)> =
                    std::sync::OnceLock::new();
                static IN_N: AtomicU32 = AtomicU32::new(0);
                let (w0, w1) =
                    *IN_WIN.get_or_init(|| match std::env::var("EMU_VDRAW_LOG") {
                        Ok(s) => {
                            let mut it = s.split(':').filter_map(|p| p.parse().ok());
                            (
                                it.next().unwrap_or(0),
                                it.next().unwrap_or(u128::MAX),
                            )
                        }
                        Err(_) => (u128::MAX, 0),
                    });
                let t_ms = crate::libs::dapi::emu_t0_elapsed_ms();
                if t_ms < w0 || t_ms > w1 {
                    return;
                }
                let n = IN_N.fetch_add(1, Ordering::Relaxed);
                if n >= 48 {
                    return;
                }
                let clip = _uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let wr = _uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let cr: Vec<i32> =
                    (0..4u32).map(|i| read_u32(_uc, clip + i * 4) as i32).collect();
                let wv: Vec<i32> =
                    (0..4u32).map(|i| read_u32(_uc, wr + i * 4) as i32).collect();
                log::warn!(
                    "PROCHMI intersects#{n} t={t_ms}ms lr={:#x} clip={:?} widget={:?}",
                    _uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                    cr,
                    wv,
                );
            },
        );
        log::info!("PROCHMI intersects page-hook 0x{page:x} installed: {}", res.is_ok());
    }

    // r780: GUI_LUA_Interface::prepareCallByRef (0x1368b68) + callInts
    // (0x136898c) probes.  GUI_Scripted::vDraw paints through a by-ref Lua
    // call (registry ref +0x6c, fn id +0x70 pre / +0x74 post); a registry
    // lookup miss returns 0 silently.  EMU_VDRAW_LOG="a:b" logs (interface,
    // ref, fn) per attempt and every callInts entry (proves the call was
    // actually prepared and executed).
    {
        let p1 = base_address + 0x0136_8b68u32 - ORIGINAL_BASE;
        let p2 = base_address + 0x1368_98cu32 - ORIGINAL_BASE;
        let page = p1 & !0xfff;
        let res = unicorn.add_code_hook(
            page as u64,
            (page + 0xfff) as u64,
            move |_uc: &mut Unicorn<'_, Context>, address, _size| {
                let a = address as u32;
                if a != p1 && a != p2 {
                    return;
                }
                static PC_WIN: std::sync::OnceLock<(u128, u128)> =
                    std::sync::OnceLock::new();
                static PC_N: AtomicU32 = AtomicU32::new(0);
                let (w0, w1) =
                    *PC_WIN.get_or_init(|| match std::env::var("EMU_VDRAW_LOG") {
                        Ok(s) => {
                            let mut it = s.split(':').filter_map(|p| p.parse().ok());
                            (
                                it.next().unwrap_or(0),
                                it.next().unwrap_or(u128::MAX),
                            )
                        }
                        Err(_) => (u128::MAX, 0),
                    });
                let t_ms = crate::libs::dapi::emu_t0_elapsed_ms();
                if t_ms < w0 || t_ms > w1 {
                    return;
                }
                let n = PC_N.fetch_add(1, Ordering::Relaxed);
                if n >= 32 {
                    return;
                }
                if a == p1 {
                    log::warn!(
                        "PROCHMI prepByRef#{n} t={t_ms}ms ifc={:#x} ref={:#x} fn={:#x}",
                        _uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                        _uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                        _uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32,
                    );
                } else {
                    log::warn!(
                        "PROCHMI callInts#{n} t={t_ms}ms ifc={:#x} out={:#x}",
                        _uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                        _uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                    );
                }
            },
        );
        log::info!("PROCHMI prepByRef page-hook 0x{page:x} installed: {}", res.is_ok());
    }

    // r780: GUI_Image::vDraw (0x13a97d8) + GUI_Button::vDraw (0x13a897c)
    // probes: dump the texture/resource handle (+0x60, +0x14 for buttons)
    // and the draw-state flag (+0x4c).  A 0 / invalid handle means the
    // widget's FI resource never bound and the draw is a silent no-op.
    // Same EMU_VDRAW_LOG window.
    for off in [0x013a_97d8u32, 0x013a_897cu32] {
        let want = base_address + off - ORIGINAL_BASE;
        let page = want & !0xfff;
        let res = unicorn.add_code_hook(
            page as u64,
            (page + 0xfff) as u64,
            move |_uc: &mut Unicorn<'_, Context>, address, _size| {
                if address as u32 != want {
                    return;
                }
                static GI_WIN: std::sync::OnceLock<(u128, u128)> =
                    std::sync::OnceLock::new();
                static GI_N: AtomicU32 = AtomicU32::new(0);
                let (w0, w1) =
                    *GI_WIN.get_or_init(|| match std::env::var("EMU_VDRAW_LOG") {
                        Ok(s) => {
                            let mut it = s.split(':').filter_map(|p| p.parse().ok());
                            (
                                it.next().unwrap_or(0),
                                it.next().unwrap_or(u128::MAX),
                            )
                        }
                        Err(_) => (u128::MAX, 0),
                    });
                let t_ms = crate::libs::dapi::emu_t0_elapsed_ms();
                if t_ms < w0 || t_ms > w1 {
                    return;
                }
                let n = GI_N.fetch_add(1, Ordering::Relaxed);
                if n >= 26 {
                    return;
                }
                let obj = _uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI imgBtnDraw#{n} t={t_ms}ms cls={} obj={obj:#x} r60={:#x} f4c={} r48={}",
                    if off == 0x013a_97d8u32 { "img" } else { "btn" },
                    read_u32(_uc, obj + 0x60),
                    read_u8(_uc, obj + 0x4c),
                    read_u32(_uc, obj + 0x48) as i32,
                );
            },
        );
        log::info!("PROCHMI img/btn vDraw hook 0x{want:x} installed: {}", res.is_ok());
    }

    // r779: GUI_GL_OpenGL::drawImage (0x1345a18) is the real HMI painter: the
    // idle screen is 2 mixLayers quads/frame, and a widget paint pass shows
    // up as a burst on this counter.  Page-range hook (dead-trap quirk),
    // counter only.
    {
        let want = base_address + 0x0134_5a18u32 - ORIGINAL_BASE;
        let page = want & !0xfff;
        let res = unicorn.add_code_hook(
            page as u64,
            (page + 0xfff) as u64,
            move |_uc: &mut Unicorn<'_, Context>, address, _size| {
                if address as u32 != want {
                    return;
                }
                DRAW_IMAGE_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // r780 forensics (MAP SCROLL black page): EMU_DRAWTRI_LOG="a:b"
                // dumps texture/rect/blend of the calls inside that ms window.
                static TRI_WIN: std::sync::OnceLock<(u128, u128)> =
                    std::sync::OnceLock::new();
                static TRI_N: AtomicU32 = AtomicU32::new(0);
                let (w0, w1) =
                    *TRI_WIN.get_or_init(|| match std::env::var("EMU_DRAWTRI_LOG") {
                        Ok(s) => {
                            let mut it = s.split(':').filter_map(|p| p.parse().ok());
                            (
                                it.next().unwrap_or(0),
                                it.next().unwrap_or(u128::MAX),
                            )
                        }
                        Err(_) => (u128::MAX, 0),
                    });
                let t_ms = crate::libs::dapi::emu_t0_elapsed_ms();
                if t_ms >= w0 && t_ms <= w1 {
                    let n = TRI_N.fetch_add(1, Ordering::Relaxed);
                    if n < 400 {
                        let tex = _uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                        let rect = _uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                        let pos = _uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                        let r: Vec<u32> =
                            (0..4u32).map(|i| read_u32(_uc, rect + i * 4)).collect();
                        log::warn!(
                            "PROCHMI drawImage#{n} t={t_ms}ms tex={tex:#x} rect=[{},{},{},{}] pos={pos:#x} lr={:#x}",
                            r[0] as i32,
                            r[1] as i32,
                            r[2] as i32,
                            r[3] as i32,
                            _uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                        );
                    }
                }
            },
        );
        log::info!("PROCHMI drawImage page-hook 0x{page:x} installed: {}", res.is_ok());
    }


    // r767: which code creates/destroys the popup's view (cid 1)?  With the
    // mm singleton decoded (current view = mm+0x18, previous = mm+0x10) the
    // fix240 vPerformViewChange crash proved view 1 was ALREADY destroyed
    // before the injected hide - find when/who destroyed it relative to the
    // 12.4 s paint, and whether pCreateView(1) ever ran at runtime.
    {
        for (offset, label) in [
            (0x0137_c024u32 - ORIGINAL_BASE, "view-destroy"),
            (0x0137_c6a0u32 - ORIGINAL_BASE, "view-create"),
        ] {
            let address = base_address + offset;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:viewfac", move |uc: &mut Unicorn<'_, Context>| {
                // Track the live main view cid (main-block creates only) -
                // needed by the hide injection even with tracing off.
                if label == "view-create" {
                    let c = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xffff;
                    let l = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                    if c != 0 && l == base_address + 0x0136_474cu32 - ORIGINAL_BASE {
                        SM_HIDE_MAIN_CID.store(c, Ordering::Relaxed);
                    }
                    // r778: a fresh view (page switch, e.g. Menu click ->
                    // menu view cid) has a brand-new draw context that the
                    // engine only dirties through paths that stay dead in
                    // the emulator; re-arm the (capped) forced dirty pass so
                    // the new view actually renders instead of staying an
                    // invisible widget tree.
                    HMI_VIEW_DIRTY_FORCE_COUNT.store(0, Ordering::Relaxed);
                    // r779: schedule the page-switch paint pump a beat later
                    // (widget creation continues after this entry returns).
                    // Only for real main-page switches: invalidating during
                    // the popup/startup phase (cid<=1, t<15 s) corrupts the
                    // live widget dispatch (A/B r779: click chain died).
                    if std::env::var_os("EMU_NO_VIEW_PAINT").is_none()
                        && c > 1
                        && crate::libs::dapi::emu_t0_elapsed_ms() > 15_000
                    {
                        VIEW_PAINT_CD.store(30, Ordering::Relaxed);
                        VIEW_PAINT_STATE.store(1, Ordering::Relaxed);
                        VIEW_PAINT_ROUND.store(1, Ordering::Relaxed);
                    }
                }
                if !nav_trace_enabled() {
                    return 0;
                }
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) >= 200 {
                    return 0;
                }
                let cid = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xffff;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::warn!(
                    "PROCHMI {label} t={}ms thread=[{}] cid={cid} lr={lr:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
                0
            });
        }
        // r784: on-screen keyboard path.  `clFi_KEYBRD` (rodata name
        // "FI_KeyBoard", RTTI 11clFi_KEYBRD) is the HSI FI that receives
        // CMKeyboard events from the HSI EventEngine and drives the keyboard
        // page; it is only alive if clFiFactory::CreateFi instantiated it.
        // These probes answer: (a) was the FI created (bl into its ctor at
        // 0x13d18dc inside CreateFi), (b) which FI ids the factory is asked
        // for, (c) service-state transitions of the KEYBRD FI (0x13e7124),
        // (d) events entering/leaving it (0x13e735c/0x13e756c), (e) whether
        // clHSI_CMKeyboard::bExecuteMessage(sMsg&) ever runs (0x1812de4).
        // All function-entry sites, gated behind EMU_PROCHMI_KEYB_TRACE.
        if keyb_trace_enabled() {
            let create_fi = base_address + 0x013d_15e4u32 - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, create_fi, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:keyb-createfi", move |uc: &mut Unicorn<'_, Context>| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) < 128 {
                    let id = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xff;
                    let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                    log::info!(
                        "PROCHMI keyb: CreateFi t={}ms id={id} lr={lr:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                    );
                }
                0
            });
            let keybr_ctor = base_address + 0x013d_18dcu32 - ORIGINAL_BASE;
            crate::os::code_stub::trap_site(unicorn, keybr_ctor, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:keyb-ctor", |_uc: &mut Unicorn<'_, Context>| {
                static ONCE: AtomicU32 = AtomicU32::new(0);
                if ONCE.fetch_add(1, Ordering::Relaxed) == 0 {
                    log::info!(
                        "PROCHMI keyb: clFi_KEYBRD CONSTRUCTED t={}ms",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                    );
                }
                0
            });
            for (offset, name, cap) in [
                (0x013e_7124u32 - ORIGINAL_BASE, "svc-state", 40u32),
                (0x013e_735cu32 - ORIGINAL_BASE, "evt-in", 80),
                (0x013e_756cu32 - ORIGINAL_BASE, "evt-out", 80),
                (0x0181_2de4u32 - ORIGINAL_BASE, "cmkbd-exec", 40),
            ] {
                let address = base_address + offset;
                crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:keyb-trace", move |uc: &mut Unicorn<'_, Context>| {
                    static HITS: [AtomicU32; 4] = [
                        AtomicU32::new(0),
                        AtomicU32::new(0),
                        AtomicU32::new(0),
                        AtomicU32::new(0),
                    ];
                    let idx = match name {
                        "svc-state" => 0usize,
                        "evt-in" => 1,
                        "evt-out" => 2,
                        _ => 3,
                    };
                    if HITS[idx].fetch_add(1, Ordering::Relaxed) < cap {
                        let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                        let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                        let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                        if name == "cmkbd-exec" {
                            // sMsg payload (per bExecuteMessage decompile):
                            // msg+0x00 sender, +0x04 message id (0x29xx),
                            // +0x08 data.  Log the decoded triple.
                            let id = read_u32(uc, r1 + 4);
                            let data = read_u32(uc, r1 + 8);
                            log::info!(
                                "PROCHMI keyb: {name} t={}ms fi={r0:#x} msg={r1:#x} id={id:#x} data={data:#x}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                            );
                        } else {
                            log::info!(
                                "PROCHMI keyb: {name} t={}ms r0={r0:#x} r1={r1:#x} r2={r2:#x}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                            );
                        }
                    }
                    0
                });
            }
            log::info!("PROCHMI keyboard-path probes installed");
        }
        // r767: skip GUI_StateMachine::vFocusGained while the injected view
        // rebuild runs (see SM_HIDE_FOCUS_SKIP).  Returning LR from the site
        // handler ends the callee immediately (return-value == PC redirect).
        let address = base_address + 0x0137_1b44u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:focus-skip", move |uc: &mut Unicorn<'_, Context>| {
            if SM_HIDE_FOCUS_SKIP.load(Ordering::Relaxed) == 0 {
                return 0;
            }
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::info!(
                "PROCHMI sm-hide: vFocusGained skipped t={}ms",
                crate::libs::dapi::emu_t0_elapsed_ms(),
            );
            lr & !1
        });
        // r767 diagnostic: the view-change pump 0x1365224 (mainloop
        // 0x133bea8) faulted with r0 == 0 right after the injected
        // rebuild (fix251 READ_UNMAPPED 0x136524c).  Its argument chain
        // [[0x2a18274] resolves to mm+0x04 = pGetView(1) = the SYSDLG VIEW
        // OBJECT: destroying the overlay view zeroed the very slot the
        // mainloop polls each frame, and the next pump frame faulted on
        // the null object.  Watch the argument across experiments.
        let address = base_address + 0x0136_5224u32 - ORIGINAL_BASE;
        crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:viewpump", move |uc: &mut Unicorn<'_, Context>| {
            if !nav_trace_enabled() {
                return 0;
            }
            static HITS: AtomicU32 = AtomicU32::new(0);
            let n = HITS.fetch_add(1, Ordering::Relaxed);
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            if r0 != 0 && n < 40 {
                return 0;
            }
            let g = read_u32(uc, 0x02a1_8274);
            log::warn!(
                "PROCHMI viewpump t={}ms thread=[{}] n={n} r0={r0:#x} [g]={g:#x} lr={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
            );
            0
        });
    }

    // r765: the popup-string read chain (fix221 run, EMU_PROCHMI_POPUP_TRACE)
    // proved the "Starting navigation." text - and with it the widget's
    // SHOW - is executed by the GUI core command interpreter: a resource
    // driven loop at 0x136bc40..0x136bf58 walks a command table
    // (`[sp,#0x874]`, entry index in r2, command word `r1 = [[sp,#0x874], sl]`)
    // and for each word resolves an action object
    // (`obj = cmd_class_table[word >> 28]`) and tail-calls its
    // `vtable+20` (vExecute) with (obj, word, arg).  A thin wrapper
    // 0x1363a10..0x1363a50 does the same for single calls.  The popup's
    // hide is (near-certainly) the sibling command in the same table run by
    // the same interpreter on the state's exit transition - which fires ~350
    // ms BEFORE the paint (fix214/215 SM-page trace).  These two cold-site
    // traps capture the executed command word, object, vtable and vExecute
    // target while the paint runs, so the show command is named and the
    // hide sibling can be found next to it in the table.
    {
        for (offset, label) in [
            (0x0136_3a10u32 - ORIGINAL_BASE, "gui-cmd-wrap"),
            (0x0136_3a4cu32 - ORIGINAL_BASE, "gui-cmd-wrap-exec"),
            (0x0136_bf54u32 - ORIGINAL_BASE, "gui-cmd-interp"),
        ] {
            let address = base_address + offset;
            crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:guicmd", move |uc: &mut Unicorn<'_, Context>| {
                if !nav_trace_enabled() {
                    return 0;
                }
                static HITS: AtomicU32 = AtomicU32::new(0);
                let hit = HITS.fetch_add(1, Ordering::Relaxed);
                if hit >= 400 {
                    return 0;
                }
                let (r0, r1, r2) = (
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32,
                );
                let (obj, vtable, exec) = if label == "gui-cmd-interp" {
                    // r0 = obj, r1 = command word, r3 = [obj] vtable already
                    // loaded; the call target is [r3,#20].
                    let vt = read_u32(uc, r0);
                    (r0, vt, read_u32(uc, vt + 20))
                } else if label == "gui-cmd-wrap-exec" {
                    // r0 = obj (r4), r3 = [obj], target [r3,#20].
                    let vt = read_u32(uc, r0);
                    (r0, vt, read_u32(uc, vt + 20))
                } else {
                    // wrapper entry: r0 = this, r1 = command word, r2 = arg.
                    (0, 0, 0)
                };
                log::warn!(
                    "PROCHMI {label} t={}ms thread=[{}] r0={r0:#x} r1(w)={r1:#x} r2={r2:#x} obj={obj:#x} vt={vtable:#x} exec={exec:#x} lr={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                );
                0
            });
        }
    }

    for (offset, store) in [
        (CL_HMI_MNGR_B_EXECUTE, "hmi_mngr"),
        (CL_GUI_WIDGET_B_EXECUTE, "gui_widget"),
    ] {
        let addr = base_address + offset;
        
crate::os::code_stub::trap_site(unicorn, addr, crate::os::code_stub::TrapMode::Arm, "libs/prochmi.rs:1145", move |uc: &mut Unicorn<'_, Context>| {
                let ptr = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                // r768: popup-uninit invalidate loop (see
                // sm_hide_fire_or_restore).  State 2 = widgets hidden,
                // targets pending; state 3 = a vInvalidate injected from
                // this site is returning.  On that return r0 is the
                // callee's result, NOT the engine `this`, so restore it
                // before letting the prologue replay (fix251: bGUIMainloop
                // ran with r0=0 off a clobbered re-entry).
                let st = SM_HIDE_STATE.load(Ordering::Relaxed);
                if st == 2 || st == 3 {
                    // Invalidate loop.  Walk to the next callable target
                    // (a zero function address means the queued object
                    // lost its context since stage 1 - skip it), call it,
                    // and on loop end hand the site back.  fix267: the
                    // return from each injected call re-fires this site,
                    // and returning 0 replays its ORIGINAL instruction
                    // (an entry push) once more per leg - so the entry
                    // context (r0-r3/sp/lr) is saved on the first visit
                    // and fully restored at loop end before the final
                    // replay (r766 pattern; fix267 crashed at site+4 with
                    // only r0 restored).
                    let n = PU_COUNT.load(Ordering::Relaxed) as usize;
                    let mut i = if st == 2 {
                        PU_THIS.store(ptr, Ordering::Relaxed);
                        for (ri, reg) in [
                            RegisterARM::R0,
                            RegisterARM::R1,
                            RegisterARM::R2,
                            RegisterARM::R3,
                        ]
                        .iter()
                        .enumerate()
                        {
                            SM_HIDE_REGS[ri].store(
                                uc.reg_read(*reg).unwrap_or(0) as u32,
                                Ordering::Relaxed,
                            );
                        }
                        SM_HIDE_LR.store(
                            uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                            Ordering::Relaxed,
                        );
                        SM_HIDE_SP.store(
                            uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32,
                            Ordering::Relaxed,
                        );
                        SM_HIDE_TID.store(
                            uc.get_data().inner.thread_id() as u32,
                            Ordering::Relaxed,
                        );
                        0usize
                    } else {
                        PU_I.load(Ordering::Relaxed) as usize + 1
                    };
                    while i < n
                        && pu_target_fn(uc, base_address, PU_OBJS[i].load(Ordering::Relaxed)).0 == 0
                    {
                        i += 1;
                    }
                    if i >= n {
                        SM_HIDE_STATE.store(0, Ordering::Relaxed);
                        let sp = SM_HIDE_SP.load(Ordering::Relaxed);
                        if sp != 0 {
                            let _ = uc.reg_write(RegisterARM::R13, sp as u64);
                        }
                        let lr = SM_HIDE_LR.load(Ordering::Relaxed);
                        if lr != 0 {
                            let _ = uc.reg_write(RegisterARM::R14, lr as u64);
                        }
                        for (ri, reg) in [
                            RegisterARM::R0,
                            RegisterARM::R1,
                            RegisterARM::R2,
                            RegisterARM::R3,
                        ]
                        .iter()
                        .enumerate()
                        {
                            let v = SM_HIDE_REGS[ri].load(Ordering::Relaxed);
                            let _ = uc.reg_write(*reg, v as u64);
                        }
                        log::info!(
                            "PROCHMI popup-uninit: invalidation loop done, site registers restored t={}ms",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                        );
                        // r768: the damage the loop posted produced no composite
                        // pass (fix256/257: prochmi idles without eglSwapBuffers
                        // and the empty GUI_Messaging receiver lists never flush
                        // it).  Nudge the dispatcher with one more internal
                        // event - 0x8f (enavi status, already handled once, its
                        // handler is idempotent) - routed like the startup burst
                        // through [messaging+4].  Any pass it wakes repaints the
                        // now-invisible popup region.
                        let messaging = GUI_MESSAGING_POINTER.load(Ordering::Relaxed);
                        if messaging != 0 {
                            let q = read_u32(uc, messaging + 4);
                            if post_gui_message_queue(uc, q, messaging, 0x8f) {
                                log::info!(
                                    "PROCHMI popup-uninit: re-posted internal event 0x8f queue={q:#x} t={}ms",
                                    crate::libs::dapi::emu_t0_elapsed_ms(),
                                );
                            }
                        }
                        // fix266: ask the host GPU layer to present+dump
                        // the HMI surface once (pure presentation of what
                        // the guest rendered; no synthesized pixels).
                        crate::gpu::request_forced_present();
                        return 0;
                    }
                    PU_I.store(i as u32, Ordering::Relaxed);
                    SM_HIDE_STATE.store(3, Ordering::Relaxed);
                    let t = PU_OBJS[i].load(Ordering::Relaxed);
                    let (fn_addr, args) = pu_target_fn(uc, base_address, t);
                    guest_call_via_lr(uc, addr, fn_addr, args, 64);
                    log::info!(
                        "PROCHMI popup-uninit: invalidate target {t:#x} fn={fn_addr:#x} {}/{} posted t={}ms",
                        i + 1,
                        n,
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                    );
                    return 0;
                }
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
    // r767 (fix241..244): the fabricated 0x8d/0x8e/0x8f burst drives the
    // whole post-startup GUI cascade - 0x8d completes the state-machine
    // wiring and shows the wait popup (async paint lands ~12.4 s), while
    // 0x8e/0x8f carry the service-ready effects (main view change 1 -> 247,
    // app-state moves).  Posted back-to-back they made the view change
    // destroy the popup's view ~285 ms BEFORE its paint (fix241: view 1
    // destroyed 12161 ms, painted 12446 ms) - pixels orphaned outside any
    // live view, which no hide channel can erase afterwards.  Pace them
    // like the real device: 0x8d on the release gate, the ready-side events
    // only after the paint (same gate as the enavi-fab RUNNING report).
    if sent > 0 && !enavi_fab_fire_now() {
        return;
    }
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
    // r761: literal-pool return stub.  The previous shape
    // (`add r12,sp,#0x34; pop {r0-r11,lr}; ldmia r12,{sp,pc}`) is an
    // unpredictable ARM encoding (LDM with PC *and* base writeback, which
    // Unicorn's TCG mis-executes: fix144/145/149/150 died with
    // READ_UNMAPPED at the ldmia although hook-verified SP, frame and r12
    // were all correct and mapped).  sp/pc now come from two literal slots
    // at stub+0x10/+0x14 that every `call_guest_function` fills before
    // launching; the emulator is serialized on the VM lock, so the shared
    // slots cannot race.
    let stub_code = [
        0xff, 0x4f, 0xbd, 0xe8, // pop {r0-r11,lr}          (+0x00)
        0x04, 0xd0, 0x9d, 0xe5, // ldr sp, [pc, #4]         (+0x04) -> +0x10
        0x04, 0xf0, 0x9f, 0xe5, // ldr pc, [pc, #4]         (+0x08) -> +0x14
        0x00, 0x00, 0x00, 0x00, // pad                      (+0x0c)
        0x00, 0x00, 0x00, 0x00, // saved sp slot            (+0x10)
        0x00, 0x00, 0x00, 0x00, // saved pc slot            (+0x14)
    ];
    if addr == 0 || unicorn.mem_write(addr as u64, &stub_code).is_err() {
        log::warn!("PROCHMI: failed to allocate ARM r0-r11+sp+pc guest-call stub");
        return None;
    }

    // r761 CRITICAL: drop any translation of the stub page.  TCG translates
    // a fresh heap page eagerly at MMAP time while it still reads as zeros;
    // the resulting TB embeds the old words and is *never* discarded on
    // mem_write (unicorn 2.x has no tb_flush on softmmu map updates).  fix151
    // (-149/-153) proved it: the stub bytes here were verified correct, yet
    // the guest executed the previous stub shape - the `ldmia` TB's embedded
    // immediate #0x34 and its reglist semantics set sp := pc-slot (= the
    // trap site 0x134ead8) at every return, faulting the next push.  The
    // same quirk behind the r79 "ghost" byte writes: stale TBs with baked-in
    // immediates keep running after the page contents change.
    let page = addr & !0xfff;
    let dropped = unicorn
        .ctl_remove_cache(page as u64, (page + 0xfff) as u64)
        .is_ok();
    log::info!(
        "PROCHMI: guest-call stub at {addr:#x}, tb-cache page {page:#x} dropped: {dropped}",
    );

    // r761 diagnostic: fire at the return stub's entry instruction (a
    // notification-only narrow hook; the page is freshly allocated so no
    // stale translation block can hide it) and dump the stack pointer the
    // called function returned with - the fix144/149 guest calls died on the
    // stub's `ldmia r12,{sp,pc}` reading an unmapped frame, and this names
    // the SP the callee restored.
    if nav_trace_enabled() {
        let res = unicorn.add_code_hook(addr as u64, addr as u64, move |uc, _a, _sz| {
            static HITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            let f0 = read_u32(uc, sp);
            let f13 = read_u32(uc, sp.wrapping_add(52));
            let f14 = read_u32(uc, sp.wrapping_add(56));
            let n = HITS.fetch_add(1, Ordering::Relaxed);
            if n < 16 || f14 == 0 || f14 > 0x4000_0000 {
                log::info!(
                    "PROCHMI guest-call stub-entry t={}ms thread=[{}] n={n} sp={sp:#x} lr={lr:#x} frame[0]={f0:#x} frame[sp]={f13:#x} frame[pc]={f14:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                );
            }
        });
        log::info!("PROCHMI guest-call stub-entry hook installed: {}", res.is_ok());
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

    // Literal slots for the return stub (see ensure_guest_call_stub): the
    // callee returns here and the stub pops r0-r11/lr off the frame, then
    // loads sp/pc from these two words.
    if !write_u32(unicorn, stub + 0x10, sp) || !write_u32(unicorn, stub + 0x14, original_pc) {
        log::warn!("PROCHMI: failed to fill guest-call stub slots at {stub:#x}");
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

    if nav_trace_enabled() {
        static LOGS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if LOGS.fetch_add(1, Ordering::Relaxed) < 16 {
            let mut raw = [0u8; 24];
            let _ = unicorn.mem_read(stub as u64, &mut raw);
            log::info!(
                "PROCHMI guest-call launch t={}ms thread=[{}] fn={function:#x} sp={sp:#x} new_sp={new_sp:#x} stub={stub:#x} resume={original_pc:#x} raw {:02x?}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                unicorn.get_data().inner.thread_id(),
                raw,
            );
        }
    }
    true
}

/// r784: [`call_guest_function`] variant that never trusts the live R13.
/// The bGUIMainloop trap site can fire while the caller code is mid-trick
/// with r13 (0x133xxxxx static-base values observed), where the plain
/// variant wrote its save frame into the code segment and killed prochmi
/// (f1 run: [52] exited(1) right after a touch injection).  This variant
/// keeps a private scratch stack page for the save/restore frame and
/// restores the real r13/r14 through the literal-slot return stub.
/// (r784: currently unused - the stub return path itself mis-restores, see
/// the view-pump comment in the mainloop hook; kept for the follow-up fix.)
#[allow(dead_code)]
fn call_guest_function_scratch(
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
    static SCRATCH_SP: AtomicU32 = AtomicU32::new(0);
    let sp_scratch = {
        let current = SCRATCH_SP.load(Ordering::Relaxed);
        if current != 0 {
            current
        } else {
            let mmu_arc = {
                let data = unicorn.get_data();
                data.mmu.clone()
            };
            let page = mmu_arc.lock().unwrap().heap_alloc(
                unicorn,
                0x1000,
                Prot::READ | Prot::WRITE,
                "[prochmi-scratch-stack]",
            );
            let sp = page + 0xf00;
            if page == 0 {
                return false;
            }
            SCRATCH_SP.store(sp, Ordering::Relaxed);
            sp
        }
    };

    let original_sp = unicorn.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
    let saved: [u32; 13] = [
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
    ];
    // The return stub pops {r0-r11,lr} (13 words) off the live sp, then
    // reloads sp/pc from its literal slots.  Lay exactly that frame on the
    // scratch page, point r13 at its base, and stash the real sp/pc in the
    // stub slots (+0x10 / +0x14), which also restores r13 itself.
    let frame = sp_scratch - (saved.len() as u32 * 4);
    if !saved
        .iter()
        .enumerate()
        .all(|(index, value)| write_u32(unicorn, frame + (index as u32 * 4), *value))
    {
        log::warn!("PROCHMI: failed to write scratch frame at {frame:#x}");
        return false;
    }
    if !write_u32(unicorn, stub + 0x10, original_sp) || !write_u32(unicorn, stub + 0x14, original_pc)
    {
        log::warn!("PROCHMI: failed to fill scratch-call stub slots at {stub:#x}");
        return false;
    }
    let _ = unicorn.reg_write(RegisterARM::R13, frame as u64);
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

/// r761: stub-free guest call: load the arguments, point LR at `return_site`
/// (a trapped instruction, i.e. an `svc #0` the dispatcher owns) and redirect
/// the PC to `function`.  The callee returns straight onto the trap site,
/// which re-fires its handler (state machines must tolerate the re-entry);
/// callee-saved registers survive by ARM ABI, so the surrounding app code is
/// intact without any save/restore frame.  Preferred over
/// [`call_guest_function`] - that mechanism's synthetic return stub executes
/// with a corrupted SP under the current Unicorn (fix144..155: verified
/// bytes + verified frame, wrong live behaviour), and injecting host code
/// pages into the guest is the part that misbehaves, not the call itself.
fn guest_call_via_lr(
    unicorn: &mut Unicorn<'_, Context>,
    return_site: u32,
    function: u32,
    args: [u32; 4],
    sp_slack: u32,
) -> bool {
    if return_site == 0 || function == 0 {
        return false;
    }
    if sp_slack != 0 {
        let sp = unicorn.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
        let _ = unicorn.reg_write(RegisterARM::R13, (sp - sp_slack) as u64);
    }
    for (register, value) in [
        (RegisterARM::R0, args[0]),
        (RegisterARM::R1, args[1]),
        (RegisterARM::R2, args[2]),
        (RegisterARM::R3, args[3]),
        (RegisterARM::R14, return_site),
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

/// r761: rescue state machine for the enavi popup-release event 0x81008001
/// (background on EVT8001_REPOST).  The event is produced organically
/// (clFi_Navi startup -> enavi sink vOnUpdateEnaviServerState -> bEvent2Engine
/// -> clEventQueueManager::bWriteQueue, fix140) but `clEventEngine::
/// vNewAppState` - driven by `clHMIMngr::vPerformNewAppState` every ~20 ms
/// framework cycle - wipes the ring via `bResetQueue` for NORMAL(3) *and*
/// OFF(4), always after the cycle's event producers and before any pump:
/// the cycle order is  dispatch (bl 0x134e968, producers post here)  ->
/// engine wipe (bl 0x134ead0)  ->  cycle end.  The organic pump lives in
/// `clHMIMngr::bExecute` (bl 0x134ece8), whose callers block on the idle
/// semaphore in this emulator, so nothing drains the ring and the release
/// event dies in every cycle.  On the device the server thread posts the
/// event asynchronously (milliseconds later, between cycles) where the next
/// pump takes it.  This machine reproduces that ordering with organic guest
/// calls only, driven from two always-on per-cycle traps:
///  * post-wipe site (0x134ead8, right after `bResetQueue` returns): if the
///    ring slot holds an unconsumed 0x81008001 (the wipe cleared the
///    cursors but not the slot), re-queue it with the manager's own
///    `bWriteQueue`;
///  * dispatch site (0x1352df4) of the *next* cycle, which runs before that
///    cycle's wipe: pump once via `clEventEngine::bExecute`, which drains
///    the ring with its own code and delivers through the engine's real
///    router port (engine+0x474 = clHSIMngr, fix139); the read cursor then
///    confirms consumption.
/// Everything from the event id onwards is untouched app code.

/// r761: fabricate the "/dev/cryptcard" card object so the idapi registry
/// lookup chain used by vHandleEnaviServerStatus resolves.
///
/// The popup-release branch of the handler (0x15e2220) requires r7 == 1
/// from `0x1614dac(idapi, key, &out)`, which tails to 0x171fcf4 and needs a
/// card object found by 0x1613ac4's vector search over idapi+0x18 (the
/// entry's vtable+0x24 "name match" must return non-NULL for the key,
/// card+0x80 must be 0 or 2 and card+0x98..0x9c must be a record
/// vector containing a record whose +0x0c == 2).  On the real head unit the
/// cryptcard driver registers that object at boot; the emulator has no
/// cryptcard, the search returns NULL, r7 stays 0, the release branch is
/// skipped, and the state-2 fall-through then faults (fix177..179:
/// READ_UNMAPPED in nQueueSpaceLeft via a NULL-derived event port).
/// Fabricating the object is the sanctioned provider-signal class: the
/// client's own lookup, compare and release logic still all run organically.
fn fake_cryptcard_into_idapi(unicorn: &mut Unicorn<'_, Context>) {
    // Handler field holding the idapi object (dumped as `idapi=` by the
    // enavi-status probe).
    const IDAPI_FIELD: u32 = 0x38f_99b0 + 0x814;
    // Name-match stub: static text at 0xfb70f0 is a bare `bx lr`, so as a
    // virtual method it returns its first argument (the found entry) -
    // exactly what the 0x1613ac4 search treats as a name match.
    const NAME_MATCH: u32 = 0x00fb_70f0;
    // Placement: one private read-write heap page allocated by the emulator
    // on first use.  Only data lives there (the guest never fetches from
    // it), so the stale-TB-on-fresh-page quirk cannot bite.
    static SCRATCH_PAGE: AtomicU32 = AtomicU32::new(0);
    let mut scratch = SCRATCH_PAGE.load(Ordering::Relaxed);
    if scratch == 0 {
        let mmu_arc = {
            let data = unicorn.get_data();
            data.mmu.clone()
        };
        scratch = mmu_arc.lock().unwrap().heap_alloc(
            unicorn,
            0x1000,
            Prot::READ | Prot::WRITE,
            "[prochmi-cryptcard-fab]",
        );
        SCRATCH_PAGE.store(scratch, Ordering::Relaxed);
    }
    if scratch == 0 {
        log::warn!("PROCHMI cryptcard-fab: heap page allocation failed, skipped");
        return;
    }
    let idapi = read_u32(unicorn, IDAPI_FIELD);
    if idapi == 0 {
        return;
    }
    if read_u32(unicorn, idapi + 0x18) == scratch + 256 {
        return; // card already registered this run
    }
    let vtbl = scratch; // [0]=card, [0x24]=name-match method
    let vec = scratch + 256; // copied idapi object vector + our card entry
    let card = scratch + 32; // 160-byte card object
    let rec = scratch + 352; // one 24-byte card-status record
    // The real object vector exists (CCA-style directory) but holds no
    // cryptcard entry.  Copy it to the private page, append the fabricated
    // card, and repoint the vector.  The 0x1613ac4 search only reads the
    // range, nothing resizes it, and the emulator heap page is never freed.
    let old_begin = read_u32(unicorn, idapi + 0x18);
    let old_end = read_u32(unicorn, idapi + 0x1c);
    let count = old_end.wrapping_sub(old_begin) / 4;
    if count > 240 || (count > 0 && old_begin == 0) {
        log::warn!("PROCHMI cryptcard-fab: idapi vector {count:#x} entries implausible, skipped");
        return;
    }
    let ok = write_u32(unicorn, vtbl, card)
        && write_u32(unicorn, vtbl + 0x24, NAME_MATCH)
        && write_u32(unicorn, vec, card);
    for i in 0..count {
        let entry = read_u32(unicorn, old_begin + 4 * i);
        let _ = write_u32(unicorn, vec + 4 * (i + 1), entry);
    }
    // card: vtable, +0x58 -> vtable (search reads [entry+0x58] then calls
    // [vtable+0x24]), +0x80 status must be 0 or 2 for 0x171fcf4 to scan the
    // record vector (2 = the "answered" state), +0x98/+0x9c record vector.
    // r798: +0x88 state enum = 3.  The state-8 body (0x17c7b64 leg) queries
    // this card BY NAME through 0x17c2a64 -> 0x1614d8c and treats result 3
    // as READY (f228: the name string at sl+0x48 is "/dev/cryptcard", the
    // 0x1614dac leg already answers 1, only the 0x1614d8c answer 0 blocked
    // it).  With +0x80 == 2, slot 3 of the 0x171fd7c table scans the record
    // vector - the same predicate 0x171fcf4 already satisfies - and returns
    // 3.  The state-7 scan treats any res != 2 as "scan continues", so 0
    // and 3 follow the identical list-exhausted leg 0x17c7a6c (res == 2,
    // i.e. +0x88 == 7, pinned state 7: f226).
    // r791q: the idapi media broadcast (svc-0x06 ServiceStatus -> IDAPI
    // media branch -> ActionReq::vMarkMediaStateOfDeviceUnknown 0x1720458)
    // virtually calls vtable slot +0x38 on every vector entry and feeds the
    // result into a trace line only; a `bx lr` gadget returns the entry
    // pointer unchanged and the method then exits through the entry's
    // +0x3c == 0 early-return.  Without this slot the SM pump branched to a
    // null vtable entry and died (runs f114..f118), starving the
    // availability chain of the svc-0x06 status.
    // r791s: IDAPI::vRequestMediaStateOfDeviceOnce (0x16138ec, run f122)
    // broadcasts slots 0x2c/0x7c/0x24 over the same vector and discards
    // every return value (each slot reloads the entry first) - the gadget
    // is a no-op broadcast response for the fabricated card.  0x34 is
    // seeded defensively (adjacent media-path slot).
    let ok = ok
        && write_u32(unicorn, card, vtbl)
        && write_u32(unicorn, vtbl + 0x2c, NAME_MATCH)
        && write_u32(unicorn, vtbl + 0x34, NAME_MATCH)
        && write_u32(unicorn, vtbl + 0x38, NAME_MATCH)
        && write_u32(unicorn, vtbl + 0x7c, NAME_MATCH)
        && write_u32(unicorn, card + 0x58, vtbl)
        && write_u32(unicorn, card + 0x80, 2)
        && write_u32(unicorn, card + 0x88, 3)
        && write_u32(unicorn, card + 0x98, rec)
        && write_u32(unicorn, card + 0x9c, rec + 24);
    // record: +0x04 = status value reported to the caller (1 = card ready),
    // +0x0c = record kind 2, the ready-status record 0x171fcf4 scans for.
    let mut ok = ok
        && write_u32(unicorn, rec + 0x04, 1)
        && write_u32(unicorn, rec + 0x0c, 2);
    // r791r: the svc-0x06 media branch also treats the card as a media
    // device: IDAPI's MarkUnknown tail (0x1614f14) hands record+0x10 to
    // enavi_tclRegistry::vSetMediaDescription as a `const fi_tclString&`,
    // which virtually calls its vtable+0x24 (run f119 faulted on the zero
    // word there).  Seed record+0x10 with a real string object: the first
    // vector entry is the guest's own media action (registered moments
    // before), whose +0x58 device-name string supplies a valid vtable and
    // character pointer.  The guest's own string methods then run
    // organically on real text.
    if count > 0 {
        let action = read_u32(unicorn, old_begin);
        let name_vt = read_u32(unicorn, action.wrapping_add(0x58));
        let name_chars = read_u32(unicorn, action.wrapping_add(0x5c));
        if name_vt > 0x10000 && name_chars > 0x10000 && name_vt != vtbl {
            ok = ok
                && write_u32(unicorn, rec + 0x10, name_vt)
                && write_u32(unicorn, rec + 0x14, name_chars);
        } else {
            log::warn!("PROCHMI cryptcard-fab: no name string on action {action:#x}");
        }
    }
    let ok = ok
        && write_u32(unicorn, idapi + 0x18, vec)
        && write_u32(unicorn, idapi + 0x1c, vec + 4 * (count + 1));
    if !ok {
        log::warn!("PROCHMI cryptcard-fab: guest write failed, aborted");
        return;
    }
    log::info!(
        "PROCHMI cryptcard-fab: idapi={idapi:#x} card={card:#x} registered t={}ms",
        crate::libs::dapi::emu_t0_elapsed_ms(),
    );
}

/// r761: fire (or finish) the fabricated enavi-server RUNNING report from
/// the post-wipe trap site.  Arming happens on the organic `bstartup-reg`
/// hit (see the enavi-chain traps); this runs 800 ms later on the next
/// app-state transition: guest-call the client's own server-state setter
/// with RUNNING(2) and return onto the trap site, then restore r0-r3 on
/// the return (the site's `mov r1,#30` replay rewrites r1 anyway, so only
/// r0/r2/r3 would otherwise be clobbered - the evt8001 re-queue guest call
/// shares this restore step).  See the enavi-chain trap comment for why
/// the server report must be fabricated at all.
/// r762: the IEnavi client singleton lives on the heap and its address is
/// run-variant (0x3a241c8 in most runs, 0x3a0b810 in fix203 - writing via a
/// stale copy of it silently poisons unrelated memory, fix194/fix203).  The
/// handler object at 0x38f99b0 is a static (stable across runs) and keeps
/// the live pointer at +0x7dc; the server-state proxy at [IEnavi+0x4c] is
/// read live on every handler run (srvstate-get dump), so chain both.
fn live_srv_proxy(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let ienavi = read_u32(unicorn, 0x38f9_9b0 + 0x7dc);
    if ienavi == 0 {
        return 0;
    }
    read_u32(unicorn, ienavi + 0x4c)
}

fn enavi_fab_fire_or_restore(unicorn: &mut Unicorn<'_, Context>, site: u32) {
    match ENAVI_FAB_STATE.load(Ordering::Relaxed) {
        1 if crate::libs::dapi::emu_t0_elapsed_ms() as u64
            >= ENAVI_FAB_DEADLINE.load(Ordering::Relaxed)
            // never race the evt8001 re-queue for the single PC slot - wait
            // for the next app-state transition while a re-queue is pending.
            && EVT8001_REPOST.load(Ordering::Relaxed) == 0
            // r762: fire only once the startup internal events have been
            // released (paint-marker + 750 ms).  Those events are what
            // (re-)show the wait popup (fix204/205: popup #2 painted at
            // ~12.4 s, right after the 12 s fallback flush, while the fab
            // release had already run at 6.5 s and closed the FIRST paint
            // - the re-shown popup then stayed because the fab is
            // one-shot).  Same gate as the flush itself: the fabricated
            // RUNNING report then arrives after the final show, exactly
            // like on the real device where the server needs seconds to
            // boot, and closes whichever paint is up (same widget).
            && enavi_fab_fire_now() =>
        {
            let base = PROCHMI_BASE.load(Ordering::Relaxed);
            let proxy = live_srv_proxy(unicorn);
            if base == 0 || proxy == 0 {
                return;
            }
            for (i, reg) in [
                RegisterARM::R0,
                RegisterARM::R1,
                RegisterARM::R2,
                RegisterARM::R3,
            ]
            .iter()
            .enumerate()
            {
                ENAVI_FAB_REGS[i].store(
                    unicorn.reg_read(*reg).unwrap_or(0) as u32,
                    Ordering::Relaxed,
                );
            }
            ENAVI_FAB_TID.store(
                unicorn.get_data().inner.thread_id() as u32,
                Ordering::Relaxed,
            );
            ENAVI_FAB_LR.store(
                unicorn.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                Ordering::Relaxed,
            );
            ENAVI_FAB_STATE.store(2, Ordering::Relaxed);
            // Shallow fabrication: write the server's RUNNING status word
            // directly (the very word getter 0x161ce68/vHandleEnaviServerStatus
            // branch on) and post the release event through bWriteQueue -
            // the exact shallow guest call the evt8001 re-queue uses daily.
            // (Deep setter->bcast->sink->post chains executed fine but
            // correlated with a deterministic 0x90000560 jump in the
            // interrupted deliver-fi burst, fix167..171 - avoided here.)
            write_u32(unicorn, proxy + 0xe0, 2);
            // The router trap site is the *entry push* of the FI-router
            // function: SP still belongs to its caller, so the injected
            // call would push its frame over the router's own saved
            // r5/r6/lr slots - the router epilogue then returned to
            // bWriteQueue's bl-return mid-function (fix187..193 fatal).
            // Give the call 32 bytes of its own (push{r4-r7,lr}+sub#12)
            // and restore SP when it lands back on the site.
            ENAVI_FAB_SP.store(
                unicorn.reg_read(RegisterARM::R13).unwrap_or(0) as u32,
                Ordering::Relaxed,
            );
            guest_call_via_lr(
                unicorn,
                site,
                base + (0x0135_a310u32 - ORIGINAL_BASE),
                [0x38ee_ec8, 0, 0x8100_8001, 0],
                32,
            );
            log::info!(
                "PROCHMI enavi-fab: state=RUNNING written, release event queued via guest call t={}ms (proxy={proxy:#x} readback={:#x})",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                read_u32(unicorn, proxy + 0xe0),
            );
        }
        // the injected setter returned onto the trap site (same thread's
        // next hit here; the epilogue pop leaves LR clobbered, so match on
        // the firing thread - fix170).
        2 if unicorn.get_data().inner.thread_id() as u32
            == ENAVI_FAB_TID.load(Ordering::Relaxed) =>
        {
            ENAVI_FAB_STATE.store(3, Ordering::Relaxed);
            let sp = ENAVI_FAB_SP.load(Ordering::Relaxed);
            if sp != 0 {
                let _ = unicorn.reg_write(RegisterARM::R13, sp as u64);
            }
            // The site instruction is the router's entry push: the cave
            // that replays it stores LR into the frame, so hand back the
            // value from firing time (fix187..195: the callees' leftover
            // lr 0x135a3c8 poisoned the frame and the epilogue returned
            // into mid-bWriteQueue).
            let lr = ENAVI_FAB_LR.load(Ordering::Relaxed);
            if lr != 0 {
                let _ = unicorn.reg_write(RegisterARM::R14, lr as u64);
            }
            for (i, reg) in [
                RegisterARM::R0,
                RegisterARM::R1,
                RegisterARM::R2,
                RegisterARM::R3,
            ]
            .iter()
            .enumerate()
            {
                let v = ENAVI_FAB_REGS[i].load(Ordering::Relaxed);
                let _ = unicorn.reg_write(*reg, v as u64);
            }
            log::info!(
                "PROCHMI enavi-fab: setter returned, site registers restored t={}ms",
                crate::libs::dapi::emu_t0_elapsed_ms(),
            );
        }
        _ => {}
    }
}

/// r768: popup widget-hide channel.  The wait popup keeps painting because
/// the overlay's widget subtree is still alive and GUI_Widget::vDraw
/// (0x137bb88) walks it every frame - and it SKIPS every child whose
/// tagged visibility ContentId at +0x3c resolves to 0 (fix254/255
/// forensics).  So the hide is exactly the store the app's own
/// GUI_Widget::bSetProperty performs for the visible property: write the
/// tagged-false literal (tag 1, value 0 = 0x10000000) over the popup
/// container widget's slot, then post damage with GUI_Widget::vInvalidate
/// (0x137bdac, param_3=0 so it runs even after the widget became invisible)
/// and the compositor repaints the region from the surviving main-view
/// tree.  Targets: the registered popup widget ([singleton+0x14] of
/// [[0x2a13a00]], captured by the release-wfind/release-pop probes) plus
/// the owner widgets captured with the painted script cells (PU_IMPS).
/// Earlier fix254 attempt ran GUI_Scripted::vUninitialize over those script
/// CELLS - they are VM-materialized cells, not GUI_Scripted objects, and
/// the freeRef chain called the HSI router with garbage (READ_UNMAPPED
/// 0x13528a8, thread death); the view-rebuild chain (r767) nulls a
/// view-pumped MenuManager slot.  Both lessons are in docs r767/r768.
/// Fire/return site is the router entry push, so each callee runs with its
/// own stack slack and LR is handed back exactly (r761 lesson: entry-push
/// sites consume SP and LR).
/// Resolve a popup-uninit invalidate-loop target to (function, args):
/// 0xFE000000|slot = whole-view GUI_GUIAPI__vInvalidate (0x134ae6c, only
/// ever queued after the host verified its MenuManager global),
/// 0xFD000000 = the app's whole-context redraw on PU_CTX exactly as
/// GUI_GUIAPI__vInvalidate performs it (`ctx->[0x14](ctx, 1)`, resolved
/// live from the object's vtable), plain value = GUI_Widget::vInvalidate
/// (0x137bdac).
fn pu_target_fn(
    uc: &mut Unicorn<'_, Context>,
    base: u32,
    t: u32,
) -> (u32, [u32; 4]) {
    if t & 0xff00_0000 == 0xfe00_0000 {
        (
            base + 0x0134_ae6cu32 - ORIGINAL_BASE,
            [t & 0xff, 0, 0, 0],
        )
    } else if t & 0xff00_0000 == 0xfd00_0000 {
        let ctx = PU_CTX.load(Ordering::Relaxed);
        let vt = read_u32(uc, ctx);
        let f = read_u32(uc, vt + 0x14);
        if (ORIGINAL_BASE..0x0286_0000).contains(&f) {
            (f, [ctx, 1, 0, 0])
        } else {
            (0, [0, 0, 0, 0])
        }
    } else if matches!(t >> 24, 0xf1..=0xf6) {
        // Render triples (DisplayManager::update's per-view block):
        //   main view3    : 0xF1 ctx->[8] | 0xF2 callDraw(v3) | 0xF3 ctx->[0xc]
        //   overlay root  : 0xF4 ctx->[8] | 0xF5 callDraw(ovr) | 0xF6 ctx->[0xc]
        // A resolved vtable slot must point into prochmi text: fix266
        // showed [view3+0x40] is a plain data wrapper (its "+8" word is
        // another pool pointer), so those markers self-skip via fn=0.
        let (obj, ctx) = match t >> 24 {
            0xf1 | 0xf2 | 0xf3 => (PU_V3.load(Ordering::Relaxed), PU_V3CTX.load(Ordering::Relaxed)),
            _ => (PU_OVR.load(Ordering::Relaxed), PU_CTX.load(Ordering::Relaxed)),
        };
        let vt = read_u32(uc, ctx);
        match t >> 24 {
            0xf1 | 0xf4 => {
                let f = read_u32(uc, vt + 8);
                if (ORIGINAL_BASE..0x0286_0000).contains(&f) {
                    (f, [ctx, 0, 0, 0])
                } else {
                    (0, [0, 0, 0, 0])
                }
            }
            0xf2 | 0xf5 => (base + 0x0137_af7cu32 - ORIGINAL_BASE, [obj, 0, 0, 0]),
            _ => {
                let f = read_u32(uc, vt + 0xc);
                if (ORIGINAL_BASE..0x0286_0000).contains(&f) {
                    (f, [ctx, 0, 0, 0])
                } else {
                    (0, [0, 0, 0, 0])
                }
            }
        }
    } else if t & 0xff00_0000 == 0xff00_0000 {
        // View-context full invalidate: slot = bits 15:8, arg = bits 7:0.
        // Queued only after stage 1 verified view != 0 && ctx != 0.
        let slot = (t >> 8) & 0xff;
        let mm = read_u32(uc, 0x02a1_3f50);
        let obj = read_u32(uc, mm + slot * 4);
        let vctx = read_u32(uc, obj + 0x40);
        let vt = read_u32(uc, vctx);
        let f = read_u32(uc, vt + 0x14);
        if (ORIGINAL_BASE..0x0286_0000).contains(&f) {
            (f, [vctx, t & 0xff, 0, 0])
        } else {
            (0, [0, 0, 0, 0])
        }
    } else {
        (base + 0x0137_bdacu32 - ORIGINAL_BASE, [t, 1, 0, 0])
    }
}

fn sm_hide_fire_or_restore(unicorn: &mut Unicorn<'_, Context>, site: u32) {
    let _ = site;
    // r768 stage 1 (router site): hide the popup through the engine's own
    // visibility data.  GUI_Widget::vDraw (0x137bb88) skips every child
    // whose tagged visibility ContentId at +0x3c resolves to 0 - the same
    // word the app's GUI_Widget::bSetProperty writes for the visible
    // property - so a tagged-false store (0x10000000) over the popup
    // container widget drops the whole painted subtree out of the next
    // draw pass.  Damage is then posted by GUI_Widget::vInvalidate
    // (0x137bdac) calls - but NOT from this site: fix255 caught the callee
    // re-entering the router site (its message machinery calls
    // clHSIMngr::bEvent2App) and executing the just-consumed router trap
    // site with clobbered r0/r1 -> READ_UNMAPPED 0x13528a8.  The
    // invalidation loop therefore runs on the clGUIWidgetEngine::bExecute
    // trap site (0x133c134, see the "1145" handler), a clean pump context
    // whose r0 the handler restores on the injected call's return (the
    // fix251 lesson: clobbered r0 there corrupts the engine `this`).
    if SM_HIDE_STATE.load(Ordering::Relaxed) != 1 {
        return;
    }
    if ENAVI_FAB_STATE.load(Ordering::Relaxed) != 3
        || EVT8001_REPOST.load(Ordering::Relaxed) != 0
        // r769: the SM-page hook that captures the 0x1e5 record only
        // exists under EMU_PROCHMI_NAV_TRACE; the paint marker is set
        // unconditionally and is equally good "popup really painted"
        // evidence for a no-env run.
        || (SM_HIDE_RECORD.load(Ordering::Relaxed) == 0
            && POPUP_PAINT_MS.load(Ordering::Relaxed) == 0)
    {
        return;
    }
    // Target list: the popup manager's registered widget ([singleton
    // +0x14], captured by the release-wfind/release-pop probes) plus the
    // owner widgets of the painted script cells (PU_IMPS), deduped and
    // only accepted when they pass a GUI_Widget shape guard (child array
    // pointer + child count).
    let mut tgts: Vec<u32> = Vec::new();
    let w = PU_WIDGET.load(Ordering::Relaxed);
    if w != 0 {
        tgts.push(w);
    }
    let ncell = PU_COUNT.load(Ordering::Relaxed) as usize;
    for k in 0..ncell.min(PU_IMPS.len()) {
        let p = PU_IMPS[k].load(Ordering::Relaxed);
        if p != 0 && !tgts.contains(&p) {
            tgts.push(p);
        }
    }
    let mut tn = 0usize;
    for t in tgts {
        let kids = read_u32(unicorn, t + 0x20);
        let arr = read_u32(unicorn, t + 0x1c);
        if kids > 64 || (arr != 0 && !(0x0380_0000..0x0800_0000).contains(&arr)) {
            log::warn!(
                "PROCHMI popup-uninit: target {t:#x} rejected (kids={kids} arr={arr:#x}) t={}ms",
                crate::libs::dapi::emu_t0_elapsed_ms(),
            );
            continue;
        }
        let old = read_u32(unicorn, t + 0x3c);
        let _ = unicorn.mem_write((t + 0x3c) as u64, &0x1000_0000u32.to_le_bytes());
        log::info!(
            "PROCHMI popup-uninit: hid widget {t:#x} (visible {old:#x} -> tagged false) t={}ms",
            crate::libs::dapi::emu_t0_elapsed_ms(),
        );
        if tn < PU_OBJS.len() {
            PU_OBJS[tn].store(t, Ordering::Relaxed);
            tn += 1;
        }
    }
    PU_COUNT.store(tn as u32, Ordering::Relaxed);
    if tn == 0 {
        SM_HIDE_STATE.store(0, Ordering::Relaxed);
        log::info!(
            "PROCHMI popup-uninit: no widget-shaped targets, channel closed t={}ms",
            crate::libs::dapi::emu_t0_elapsed_ms(),
        );
        return;
    }
    // r768 forensics + heavy trigger: the widget-level vInvalidate of
    // fix256 ran clean but produced no composite (prochmi issues no
    // eglSwapBuffers once idle - the damage never reached the view draw
    // pass).  Also queue the app's whole-view GUI_GUIAPI__vInvalidate
    // (0x134ae6c, slot 0 = main / 1 = overlay): it fully null-guarded, but
    // its MenuManager global is [0x2a12ae0] - only arm a slot when that
    // global matches the capture-proven mm ([0x2a13f50]) and the slot's
    // view object is non-null.  The parent-chain dump below (host reads)
    // shows why the widget rect did not schedule.
    let mm = read_u32(unicorn, 0x02a1_3f50);
    let mm2 = read_u32(unicorn, 0x02a1_2ae0);
    let mut chain = Vec::new();
    {
        let mut w = PU_OBJS[0].load(Ordering::Relaxed);
        for _ in 0..6 {
            if w == 0 {
                break;
            }
            // GUI_Widget::getDrawContext (0x137ae94): [[w+0x28]+0x40].
            let w28 = read_u32(unicorn, w + 0x28);
            let ctx = if w28 != 0 {
                read_u32(unicorn, w28 + 0x40)
            } else {
                0
            };
            chain.push(format!(
                "{w:#x}(vis={:#x} kids={} w28={w28:#x} ctx={ctx:#x} vt={:#x})",
                read_u32(unicorn, w + 0x3c),
                read_u32(unicorn, w + 0x20),
                read_u32(unicorn, w)
            ));
            w = read_u32(unicorn, w + 0x18);
        }
    }
    log::info!(
        "PROCHMI popup-uninit: mm={mm:#x} mm_guiapi={mm2:#x} parent-chain=[{}] t={}ms",
        chain.join(" -> "),
        crate::libs::dapi::emu_t0_elapsed_ms(),
    );
    if mm != 0 && mm2 == mm {
        for slot in [0u32, 1] {
            let obj = read_u32(unicorn, mm + slot * 4);
            if obj != 0 && tn < PU_OBJS.len() {
                PU_OBJS[tn].store(0xfe00_0000 | slot, Ordering::Relaxed);
                tn += 1;
                log::info!(
                    "PROCHMI popup-uninit: queued GUI_GUIAPI__vInvalidate(slot={slot}) view={obj:#x}"
                );
            }
        }
        PU_COUNT.store(tn as u32, Ordering::Relaxed);
    }
    // fix261: the REAL render gate, decoded from clGUIWidgetEngine::
    // bGUIMainloop (0x133be70): every pump iteration calls
    // GUI_DM_DisplayManager::update() (0x133d99c) only when isDirty() -
    // and isViewDirty (0x133d758) = (*(ctx->vtable+4))() on the VIEW's own
    // draw context [pGetView(i)+0x40].  When dirty, update() does the
    // device's own per-view redraw: ctx->[8] (drawBegin) +
    // GUI_Widget::callDraw(view,0,0) + ctx->[0xc] (drawEnd) for every
    // dirty view.  The widget-level damage fix256-258 posted went to the
    // WIDGET-resolved ctx and relied on messaging to mark the view dirty
    // - the dead channel.  So queue the app's whole-context invalidate
    // directly on both views' contexts: main (slot 0, repaints FM1 over
    // the popup area) and overlay (slot 1, the popup's own view - redraws
    // without the now-invisible container).  Marker 0xFF000001|slot.
    {
        let ef = 1u32; // GUI_GUIAPI__vInvalidate passes 1 to ctx->[0x14]
        for slot in [0u32, 1] {
            let obj = read_u32(unicorn, mm + slot * 4);
            let vctx = if obj != 0 { read_u32(unicorn, obj + 0x40) } else { 0 };
            if vctx != 0 && tn < PU_OBJS.len() {
                PU_OBJS[tn].store(0xff00_0000 | (slot << 8) | ef, Ordering::Relaxed);
                tn += 1;
                log::info!(
                    "PROCHMI popup-uninit: queued view[slot={slot}]={obj:#x} ctx={vctx:#x} full invalidate (render-gate open)"
                );
            } else {
                log::info!(
                    "PROCHMI popup-uninit: view[slot={slot}]={obj:#x} ctx={vctx:#x} - no ctx, cannot dirty"
                );
            }
        }
        PU_COUNT.store(tn as u32, Ordering::Relaxed);
    }
    // fix264 finding: the DisplayManager global [0x2a12928] is junk here
    // (0x47514c70 with CODE at +0 - never initialized in our unit), so
    // bGUIMainloop's update()-gate reads random memory and never renders.
    // The engine object gates the swap helper (0x1345d50) on [engine+0x134]
    // - dump the engine's display-related slots to find the REAL display
    // /DisplayManager candidate to repoint the global at.
    {
        let eng = HMI_GUI_POINTER.load(Ordering::Relaxed);
        let mut slots = Vec::new();
        for off in [0x3cu32, 0x40, 0x44, 0x130, 0x134, 0x138, 0x13c] {
            slots.push(format!("{off:#x}={:#x}", read_u32(unicorn, eng + off)));
        }
        // pGetView slots 2/3 too - the FM1 widgets must hang off SOME view.
        let mut vs = Vec::new();
        for slot in [2u32, 3] {
            let v = read_u32(unicorn, mm + slot * 4);
            let kc = read_u32(unicorn, v + 0x20);
            vs.push(format!("v{slot}={v:#x}(kids={kc} ctx={:#x})", read_u32(unicorn, v + 0x40)));
        }
        log::info!(
            "PROCHMI popup-uninit: eng={eng:#x} [{}] {} t={}ms",
            slots.join(" "),
            vs.join(" "),
            crate::libs::dapi::emu_t0_elapsed_ms(),
        );
    }
    // fix263 deep diagnostics: GUI_View IS a GUI_Widget (update() passes
    // the view itself to callDraw), so the main tree hangs in view0's own
    // child slots; ctx = [view+0x40].  Dump both contexts' vtables to map
    // the invalidate/isPending slots, the DM dirty byte, and view0's kids.
    {
        let view0 = read_u32(unicorn, mm);
        let mut kids = Vec::new();
        if view0 != 0 {
            let kc = read_u32(unicorn, view0 + 0x20).min(4);
            let arr = read_u32(unicorn, view0 + 0x1c);
            for k in 0..kc {
                let cw = if arr != 0 { read_u32(unicorn, arr + k * 4) } else { 0 };
                kids.push(format!(
                    "{cw:#x}(w28={:#x} vis={:#x})",
                    read_u32(unicorn, cw + 0x28),
                    read_u32(unicorn, cw + 0x3c)
                ));
            }
        }
        let vctx0 = if view0 != 0 { read_u32(unicorn, view0 + 0x40) } else { 0 };
        let vt0 = read_u32(unicorn, vctx0);
        let vt1 = read_u32(unicorn, PU_CTX.load(Ordering::Relaxed));
        let s0: Vec<String> = (0..7u32)
            .map(|i| format!("{:#x}", read_u32(unicorn, vt0 + i * 4)))
            .collect();
        let s1: Vec<String> = (0..7u32)
            .map(|i| format!("{:#x}", read_u32(unicorn, vt1 + i * 4)))
            .collect();
        let dm = read_u32(unicorn, 0x02a1_2928);
        log::info!(
            "PROCHMI popup-uninit: view0={view0:#x} kids=[{}] ctx0={vctx0:#x} vt0=[{}] | overlay ctx={:#x} vt1=[{}] | dm={dm:#x} dm+0={:#x} dirty3c={} t={}ms",
            kids.join(","),
            s0.join(","),
            PU_CTX.load(Ordering::Relaxed),
            s1.join(","),
            read_u32(unicorn, dm),
            read_u32(unicorn, dm + 0x3c) & 0xff,
            crate::libs::dapi::emu_t0_elapsed_ms(),
        );
        // Queue the MAIN view itself as a vInvalidate target: as a widget
        // its own +0x28 ctx-holder slot may be 0, but its damage path via
        // update() reads [view0+0x40] directly - the guest-call harmlessly
        // no-ops if getDrawContext misses; keep it for the side effects.
        if view0 != 0 && tn < PU_OBJS.len() {
            PU_OBJS[tn].store(view0, Ordering::Relaxed);
            tn += 1;
            PU_COUNT.store(tn as u32, Ordering::Relaxed);
            log::info!("PROCHMI popup-uninit: queued GUI_Widget::vInvalidate(view0 {view0:#x})");
        }
    }
    // fix262: the view-ctx vtable's slot +0x14 is zero (0x391b8e0 is not a
    // fix259: the widget-level damage never schedules a pass, but the ctx
    // is alive (fix259: [[w+0x28]+0x40] = 0x38f7b28) - queue the app's
    // OWN whole-context invalidate: GUI_GUIAPI__vInvalidate performs
    // (*(ctx->[0x14]))(ctx, 1) on the view's draw context (0x134ae6c), the
    // device's sledgehammer redraw trigger.  We cannot call that function
    // (its MenuManager global [0x2a12ae0] is junk here), but the ctx
    // object and vtable call are identical - marker 0xFD000000 resolves
    // the vtable method stage-side and runs it on PU_CTX.
    let w0 = PU_OBJS[0].load(Ordering::Relaxed);
    let w28 = read_u32(unicorn, w0 + 0x28);
    let ctx = if w28 != 0 { read_u32(unicorn, w28 + 0x40) } else { 0 };
    if ctx != 0 && tn < PU_OBJS.len() {
        PU_CTX.store(ctx, Ordering::Relaxed);
        PU_OBJS[tn].store(0xfd00_0000, Ordering::Relaxed);
        tn += 1;
        PU_COUNT.store(tn as u32, Ordering::Relaxed);
        log::info!("PROCHMI popup-uninit: queued ctx->[0x14](ctx={ctx:#x}, 1) whole-context invalidate");
        // Overlay-side triple (fix266): the popup's own layer is drawn by
        // this ctx; repainting its now-empty tree with the update() recipe
        // refreshes that texture without the popup's pixels.
        PU_OVR.store(w28, Ordering::Relaxed);
        for m in [0xf400_0000u32, 0xf500_0000, 0xf600_0000] {
            if tn < PU_OBJS.len() {
                PU_OBJS[tn].store(m, Ordering::Relaxed);
                tn += 1;
            }
        }
        PU_COUNT.store(tn as u32, Ordering::Relaxed);
        log::info!("PROCHMI popup-uninit: queued overlay render triple on ovr={w28:#x} ctx={ctx:#x}");
    }
    // fix266: replay DisplayManager::update()'s OWN render triple on the
    // live main view (pGetView(3), 50 kids = the FM1 tree, fix264) since
    // the DM global [0x2a12928] is junk here and its update()-gate can
    // never run: ctx->[8](ctx) + GUI_Widget::callDraw(v3,0,0) +
    // ctx->[0xc](ctx), exactly as update()'s per-view block does it.  A
    // repaint of the full main tree replaces every pixel of the region,
    // including the orphaned popup's, from the surviving widget data.
    {
        let v3 = read_u32(unicorn, mm + 3 * 4);
        let v3ctx = if v3 != 0 { read_u32(unicorn, v3 + 0x40) } else { 0 };
        let vt = read_u32(unicorn, v3ctx);
        log::info!(
            "PROCHMI popup-uninit: v3={v3:#x} ctx={v3ctx:#x} vt={vt:#x} [4)={:#x} [8)={:#x} [c)={:#x} [10)={:#x}",
            read_u32(unicorn, vt + 4),
            read_u32(unicorn, vt + 8),
            read_u32(unicorn, vt + 0xc),
            read_u32(unicorn, vt + 0x10),
        );
        if v3 != 0 && v3ctx != 0 && read_u32(unicorn, v3 + 0x20) > 0 {
            PU_V3.store(v3, Ordering::Relaxed);
            PU_V3CTX.store(v3ctx, Ordering::Relaxed);
            // The wrapper at [v3+0x40] is data-only (fix266), so only the
            // callDraw leg can run - and only if v3's widget ctx chain
            // resolves ([[v3+0x28]+0x40] non-null, else callDraw's own
            // getDrawContext deref faults).
            let w28v = read_u32(unicorn, v3 + 0x28);
            let wctxv = if w28v != 0 { read_u32(unicorn, w28v + 0x40) } else { 0 };
            log::info!("PROCHMI popup-uninit: v3 w28={w28v:#x} wctx={wctxv:#x}");
            let mut ms = 0u32;
            if wctxv != 0 {
                ms = 0xf200_0000;
            }
            if ms != 0 && tn < PU_OBJS.len() {
                PU_OBJS[tn].store(ms, Ordering::Relaxed);
                tn += 1;
                PU_COUNT.store(tn as u32, Ordering::Relaxed);
                log::info!("PROCHMI popup-uninit: queued main-view callDraw on v3={v3:#x}");
            }
        }
    }
    // Hand over to the bExecute site (0x133c134) for the invalidate loop.
    SM_HIDE_STATE.store(2, Ordering::Relaxed);
    log::info!(
        "PROCHMI popup-uninit: {tn} widgets hidden, invalidation queued on bExecute site t={}ms",
        crate::libs::dapi::emu_t0_elapsed_ms(),
    );
}

fn maybe_repost_evt8001(unicorn: &mut Unicorn<'_, Context>, original_pc: u32, at_post_wipe: bool) {
    // EVT8001_REPOST values: 0 idle/watch, 1 re-queued awaiting pump,
    // 2 pumped awaiting confirmation, 3 delivered (latched), 4 gave up.
    static FIRES: AtomicU32 = AtomicU32::new(0);
    static LOGS: AtomicU32 = AtomicU32::new(0);
    let qm = 0x38ee_ec8u32;
    // clEventQueueManager queue 0 control (fix141 read): read cursor at
    // qm+0x14, write cursor at qm+0x28, queued count at qm+0x50, ring slot 0
    // at qm+0x64.  bQueueEmpty is read==write; bResetQueue zeroes the three
    // counters but not the ring slot itself.
    let reader = read_u32(unicorn, qm + 0x14);
    let writer = read_u32(unicorn, qm + 0x28);
    let slot = read_u32(unicorn, qm + 0x19 * 4);
    let state = EVT8001_REPOST.load(Ordering::Relaxed);
    if state == 3 || state == 4 {
        return;
    }
    let base = PROCHMI_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    if at_post_wipe {
        // r761: the enavi server's startup state machine lives at
        // [[IEnavi]+0x4c proxy]+0xe0 (getter 0x161ce68, the value
        // vHandleEnaviServerStatus stores to handler+0x148 and branches on:
        // 2 -> release wait popup, 5 -> running).  This trap runs on every
        // app-state transition - exactly when the server posts a state-change
        // event - so log unconditionally (fires rarely).
        {
            let proxy = live_srv_proxy(unicorn);
            let srv_state = if proxy != 0 {
                read_u32(unicorn, proxy + 0xe0)
            } else {
                0
            };
            let sink = read_u32(unicorn, 0x38f9_9b0 + 0x82c);
            let gate830 = read_u32(unicorn, 0x38f9_9b0 + 0x830);
            log::info!(
                "PROCHMI enavi-srv-state t={}ms state={srv_state} proxy={proxy:#x} sink={sink:#x} gate830={gate830:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
            );
        }
        // r763: paced popup exit-hide posts.  The wait popup's visual hide
        // is the GUI state machine transition 0x2061 (0x1e5 -> 0x1e4,
        // decoded from the static transition table at 0x25a9568), driven by
        // GUI event 0x8d.  The startup-event flush posts 0x8d ~30 ms BEFORE
        // the SM reaches 0x1e5 (fix214/215 SM-page trace), so it is
        // swallowed by the earlier state and the popup stays painted.  One
        // re-post is not enough either: event 0x8d also has a CHILD handler
        // (0xa02, 0x1e6 -> 0x1e8) that consumes it while the leaf is 0x1e6,
        // exactly like the r757-era batch that had to churn 0x1e6 -> 0x1e8
        // before the 0x1e5 parent's 0x2061 bubbled and hid the widget.  So
        // post a few paced copies (one per app-state transition, ~1 s
        // apart) once the release handler completed.  This site is the
        // post-wipe trap: it runs on every transition even with the GUI
        // idle, unlike bOnMessage which goes silent (fix216: only 3 hits
        // after the release-pop, all before the settle window).
        if FAB_HIDE_EVENT_REMAIN.load(Ordering::SeqCst) != 0 {
            let messaging = GUI_MESSAGING_POINTER.load(Ordering::Relaxed);
            if messaging != 0 {
                let queue = read_u32(unicorn, messaging + 4);
                let posted = post_gui_message_queue(unicorn, queue, messaging, 0x8d);
                let left = if posted {
                    FAB_HIDE_EVENT_REMAIN.fetch_sub(1, Ordering::SeqCst) - 1
                } else {
                    FAB_HIDE_EVENT_REMAIN.load(Ordering::SeqCst)
                };
                log::info!(
                    "PROCHMI: post-fab hide event=0x8d ok={posted} queue={queue:#x} remain={left} t={}ms",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                );
            }
        }
        if state != 0 {
            if state != 3 && state != 4 {
                static REFIRED: AtomicU32 = AtomicU32::new(0);
                if nav_trace_enabled() && REFIRED.fetch_add(1, Ordering::Relaxed) < 8 {
                    log::info!(
                        "PROCHMI evt8001 pump-site re-fire t={}ms state={state} sp={:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        unicorn.reg_read(RegisterARM::R13).unwrap_or(0) as u32,
                    );
                }
            }
            return;
        }
        if FIRES.fetch_add(1, Ordering::Relaxed) >= 60 {
            EVT8001_REPOST.store(4, Ordering::SeqCst);
            log::warn!("PROCHMI evt8001 rescue gave up after 60 write attempts");
            return;
        }
        // Unconsumed release event sitting in a freshly wiped ring?  (The
        // organic write happened earlier this cycle, before the wipe.)
        if reader == 0 && writer == 0 && slot == 0x8100_8001
            // don't clobber the enavi-fab guest call's pending return slot
            && ENAVI_FAB_STATE.load(Ordering::Relaxed) != 2
        {
            let ok = guest_call_via_lr(
                unicorn,
                original_pc,
                base + (0x0135_a310u32 - ORIGINAL_BASE),
                [qm, 0, 0x8100_8001, 0],
                0,
            );
            EVT8001_REPOST.store(1, Ordering::SeqCst);
            if LOGS.fetch_add(1, Ordering::Relaxed) < 8 {
                log::info!(
                    "PROCHMI evt8001 re-queued after wipe t={}ms ok={ok}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                );
            }
        }
        return;
    }
    // dispatch site (runs before this cycle's wipe)
    match state {
        1 => {
            if nav_trace_enabled() {
                static DLOGS: AtomicU32 = AtomicU32::new(0);
                if DLOGS.fetch_add(1, Ordering::Relaxed) < 8 {
                    log::info!(
                        "PROCHMI evt8001 dispatch-site t={}ms reader={reader:#x} writer={writer:#x} sp={:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        unicorn.reg_read(RegisterARM::R13).unwrap_or(0) as u32,
                    );
                }
            }
            if writer != 0 && reader == 0 {
                EVT8001_PUMP_SP.store(
                    unicorn.reg_read(RegisterARM::R13).unwrap_or(0) as u32,
                    Ordering::Relaxed,
                );
                EVT8001_PUMP_LR.store(
                    unicorn.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                    Ordering::Relaxed,
                );
                let ok = guest_call_via_lr(
                    unicorn,
                    original_pc,
                    base + (0x0135_9ca0u32 - ORIGINAL_BASE),
                    [0x38ee_ebc, 0, 0, 0],
                    32,
                );
                EVT8001_REPOST.store(2, Ordering::SeqCst);
                if LOGS.fetch_add(1, Ordering::Relaxed) < 8 {
                    log::info!(
                        "PROCHMI evt8001 engine pumped t={}ms ok={ok}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                    );
                }
            } else if writer == 0 {
                // wiped again before pumping: back to watching (the
                // post-wipe site will re-queue).
                EVT8001_REPOST.store(0, Ordering::SeqCst);
            }
        }
        2 => {
            // The pump returned onto the trap site (its entry push): undo
            // the sp_slack reserved for it and hand back the LR from fire
            // time before the site instruction replays - the cave re-runs
            // the entry `push {...,lr}` and would otherwise store the
            // pump's leftover lr into the dispatcher's frame (same
            // landmine the enavi-fab LR restore closes).
            let sp = EVT8001_PUMP_SP.swap(0, Ordering::Relaxed);
            if sp != 0 {
                let _ = unicorn.reg_write(RegisterARM::R13, sp as u64);
            }
            let lr = EVT8001_PUMP_LR.swap(0, Ordering::Relaxed);
            if lr != 0 {
                let _ = unicorn.reg_write(RegisterARM::R14, lr as u64);
            }
            if reader != 0 {
                EVT8001_REPOST.store(3, Ordering::SeqCst);
                log::info!(
                    "PROCHMI evt8001 consumed by engine pump t={}ms, rescue done",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                );
            } else {
                EVT8001_REPOST.store(0, Ordering::SeqCst);
            }
        }
        _ => {}
    }
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



/// Host mouse -> guest touch: pop at most one pending panel event and drive
/// the guest's own `GUI_TouchPad_v*` entry points, the same functions the
/// device's `clHSI_CMKeyboard::bExecuteMessage` calls for touch-controller
/// samples.  Runs on the GUI mainloop thread at `bGUIMainloop` entry via
/// `guest_call_via_lr` (the stub-based `call_guest_function` mis-executes its
/// synthetic return on current Unicorn - fix144..155), so the callee returns
/// straight onto this same trap site; `TOUCH_RETURN_PENDING` below restores
/// the entry r0/lr that the call clobbered.  Deliberately skipped on the
/// `forced` branches, which already arm their own guest call.
fn drain_gui_touch_event(unicorn: &mut Unicorn<'_, Context>, resume_pc: u32) {
    inject_scripted_touch_test();
    if TOUCH_POST_DUMP.swap(0, Ordering::Relaxed) == 1 {
        let base = PROCHMI_BASE.load(Ordering::Relaxed);
        let slot = read_u32(unicorn, base + GOT_GUI_MENU_MANAGER_INSTANCE);
        let mm = read_u32(unicorn, slot);
        let mut line = format!("PROCHMI touch-post-dump slot={slot:#x} mm={mm:#x}");
        for slot in 0..4u32 {
            let obj = if mm != 0 { read_u32(unicorn, mm + slot * 4) } else { 0 };
            let vt = if obj != 0 { read_u32(unicorn, obj) } else { 0 };
            line.push_str(&format!(" s{slot}={obj:#x}/vt{vt:#x}"));
        }
        let total = crate::gpu::hmi_draw_calls();
        let snap = TOUCH_DRAW_SNAP.load(Ordering::Relaxed);
        let di = DRAW_IMAGE_CALLS.load(Ordering::Relaxed);
        let di_snap = TOUCH_DI_SNAP.load(Ordering::Relaxed);
        line.push_str(&format!(
            " draws={total} d_release={} paint={} d_paint={} vdraw={} mapdraws={}",
            total - snap,
            di,
            di - di_snap,
            VDRAW_CALLS.load(Ordering::Relaxed),
            crate::gpu::map_draw_calls(),
        ));
        TOUCH_POST_COUNTDOWN.store(120, Ordering::Relaxed);
        TOUCH_DRAW_SNAP.store(total, Ordering::Relaxed);
        TOUCH_DI_SNAP.store(di, Ordering::Relaxed);
        log::warn!("{line}");
    } else if TOUCH_POST_COUNTDOWN.load(Ordering::Relaxed) > 0 {
        let left = TOUCH_POST_COUNTDOWN.fetch_sub(1, Ordering::Relaxed);
        if left == 1 {
            let total = crate::gpu::hmi_draw_calls();
            let di = DRAW_IMAGE_CALLS.load(Ordering::Relaxed);
            log::warn!(
                "PROCHMI touch-draw-rate total={} delta_120_iters={} paint={} d_paint_120={}",
                total,
                total - TOUCH_DRAW_SNAP.load(Ordering::Relaxed),
                di,
                di - TOUCH_DI_SNAP.load(Ordering::Relaxed),
            );
        }
    }
    // Page-switch paint pump (see VIEW_PAINT_STATE).  One step per mainloop
    // iteration; never blocks.
    match VIEW_PAINT_STATE.load(Ordering::Relaxed) {
        0 => {}
        1 => {
            if VIEW_PAINT_CD.fetch_sub(1, Ordering::Relaxed) <= 1 {
                // Collect invalidation targets: every live menu-manager slot
                // view's draw context vtable slot +0x14 (GUI_GUIAPI__vInvalidate).
                let base = PROCHMI_BASE.load(Ordering::Relaxed);
                let slot = read_u32(unicorn, base + GOT_GUI_MENU_MANAGER_INSTANCE);
                let mm = read_u32(unicorn, slot);
                let mut n = 0u32;
                for i in 0..4u32 {
                    let obj = if mm != 0 { read_u32(unicorn, mm + i * 4) } else { 0 };
                    let view = if obj != 0 { read_u32(unicorn, obj + 0x28) } else { 0 };
                    let ctx = if view != 0 { read_u32(unicorn, view + 0x40) } else { 0 };
                    let vt = if ctx != 0 { read_u32(unicorn, ctx) } else { 0 };
                    let f = if vt != 0 { read_u32(unicorn, vt + 0x14) } else { 0 };
                    if f >= base && f < base + 0x0286_0000u32 - ORIGINAL_BASE {
                        VIEW_PAINT_FNS[n as usize].store(f, Ordering::Relaxed);
                        VIEW_PAINT_CTXS[n as usize].store(ctx, Ordering::Relaxed);
                        n += 1;
                    }
                }
                VIEW_PAINT_I.store(0, Ordering::Relaxed);
                VIEW_PAINT_CD.store(n, Ordering::Relaxed);
                VIEW_PAINT_STATE.store(if n == 0 { 3 } else { 2 }, Ordering::Relaxed);
                log::warn!(
                    "PROCHMI view-paint: collected {n} invalidate targets t={}ms",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                );
                // r784: which menu-manager slots/views are these?  cids tell
                // whether the destination-entry view (294/653) is even
                // installed in a slot the pump can invalidate.
                if nav_trace_enabled() {
                    for i in 0..4u32 {
                        let obj = if mm != 0 { read_u32(unicorn, mm + i * 4) } else { 0 };
                        let view = if obj != 0 { read_u32(unicorn, obj + 0x28) } else { 0 };
                        let cid = if obj != 0 {
                            read_u32(unicorn, mm + 0x10 + i * 2) & 0xffff
                        } else {
                            0xffff
                        };
                        log::warn!(
                            "PROCHMI view-paint: slot{i} obj={obj:#x} view={view:#x} cid={cid}"
                        );
                        // r784: word-level layout diff between a page that
                        // paints (menu cid) and the invisible destination
                        // page - compare object/view/draw-context bodies.
                        if VIEW_PAINT_ROUND.load(Ordering::Relaxed) <= 1 {
                        for (tag, objaddr) in [
                            ("wid", obj),
                            ("view", view),
                            ("ctx", if view != 0 { read_u32(unicorn, view + 0x40) } else { 0 }),
                        ] {
                            if objaddr == 0 {
                                continue;
                            }
                            let words: Vec<u32> = (0..24u32)
                                .map(|k| read_u32(unicorn, objaddr + k * 4))
                                .collect();
                            log::warn!(
                                "PROCHMI view-paint: slot{i} {tag} {objaddr:#x} w={words:08x?}"
                            );
                        }
                        }
                    }
                }
            }
        }
        2 => {
            let i = VIEW_PAINT_I.load(Ordering::Relaxed) as usize;
            let f = VIEW_PAINT_FNS[i].load(Ordering::Relaxed);
            let ctx = VIEW_PAINT_CTXS[i].load(Ordering::Relaxed);
            if f != 0 {
                // Same register-wrap contract as the touch injection: the
                // callee returns onto this trap site and the entry state is
                // restored by the TOUCH_RETURN_PENDING branch in site 606.
                // r784 note: two safer-looking replacements were tried and
                // both killed prochmi (mainloop exit(1)): call_guest_function
                // writes its frame at live r13, which this site sometimes
                // sees mid-r13-trick (f1: frame landed in the code segment),
                // and call_guest_function_scratch reached the return stub
                // with the frame ignored (f2: stub-entry lr=0x47, pop read
                // the app stack).  The PENDING restore can additionally be
                // consumed by an unrelated site re-fire (c6-era this=0x0 x6).
                // Kept as-is; defect filed in docs/issues/correctness.md.
                for (index, register) in [
                    RegisterARM::R0,
                    RegisterARM::R1,
                    RegisterARM::R2,
                    RegisterARM::R3,
                    RegisterARM::R12,
                    RegisterARM::R13,
                    RegisterARM::R14,
                ]
                .into_iter()
                .enumerate()
                {
                    TOUCH_RETURN_REGS[index].store(
                        unicorn.reg_read(register).unwrap_or(0) as u32,
                        Ordering::Relaxed,
                    );
                }
                TOUCH_RETURN_PENDING.store(1, Ordering::Relaxed);
                let ok = guest_call_via_lr(unicorn, resume_pc, f, [ctx, 1, 0, 0], 64);
                if !ok {
                    TOUCH_RETURN_PENDING.store(0, Ordering::Relaxed);
                }
                log::warn!(
                    "PROCHMI view-paint: invalidated ctx={ctx:#x} fn={f:#x} {}/{} ok={ok} t={}ms",
                    i + 1,
                    VIEW_PAINT_CD.load(Ordering::Relaxed),
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                );
            }
            let next = i as u32 + 1;
            if next >= VIEW_PAINT_CD.load(Ordering::Relaxed) {
                VIEW_PAINT_STATE.store(3, Ordering::Relaxed);
            } else {
                VIEW_PAINT_I.store(next, Ordering::Relaxed);
            }
        }
        _ => {
            // r779: un-block the render pass first.  update() skips the
            // per-view draw block while GUI_DM_EAManager::isBlocked(view)
            // returns true; with the live EA record ([ea+0x124]) its state
            // word (+8) is stuck at 0, while the code path treats 2
            // ("shown") as pass-through (r779 diagnosis: w=[3,4,0,0,1,0]
            // returns 1 only because [eaw+8]!=2).  This is EA lifecycle data
            // the head-unit receives from the media stack - faked device
            // state, same class as the forced app-state/display gates; the
            // drawing itself stays entirely the engine's.
            let dm_slot = read_u32(
                unicorn,
                PROCHMI_BASE.load(Ordering::Relaxed) + GOT_GUI_DM_DISPLAY_MANAGER_SINGLETON,
            );
            let dm = read_u32(unicorn, dm_slot);
            let ea = if dm != 0 { read_u32(unicorn, dm) } else { 0 };
            let eaw = if ea != 0 { read_u32(unicorn, ea + 0x124) } else { 0 };
            if eaw != 0 && read_u32(unicorn, eaw + 8) != 2 {
                let _ = unicorn.mem_write((eaw + 8) as u64, &2u32.to_le_bytes());
                log::warn!(
                    "PROCHMI view-paint: forced EA record {eaw:#x} state->2 t={}ms",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                );
            }
            // Dispatcher nudge + forced present (milestone-1 r768 recipe).
            let messaging = GUI_MESSAGING_POINTER.load(Ordering::Relaxed);
            if messaging != 0 {
                let q = read_u32(unicorn, messaging + 4);
                if post_gui_message_queue(unicorn, q, messaging, 0x8f) {
                    log::warn!(
                        "PROCHMI view-paint: posted dispatcher kick 0x8f t={}ms",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                    );
                }
            }
            crate::gpu::request_forced_present();
            // r784: multi-round pump.  A destination-entry page finishes
            // building its widgets only after the RNW/country lookups land
            // (~2 s after the view-create that armed this pump), so a single
            // invalidate pass runs too early.  Re-arm the (idempotent) pass
            // a few times spaced ~2 s apart.  Default 1 (= r783 behaviour);
            // EMU_VIEW_PAINT_ROUNDS=N enables the extra rounds for forensics
            // (the extra injected calls hit the TOUCH_RETURN_PENDING restore
            // race documented above, so it is not on by default yet).
            static ROUNDS: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
            let cap = *ROUNDS.get_or_init(|| {
                std::env::var("EMU_VIEW_PAINT_ROUNDS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1u32)
            });
            let round = VIEW_PAINT_ROUND.load(Ordering::Relaxed);
            if round < cap {
                VIEW_PAINT_ROUND.store(round + 1, Ordering::Relaxed);
                VIEW_PAINT_CD.store(60, Ordering::Relaxed);
                VIEW_PAINT_STATE.store(1, Ordering::Relaxed);
            } else {
                VIEW_PAINT_ROUND.store(0, Ordering::Relaxed);
                VIEW_PAINT_STATE.store(0, Ordering::Relaxed);
            }
        }
    }
    if GUI_ENGINE_ADDRESS.load(Ordering::Relaxed) == 0 {
        // Widget engine (and with it the GUI_TouchPad timers) not up yet.
        return;
    }
    let Some(event) = crate::input::pop_touch() else {
        // No touch this iteration: the hardwired-key bridge may use the
        // injection slot instead (also one event per pump iteration).
        drain_gui_key_event(unicorn, resume_pc);
        return;
    };
    let base = PROCHMI_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    let (offset, name) = match event.kind {
        crate::input::TouchKind::Press => (TOUCH_PAD_V_PRESSED, "vPressed"),
        crate::input::TouchKind::Motion => (TOUCH_PAD_V_MOVED, "vMoved"),
        crate::input::TouchKind::Release => (TOUCH_PAD_V_RELEASED, "vReleased"),
    };
    // `guest_call_via_lr` overwrites r0-r3/lr (and r13 via the slack):
    // remember the mainloop's entry values and restore them when the callee
    // returns onto this trap site.  (r784: stub-based variants regress here,
    // see the view-paint comment above.)
    for (index, register) in [
        RegisterARM::R0,
        RegisterARM::R1,
        RegisterARM::R2,
        RegisterARM::R3,
        RegisterARM::R12,
        RegisterARM::R13,
        RegisterARM::R14,
    ]
    .into_iter()
    .enumerate()
    {
        TOUCH_RETURN_REGS[index].store(
            unicorn.reg_read(register).unwrap_or(0) as u32,
            Ordering::Relaxed,
        );
    }
    TOUCH_RETURN_PENDING.store(1, Ordering::Relaxed);
    let delivered = guest_call_via_lr(
        unicorn,
        resume_pc,
        base + offset,
        [event.x as u32, event.y as u32, 0, 0],
        64,
    );
    if !delivered {
        TOUCH_RETURN_PENDING.store(0, Ordering::Relaxed);
    }
    let seen = TOUCH_EVENTS_DELIVERED.fetch_add(1, Ordering::Relaxed);
    if event.kind == crate::input::TouchKind::Release && delivered {
        // The page switch runs inside the vReleased call: dump the live
        // menu-manager slots (the r760 dirty walk's objects, with vtables so
        // the classes resolve in Ghidra) on the next drain entry, i.e. after
        // the switch completed.
        TOUCH_DRAW_SNAP.store(crate::gpu::hmi_draw_calls(), Ordering::Relaxed);
        TOUCH_DI_SNAP.store(DRAW_IMAGE_CALLS.load(Ordering::Relaxed), Ordering::Relaxed);
        TOUCH_POST_DUMP.store(1, Ordering::Relaxed);
    }
    if delivered {
        if seen < 40 || seen % 200 == 0 {
            log::info!(
                "PROCHMI: touch #{seen} {name} ({},{}) t={}ms",
                event.x,
                event.y,
                crate::libs::dapi::emu_t0_elapsed_ms(),
            );
        }
    } else if seen < 40 {
        log::warn!(
            "PROCHMI: touch {name} ({},{}) guest call refused",
            event.x,
            event.y
        );
    }
}

/// Headless verification path (`EMU_TOUCH_TEST="168:400:240,172:100:60"`):
/// scripted taps at wall-t seconds after emulator t0, injected through the
/// very same queue the SDL producer feeds, so the guest-side delivery path is
/// identical with and without a display.  Each tap is a Press+Release pair
/// consumed on two consecutive mainloop iterations (~35 ms apart).
fn inject_scripted_touch_test() {
    static SPEC: std::sync::OnceLock<Vec<(u64, u16, u16)>> = std::sync::OnceLock::new();
    let spec = SPEC.get_or_init(|| {
        std::env::var("EMU_TOUCH_TEST")
            .unwrap_or_default()
            .split(',')
            .filter_map(|entry| {
                let parts: Vec<&str> = entry.trim().split(':').collect();
                if parts.len() != 3 {
                    return None;
                }
                let secs = parts[0].trim().parse::<f64>().ok()?;
                let x = parts[1].trim().parse::<i32>().ok()?;
                let y = parts[2].trim().parse::<i32>().ok()?;
                let (x, y) = crate::input::clamp_panel(x, y);
                Some(((secs * 1000.0) as u64, x, y))
            })
            .collect()
    });
    if spec.is_empty() {
        return;
    }
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let index = NEXT.load(std::sync::atomic::Ordering::Relaxed);
    if index >= spec.len() {
        return;
    }
    let (due_ms, x, y) = spec[index];
    let now = crate::libs::dapi::emu_t0_elapsed_ms() as u64;
    if now < due_ms {
        return;
    }
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    crate::input::push_touch(crate::input::TouchKind::Press, x, y);
    crate::input::push_touch(crate::input::TouchKind::Release, x, y);
    log::info!(
        "PROCHMI: touch-test tap #{index} scheduled ({x},{y}) at {due_ms}ms (now {now}ms)"
    );
}

/// All hardwired keys the host keyboard bridge supports, with the
/// `EMU_KEY_TEST` / parse name for each.
const ALL_HARDKEYS: &[(&str, crate::input::HardKey)] = &[
    ("back", crate::input::HardKey::Back),
    ("phone", crate::input::HardKey::Phone),
    ("radio", crate::input::HardKey::Radio),
    ("cd", crate::input::HardKey::Cd),
    ("aux", crate::input::HardKey::Aux),
    ("previous", crate::input::HardKey::Previous),
    ("next", crate::input::HardKey::Next),
    ("camera", crate::input::HardKey::Camera),
    ("map", crate::input::HardKey::Map),
    ("nav", crate::input::HardKey::Nav),
    ("info", crate::input::HardKey::Info),
    ("nightday", crate::input::HardKey::NightDay),
    ("setup", crate::input::HardKey::Setup),
];

/// Host semantic key -> pinned `GUI_KeyInjector` id.  Pinned statically from
/// the rodata key table (`0x0257c85c` = `vKeyPressed`'s pc-relative literal
/// `0x2a12818 + 0xffb6a044`, stride 24) cross-checked against
/// `clHSI_CMKeyboard::bExecuteMessage`'s 47 immediate-id `vKeyPressed` call
/// sites (0x1813e44..0x1814168 dispatch): the injector ids ARE the table
/// rows, so these pairs reproduce real key taps exactly.  `back` rides the
/// SWC-back row (there is no panel `HK_BACK_PRESS` press row; the real unit
/// routes "back" through `SWC_BACK_SHORT`, row 45).  The resolver still
/// cross-checks each row's press content name at runtime and logs the pair.
const PINNED_HARDKEY_ROWS: &[(&str, crate::input::HardKey, u16, &str)] = &[
    ("back", crate::input::HardKey::Back, 45, "SWC_BACK_SHORT"),
    ("phone", crate::input::HardKey::Phone, 8, "HK_PHONE_PRESS"),
    ("radio", crate::input::HardKey::Radio, 7, "HK_RADIO_SHORT"),
    ("cd", crate::input::HardKey::Cd, 10, "HK_CD_PRESS"),
    ("aux", crate::input::HardKey::Aux, 11, "HK_AUX_PRESS"),
    ("previous", crate::input::HardKey::Previous, 12, "HK_PREVIOUS_RELEASE"),
    ("next", crate::input::HardKey::Next, 13, "HK_NEXT_RELEASE"),
    ("camera", crate::input::HardKey::Camera, 32, "HK_CAMERA_RELEASE"),
    ("map", crate::input::HardKey::Map, 17, "HK_MAP_PRESS"),
    ("nav", crate::input::HardKey::Nav, 14, "HK_NAV_PRESS"),
    ("info", crate::input::HardKey::Info, 26, "HK_INFO_PRESS"),
    ("nightday", crate::input::HardKey::NightDay, 30, "HK_ILLUM_PRESS"),
    ("setup", crate::input::HardKey::Setup, 18, "HK_SETTINGS_PRESS"),
];

/// The press-event content name of each supported hardwired key, as named in
/// `GUI_ContentString` (fallback resolution for builds whose pinned rows
/// drift; the row for a key is whichever key-table row carries that content
/// id at +0x0c).
fn hardkey_press_content_name(key: crate::input::HardKey) -> &'static str {
    use crate::input::HardKey;
    match key {
        HardKey::Back => "HK_BACK_PRESS",
        HardKey::Phone => "HK_PHONE_PRESS",
        HardKey::Radio => "HK_RADIO_PRESS",
        HardKey::Cd => "HK_CD_PRESS",
        HardKey::Aux => "HK_AUX_PRESS",
        HardKey::Previous => "HK_PREVIOUS_PRESS",
        HardKey::Next => "HK_NEXT_PRESS",
        HardKey::Camera => "HK_CAMERA_PRESS",
        HardKey::Map => "HK_MAP_PRESS",
        HardKey::Nav => "HK_NAV_PRESS",
        HardKey::Info => "HK_INFO_PRESS",
        HardKey::NightDay => "HK_ILLUM_PRESS",
        HardKey::Setup => "HK_SETTINGS_PRESS",
    }
}

static KEY_INJECTOR_ROWS: OnceLock<
    std::sync::Mutex<std::collections::HashMap<crate::input::HardKey, u16>>,
> = OnceLock::new();

/// Resolve semantic hardkeys to `GUI_KeyInjector_vKeyPressed` ids once, by
/// walking the key table and translating each row's press content id through
/// `GUI_ContentString::ms_apcStrings`.  Dumps the full resolved table so the
/// row layout (and any name the panel build does not actually use) is visible
/// in the log instead of being assumed.
fn resolve_key_injector_rows(unicorn: &mut Unicorn<'_, Context>, base: u32) {
    static DONE: AtomicU32 = AtomicU32::new(0);
    let rows = KEY_INJECTOR_ROWS.get_or_init(Default::default);
    if DONE.swap(1, Ordering::Relaxed) == 1 {
        return;
    }
    // Content id -> name (only HK_* entries matter, but record all names for
    // the row dump).
    let mut names: Vec<String> = Vec::new();
    let mut content_ids: std::collections::HashMap<String, u32> = Default::default();
    for index in 0..8192u32 {
        let pointer = read_u32(unicorn, base + GUI_CONTENT_STRINGS + 4 * index);
        let name = if pointer == 0 {
            String::new()
        } else {
            read_sm_name(unicorn, pointer)
        };
        if !name.is_empty() && name.starts_with("HK_") {
            content_ids.entry(name.clone()).or_insert(index);
        }
        names.push(name);
    }
    let name_of = |id: u32| -> String {
        names.get(id as usize).cloned().unwrap_or_default()
    };
    let mut press_to_row: std::collections::HashMap<u32, u16> = Default::default();
    for row in 0..51u32 {
        let entry = base + KEY_INJECTOR_KEY_TABLE + 24 * row;
        let long_press_ms = read_u32(unicorn, entry);
        let press = read_u16(unicorn, entry + 0xc);
        let short_release = read_u16(unicorn, entry + 0x12);
        let long_release = read_u16(unicorn, entry + 0x14);
        if press != 0 && press != 0xffff {
            press_to_row.entry(press).or_insert(row as u16);
        }
        log::info!(
            "PROCHMI: keyinj row {row} lp={long_press_ms} press={} short={} long={}",
            name_of(press),
            name_of(short_release),
            name_of(long_release),
        );
    }
    // Encoder/volume-event ids reported by the static analysis of
    // vEncoderMoved/vVolumeEncoderMoved: name them for the record.
    for id in [41u32, 42, 68, 69] {
        log::info!("PROCHMI: keyinj content[{id}] = {}", name_of(id));
    }
    let mut resolved = rows.lock().unwrap();
    for (test_name, key, id, expect) in PINNED_HARDKEY_ROWS {
        let press_id =
            read_u16(unicorn, base + KEY_INJECTOR_KEY_TABLE + 24 * u32::from(*id) + 0xc);
        resolved.insert(*key, *id);
        log::info!(
            "PROCHMI: keyinj {test_name} -> id {id} press={} (expect {expect})",
            name_of(press_id),
        );
    }
    for (test_name, key) in ALL_HARDKEYS {
        if resolved.contains_key(key) {
            continue;
        }
        let content_name = hardkey_press_content_name(*key);
        match content_ids.get(content_name).and_then(|id| press_to_row.get(id)) {
            Some(row) => {
                resolved.insert(*key, *row);
            }
            None => log::warn!("PROCHMI: keyinj no table row for {content_name} ({test_name})"),
        }
    }
    log::info!("PROCHMI: keyinj resolved {} hardkeys", resolved.len());
}

/// Host keyboard -> guest hardwired keys: pops at most one queued key event
/// per GUI mainloop iteration and drives the guest's own
/// `GUI_KeyInjector_vKeyPressed/vKeyReleased/v(Volume)EncoderMoved` entry
/// points - the same ones `clHSI_CMKeyboard::bExecuteMessage` calls for real
/// KBD-FI key samples.  Same register-save / `guest_call_via_lr` protocol as
/// the touch bridge (the injected call returns onto this trap site, where
/// `TOUCH_RETURN_PENDING` restores the clobbered entry registers).
fn drain_gui_key_event(unicorn: &mut Unicorn<'_, Context>, resume_pc: u32) {
    inject_scripted_key_test();
    let Some(event) = crate::input::pop_key() else {
        return;
    };
    if GUI_ENGINE_ADDRESS.load(Ordering::Relaxed) == 0 {
        return;
    }
    let base = PROCHMI_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    resolve_key_injector_rows(unicorn, base);
    let (offset, name, arg) = match event {
        crate::input::KeyEvent::Encoder { volume, dir } => (
            if volume {
                KEY_INJECTOR_V_VOLUME_ENCODER_MOVED
            } else {
                KEY_INJECTOR_V_ENCODER_MOVED
            },
            if volume { "volEncoder" } else { "encoder" },
            (dir as u8) as u32,
        ),
        crate::input::KeyEvent::Press(key) | crate::input::KeyEvent::Release(key) => {
            let release = matches!(event, crate::input::KeyEvent::Release(_));
            let row = KEY_INJECTOR_ROWS.get().and_then(|rows| {
                let map = rows.lock().unwrap();
                map.get(&key).copied()
            });
            let Some(row) = row else { return };
            (
                if release {
                    KEY_INJECTOR_V_KEY_RELEASED
                } else {
                    KEY_INJECTOR_V_KEY_PRESSED
                },
                if release { "released" } else { "pressed" },
                row as u32,
            )
        }
    };
    for (index, register) in [
        RegisterARM::R0,
        RegisterARM::R1,
        RegisterARM::R2,
        RegisterARM::R3,
        RegisterARM::R12,
        RegisterARM::R13,
        RegisterARM::R14,
    ]
    .into_iter()
    .enumerate()
    {
        TOUCH_RETURN_REGS[index].store(
            unicorn.reg_read(register).unwrap_or(0) as u32,
            Ordering::Relaxed,
        );
    }
    TOUCH_RETURN_PENDING.store(1, Ordering::Relaxed);
    let delivered = guest_call_via_lr(
        unicorn,
        resume_pc,
        base + offset,
        [arg, 0, 0, 0],
        64,
    );
    if !delivered {
        TOUCH_RETURN_PENDING.store(0, Ordering::Relaxed);
    }
    log::info!(
        "PROCHMI: key {name} {event:?} delivered={delivered} t={}ms",
        crate::libs::dapi::emu_t0_elapsed_ms(),
    );
}

/// Headless verification path (`EMU_KEY_TEST="24:back,30:map,36:zoomin"`):
/// scripted key presses at wall-t seconds after t0 through the same queue the
/// SDL keyboard feeds.  Hardkeys are a Press+Release pair consumed on two
/// consecutive mainloop iterations; encoder events are a single nudge.
fn inject_scripted_key_test() {
    static SPEC: OnceLock<Vec<(u64, crate::input::KeyEvent)>> = OnceLock::new();
    let spec = SPEC.get_or_init(|| {
        std::env::var("EMU_KEY_TEST")
            .unwrap_or_default()
            .split(',')
            .filter_map(|entry| {
                let parts: Vec<&str> = entry.trim().split(':').collect();
                if parts.len() != 2 {
                    return None;
                }
                let secs = parts[0].trim().parse::<f64>().ok()?;
                let name = parts[1].trim().to_ascii_lowercase();
                let event = match name.as_str() {
                    "volup" => crate::input::KeyEvent::Encoder { volume: true, dir: 1 },
                    "voldown" => crate::input::KeyEvent::Encoder { volume: true, dir: -1 },
                    "zoomin" => crate::input::KeyEvent::Encoder { volume: false, dir: 1 },
                    "zoomout" => crate::input::KeyEvent::Encoder { volume: false, dir: -1 },
                    _ => {
                        let (_, key) = ALL_HARDKEYS.iter().find(|(n, _)| *n == name.as_str())?;
                        crate::input::KeyEvent::Press(*key)
                    }
                };
                Some(((secs * 1000.0) as u64, event))
            })
            .collect()
    });
    if spec.is_empty() {
        return;
    }
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let index = NEXT.load(Ordering::Relaxed) as usize;
    if index >= spec.len() {
        return;
    }
    let (due_ms, event) = spec[index];
    let now = crate::libs::dapi::emu_t0_elapsed_ms() as u64;
    if now < due_ms {
        return;
    }
    NEXT.fetch_add(1, Ordering::Relaxed);
    match event {
        crate::input::KeyEvent::Press(key) => {
            crate::input::push_key(crate::input::KeyEvent::Press(key));
            crate::input::push_key(crate::input::KeyEvent::Release(key));
        }
        other => crate::input::push_key(other),
    }
    log::info!(
        "PROCHMI: key-test #{index} {event:?} scheduled at {due_ms}ms (now {now}ms)"
    );
}

/// Best-effort: if `ptr` points into prochmi's rodata and starts a printable
/// ASCII string, return it (SM/record name resolution for diagnostics).
fn read_sm_name(unicorn: &mut Unicorn<'_, Context>, ptr: u32) -> String {
    let base = PROCHMI_BASE.load(Ordering::Relaxed);
    if base == 0 || ptr < base + 0x0200_0000u32 - ORIGINAL_BASE || ptr > base + 0x0400_0000u32 {
        return String::new();
    }
    let mut out = Vec::new();
    for i in 0..48u32 {
        let b = read_u8(unicorn, ptr + i);
        if b == 0 {
            break;
        }
        if !(0x20..0x7f).contains(&b) {
            return String::new();
        }
        out.push(b);
    }
    if out.len() < 3 {
        return String::new();
    }
    String::from_utf8_lossy(&out).into_owned()
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
        if nav_trace_enabled() && HMI_VIEW_DIRTY_FORCE_COUNT.load(Ordering::Relaxed) % 40 == 0 {
            log::warn!(
                "PROCHMI dirty-walk index={index} mm={menu_manager:#x} widget={widget:#x} view={view:#x} ctx={draw_context:#x} ctx28={:#x} ctxb8={:#x}",
                if draw_context != 0 { read_u8(unicorn, draw_context + 0x28) } else { 0xff },
                if draw_context != 0 { read_u32(unicorn, draw_context + 0xb8) } else { 0 },
            );
        }
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
                if name == "GUI_DM_EAManager::isBlocked" {
                    // Reaching this observer already means isViewDirty(view)
                    // was TRUE (update() calls isBlocked only then): the hit
                    // rate IS the dirty-view rate.  Dump the decision inputs
                    // (r779 page-switch render-gate diagnosis, capped).
                    static HITS: AtomicU32 = AtomicU32::new(0);
                    static LATE: AtomicU32 = AtomicU32::new(0);
                    let h = HITS.fetch_add(1, Ordering::Relaxed);
                    let t_ms = crate::libs::dapi::emu_t0_elapsed_ms();
                    // Late-window (tap-window forensics): default covers the
                    // 60 s scripted tap; EMU_ISB_LATE="start:end" (ms) moves
                    // it over other tap schedules (clickability scans).
                    static WIN: std::sync::OnceLock<(u128, u128)> =
                        std::sync::OnceLock::new();
                    let (w0, w1) = *WIN.get_or_init(|| match std::env::var("EMU_ISB_LATE") {
                        Ok(s) => {
                            let mut it = s.split(':').filter_map(|p| p.parse().ok());
                            (
                                it.next().unwrap_or(0),
                                it.next().unwrap_or(u128::MAX),
                            )
                        }
                        Err(_) => (59_500, 66_000),
                    });
                    let late = t_ms > w0 && t_ms < w1;
                    let late_n = if late { LATE.fetch_add(1, Ordering::Relaxed) } else { 999 };
                    if h < 8 || (late && late_n % 60 < 2) {
                        let ea = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                        let view = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                        let eaw = read_u32(uc, ea + 0x124);
                        let ctx = read_u32(uc, view + 0x40);
                        let vt = if ctx != 0 { read_u32(uc, ctx) } else { 0 };
                        log::warn!(
                            "PROCHMI isBlocked#{h} t={}ms ea={ea:#x} view={view:#x} eaw={eaw:#x} w=[{},{},{},{},{},{}] ctx={ctx:#x} vt={vt:#x} s0={:#x} s8={:#x} s10={:#x}",
                            t_ms,
                            if eaw != 0 { read_u8(uc, eaw + 0x34) } else { 0xff },
                            if eaw != 0 { read_u32(uc, eaw + 0x24) } else { 0 },
                            if eaw != 0 { read_u32(uc, eaw + 8) } else { 0 },
                            if eaw != 0 { read_u32(uc, eaw + 0x1c) } else { 0 },
                            if eaw != 0 { read_u32(uc, eaw + 0x20) } else { 0 },
                            if eaw != 0 { read_u8(uc, eaw + 0x2c) } else { 0xff },
                            if vt != 0 { read_u32(uc, vt) } else { 0 },
                            if vt != 0 { read_u32(uc, vt + 8) } else { 0 },
                            if vt != 0 { read_u32(uc, vt + 0x10) } else { 0 },
                        );
                    }
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
