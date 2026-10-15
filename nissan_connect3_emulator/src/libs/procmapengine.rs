use crate::emulator::context::Context;
use crate::gpu;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use unicorn_engine::unicorn_const::Prot;
use unicorn_engine::{RegisterARM, Unicorn};

const ORIGINAL_BASE: u32 = 0x0000_8000;
const AIL_VSTART_APP_ENTRY: u32 = 0x0065_d664 - ORIGINAL_BASE;
const APP_NEW_STATE: u32 = 0x0039_0064 - ORIGINAL_BASE;
const INIT_APP_MAP_ENGINE: u32 = 0x0038_ab50 - ORIGINAL_BASE;
const INIT_APP_REGISTRY_CHECK: u32 = 0x0038_abE4 - ORIGINAL_BASE;
const INIT_APP_MAP_GLOBAL_CHECK: u32 = 0x0038_abf4 - ORIGINAL_BASE;
const INIT_APP_MAP_NEW_RESULT: u32 = 0x0038_ac04 - ORIGINAL_BASE;
const INIT_MAP_ENGINE_VTABLE_RESULT: u32 = 0x0038_ac30 - ORIGINAL_BASE;
const MAP_ENGINE_GLOBAL: u32 = 0x0071_5514 - ORIGINAL_BASE;
const APP_STATE_OFFSET: u32 = 0x30;
const APP_IN_QUEUE_OFFSET: u32 = 0x0c;
const APP_IPC_WAIT_PARAM_OFFSET: u32 = 0x78;
const PWR_PROXY_START_APP_STATE: u32 = 2;
const CCA_DISPATCH: u32 = 0x0066_c8e8 - ORIGINAL_BASE;
const CCA_POWER_HANDLER: u32 = 0x0066_7a30 - ORIGINAL_BASE;
const PORTCONTROL_AIL_BONINIT_RESULT: u32 = 0x0039_037c - ORIGINAL_BASE;
const PORTCONTROL_INIT_RESULT: u32 = 0x0039_03b0 - ORIGINAL_BASE;
const PORTCONTROL_HANDLER_RESULT: u32 = 0x0039_0424 - ORIGINAL_BASE;
const PORTCONTROL_START_MAP_ENGINE: u32 = 0x0053_ae2c - ORIGINAL_BASE;
const AIL_START_MAP_ENGINE_CALL: u32 = 0x0039_0168 - ORIGINAL_BASE;
const AIL_START_MAP_ENGINE_RESULT: u32 = 0x0039_016c - ORIGINAL_BASE;
const PORTCONTROL_HANDLER_FAILURE: u32 = 0x0039_04a4 - ORIGINAL_BASE;
const MAP_TRACE_ERRMEM: u32 = 0x0047_eac4 - ORIGINAL_BASE;
const AIL_HGET_LPM_IN_QUEUE_RESULT: u32 = 0x0065_d660 - ORIGINAL_BASE;
const AIL_SEND_CCA_POWER_MSG: u32 = 0x0066_14e4 - ORIGINAL_BASE;
const AIL_SEND_CCA_POWER_MSG_RESULT: u32 = 0x0066_1578 - ORIGINAL_BASE;
const AIL_POST_IPC_MESSAGE: u32 = 0x0067_0634 - ORIGINAL_BASE;
const GUEST_CALL_STUB_SIZE: u32 = 4;
const PROCMAP_ANSWER_SCRATCH_OFFSET: u32 = 0x100;
const ACTIVE_APP_STATE: u32 = 3;
const MAP_ENGINE_STATE_VAR: u32 = 0x0071_5520 - ORIGINAL_BASE;
const MAP_ENGINE_CONTROL_STATE: u32 = 0x0079_07ac - ORIGINAL_BASE;
const RENDER_CONTROL_START: u32 = 0x003a_95ec - ORIGINAL_BASE;
const RENDER_CONTROL_CREATE_VIEW_JOB: u32 = 0x004c_a9e0 - ORIGINAL_BASE;
const RENDER_CONTROL_CREATE_VIEW_RESULT: u32 = 0x004c_aacc - ORIGINAL_BASE;
const RENDER_CONTROL_GET_VIEW_RESULT: u32 = 0x003a_5d48 - ORIGINAL_BASE;
const RENDER_CONTROL_MAINLOOP_ENTRY: u32 = 0x003a_8b6c - ORIGINAL_BASE;
const RENDER_CONTROL_QUEUE_STATE: u32 = 0x003a_8bec - ORIGINAL_BASE;
const RENDER_CONTROL_WAIT_CALL: u32 = 0x003a_8dd0 - ORIGINAL_BASE;
const RENDER_CONTROL_WAIT_RETURN: u32 = 0x003a_8dd4 - ORIGINAL_BASE;
const MAP_VIEW_MAINLOOP_ENTRY: u32 = 0x0052_2884 - ORIGINAL_BASE;
const MAP_VIEW_WAIT_CALL: u32 = 0x0052_28ac - ORIGINAL_BASE;
const MAP_VIEW_WAIT_RETURN: u32 = 0x0052_28b0 - ORIGINAL_BASE;
const RENDER_JOB_QUEUE_WAIT_ENABLED_ENTRY: u32 = 0x004a_45b8 - ORIGINAL_BASE;
const RENDER_JOB_QUEUE_WAIT_RC_RETURN: u32 = 0x003a_8b88 - ORIGINAL_BASE;
const MAP_VIEW_JOB_QUEUE_WAIT_RETURN: u32 = 0x0052_28a0 - ORIGINAL_BASE;
const RENDER_CONTROL_ADD_JOB: u32 = 0x003a_79b8 - ORIGINAL_BASE;
/// `map_tclJobRC::CreateRCJobRenderPixmapView(view_id_byte*, answer_info*)`
/// (0x4c5b0c): thin wrapper over `Create` producing the render job that runs
/// the DrawList pass.  The canned `MapEngine_TriggerPixmapViewRender` handler
/// bakes it next to a `CreateRCJobSetCameraParameters` (0x4c5ac0) carrying the
/// Flint demo-position literals; calling this factory alone gives the draw
/// kick without the camera churn (r655).
const CREATE_RC_JOB_RENDER_PIXMAP_VIEW: u32 = 0x004c_5b0c - ORIGINAL_BASE;
/// Return site (`ldmia sp!,{r4,pc}`, r0 = job) of the wrapper above.
const CREATE_RC_JOB_RENDER_RESULT: u32 = 0x004c_5b20 - ORIGINAL_BASE;
const RENDER_CONTROL_RENDER_VIEW: u32 = 0x003a_7f78 - ORIGINAL_BASE;
const RC_JOB_START_QUEUE_EXECUTE: u32 = 0x0058_6318 - ORIGINAL_BASE;
const RC_JOB_START_AND_RENDER_EXECUTE: u32 = 0x0058_69e8 - ORIGINAL_BASE;
const RC_JOB_RENDER_PIXMAP_EXECUTE: u32 = 0x004d_61a4 - ORIGINAL_BASE;
const RC_JOB_CREATE_VIEW_EXECUTE: u32 = 0x004c_a6f4 - ORIGINAL_BASE;
const RC_JOB_RENDER_VIEW_EXECUTE: u32 = 0x004d_6b24 - ORIGINAL_BASE;
const MAP_VIEW_RENDER_ENTRY: u32 = 0x0052_87f8 - ORIGINAL_BASE;
const VIEW_RENDER_THREAD_ENTRY: u32 = 0x0053_0264 - ORIGINAL_BASE;
const MAP_RENDERER_SKIP_BRANCH: u32 = 0x0053_0408 - ORIGINAL_BASE;
const MAP_RENDERER_EXECUTE_BODY: u32 = 0x0053_0480 - ORIGINAL_BASE;
const MAP_RENDERER_EXECUTE_ENTRY: u32 = 0x0052_d618 - ORIGINAL_BASE;
const MAP_RENDERER_DRAW_LIST_ENTRY: u32 = 0x0052_ccc8 - ORIGINAL_BASE;
const MAP_RENDERER_DRAW_LIST_VCALL_38: u32 = 0x0052_cd40 - ORIGINAL_BASE;
const MAP_DRAW_BLIT_ICON_ENTRY: u32 = 0x0043_e78c - ORIGINAL_BASE;
const MAP_DRAW_BLIT_ICON_GLOBAL: u32 = 0x0043_ed_e4 - ORIGINAL_BASE;
const MAP_DRAW_BLIT_ICON_TABLE_GLOBAL: u32 = 0x0043_ed_fc - ORIGINAL_BASE;
const MAP_DRAW_BLIT_ICON_GLOBAL_END: u32 = 0x0043_ee_80 - ORIGINAL_BASE;
const MAP_DRAW_BLIT_ICON_NULL_RETURN: u32 = 0x0043_e7_b0 - ORIGINAL_BASE;
const MAP_DRAW_BLIT_ICON_BRANCH_1: u32 = 0x0043_e8_54 - ORIGINAL_BASE;
const MAP_DRAW_BLIT_ICON_BRANCH_2: u32 = 0x0043_e8_68 - ORIGINAL_BASE;
const MAP_DRAW_BLIT_ICON_BRANCH_3: u32 = 0x0043_eb_48 - ORIGINAL_BASE;
const MAP_DRAW_BLIT_ICON_BRANCH_4: u32 = 0x0043_eb_64 - ORIGINAL_BASE;
const MAP_RENDERER_DRAW_LIST_VCALL_3C: u32 = 0x0052_cd9c - ORIGINAL_BASE;
const MAP_RENDERER_DRAW_LIST_TARGET_3C: u32 = 0x003d_59c0 - ORIGINAL_BASE;
const MAP_ELEMENT_LIST_ADD: u32 = 0x003f_1398 - ORIGINAL_BASE;
const MAP_CURRENT_ARRAY_USAGE: u32 = 0x0073_ca14 - ORIGINAL_BASE;
const MAP_DRAW_LAYER_LISTS_ADD_ELEMENT: u32 = 0x003c_373c - ORIGINAL_BASE;
const MAP_JOB_QUEUE_ADD_ELEMENT_ENTRY: u32 = 0x004a_4284 - ORIGINAL_BASE;
const MAP_JOB_QUEUE_ADD_ELEMENT_PRE_POST: u32 = 0x004a_436c - ORIGINAL_BASE;
const MAP_JOB_QUEUE_ADD_ELEMENT_AFTER_POST: u32 = 0x004a_4370 - ORIGINAL_BASE;
const MAP_JOB_QUEUE_ADD_ELEMENT_RETURN: u32 = 0x004a_42d0 - ORIGINAL_BASE;
const MAP_JOB_QUEUE_ADD_ELEMENT_RETURN_ALT: u32 = 0x004a_42d4 - ORIGINAL_BASE;
const MAP_JOB_QUEUE_ADD_ELEMENT_PRE_CRASH: u32 = 0x004a_4380 - ORIGINAL_BASE;
// ail_tclInternalDispatch::vOnServerApplicationStatus: the mbx_1024 self-loop
// posts originate inside it (return address 0x66869c). Observe the
// (service, serverApp, state) tuples it is being fed.
const AIL_ON_SERVER_APP_STATUS: u32 = 0x0066_8578 - ORIGINAL_BASE;
// PortControl_CCAClientHandler_Navi::vHandleNavPosition - the consumer of
// `posfi_tclMsgNavPositionStatus` (svc 0x1e, fn 0x10, op 6 per its accessor
// thunks; the client-function map at file VA 0x70bf20 keys {fn 0x10, svc 0x1e}
// straight onto it).  Feeds the CCP and triggers UpdateMapDataRequest.
const NAVI_HANDLER_VHANDLE_NAV_POSITION: u32 = 0x0054_87b8 - ORIGINAL_BASE;
// ail_bHandleMsgServiceData - the body-thread claim function reached from
// bDispatchCCAMessages' 0x45 branch; routes service data to registered
// client handlers by service id.
const AIL_HANDLE_MSG_SERVICE_DATA: u32 = 0x0066_c128 - ORIGINAL_BASE;
// map_tclPortControl_Platform::vOnNewMessage - the app-interface +0x20
// virtual the claim tail-calls; looks the client handler up in the map at
// platform+0xa0 keyed by the message's u16 at content+0x00.
const PORTCONTROL_VON_NEW_MESSAGE: u32 = 0x003a_4048 - ORIGINAL_BASE;
// ail_tclServiceReference::bIsDataSet(4-arg, wildcard) and
// ail_tclServiceReference::u8GetServiceState - the walk predicates of the
// claim path's server-reference loop (0x66c568/0x66c594); observing them
// live shows the exact tuple compared and the state byte for the fabricated
// posfi reference.  ail_tclServiceRegistry::u8GetServiceState covers the
// other walk.
const AIL_REF_IS_DATASET_FN: u32 = 0x0066_e3d0 - ORIGINAL_BASE;
/// `ail_tclServiceReference::ail_tclServiceReference(app, svc, regid, unique)`
/// - the record the register-confirmation handler appends to the engine's
/// service-reference list (+0 svc, +2 server app, +4 register id, +8 unique
/// id, +0xa/+0xc/+0xe = 0xffff wildcards).
const AIL_REF_CTOR_FN: u32 = 0x0066_e2ec - ORIGINAL_BASE;
const AIL_REF_GET_STATE_FN: u32 = 0x0066_e940 - ORIGINAL_BASE;
const AIL_REGISTRY_GET_STATE_FN: u32 = 0x0066_e444 - ORIGINAL_BASE;
// map_tclNavPropertyHandler::vProcessNavPositionStatus(handler, msg) - called
// with the fully parsed message object; dump the object to map wire offsets.
const NAV_HANDLER_VPROCESS_NAV_POS: u32 = 0x0059_216c - ORIGINAL_BASE;
// map_tclMapDataManager::UpdateMapDataRequest(prio, freq, u32, rect{minlon,
// maxlon, minlat, maxlat}) - the direct consequence of a consumed position.
const MAP_DATA_MANAGER_UPDATE_REQUEST: u32 = 0x0050_494c - ORIGINAL_BASE;
// map_tclPortControl_Platform ctor: caches the platform object so the Navi
// client handler can be found in its handler vector (member +0x94 vector of
// PortControl_CCAClientHandlerBase*, Navi identified by its vtable).
const PORTCONTROL_PLATFORM_CTOR: u32 = 0x0039_f268 - ORIGINAL_BASE;
// Navi client handler vtable (Ghidra VA 0x6bb898 == the runtime value stored
// at handler +0x00).
const NAVI_CLIENT_HANDLER_VTABLE: u32 = 0x006b_b898;
const SURFACE_FLIP_ENTRY: u32 = 0x0055_20d8 - ORIGINAL_BASE;
const RC_JOB_CREATE_VIEW_FINAL: u32 = 0x004c_a780 - ORIGINAL_BASE;
const RC_JOB_RENDER_PIXMAP_INSTANCE_RESULT: u32 = 0x004d_62e8 - ORIGINAL_BASE;
const RC_JOB_RENDER_PIXMAP_VIEW_RESULT: u32 = 0x004d_62fc - ORIGINAL_BASE;
const RC_JOB_RENDER_PIXMAP_VIEW_STATE: u32 = 0x004d_625c - ORIGINAL_BASE;
const RENDER_CONTROL_PEEK_RESULT: u32 = 0x003a_8c20 - ORIGINAL_BASE;
const RENDER_CONTROL_JOB_CAN_EXECUTE_RESULT: u32 = 0x003a_8c34 - ORIGINAL_BASE;
const RENDER_CONTROL_JOB_RESULT: u32 = 0x003a_8df8 - ORIGINAL_BASE;
const RENDER_CONTROL_SECOND_VIRTUAL_RESULT: u32 = 0x003a_8e0c - ORIGINAL_BASE;
const RENDER_CONTROL_EXECUTE_RESULT: u32 = 0x003a_8e9c - ORIGINAL_BASE;
const MAP_DATA_JOB_MAP_DATA_REQUEST_CALL: u32 = 0x004b_9ca4 - ORIGINAL_BASE;
const MAP_DATA_JOB_MAP_DATA_REQUEST_RESULT: u32 = 0x004b_9ca8 - ORIGINAL_BASE;
const MAP_DATA_JOB_HANDLE_REQUEST_RESULT: u32 = 0x004b_6830 - ORIGINAL_BASE;
const MAP_DATA_INT_GET_BLOCK_IDS_CALL: u32 = 0x0051_1fd4 - ORIGINAL_BASE;
const MAP_DATA_INT_GET_BLOCK_IDS_RESULT: u32 = 0x0051_1fd8 - ORIGINAL_BASE;
const MAP_DATA_DAPI_GET_BLOCK_IDS_CALL: u32 = 0x0053_a274 - ORIGINAL_BASE;
const MAP_DATA_DAPI_GET_BLOCK_IDS_ENTRY: u32 = 0x0038_f918 - ORIGINAL_BASE;
const MAP_DATA_CCA_GET_BLOCK_IDS_VCALL: u32 = 0x0053_ddc8 - ORIGINAL_BASE;
const MAP_DATA_CCA_SEND_REQUEST_ENTRY: u32 = 0x0059_1af0 - ORIGINAL_BASE;
const MAP_DATA_CLIENT_VREGISTER_SERVICE: u32 = 0x0059_0668 - ORIGINAL_BASE;
const MAP_DATA_DAPI_SERVICE_ID: u32 = 0x26;
const MAP_DATA_SEND_REQUEST_CONSTRUCTOR_RESULT: u32 = 0x0059_1b68 - ORIGINAL_BASE;
const MAP_DATA_SEND_SERVICE_VCALL: u32 = 0x0059_05ec - ORIGINAL_BASE;
const MAP_DATA_SEND_SERVICE_RESULT: u32 = 0x0059_05f0 - ORIGINAL_BASE;
const MAP_DATA_CCA_GET_BLOCK_IDS_RESULT: u32 = 0x0053_ddcc - ORIGINAL_BASE;
const MAP_DATA_VHANDLE_GET_BLOCK_IDS_ENTRY: u32 = 0x0053_e7ac - ORIGINAL_BASE;
const MAP_DATA_VHANDLE_OPEN_FMC_ENTRY: u32 = 0x0053_e4bc - ORIGINAL_BASE;
const MAP_DATA_VHANDLE_DEFAULT_DATASET_ENTRY: u32 = 0x0053_eb30 - ORIGINAL_BASE;
const MAP_DATA_VHANDLE_LOAD_BLOCKS_ENTRY: u32 = 0x0053_f378 - ORIGINAL_BASE;
const MAP_DATA_VHANDLE_DATABLOCKS_CHANGED_ENTRY: u32 = 0x0053_f6f4 - ORIGINAL_BASE;
const MAP_DATA_DAPI_GET_BLOCK_IDS_RESULT: u32 = 0x0053_a278 - ORIGINAL_BASE;
const MAP_DATA_DAPI_LOAD_BLOCKS_CALL: u32 = 0x0053_a230 - ORIGINAL_BASE;
const MAP_DATA_DAPI_LOAD_BLOCKS_RESULT: u32 = 0x0053_a234 - ORIGINAL_BASE;
const MAP_DATA_DATASET_ID_GLOBAL: u32 = 0x0078_f7f0 - ORIGINAL_BASE;
const MAP_DATA_DATASET_ID_VALIDATED_GLOBAL: u32 = 0x0078_f874 - ORIGINAL_BASE;

/// One-shot instruction tracer over the AIL administration module
/// (0x660000-0x66f000), armed by the `ExamineInfluenceToAdministration` probe
/// when it sees admin type 2 (ServiceStatus). Ghidra misdecodes parts of this
/// region as ARM although the unit runs Thumb, so every static address derived
/// from that listing is suspect; the fetch trace is the ground truth for which
/// code a ServiceStatus processing actually walks.
/// Steps remaining (0 = off), arming thread, and arm count.
static EXAMINE_TRACE_STEPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static EXAMINE_TRACE_TID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0xffff_ffff);
static EXAMINE_TRACE_ARMS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Same for one `ail_bHandleMsgServiceRegisterConf` processing.
static CONF_TRACE_ARMS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
const MAP_DATA_FASTMAP_AVAILABLE_GLOBAL: u32 = 0x0078_f7f8 - ORIGINAL_BASE;
const MAP_DATA_STOP_LOADING_GLOBAL: u32 = 0x0078_f7a0 - ORIGINAL_BASE;
/// Dataset id of the inserted card, taken from the CRYPTNAV volume's
/// DATA/DATASET.CFG (`DATASET_ID{ '1758962541' }`, "sd2-GENJ_EU_I.A v2"). DAPIAPP
/// validates the id of every map-data request against the dataset it read from
/// the medium (`dap_tclDataAccess::u16CheckDatasetIdLocked`), so the id the map
/// engine asks for has to be the one the card carries.
const MAP_DATA_FAKE_DATASET_ID: u32 = 1_758_962_541;
const MAP_DATA_JOB_QUEUE_GLOBAL: u32 = 0x0050_c08c - ORIGINAL_BASE;
const MAP_DATA_MAINLOOP_QUEUE_STATE: u32 = 0x0050_be18 - ORIGINAL_BASE;
const MAP_DATA_MAINLOOP_GET_ELEMENT_RESULT: u32 = 0x0050_be64 - ORIGINAL_BASE;
const MAP_DATA_MAINLOOP_JOB_CAN_EXECUTE: u32 = 0x0050_be94 - ORIGINAL_BASE;
const MAP_DATA_MAINLOOP_WAIT_GET_ELEMENT_RESULT: u32 = 0x0050_bf00 - ORIGINAL_BASE;
const MAP_DATA_MAINLOOP_WAIT_JOB_CAN_EXECUTE: u32 = 0x0050_bf30 - ORIGINAL_BASE;
const AIL_EN_CHECK_CLIENT_MAIL_VALIDITY: u32 = 0x0066_dd00 - ORIGINAL_BASE;
const AIL_EN_CHECK_CLIENT_MAIL_VALIDITY_EPILOGUE: u32 = 0x0066_de18 - ORIGINAL_BASE;
const AIL_EN_POST_MESSAGE_ENTRY: u32 = 0x0066_1a3c - ORIGINAL_BASE;
/// `ail_tclOneThreadAppInterface::bRegisterAsyncExecute`: posts the
/// ServiceRegister request and arms the 9000 ms retry timer.
const AIL_EN_REGISTER_ASYNC_EXECUTE: u32 = 0x0066_0f74 - ORIGINAL_BASE;
/// Retry timer of an unanswered registration request.
const AIL_EN_ASYNC_REGISTER_TOUT: u32 = 0x0066_5920 - ORIGINAL_BASE;
/// Client-side processing of a ServiceRegister conf.
const AIL_EN_HANDLE_ASYNC_REGISTER_CONF: u32 = 0x0066_5720 - ORIGINAL_BASE;
const AIL_EN_POST_MESSAGE_AFTER_VALIDITY: u32 = 0x0066_1b08 - ORIGINAL_BASE;
const AIL_EN_POST_MESSAGE_SUCCESS_PATH: u32 = 0x0066_1bb8 - ORIGINAL_BASE;
const AIL_EN_POST_MESSAGE_RECEIVER_QUEUE_CALL: u32 = 0x0066_1c2c - ORIGINAL_BASE;
const AIL_EN_POST_MESSAGE_POST_RESULT: u32 = 0x0066_1c48 - ORIGINAL_BASE;
const AIL_EN_POST_MESSAGE_DELETE_PATH: u32 = 0x0066_1b9c - ORIGINAL_BASE;
const MAP_TRIGGER_PIXMAP_RENDER: u32 = 0x0053_b36c - ORIGINAL_BASE;
// Diagnostic PLT-site trace points inside rl_tclWindow_ContextHandler_Platform::bCreateWindow
// (procmap+0x581938). These are the instructions immediately after each `bl <svg-plt>` so
// r0 carries the returned handle. Pure logging; does not alter control flow.
const B_CREATE_WINDOW_ENTRY: u32 = 0x0058_1938 - ORIGINAL_BASE;
const B_CREATE_WINDOW_RES_SURFACE_1_RESULT: u32 = 0x0058_2160 - ORIGINAL_BASE;
const B_CREATE_WINDOW_RES_SURFACE_2_RESULT: u32 = 0x0058_2178 - ORIGINAL_BASE;
const B_CREATE_WINDOW_RES_SURFACE_3_RESULT: u32 = 0x0058_2190 - ORIGINAL_BASE;
const B_CREATE_WINDOW_LAYER_TRIPLE_RESULT: u32 = 0x0058_21b4 - ORIGINAL_BASE;
const B_CREATE_WINDOW_EGL_WINDOW_SURFACE_ENTRY: u32 = 0x0058_1f80 - ORIGINAL_BASE;
const B_CREATE_WINDOW_APPLY_IN_SYNC_RESULT: u32 = 0x0058_1d50 - ORIGINAL_BASE;
const MAP_RENDER_VIEW_ID: u32 = 1;
const MAP_RENDER_TRIGGER_INTERVAL: u32 = 20;
const OSAL_THREAD_WAIT: u32 = 0x4851_4fa8;

static PROCMAP_BASE: AtomicU32 = AtomicU32::new(0);
// Set once the engine's administration walk applies our synthesized directory
// ServiceStatus to the svc-0x26 client reference (`vOnServiceStateChange`).
pub(crate) static FASTMAP_STATUS_APPLIED: AtomicBool = AtomicBool::new(false);
static PROCMAP_GUEST_CALL_STUB: AtomicU32 = AtomicU32::new(0);
static APP_STATE_STARTED: AtomicBool = AtomicBool::new(false);
static MAP_POWER_CCA_MODE_SET: AtomicBool = AtomicBool::new(false);
static MAP_IPC_WAIT_REPAIRED: AtomicBool = AtomicBool::new(false);
static MAP_ENGINE_STARTED_AFTER_INIT: AtomicBool = AtomicBool::new(false);
static MAP_RENDER_CONTROL_START_PENDING: AtomicBool = AtomicBool::new(false);
static MAP_VIEW_CREATE_REQUESTED: AtomicBool = AtomicBool::new(false);
static MAP_VIEW_CREATE_MUTATING: AtomicBool = AtomicBool::new(false);
static MAP_VIEW_CREATE_PENDING: AtomicBool = AtomicBool::new(false);
static MAP_VIEW_CREATE_ENQUEUE_PENDING: AtomicBool = AtomicBool::new(false);
static MAP_VIEW_CREATE_JOB_ADDRESS: AtomicU32 = AtomicU32::new(0);
static MAP_VIEW_READY: AtomicBool = AtomicBool::new(false);
static MAP_RENDER_TRIGGER_PENDING: AtomicBool = AtomicBool::new(false);
static MAP_INITIAL_RENDER_TRIGGERED: AtomicBool = AtomicBool::new(false);
/// r655: set once the SceneManager copy-to-active branch has run with a
/// non-empty MO array (the first Kraków roads landed in the active list).
/// From then on the periodic wait-skip kick injects *pure* render jobs
/// (`CreateRCJobRenderPixmapView` + `AddJobToRenderControlQueue`) instead of
/// the canned `MapEngine_TriggerPixmapViewRender`, whose answer chain makes
/// the HMI re-arm the Flint demo camera.
static MAP_MO_COPY_DONE: AtomicBool = AtomicBool::new(false);
static MAP_RENDER_KICK_CAPTURING: AtomicBool = AtomicBool::new(false);
static MAP_RENDER_KICK_ENQUEUE_PENDING: AtomicBool = AtomicBool::new(false);
static MAP_RENDER_KICK_JOB: AtomicU32 = AtomicU32::new(0);
static MAP_RENDER_KICK_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_RENDER_LOOP_TRIGGER_TICK: AtomicU32 = AtomicU32::new(0);
static MAP_RENDER_TRIGGER_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_OSAL_WAIT_SKIP_TICK: AtomicU32 = AtomicU32::new(0);
static MAP_OSAL_WAIT_SKIP_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static RENDER_CONTROL_MAINLOOP_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_VIEW_MAINLOOP_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_JOB_WAIT_SKIP_TICK: AtomicU32 = AtomicU32::new(0);
static MAP_JOB_WAIT_SKIP_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static RENDER_JOB_WAIT_SKIP_TICK: AtomicU32 = AtomicU32::new(0);
static MAP_VIEW_JOB_WAIT_SKIP_TICK: AtomicU32 = AtomicU32::new(0);
static RENDER_JOB_WAIT_SKIP_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static NATURAL_MAP_START_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static NATURAL_MAP_START_RESULT_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_ENGINE_NATURALLY_STARTED: AtomicBool = AtomicBool::new(false);
static MAP_RENDERER_FORCE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_RENDERER_EXECUTE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_RENDERER_DRAW_LIST_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_ELEMENT_LIST_ADD_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DRAW_LAYER_ADD_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_RENDERER_DRAW_LIST_VCALL_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DRAW_BLIT_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DRAW_BLIT_FAKE_CONTEXT: AtomicU32 = AtomicU32::new(0);
static MAP_DRAW_BLIT_BRANCH_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_JOB_QUEUE_ADD_ENTRY_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_JOB_QUEUE_AFTER_POST_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_JOB_QUEUE_PRE_CRASH_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static SURFACE_FLIP_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static AIL_SERVER_APP_STATUS_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static AIL_IPC_POST_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static AIL_B_POST_IPC_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static NAV_POSITION_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static NAV_PROCESS_POS_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static HANDLE_SERVICE_DATA_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static VON_NEW_MESSAGE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static REF_WALK_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static REF_CTOR_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_UPDATE_REQUEST_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static PORTCONTROL_PLATFORM: AtomicU32 = AtomicU32::new(0);
static NAVI_REGISTER_KICK_PENDING: AtomicU32 = AtomicU32::new(0);
static MAP_WAIT_HANDLE_FIXUP: AtomicU32 = AtomicU32::new(0);
static RENDER_LOOP_QUEUE_STATE_LAST: AtomicU32 = AtomicU32::new(0xffff_ffff);
static RENDER_LOOP_QUEUE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static RENDER_CONTROL_ADD_JOB_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static RENDER_CONTROL_RENDER_VIEW_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static RC_JOB_EXECUTE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static RENDER_CONTROL_PEEK_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static RENDER_CONTROL_JOB_CAN_EXECUTE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static RENDER_CONTROL_JOB_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static RENDER_CONTROL_SECOND_VIRTUAL_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static RENDER_CONTROL_EXECUTE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static RENDER_CONTROL_GET_VIEW_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_MAINLOOP_STATE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_MAINLOOP_JOB_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_REQUEST_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_ADDJOB_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_ADDJOB_BRANCH_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
/// Address of the fabricated posfi (request=4) MDM job, set when
/// CreateMDMJobMapDataRequest returns it, so later probes can follow that
/// exact job object through the map-data queue.
static MAP_DATA_POSFI_JOB: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_PMDR_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_PMDR_GATE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_TRIGGER_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_HANDLE_REQUEST_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_DAPI_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static AIL_MAIL_VALIDITY_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static AIL_MAIL_VALIDITY_RESULT_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static AIL_POST_MESSAGE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_CLIENT_SERVICE_REF_NODE: AtomicU32 = AtomicU32::new(0);
// The `FIApplication` instance the engine stored in its map view object
// (`view + 0x10`), captured from `u32SetCameraParameters`.  A fabricated
// rendererfi request has to carry it in payload+0x04: `getRenfIMapViewId`
// 0x3947cc drops the request when it does not equal that field.
pub(crate) static RENFI_APPLICATION_INSTANCE: AtomicU32 = AtomicU32::new(0);
// r639: host-ms timestamp of the last write of the Milwaukee default camera
// (0xc4f45dfb) into `view+0x84`.  The HMI's `MapEngine_TriggerPixmapViewRender`
// handler unconditionally posts `CreateRCJobSetCameraParameters(...Milwaukee)`
// (decompiled 0x53b36c) every ~1.5 s, so the FillActiveMO job rect is Milwaukee
// half the time and the harvest gate is a lottery.  The renfi driver watches
// this stamp and echo-backs a Kraków nudge right after each Milwaukee write,
// making the (job, DOM) centre pair Kraków-aligned in nearly every cycle.
pub(crate) static CAM_MILWAUKEE_STAMP: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static RENFI_HEADER_DUMPS: AtomicU32 = AtomicU32::new(0);
/// Register-id DAPIAPP assigned to the map-data (CCA svc 0x26) client
/// registration, taken from its ServiceRegister conf. DAPI's map worker
/// rejects requests carrying the wildcard 0xffff with error 6 "unknown
/// register", so the emulated client reference must remember it.
static MAP_DATA_REGISTER_ID: AtomicU32 = AtomicU32::new(0xffff);

/// Register-id the emulated client currently holds for the map-data service,
/// or 0xffff while nothing is known.
pub fn map_data_register_id() -> u16 {
    (MAP_DATA_REGISTER_ID.load(Ordering::Relaxed) & 0xffff) as u16
}

/// Called by the OSAL mailbox bridge when a ServiceRegister conf for a CCA
/// service is delivered to procmapengine.
pub fn note_dapi_register_conf(service: u16, register_id: u16) {
    if service as u32 == MAP_DATA_DAPI_SERVICE_ID && register_id != 0xffff {
        MAP_DATA_REGISTER_ID.store(register_id as u32, Ordering::Relaxed);
        log::info!("PROCMAPENGINE: DAPI register id svc={service:#06x} register={register_id:#06x}");
    }
}
/// True while a DAPI client REGISTER has been fired but no RegisterConf
/// (handler register-id still 0xffff) has come back yet.
static MAP_DATA_DAPI_REGISTER_PENDING: AtomicBool = AtomicBool::new(false);
static MAP_DATA_MEDIUM_INITIALIZED: AtomicBool = AtomicBool::new(false);
static MAP_DATA_MEDIUM_FAILURE_LOGGED: AtomicBool = AtomicBool::new(false);
static CCA_BODY_FORWARD_SUPPRESSED: AtomicU32 = AtomicU32::new(0);
static CCA_BODY_PASS_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static INIT_MAP_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static CCA_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static PORTCONTROL_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_INIT_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static AIL_POWER_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static AIL_POWER_DISPATCH_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static AIL_POWER_SWITCH_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static DUPLICATE_PROCMAP_START_CONF_SUPPRESSED: AtomicU32 = AtomicU32::new(0);
const AIL_POWER_SWITCH_TRACE_LIMIT: u32 = 500;

#[derive(Clone, Copy)]
struct SavedCalleeRegs {
    r4: u32,
    r5: u32,
    r6: u32,
    r7: u32,
    r8: u32,
    r9: u32,
    r10: u32,
    sp: u32,
}

thread_local! {
    static MAP_JOB_QUEUE_SAVED_CALLEE_REGS: Cell<Option<SavedCalleeRegs>> = const { Cell::new(None) };
}

fn save_map_job_callee_regs(unicorn: &Unicorn<'_, Context>) {
    let regs = SavedCalleeRegs {
        r4: unicorn.reg_read(RegisterARM::R4).unwrap_or(0) as u32,
        r5: unicorn.reg_read(RegisterARM::R5).unwrap_or(0) as u32,
        r6: unicorn.reg_read(RegisterARM::R6).unwrap_or(0) as u32,
        r7: unicorn.reg_read(RegisterARM::R7).unwrap_or(0) as u32,
        r8: unicorn.reg_read(RegisterARM::R8).unwrap_or(0) as u32,
        r9: unicorn.reg_read(RegisterARM::R9).unwrap_or(0) as u32,
        r10: unicorn.reg_read(RegisterARM::R10).unwrap_or(0) as u32,
        sp: unicorn.reg_read(RegisterARM::SP).unwrap_or(0) as u32,
    };
    MAP_JOB_QUEUE_SAVED_CALLEE_REGS.with(|cell| cell.set(Some(regs)));
}

fn restore_map_job_callee_regs(unicorn: &mut Unicorn<'_, Context>, expected_sp: u32) {
    let saved = MAP_JOB_QUEUE_SAVED_CALLEE_REGS.with(|cell| cell.take());
    if let Some(saved) = saved {
        if saved.sp != expected_sp {
            return;
        }
        unicorn.reg_write(RegisterARM::R4, saved.r4 as u64).unwrap();
        unicorn.reg_write(RegisterARM::R5, saved.r5 as u64).unwrap();
        unicorn.reg_write(RegisterARM::R6, saved.r6 as u64).unwrap();
        unicorn.reg_write(RegisterARM::R7, saved.r7 as u64).unwrap();
        unicorn.reg_write(RegisterARM::R8, saved.r8 as u64).unwrap();
        unicorn.reg_write(RegisterARM::R9, saved.r9 as u64).unwrap();
        unicorn.reg_write(RegisterARM::R10, saved.r10 as u64).unwrap();
    }
}

fn clear_map_job_saved_callee_regs() {
    MAP_JOB_QUEUE_SAVED_CALLEE_REGS.with(|cell| cell.set(None));
}

/// Diagnostics/probe hooks of the render pipeline are registered only when
/// `EMU_PROCMAP_TRACE=1` (they cost a Unicorn callback per executed
/// instruction on their watched pages, see AGENTS.md page-hook notes).
pub fn render_trace_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("EMU_PROCMAP_TRACE").is_some_and(|v| !v.is_empty() && v != "0"))
}

pub fn procmapengine_add_code_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    PROCMAP_BASE.store(base_address, Ordering::Relaxed);
    APP_STATE_STARTED.store(false, Ordering::Relaxed);
    MAP_POWER_CCA_MODE_SET.store(false, Ordering::Relaxed);
    MAP_IPC_WAIT_REPAIRED.store(false, Ordering::Relaxed);
    MAP_ENGINE_STARTED_AFTER_INIT.store(false, Ordering::Relaxed);
    MAP_RENDER_CONTROL_START_PENDING.store(false, Ordering::Relaxed);
    MAP_VIEW_CREATE_REQUESTED.store(false, Ordering::Relaxed);
    MAP_VIEW_CREATE_MUTATING.store(false, Ordering::Relaxed);
    MAP_VIEW_CREATE_PENDING.store(false, Ordering::Relaxed);
    MAP_VIEW_CREATE_ENQUEUE_PENDING.store(false, Ordering::Relaxed);
    MAP_VIEW_CREATE_JOB_ADDRESS.store(0, Ordering::Relaxed);
    MAP_VIEW_READY.store(false, Ordering::Relaxed);
    MAP_RENDER_TRIGGER_PENDING.store(false, Ordering::Relaxed);
    MAP_INITIAL_RENDER_TRIGGERED.store(false, Ordering::Relaxed);
    MAP_MO_COPY_DONE.store(false, Ordering::Relaxed);
    MAP_RENDER_KICK_CAPTURING.store(false, Ordering::Relaxed);
    MAP_RENDER_KICK_ENQUEUE_PENDING.store(false, Ordering::Relaxed);
    MAP_RENDER_KICK_JOB.store(0, Ordering::Relaxed);
    MAP_RENDER_KICK_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_RENDER_LOOP_TRIGGER_TICK.store(0, Ordering::Relaxed);
    MAP_RENDER_TRIGGER_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_OSAL_WAIT_SKIP_TICK.store(0, Ordering::Relaxed);
    MAP_OSAL_WAIT_SKIP_TRACE_COUNT.store(0, Ordering::Relaxed);
    RENDER_CONTROL_MAINLOOP_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_VIEW_MAINLOOP_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_JOB_WAIT_SKIP_TICK.store(0, Ordering::Relaxed);
    MAP_JOB_WAIT_SKIP_TRACE_COUNT.store(0, Ordering::Relaxed);
    RENDER_JOB_WAIT_SKIP_TICK.store(0, Ordering::Relaxed);
    MAP_VIEW_JOB_WAIT_SKIP_TICK.store(0, Ordering::Relaxed);
    RENDER_JOB_WAIT_SKIP_TRACE_COUNT.store(0, Ordering::Relaxed);
    NATURAL_MAP_START_TRACE_COUNT.store(0, Ordering::Relaxed);
    NATURAL_MAP_START_RESULT_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_ENGINE_NATURALLY_STARTED.store(false, Ordering::Relaxed);
    MAP_RENDERER_FORCE_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_RENDERER_EXECUTE_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_RENDERER_DRAW_LIST_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_ELEMENT_LIST_ADD_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_DRAW_LAYER_ADD_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_RENDERER_DRAW_LIST_VCALL_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_DRAW_BLIT_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_DRAW_BLIT_BRANCH_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_JOB_QUEUE_ADD_ENTRY_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_JOB_QUEUE_AFTER_POST_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_JOB_QUEUE_PRE_CRASH_TRACE_COUNT.store(0, Ordering::Relaxed);
    SURFACE_FLIP_TRACE_COUNT.store(0, Ordering::Relaxed);
    AIL_SERVER_APP_STATUS_TRACE_COUNT.store(0, Ordering::Relaxed);
    AIL_IPC_POST_TRACE_COUNT.store(0, Ordering::Relaxed);
    AIL_B_POST_IPC_TRACE_COUNT.store(0, Ordering::Relaxed);
    NAV_POSITION_TRACE_COUNT.store(0, Ordering::Relaxed);
    NAV_PROCESS_POS_TRACE_COUNT.store(0, Ordering::Relaxed);
    HANDLE_SERVICE_DATA_TRACE_COUNT.store(0, Ordering::Relaxed);
    VON_NEW_MESSAGE_TRACE_COUNT.store(0, Ordering::Relaxed);
    REF_WALK_TRACE_COUNT.store(0, Ordering::Relaxed);
    REF_CTOR_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_DATA_UPDATE_REQUEST_TRACE_COUNT.store(0, Ordering::Relaxed);
    PORTCONTROL_PLATFORM.store(0, Ordering::Relaxed);
    NAVI_REGISTER_KICK_PENDING.store(0, Ordering::Relaxed);
    MAP_WAIT_HANDLE_FIXUP.store(0, Ordering::Relaxed);
    POSFI_REF_BLOB.store(0, Ordering::Relaxed);
    POSFI_REF_INSERTED.store(0, Ordering::Relaxed);
    RENDER_LOOP_QUEUE_STATE_LAST.store(0xffff_ffff, Ordering::Relaxed);
    RENDER_LOOP_QUEUE_TRACE_COUNT.store(0, Ordering::Relaxed);
    RENDER_CONTROL_ADD_JOB_TRACE_COUNT.store(0, Ordering::Relaxed);
    RENDER_CONTROL_RENDER_VIEW_TRACE_COUNT.store(0, Ordering::Relaxed);
    RC_JOB_EXECUTE_TRACE_COUNT.store(0, Ordering::Relaxed);
    RENDER_CONTROL_PEEK_TRACE_COUNT.store(0, Ordering::Relaxed);
    RENDER_CONTROL_JOB_CAN_EXECUTE_TRACE_COUNT.store(0, Ordering::Relaxed);
    RENDER_CONTROL_JOB_TRACE_COUNT.store(0, Ordering::Relaxed);
    RENDER_CONTROL_SECOND_VIRTUAL_TRACE_COUNT.store(0, Ordering::Relaxed);
    RENDER_CONTROL_EXECUTE_TRACE_COUNT.store(0, Ordering::Relaxed);
    RENDER_CONTROL_GET_VIEW_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_DATA_MAINLOOP_STATE_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_DATA_MAINLOOP_JOB_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_DATA_REQUEST_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_DATA_ADDJOB_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_DATA_ADDJOB_BRANCH_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_DATA_POSFI_JOB.store(0, Ordering::Relaxed);
    MAP_DATA_PMDR_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_DATA_PMDR_GATE_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_DATA_TRIGGER_TRACE_COUNT.store(0, Ordering::Relaxed);
    CCA_BODY_FORWARD_SUPPRESSED.store(0, Ordering::Relaxed);
    CCA_BODY_PASS_TRACE_COUNT.store(0, Ordering::Relaxed);
    PROCMAP_GUEST_CALL_STUB.store(0, Ordering::Relaxed);
    INIT_MAP_TRACE_COUNT.store(0, Ordering::Relaxed);
    CCA_TRACE_COUNT.store(0, Ordering::Relaxed);
    PORTCONTROL_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_INIT_TRACE_COUNT.store(0, Ordering::Relaxed);
    AIL_POWER_TRACE_COUNT.store(0, Ordering::Relaxed);
    AIL_POWER_DISPATCH_TRACE_COUNT.store(0, Ordering::Relaxed);

    // Procmap's entry thread passes `lpm_in_queue` (mbx_0) as the
    // `param_2` forward-target, expecting the body thread to pick it
    // up. Our PWR-proxy delivers START_CONF to the entry thread the
    // moment mbx_1024 has a pending message, i.e. before the body
    // thread has had a chance to run `vAppBody` (which is what
    // promotes state from 1 to INITIALIZED on real hardware). If we
    // let the forward happen the body-thread queue receives the
    // message but the entry thread's `bDispatchCCAMessages` still
    // drops it (state<2 rule) and the loop then exits. Zeroing R2
    // pins the dispatch on the entry thread; combined with the state
    // bump in `force_map_power_cca_mode` this gets us a working
    // PWR_APP_INITIALIZED without touching any guest code paths.
    let cca_dispatch_addr = base_address + CCA_DISPATCH;
    unicorn
        .add_code_hook(cca_dispatch_addr as u64, cca_dispatch_addr as u64, move |uc, _, _| {
            let queue = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            // One-shot: the early CCA-startup repair seeds the body-thread
            // wait / entry-thread forward target (app+0x78) with the entry
            // queue because MQB_1024 does not exist yet at that point.  The
            // engine assigns its own body-queue handle there as soon as the
            // body thread opens it (r414: first wait used 0x40093a20), and
            // both threads re-read the field every loop iteration - restore
            // the engine's value once the body queue is known so the entry
            // thread forwards onto MQB_1024 (guest post -> IOSC event ->
            // u32Get) and the body thread waits there with its claiming
            // pass, exactly as on the unit.
            if MAP_WAIT_HANDLE_FIXUP.load(Ordering::Relaxed) == 0 {
                let mqb = crate::libs::libosal_linux::message::MQB_1024_INFO
                    .load(Ordering::Relaxed);
                if mqb != 0 && mqb != u32::MAX {
                    let app = read_u32_or_invalid(uc, base_address + MAP_ENGINE_GLOBAL);
                    if app < 0xf000_0000 {
                        let current = read_u32_or_invalid(uc, app + APP_IPC_WAIT_PARAM_OFFSET);
                        if current != mqb && current < 0xf000_0000 {
                            if uc
                                .mem_write(
                                    (app + APP_IPC_WAIT_PARAM_OFFSET) as u64,
                                    &mqb.to_le_bytes(),
                                )
                                .is_ok()
                            {
                                MAP_WAIT_HANDLE_FIXUP.store(1, Ordering::Relaxed);
                                log::info!(
                                    "PROCMAPENGINE: restored body-queue handle {mqb:#x} into map app {app:#x} wait/forward param (was {current:#x})"
                                );
                            }
                        }
                    }
                }
            }
            if queue == u32::MAX {
                // The body thread's claiming pass (param_2 == 0xffffffff).
                // r421 showed tid62 consuming the posfi wake message with
                // zero claim-probe hits, so log every body-pass dispatch's
                // envelope to find where the message is dropped.
                let count = CCA_BODY_PASS_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 24 {
                    let thread = uc.get_data().inner.thread_id();
                    let wrapper = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let header = read_u32_or_invalid(uc, wrapper.wrapping_add(4));
                    log::info!(
                        "PROCMAPENGINE: body-pass dispatch wrapper={wrapper:#x} content={header:#x} sub={:#x} dst={:#x} src={:#x} class={:#x} svc={:#x} regid={:#x} op={:#x} thread=[{thread}] trace={}",
                        read_u32_or_invalid(uc, header) & 0xffff,
                        (read_u32_or_invalid(uc, header) >> 16) & 0xffff,
                        read_u32_or_invalid(uc, header + 8) & 0xffff,
                        read_u32_or_invalid(uc, header + 0xb) & 0xff,
                        read_u32_or_invalid(uc, header + 0x14) & 0xffff,
                        read_u32_or_invalid(uc, header + 0x16) & 0xffff,
                        (read_u32_or_invalid(uc, header + 0x18) >> 16) & 0xff,
                        count + 1
                    );
                }
                return;
            }

            // Keep-on-entry-thread suppression is only wanted for the
            // power/START_CONF handshake messages; CCA service traffic
            // (register, register-conf, service-status, service-data) must
            // keep its forward target or the receiving loop exits and
            // crashes with a stale dispatch state. The message class byte
            // lives at wire offset 0xb.
            let wrapper = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let header = read_u32_or_invalid(uc, wrapper.wrapping_add(4));
            let msg_class = read_u16_or_invalid(uc, header.wrapping_add(0xb)) & 0xff;
            // CCA traffic (register, register-conf, service-status,
            // service-data) keeps its forward target: repair_map_ipc_wait_param
            // pointed the app's wait/forward handle (app+0x78) at the entry
            // queue, and the body thread re-reads that field after every
            // message, so both AIL threads wait on mbx_1024 - the entry
            // thread's same-queue forward is picked up by the body thread's
            // claiming pass (param_2=0xffffffff) instead of a queue nobody
            // drains.  An earlier experiment redirected service-data forwards
            // to MQB_1024 and starved exactly that (r404/r409: one claim,
            // then all forwards stranded).
            if matches!(msg_class, 0x42..=0x45) {
                return;
            }

            if uc.reg_write(RegisterARM::R2, u32::MAX as u64).is_err() {
                return;
            }

            let count = CCA_BODY_FORWARD_SUPPRESSED.fetch_add(1, Ordering::Relaxed);
            if count < 20 {
                log::info!(
                    "PROCMAPENGINE: suppressed body-thread CCA forward by clearing queue handle 0x{:x} at dispatch",
                    queue
                );
            }
        })
        .unwrap();

    // r743: page probes below re-enabled unconditionally: bisection shows the
    // boot stalls without them (TB-layout / pacing effect), keep registered.
    // r641: the class-0x44 (service-status) branch of `bDispatchCCAMessages`
    // gates on the app-interface state at `[this]+0x30` being 3:
    //   0x66cb20 ldr r2,[r8] / ldr r3,[r2,#0x30] / cmp r3,#3 / bne 0x66c93c
    // and 0x66c93c is the silent drop (`mov r6,#1; bl 0x388ef0` releases the
    // message pool cell).  Stalled runs (r638/r639/r640a/b) pop the fabricated
    // svc-0x26 pair 18 times and `vServiceStateChanged` never runs - catch
    // whether the pair reaches the gate at all and what the state value is.
    {
        let gate_hits = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let drop_hits = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let examine_hits = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let post_hits = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let page_lo = base_address + (0x0066_c000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                let r10 = uc.reg_read(RegisterARM::R10).unwrap_or(0) as u32;
                if original == 0x0066_cb20u32 {
                    let count = gate_hits.fetch_add(1, Ordering::Relaxed);
                    if count >= 40 {
                        return;
                    }
                    let thisx = uc.reg_read(RegisterARM::R8).unwrap_or(0) as u32;
                    let app = read_u32_or_invalid(uc, thisx);
                    let state = read_u32_or_invalid(uc, app.wrapping_add(0x30));
                    log::info!(
                        "PROCMAPENGINE: CCA class-44 gate t={}ms thread=[{}] app={:#x} state={:#x} fwdq={:#x} n={}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        app,
                        state,
                        r10,
                        count
                    );
                } else if original == 0x0066_c93cu32 {
                    let count = drop_hits.fetch_add(1, Ordering::Relaxed);
                    if count >= 24 {
                        return;
                    }
                    let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE: CCA dispatch drop t={}ms thread=[{}] cmp_r3={:#x} fwdq={:#x} n={}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        r3,
                        r10,
                        count
                    );
                } else if original == 0x0066_cfa8u32 {
                    // `bl ail_bExamineInfluenceToAdministration(this, 2, msg)`
                    // return for class-0x44 with a forward queue set:
                    // r0 != 0 means administration consumed the status
                    // message here; r0 == 0 falls through to the body post
                    // at 0x66cfc8 (`bl bPostIpcMessage`, logged at the OSAL
                    // layer as `post MQB_1024`).
                    let count = examine_hits.fetch_add(1, Ordering::Relaxed);
                    if count >= 40 {
                        return;
                    }
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let msg = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                    let content = read_u32_or_invalid(uc, msg.wrapping_add(4));
                    log::info!(
                        "PROCMAPENGINE: CCA class-44 examine t={}ms thread=[{}] consumed={:#x} content={:#x} head={:#x} w8={:#x} w14={:#x} state_b18={:#x} n={}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        r0,
                        content,
                        read_u32_or_invalid(uc, content),
                        read_u32_or_invalid(uc, content.wrapping_add(8)),
                        read_u32_or_invalid(uc, content.wrapping_add(0x14)),
                        read_u32_or_invalid(uc, content.wrapping_add(0x18)) & 0xff,
                        count
                    );
                } else if original == 0x0066_cfccu32 {
                    // `bl bPostIpcMessage(..., fwdq, msg, 1)` return for
                    // class-0x44: r0 == 0 means the forward failed and the
                    // message gets rejected through vtable+0x74 (0x66cff8).
                    let count = post_hits.fetch_add(1, Ordering::Relaxed);
                    if count >= 40 {
                        return;
                    }
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let msg = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                    let content = read_u32_or_invalid(uc, msg.wrapping_add(4));
                    log::info!(
                        "PROCMAPENGINE: CCA class-44 post-call t={}ms thread=[{}] ok={:#x} content={:#x} head={:#x} state_b18={:#x} n={}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        r0,
                        content,
                        read_u32_or_invalid(uc, content),
                        read_u32_or_invalid(uc, content.wrapping_add(0x18)) & 0xff,
                        count
                    );
                }
            })
            .unwrap();
    }

    // r643: `ail_bExamineInfluenceToAdministration` case-2 consumed every
    // fabricated svc-0x26 status pair in the stalled runs (r642: consumed=1
    // 40x, no MQB post).  Its tail iterates the client reference list and
    // applies the state via `ail_tclServiceReference::vSetServiceState`
    // (plain byte store at ref+6) ONLY when `bIsDataSet(ref, svc, type,
    // regid, sub)` matches (0xffff args are wildcards; ref+0 svc, +2,
    // +4 regid, +6 state, +8 sub).  Log both to learn whether the pair ever
    // matches a reference (i.e. ever gets applied) or falls into the
    // no-match AutoFunctionUnregister leg.
    {
        let bisect_hits = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let vset_hits = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let page_lo = base_address + (0x0066_e000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original == 0x0066_e3d0u32 {
                    let refx = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let a1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    // Posfi wake messages hit this loop several times a
                    // second; only the svc-0x26 (DAPI fastmap) probes carry
                    // information.
                    if a1 != 0x0026 {
                        return;
                    }
                    let count = bisect_hits.fetch_add(1, Ordering::Relaxed);
                    if count >= 120 {
                        return;
                    }
                    let mut refbuf = [0u8; 12];
                    let ref_ok = uc.mem_read(refx as u64, &mut refbuf).is_ok();
                    let a2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let a3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0);
                    let mut sbuf = [0u8; 4];
                    let stack = if uc.mem_read(sp, &mut sbuf).is_ok() {
                        u32::from_le_bytes(sbuf)
                    } else {
                        0xffff_ffff
                    };
                    let g16 = |o: usize| -> u32 {
                        if ref_ok {
                            u16::from_le_bytes([refbuf[o], refbuf[o + 1]]) as u32
                        } else {
                            0xffff_ffff
                        }
                    };
                    log::info!(
                        "PROCMAPENGINE: CCA ref-match t={}ms thread=[{}] ref={:#x} svc={:#x} w2={:#x} regid={:#x} state={:#x} sub={:#x} args={:#x},{:#x},{:#x},{:#x} n={}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        refx,
                        g16(0),
                        g16(2),
                        g16(4),
                        if ref_ok { refbuf[6] as u32 } else { 0xffff_ffff },
                        g16(8),
                        a1,
                        a2,
                        a3,
                        stack,
                        count
                    );
                } else if original == 0x0066_e43cu32 {
                    let count = vset_hits.fetch_add(1, Ordering::Relaxed);
                    if count >= 24 {
                        return;
                    }
                    let refx = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let state = (uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32) & 0xff;
                    log::info!(
                        "PROCMAPENGINE: CCA ref-state-set t={}ms thread=[{}] ref={:#x} new_state={} n={}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        refx,
                        state,
                        count
                    );
                }
            })
            .unwrap();
    }

    // r646: `GetLeftBlocks` only fills the RCMOUPD job's MO list when the
    // raw-data block for a requested block id is already in
    // `map_tclRawMapDataCache` (global 0x790704: [obj]=count, [obj+0xc]=id
    // array, entries -> block, block+4/+8 = uid).  Empty cache => empty job
    // list => `last mo of lod` (job+0x50) stays 0 => the copy-to-active
    // branch never runs.  Log every ContainsBlockID lookup with the cache
    // size to see whether the raw blocks ever arrive (LoadBlocks tail).
    {
        let probes = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let glo = base_address + (0x0051_5eecu32 - ORIGINAL_BASE);
        let page_lo = base_address + (0x0051_5000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original != glo {
                    return;
                }
                let count = probes.fetch_add(1, Ordering::Relaxed);
                if count >= 300 {
                    return;
                }
                let uid = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let cache = read_u32_or_invalid(
                    uc,
                    base_address + (0x0079_0704u32 - ORIGINAL_BASE),
                );
                let uid0 = read_u32_or_invalid(uc, uid);
                let uid1 = read_u32_or_invalid(uc, uid.wrapping_add(4));
                let ccount = read_u32_or_invalid(uc, cache);
                let arr = read_u32_or_invalid(uc, cache.wrapping_add(0xc));
                let first = read_u32_or_invalid(uc, arr);
                let first_uid = read_u32_or_invalid(uc, first.wrapping_add(4));
                log::info!(
                    "PROCMAPENGINE render trace rawcache-lookup t={}ms thread=[{}] uid={:#x},{:#x} cache={:#x} count={:#x} first={:#x},{:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    uid0,
                    uid1,
                    cache,
                    ccount,
                    first,
                    first_uid,
                    count
                );
            })
            .unwrap();
    }

    // r647: the raw-data cache only gets entries through
    // `Convert{FastMap,FastMap3DBinary,FastMapImage,Pyramid}BlockFromShared
    // Memory` -> `CreateNewRawDataBlock`/`AddNewRawDataBlock` (DAPI hands the
    // block over in shared memory).  r646: 50 ContainsBlockID lookups with
    // real Krakow uids but rawcount stayed 0 - find whether the convert
    // functions run at all and whether Create/Add are reached.
    {
        let probes = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let glo = base_address + (0x0050_d1a0u32 - ORIGINAL_BASE);
        let page_lo = base_address + (0x0050_d000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original != glo {
                    return;
                }
                let count = probes.fetch_add(1, Ordering::Relaxed);
                if count >= 100 {
                    return;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                let w0 = read_u32_or_invalid(uc, r0);
                let w1 = read_u32_or_invalid(uc, r1);
                log::info!(
                    "PROCMAPENGINE render trace cvtFMB-enter t={}ms thread=[{}] r0={:#x} r1={:#x} w0={:#x} w1={:#x} lr={:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    r0,
                    r1,
                    w0,
                    w1,
                    lr,
                    count
                );
            })
            .unwrap();
        let probes2 = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let t_create = base_address + (0x0051_66dcu32 - ORIGINAL_BASE);
        let t_add = base_address + (0x0051_64e0u32 - ORIGINAL_BASE);
        let page_lo = base_address + (0x0051_6000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                let which = if original == t_create {
                    "Create"
                } else if original == t_add {
                    "Add"
                } else {
                    return;
                };
                let count = probes2.fetch_add(1, Ordering::Relaxed);
                if count >= 100 {
                    return;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE render trace rawcache-{which} t={}ms thread=[{}] r0={:#x} w0={:#x},{:#x} lr={:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    r0,
                    read_u32_or_invalid(uc, r0),
                    read_u32_or_invalid(uc, r0.wrapping_add(4)),
                    lr,
                    count
                );
            })
            .unwrap();
    }

    // r649: `GetLeftBlocks` sets the RCMOUPD job's `last mo of lod` byte via
    // the single store `0x506370 strbne r3,[r7,#0x50]` (r3=1 only when r7,
    // the job pointer at leg entry 0x506340, is non-null).  r648 reached
    // state2 (`stored28/new28=0x2`) and a non-empty raw-data cache but
    // `new50=0` at every Combine - instrument the leg entry (r7, +0x1f4
    // refetch counter, +0x74 state, block count, the [r8]>1 gate), the two
    // other Create legs (0x506248/0x506264), the store itself, and
    // `GetLinkedMapObjectsForRCUse` (0x513a6c) which fills the job's MO list
    // from a cache-hit block.
    {
        let leg = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let t_leg = base_address + (0x0050_6340u32 - ORIGINAL_BASE);
        let t_store = base_address + (0x0050_6370u32 - ORIGINAL_BASE);
        let t_c2 = base_address + (0x0050_6248u32 - ORIGINAL_BASE);
        let t_c3 = base_address + (0x0050_6264u32 - ORIGINAL_BASE);
        let page_lo = base_address + (0x0050_6000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                let name = match original {
                    a if a == t_leg => "gbi-leg",
                    a if a == t_store => "gbi-store50",
                    a if a == t_c2 => "gbi-create-a",
                    a if a == t_c3 => "gbi-create-b",
                    _ => return,
                };
                let count = leg.fetch_add(1, Ordering::Relaxed);
                if count >= 300 {
                    return;
                }
                let req = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                let job = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                let r8v = uc.reg_read(RegisterARM::R8).unwrap_or(0) as u32;
                let state = read_u32_or_invalid(uc, req.wrapping_add(0x74));
                let n = if state < 8 { state } else { 0 };
                let cnt114 = read_u32_or_invalid(uc, req.wrapping_add(n * 0x10 + 0x114));
                let f1f4 = read_u32_or_invalid(uc, req.wrapping_add(0x1f4));
                let f1f8 = read_u32_or_invalid(uc, req.wrapping_add(0x1f8));
                let g8 = read_u32_or_invalid(uc, r8v);
                log::info!(
                    "PROCMAPENGINE render trace gbl-{name} t={}ms thread=[{}] req={:#x} job={:#x} state74={:#x} cnt114={:#x} f1f4={:#x} f1f8={:#x} r8={:#x}->{:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    req,
                    job,
                    state,
                    cnt114,
                    f1f4,
                    f1f8,
                    r8v,
                    g8,
                    count
                );
            })
            .unwrap();
        let linked = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let t_linked = base_address + (0x0051_3a6cu32 - ORIGINAL_BASE);
        let page_lo = base_address + (0x0051_3000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original != t_linked {
                    return;
                }
                let count = linked.fetch_add(1, Ordering::Relaxed);
                if count >= 300 {
                    return;
                }
                let block = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let out = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE render trace linked-MO t={}ms thread=[{}] block={:#x} uid={:#x},{:#x} out={:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    block,
                    read_u32_or_invalid(uc, block.wrapping_add(4)),
                    read_u32_or_invalid(uc, block.wrapping_add(8)),
                    out,
                    count
                );
            })
            .unwrap();
    }

    // r650: the linked-MO hits of r649 come from legs *before* the +0x50
    // leg (0x5059a4/0x505f74/0x506004/0x506594).  Probe the GetLeftBlocks
    // decision chain: entry 0x5054f4, the refetch-counter leg 0x505828
    // (r12=[0x1f4]+1 vs [lod+0x114] count), the `0x505858 cmp [r8],#1`
    // gate, the `0x505860/0x505868` sentinel tests and the type-5 branch
    // 0x50587c that jumps to the +0x50 store leg.
    {
        let legs = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let targets: [(u32, &str); 7] = [
            (0x0050_54f4, "entry"),
            (0x0050_5634, "leg-idle"),
            (0x0050_5664, "leg-skip"),
            (0x0050_5700, "leg-low-gate"),
            (0x0050_5828, "gate-f1f4"),
            (0x0050_5858, "gate-r8"),
            (0x0050_5868, "gate-f1f8"),
        ];
        let page_lo = base_address + (0x0050_5000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                let Some((_, name)) = targets.iter().find(|(a, _)| *a == original) else {
                    return;
                };
                let count = legs.fetch_add(1, Ordering::Relaxed);
                if count >= 500 {
                    return;
                }
                let req = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                let job = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                let r8v = uc.reg_read(RegisterARM::R8).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let r12 = uc.reg_read(RegisterARM::R12).unwrap_or(0) as u32;
                let state = read_u32_or_invalid(uc, req.wrapping_add(0x74));
                let n = if state < 8 { state } else { 0 };
                let cnt114 = read_u32_or_invalid(uc, req.wrapping_add(n * 0x10 + 0x114));
                let f1f4 = read_u32_or_invalid(uc, req.wrapping_add(0x1f4));
                let f1f8 = read_u32_or_invalid(uc, req.wrapping_add(0x1f8));
                let tp = read_u32_or_invalid(uc, req);
                let g8 = read_u32_or_invalid(uc, r8v);
                log::info!(
                    "PROCMAPENGINE render trace gbl-{name} t={}ms thread=[{}] req={:#x} job={:#x} type={:#x} state74={:#x} cnt114={:#x} f1f4={:#x} f1f8={:#x} r1={:#x} r12={:#x} r8={:#x}->{:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    req,
                    job,
                    tp,
                    state,
                    cnt114,
                    f1f4,
                    f1f8,
                    r1,
                    r12,
                    r8v,
                    g8,
                    count
                );
            })
            .unwrap();
    }

    // r655: capture the job pointer produced by the pure-render kick.  The
    // factory wrapper 0x4c5b0c returns through 0x4c5b20 (`ldmia sp!,{r4,pc}`,
    // r0 = job); hook the whole page and dispatch on the fetch address (a
    // narrow hook on an already-translated Thumb TB can silently never fire).
    {
        let page_lo = base_address + (0x004c_5000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original != CREATE_RC_JOB_RENDER_RESULT + ORIGINAL_BASE {
                    return;
                }
                if !MAP_RENDER_KICK_CAPTURING.swap(false, Ordering::Relaxed) {
                    return;
                }
                let job = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                if job == 0 || job == u32::MAX {
                    log::warn!("PROCMAPENGINE: pure render kick factory returned no job");
                    return;
                }
                // `map_tclJobRCRenderPixmapView::Execute` looks the view up
                // through the byte at job+0x1c; the factory does not fill it
                // (same gap `queue_view_creation_job` patches for the create
                // job), so stamp the render view id here or Execute fails
                // `GetView(0)` and the job never runs (r655).  The byte at
                // +0x1d is the job's phase flag: the pooled memory comes back
                // from the allocator with `1` left by a previous lifecycle
                // (r656), which sends Execute into the re-queue-into-view-queue
                // phase and the job never draws - force phase 0.
                if uc
                    .mem_write((job + 0x1c) as u64, &[MAP_RENDER_VIEW_ID as u8, 0u8])
                    .is_err()
                {
                    log::warn!("PROCMAPENGINE: failed to stamp view id on pure render job {:#x}", job);
                    return;
                }
                MAP_RENDER_KICK_JOB.store(job, Ordering::Relaxed);
                MAP_RENDER_KICK_ENQUEUE_PENDING.store(true, Ordering::Relaxed);
                let count = MAP_RENDER_KICK_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 20 {
                    log::info!(
                        "PROCMAPENGINE: pure render kick created job {:#x} t={}ms thread=[{}] n={}",
                        job,
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        count
                    );
                }
            })
            .unwrap();
    }

    // r743: page probes below re-enabled unconditionally: bisection shows the
    // boot stalls without them (TB-layout / pacing effect), keep registered.
    // r658: which leg does `map_tclJobRCRenderPixmapView::Execute` take?  r657
    // shows the kick jobs taken (GetPeekedElement x14) with zero GL, so
    // Execute must bail before RenderView.  Probe the four decision legs:
    // mapstate error (0x4d6218), GetView==0 (0x4d6234, r0 = view), the
    // view+0xf50 test (0x4d6248, r0 = view), the draw leg (0x4d6500) and the
    // phase-B re-queue leg (0x4d62cc).
    {
        let ex_legs = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let page_lo = base_address + (0x004d_6000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                let name = match original {
                    0x004d_6234u32 => "getview1",
                    0x004d_6248u32 => "f50-test",
                    0x004d_6500u32 => "draw-leg",
                    0x004d_62ccu32 => "phaseB",
                    0x004d_6218u32 => "bad-mapstate",
                    _ => return,
                };
                let count = ex_legs.fetch_add(1, Ordering::Relaxed);
                if count >= 60 {
                    return;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let f50 = if r0 == 0 || r0 == u32::MAX {
                    u32::MAX
                } else {
                    read_u32_or_invalid(uc, r0.wrapping_add(0xf50))
                };
                log::info!(
                    "PROCMAPENGINE render trace rpex-{name} t={}ms thread=[{}] r0={:#x} f50={:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    r0,
                    f50,
                    count,
                );
            })
            .unwrap();
    }

    // r659: DTMStreet::Draw gates the whole GL leg on the shader program
    // slot (`ldr r3,[r2,r4]; cmp r3,#0; beq skip` at 0x55b360..68, slot =
    // rl_tclOpenGLShader::ms_arShaderPrograms + {0x19,0x20}*0x150).  r658
    // binds 526 VBOs after the copy but never reaches a glDraw* - probe the
    // handle to see which slot is empty.
    {
        let sh_checks = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let page_lo = base_address + (0x0055_b000u32 - ORIGINAL_BASE);
        let page_hi = base_address + (0x0055_bfffu32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original != 0x0055_b364u32 {
                    return;
                }
                let count = sh_checks.fetch_add(1, Ordering::Relaxed);
                if count >= 40 {
                    return;
                }
                let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                let slot = r2.wrapping_add(r4);
                let name_ptr = read_u32_or_invalid(uc, slot.wrapping_add(0x14c));
                let name = if name_ptr == 0 || name_ptr == u32::MAX || name_ptr < 0x1000 {
                    "<none>".to_string()
                } else {
                    read_cstring(uc, name_ptr)
                };
                // Resolve the uniform locations the engine will upload the
                // clip matrices to (u_matViewProj at +0x4, u_matModelView at
                // +0x8 - see `vFindUniformLocations` name table) so the
                // `glUniformMatrix4fv` trace can be correlated with them.
                let loc_vp = read_u32_or_invalid(uc, slot.wrapping_add(0x4));
                let loc_mv = read_u32_or_invalid(uc, slot.wrapping_add(0x8));
                log::info!(
                    "PROCMAPENGINE render trace dtm-shader t={}ms thread=[{}] slot={:#x} handle={:#x} name={} loc_vp={:#x} loc_mv={:#x} mcp={:#x} lod={:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    slot,
                    r3,
                    name,
                    loc_vp,
                    loc_mv,
                    r6,
                    read_u8_or_invalid(uc, r6.wrapping_add(5)),
                    count,
                );
            })
            .unwrap();
    }

    if std::env::var_os("EMU_PROCMAPENGINE_TRACE_INIT")
        .is_some_and(|value| !value.is_empty() && value != "0")
    {
        add_init_trace_hooks(unicorn, base_address);
        add_port_control_trace_hooks(unicorn, base_address);
        add_map_engine_init_trace_hooks(unicorn, base_address);
        add_render_control_trace_hooks(unicorn, base_address);
        add_b_create_window_trace_hooks(unicorn, base_address);
        add_ail_power_trace_hooks(unicorn, base_address);
        add_cca_trace_hooks(unicorn, base_address);
    }
    if render_trace_enabled() {
    add_ail_power_dispatch_state_hook(unicorn, base_address);
    add_procmap_switch_trace_hook(unicorn, base_address);
    }
    suppress_duplicate_procmap_start_conf(unicorn, base_address);
    if render_trace_enabled() {
    add_procmap_type3_state_hook(unicorn, base_address);
    }

    let repair_addr = base_address + PORTCONTROL_INIT_RESULT;
    let activate_after_init = std::env::var_os("EMU_PROCMAPENGINE_SKIP_INITIAL_RENDER_TRIGGER")
        .is_none_or(|value| value.is_empty() || value == "0");
    let force_start_after_init = std::env::var_os("EMU_PROCMAPENGINE_FORCE_START_AFTER_INIT")
        .is_some_and(|value| !value.is_empty() && value != "0");
    unicorn
        .add_code_hook(repair_addr as u64, repair_addr as u64, move |uc, _, _| {
            if uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32 != 0 {
                repair_map_ipc_wait_param(uc);
                if force_start_after_init {
                    start_map_engine_after_init(uc, repair_addr);
                }
            }
        })
        .unwrap();

    add_street_coord_buffer_probe_hooks(unicorn, base_address);

    add_natural_map_engine_start_hooks(unicorn, base_address);

    let handler_hook_addr = base_address + PORTCONTROL_HANDLER_RESULT;
    unicorn
        .add_code_hook(
            handler_hook_addr as u64,
            handler_hook_addr as u64,
            move |uc, _, _| {
                if uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32 != 0 {
                    trigger_pending_map_render(uc, handler_hook_addr, false);
                }
            },
        )
        .unwrap();

    if activate_after_init {
        let render_hook_addr = base_address + RENDER_CONTROL_QUEUE_STATE;
        unicorn
            .add_code_hook(
                render_hook_addr as u64,
                render_hook_addr as u64,
                move |uc, _, _| {
                    if !MAP_VIEW_READY.load(Ordering::Relaxed) {
                        return;
                    }

                    let tick = MAP_RENDER_LOOP_TRIGGER_TICK.fetch_add(1, Ordering::Relaxed) + 1;
                    if tick == 1 || tick % MAP_RENDER_TRIGGER_INTERVAL == 0 {
                        trigger_pending_map_render(uc, render_hook_addr, tick != 1);
                    }
                },
            )
            .unwrap();
        add_render_thread_trace_hooks(unicorn, base_address);
        add_render_job_queue_wait_skip_hook(unicorn, base_address);
        add_forced_renderer_execute_hook(unicorn, base_address);
        add_map_job_queue_add_element_trace_hooks(unicorn, base_address);
        add_job_queue_wait_skip_hook(unicorn, base_address);
    }

    add_map_view_hooks(unicorn, base_address);
    // NB: these two families look like trace hooks but their callbacks
    // fabricate the DAPI map service reference and drive guest jobs
    // (`ensure_dapi_map_service_reference`, `synthesize_client_service_reference`,
    // `call_guest_function`) - they must always be registered (r743 bisection).
    add_map_data_main_loop_trace_hooks(unicorn, base_address);
    add_get_block_ids_answer_trace_hooks(unicorn, base_address);
    // r743: NOT a trace - `vOnServiceStateChange` observer sets
    // FASTMAP_STATUS_APPLIED (stops the directory re-post driver) and the
    // family watches MDM globals the provider logic reads.  Keep always on.
    add_fastmap_channel_trace_hooks(unicorn, base_address);

    let force_early_active = std::env::var("EMU_PROCMAPENGINE_FORCE_ACTIVE_STATE")
        .is_ok_and(|value| !value.is_empty() && value != "0");
    if !force_early_active {
        log::info!(
            "PROCMAPENGINE: startup hooks loaded; active state is requested after PortControl init"
        );
    }

    let hook_addr = base_address + AIL_VSTART_APP_ENTRY;
    let state_function = base_address + APP_NEW_STATE;

    unicorn
        .add_code_hook(hook_addr as u64, hook_addr as u64, move |uc, _, _| {
            let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            if this == 0 {
                log::warn!("PROCMAPENGINE: vStartAppEntry called with null app object");
                return;
            }

            if force_early_active && !APP_STATE_STARTED.swap(true, Ordering::Relaxed) {
                if call_guest_function(
                    uc,
                    hook_addr,
                    state_function,
                    [this, 0, ACTIVE_APP_STATE, 0],
                ) {
                    log::info!(
                        "PROCMAPENGINE: forced active app state on map engine object {:#x}",
                        this
                    );
                } else {
                    APP_STATE_STARTED.store(false, Ordering::Relaxed);
                }
            }
        })
        .unwrap();
}

fn add_init_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    for (name, offset) in [
        ("s32InitAppMapEngine", INIT_APP_MAP_ENGINE),
        ("scd_bAppRegistryAvailable", INIT_APP_REGISTRY_CHECK),
        (
            "map-engine-global-check",
            INIT_APP_MAP_GLOBAL_CHECK,
        ),
        ("map-engine-new-result", INIT_APP_MAP_NEW_RESULT),
        ("map-engine-vtable-result", INIT_MAP_ENGINE_VTABLE_RESULT),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = INIT_MAP_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 20 {
                    return;
                }

                let regs = [
                    RegisterARM::R0,
                    RegisterARM::R1,
                    RegisterARM::R2,
                    RegisterARM::R3,
                    RegisterARM::R4,
                    RegisterARM::R5,
                    RegisterARM::R6,
                ]
                .map(|r| uc.reg_read(r).unwrap_or(0) as u32);
                log::info!(
                    "PROCMAPENGINE init trace {} at {:#x}: r0={:#x} r1={:#x} r2={:#x} \
                     r3={:#x} r4={:#x} r5={:#x} r6={:#x}",
                    name,
                    addr,
                    regs[0],
                    regs[1],
                    regs[2],
                    regs[3],
                    regs[4],
                    regs[5],
                    regs[6]
                );
            })
            .unwrap();
    }
}

fn add_port_control_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    for (name, offset, kind) in [
        (
            "ail_tclAppInterfaceRestricted::bOnInit result",
            PORTCONTROL_AIL_BONINIT_RESULT,
            0u8,
        ),
        (
            "map_tclPortControl::Init result",
            PORTCONTROL_INIT_RESULT,
            0u8,
        ),
        ("CCAClientHandler init result", PORTCONTROL_HANDLER_RESULT, 1u8),
        ("CCAClientHandler init failure", PORTCONTROL_HANDLER_FAILURE, 2u8),
        ("map_tclTraceTools::TraceMsgToErrmem", MAP_TRACE_ERRMEM, 3u8),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = PORTCONTROL_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 80 {
                    return;
                }

                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                let message = if kind == 3 {
                    read_cstring(uc, r1)
                } else {
                    String::new()
                };
                let handler_name = match kind {
                    1 | 2 => {
                        let name_ptr = read_u32_or_invalid(uc, r4 + 0x1c);
                        match name_ptr {
                            0 | 0xffff_ffff => "<null>".to_string(),
                            _ => read_cstring(uc, name_ptr),
                        }
                    }
                    _ => String::new(),
                };

                match kind {
                    0 => log::info!(
                        "PROCMAPENGINE port-control trace {} at {:#x}: r0={:#x}",
                        name,
                        addr,
                        r0
                    ),
                    3 => log::info!(
                        "PROCMAPENGINE errmem trace {} at {:#x}: r0={:#x} lr={:#x} msg={}",
                        name,
                        addr,
                        r0,
                        lr,
                        message
                    ),
                    _ => log::info!(
                        "PROCMAPENGINE port-control trace {} at {:#x}: r0={:#x} handler={:#x} name={}",
                        name,
                        addr,
                        r0,
                        r4,
                        handler_name
                    ),
                }
            })
            .unwrap();
    }
}

fn add_map_engine_init_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    for (name, original_addr) in [
        ("map_tclPortControl::Init", 0x0053_ae38u32),
        ("cc_tclOSALAccess::Init", 0x0051_e028),
        ("cc_tclCommonComponent::Init", 0x0051_e038),
        ("cc_tclOSALFile::vSetDefaultDevice", 0x0051_e06c),
        ("svgInitFonts", 0x0051_e08c),
        ("map_tclScaleLevel::Init", 0x0051_e0b0),
        (
            "tclDTMDrawControlParams::vReadTableFromConfig",
            0x0051_e0b4,
        ),
        ("map_tclMapMathUtils::Init", 0x0051_e134),
        ("NPRManager::bInitNPR", 0x0051_e14c),
        ("operator new SceneManagerHeap", 0x0051_e218),
        (
            "mem_tclMemoryHeap::mem_tclMemoryHeap SceneManagerHeap",
            0x0051_e224,
        ),
        ("mem_tclMemoryHeap::Init SceneManagerHeap", 0x0051_e234),
        ("operator new DataManagerHeap", 0x0051_e2d8),
        (
            "mem_tclMemoryHeap::mem_tclMemoryHeap DataManagerHeap",
            0x0051_e2e4,
        ),
        ("mem_tclMemoryHeap::Init DataManagerHeap", 0x0051_e2f4),
        ("operator new LayouterHeap", 0x0051_e37c),
        (
            "mem_tclMemoryHeap::mem_tclMemoryHeap LayouterHeap",
            0x0051_e388,
        ),
        ("mem_tclMemoryHeap::Init LayouterHeap", 0x0051_e398),
        ("rl_tclRenderLayer::GetInstance", 0x0051_e784),
        ("rl_tclRenderLayer::Init", 0x0051_e794),
        ("map_tclObjectCache::GetInstance", 0x0051_e71c),
        ("map_tclObjectCache::Init", 0x0051_e72c),
        ("map_tclMapEngineConfig::GetInstance", 0x0051_e6c8),
        ("map_tclMapEngineConfig::Init", 0x0051_e6d8),
        ("map_tclSceneManager::GetInstance", 0x0051_e660),
        ("map_tclSceneManager::Init", 0x0051_e670),
        ("map_tclLocalisationManager::GetInstance", 0x0051_e528),
        ("map_tclLocalisationManager::Init", 0x0051_e538),
        ("map_tclMapAnimationManager::GetInstance", 0x0051_e4c0),
        ("map_tclMapAnimationManager::Init", 0x0051_e4d0),
        ("map_tclMapDataManager::GetInstance", 0x0051_e458),
        ("map_tclMapDataManager::Init", 0x0051_e468),
        ("map_tclMDMWeatherManager::GetSingleton", 0x0051_e654),
        ("map_tclRCWeatherManager::GetInstance", 0x0051_e5f8),
        ("map_tclRCWeatherManager::Init", 0x0051_e608),
        ("map_tclRenderControl::GetInstance", 0x0051_e3f0),
        ("map_tclRenderControl::Init", 0x0051_e400),
        ("map_tclDynamicElementManager::GetInstance", 0x0051_e590),
        ("map_tclDynamicElementManager::Init", 0x0051_e5a0),
    ] {
        let addr = base_address + (original_addr - ORIGINAL_BASE);
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = MAP_INIT_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 200 {
                    return;
                }

                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE map-init trace {} at {:#x}: r0={:#x} r1={:#x}",
                    name,
                    addr,
                    r0,
                    r1
                );
            })
            .unwrap();
    }
}

/// Bump the map application's CCA state to `INITIALIZED` (2) if it is
/// still at `NOT_STARTED` (1). The real firmware reaches state 2 by
/// having the body thread run `vAppBody` before the entry thread's
/// `Wait` returns; in our single-VM emulator the entry thread wins
/// that race. Without this bump the PWR-proxy's START_CONF is silently
/// dropped by `bDispatchCCAMessages` (`if state < 2 goto drop`) and
/// procmap never sends `PWR_APP_INITIALIZED`. Only touches the state
/// field; the entry thread otherwise runs unmodified.
pub fn prepare_app_state_for_start_conf(unicorn: &mut Unicorn<'_, Context>) {
    let base_address = PROCMAP_BASE.load(Ordering::Relaxed);
    if base_address == 0 {
        return;
    }
    let app = read_u32_or_invalid(unicorn, base_address + MAP_ENGINE_GLOBAL);
    if app == 0 || app > 0xf000_0000 {
        return;
    }
    let state = read_u32_or_invalid(unicorn, app + APP_STATE_OFFSET);
    if state >= PWR_PROXY_START_APP_STATE {
        return;
    }
    if unicorn
        .mem_write((app + APP_STATE_OFFSET) as u64, &PWR_PROXY_START_APP_STATE.to_le_bytes())
        .is_ok()
    {
        log::info!(
            "PROCMAPENGINE: bumped map app {:#x} CCA state {:#x} -> {:#x} so entry thread can process START_CONF",
            app,
            state,
            PWR_PROXY_START_APP_STATE
        );
    }
}

pub fn force_map_power_cca_mode(unicorn: &mut Unicorn<'_, Context>) -> bool {
    if MAP_POWER_CCA_MODE_SET.swap(true, Ordering::Relaxed) {
        return true;
    }

    let base_address = PROCMAP_BASE.load(Ordering::Relaxed);
    if base_address == 0 {
        MAP_POWER_CCA_MODE_SET.store(false, Ordering::Relaxed);
        return false;
    }

    let app = read_u32_or_invalid(unicorn, base_address + MAP_ENGINE_GLOBAL);
    if app == 0 || app > 0xf000_0000 {
        MAP_POWER_CCA_MODE_SET.store(false, Ordering::Relaxed);
        return false;
    }

    let state = read_u32_or_invalid(unicorn, app + APP_STATE_OFFSET);
    if state < PWR_PROXY_START_APP_STATE {
        if unicorn
            .mem_write((app + APP_STATE_OFFSET) as u64, &PWR_PROXY_START_APP_STATE.to_le_bytes())
            .is_err()
        {
            MAP_POWER_CCA_MODE_SET.store(false, Ordering::Relaxed);
            return false;
        }
        log::info!(
            "PROCMAPENGINE: moved map app {:#x} CCA state {:#x} -> {:#x} for vApplicationStart",
            app,
            state,
            PWR_PROXY_START_APP_STATE
        );
    } else {
        log::debug!(
            "PROCMAPENGINE: leaving map app {:#x} in CCA state {:#x}; synthetic power message will drive vApplicationStart",
            app,
            state
        );
    }

    let target = app + APP_IPC_WAIT_PARAM_OFFSET;
    let queue = read_u32_or_invalid(unicorn, app + APP_IN_QUEUE_OFFSET);
    let wait_param = if queue == 0 || queue == u32::MAX || queue > 0xf000_0000 {
        u32::MAX
    } else {
        queue
    };
    if unicorn.mem_write(target as u64, &wait_param.to_le_bytes()).is_err()
    {
        MAP_POWER_CCA_MODE_SET.store(false, Ordering::Relaxed);
        return false;
    }

    if wait_param == u32::MAX {
        log::info!(
            "PROCMAPENGINE: forced map app {:#x} IPC wait param to -1 for CCA power startup",
            app
        );
    } else {
        MAP_IPC_WAIT_REPAIRED.store(true, Ordering::Relaxed);
        log::info!(
            "PROCMAPENGINE: set map app {:#x} IPC wait param to entry queue {:#x} for CCA power startup",
            app,
            queue
        );
    }
    true
}

fn repair_map_ipc_wait_param(unicorn: &mut Unicorn<'_, Context>) -> bool {
    if MAP_IPC_WAIT_REPAIRED.load(Ordering::Relaxed) {
        return true;
    }

    let base_address = PROCMAP_BASE.load(Ordering::Relaxed);
    if base_address == 0 {
        return false;
    }

    let app = read_u32_or_invalid(unicorn, base_address + MAP_ENGINE_GLOBAL);
    if app == 0 || app > 0xf000_0000 {
        return false;
    }

    let queue = read_u32_or_invalid(unicorn, app + APP_IN_QUEUE_OFFSET);
    if queue == 0 || queue == u32::MAX || queue > 0xf000_0000 {
        return false;
    }

    if unicorn
        .mem_write(
            (app + APP_IPC_WAIT_PARAM_OFFSET) as u64,
            &queue.to_le_bytes(),
        )
        .is_err()
    {
        return false;
    }

    MAP_IPC_WAIT_REPAIRED.store(true, Ordering::Relaxed);
    log::info!(
        "PROCMAPENGINE: repaired map app {:#x} IPC wait param from -1 to entry queue {:#x}",
        app,
        queue
    );
    true
}

fn start_map_engine_after_init(unicorn: &mut Unicorn<'_, Context>, original_pc: u32) -> bool {
    if MAP_ENGINE_STARTED_AFTER_INIT.swap(true, Ordering::Relaxed) {
        return true;
    }

    let base_address = PROCMAP_BASE.load(Ordering::Relaxed);
    if base_address == 0 {
        MAP_ENGINE_STARTED_AFTER_INIT.store(false, Ordering::Relaxed);
        return false;
    }

    let map_state = read_u32_or_invalid(unicorn, base_address + MAP_ENGINE_STATE_VAR);
    if map_state == u32::MAX {
        log::warn!("PROCMAPENGINE: skipped map engine Start after PortControl init; state unreadable");
        MAP_ENGINE_STARTED_AFTER_INIT.store(false, Ordering::Relaxed);
        return false;
    }

    let control_state = read_u32_or_invalid(unicorn, base_address + MAP_ENGINE_CONTROL_STATE);
    if control_state == u32::MAX {
        log::warn!("PROCMAPENGINE: skipped map engine Start after PortControl init; control state unreadable");
        MAP_ENGINE_STARTED_AFTER_INIT.store(false, Ordering::Relaxed);
        return false;
    }

    if control_state != 1 && control_state != 3 {
        if !write_u32(unicorn, base_address + MAP_ENGINE_CONTROL_STATE, 1) {
            log::warn!("PROCMAPENGINE: failed to force map engine control state to stopped");
            MAP_ENGINE_STARTED_AFTER_INIT.store(false, Ordering::Relaxed);
            return false;
        }
        log::info!(
            "PROCMAPENGINE: forced map engine control state {:#x} to stopped for synthetic start",
            control_state
        );
    }

    let function = base_address + PORTCONTROL_START_MAP_ENGINE;
    if !call_guest_function(unicorn, original_pc, function, [0, 0, 0, 0]) {
        log::warn!("PROCMAPENGINE: failed to call PortControl::StartMapEngine after init");
        MAP_ENGINE_STARTED_AFTER_INIT.store(false, Ordering::Relaxed);
        return false;
    }

    MAP_RENDER_CONTROL_START_PENDING.store(true, Ordering::Relaxed);
    log::info!("PROCMAPENGINE: requested map engine start after PortControl init");
    true
}

fn ensure_answer_scratch(unicorn: &mut Unicorn<'_, Context>) -> Option<u32> {
    let stub = ensure_guest_call_stub(unicorn)?;
    let addr = stub.checked_add(PROCMAP_ANSWER_SCRATCH_OFFSET)?;
    if unicorn.mem_write(addr as u64, &[0u8; 16]).is_err() {
        log::warn!("PROCMAPENGINE: failed to zero answer-info scratch at {:#x}", addr);
        return None;
    }
    Some(addr)
}

fn queue_view_creation_job(unicorn: &mut Unicorn<'_, Context>, original_pc: u32) -> bool {
    let base_address = PROCMAP_BASE.load(Ordering::Relaxed);
    if base_address == 0 {
        return false;
    }

    let Some(scratch) = ensure_answer_scratch(unicorn) else {
        return false;
    };

    MAP_VIEW_CREATE_MUTATING.store(true, Ordering::Relaxed);
    MAP_VIEW_CREATE_PENDING.store(true, Ordering::Relaxed);
    let function = base_address + RENDER_CONTROL_CREATE_VIEW_JOB;
    if !call_guest_function(unicorn, original_pc, function, [scratch, 0, 0, 0]) {
        MAP_VIEW_CREATE_MUTATING.store(false, Ordering::Relaxed);
        MAP_VIEW_CREATE_PENDING.store(false, Ordering::Relaxed);
        log::warn!("PROCMAPENGINE: failed to call map view create job factory");
        return false;
    }

    log::info!(
        "PROCMAPENGINE: requested map view {} creation job with answer scratch {:#x}",
        MAP_RENDER_VIEW_ID,
        scratch
    );
    true
}

/// Two-stage pure render kick (r655): stage 1 calls the
/// `CreateRCJobRenderPixmapView` factory (the page hook at its return site
/// captures the job), stage 2 enqueues it with `AddJobToRenderControlQueue`.
/// Both stages are injected through the ordinary guest-call stub from the
/// engine's own wait-skip tick, so nothing outside the engine's address space
/// is touched.
fn kick_pixmap_render(
    unicorn: &mut Unicorn<'_, Context>,
    original_pc: u32,
    base_address: u32,
) -> bool {
    if MAP_RENDER_KICK_ENQUEUE_PENDING.swap(false, Ordering::Relaxed) {
        let job = MAP_RENDER_KICK_JOB.load(Ordering::Relaxed);
        if job == 0 {
            return false;
        }
        let function = base_address + RENDER_CONTROL_ADD_JOB;
        if !call_guest_function(unicorn, original_pc, function, [job, 0, 0, 0]) {
            MAP_RENDER_KICK_ENQUEUE_PENDING.store(true, Ordering::Relaxed);
            log::warn!("PROCMAPENGINE: failed to enqueue pure render job {:#x}", job);
            return false;
        }
        let count = MAP_RENDER_KICK_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
        if count < 30 {
            log::info!(
                "PROCMAPENGINE: enqueued pure render job {:#x} t={}ms thread=[{}] n={}",
                job,
                crate::libs::dapi::emu_t0_elapsed_ms(),
                unicorn.get_data().inner.thread_id(),
                count
            );
        }
        return true;
    }

    if MAP_RENDER_KICK_CAPTURING.load(Ordering::Relaxed) {
        // A created job whose capture did not run (should not happen): give
        // the next tick a fresh attempt instead of wedging the state machine.
        if MAP_RENDER_KICK_JOB.load(Ordering::Relaxed) != 0
            && MAP_RENDER_KICK_TRACE_COUNT.load(Ordering::Relaxed) % 64 == 63
        {
            MAP_RENDER_KICK_CAPTURING.store(false, Ordering::Relaxed);
        }
        return false;
    }

    let map_state = read_u32_or_invalid(unicorn, base_address + MAP_ENGINE_STATE_VAR);
    if map_state == 0 || map_state == u32::MAX || map_state == 4 || map_state == 5 {
        return false;
    }

    let Some(scratch) = ensure_answer_scratch(unicorn) else {
        return false;
    };
    // Factory signature: r0 -> view id byte, r1 -> answer info.  The answer
    // info stays all-zero on purpose (invalid port, no HMI answer chain); the
    // view id byte lives at the top of the zeroed scratch.
    if !write_u8(unicorn, scratch + 15, MAP_RENDER_VIEW_ID as u8) {
        return false;
    }
    MAP_RENDER_KICK_CAPTURING.store(true, Ordering::Relaxed);
    let function = base_address + CREATE_RC_JOB_RENDER_PIXMAP_VIEW;
    if !call_guest_function(
        unicorn,
        original_pc,
        function,
        [scratch + 15, scratch, 0, 0],
    ) {
        MAP_RENDER_KICK_CAPTURING.store(false, Ordering::Relaxed);
        log::warn!("PROCMAPENGINE: failed to call pure render job factory");
        return false;
    }
    true
}

fn trigger_pending_map_render(
    unicorn: &mut Unicorn<'_, Context>,
    original_pc: u32,
    force_periodic: bool,
) -> bool {
    let base_address = PROCMAP_BASE.load(Ordering::Relaxed);
    if base_address == 0 {
        return false;
    }

    // r655: once the copy-to-active branch has filled the active MO lists,
    // every periodic kick becomes a *pure* render job injection.  The full
    // trigger handler is not needed any more and its answer chain (answers
    // 0x78/0x5a/0x57/0x63 to port 0x2711) makes the HMI demo handler re-write
    // the Milwaukee camera into view+0x84, restarting the whole request
    // machine.  A RenderPixmapView job with an all-zero answer info draws the
    // active lists and the answer lands on invalid port 0.
    if MAP_MO_COPY_DONE.load(Ordering::Relaxed) {
        return kick_pixmap_render(unicorn, original_pc, base_address);
    }

    if MAP_RENDER_CONTROL_START_PENDING.swap(false, Ordering::Relaxed) {
        let function = base_address + RENDER_CONTROL_START;
        if !call_guest_function(unicorn, original_pc, function, [0, 0, 0, 0]) {
            log::warn!("PROCMAPENGINE: failed to call RenderControl::Start");
            MAP_RENDER_CONTROL_START_PENDING.store(true, Ordering::Relaxed);
            return false;
        }

        MAP_VIEW_CREATE_REQUESTED.store(true, Ordering::Relaxed);
        log::info!("PROCMAPENGINE: requested RenderControl Start after PortControl init");
        return true;
    }

    if MAP_VIEW_CREATE_REQUESTED.load(Ordering::Relaxed)
        && !MAP_VIEW_READY.load(Ordering::Relaxed)
    {
        if MAP_VIEW_CREATE_JOB_ADDRESS.load(Ordering::Relaxed) == 0
            && !MAP_VIEW_CREATE_PENDING.load(Ordering::Relaxed)
        {
            return queue_view_creation_job(unicorn, original_pc);
        }

        if MAP_VIEW_CREATE_ENQUEUE_PENDING.swap(false, Ordering::Relaxed) {
            let job = MAP_VIEW_CREATE_JOB_ADDRESS.load(Ordering::Relaxed);
            if job == 0 {
                return false;
            }

            let function = base_address + RENDER_CONTROL_ADD_JOB;
            if !call_guest_function(unicorn, original_pc, function, [job, 0, 0, 0]) {
                MAP_VIEW_CREATE_ENQUEUE_PENDING.store(true, Ordering::Relaxed);
                log::warn!("PROCMAPENGINE: failed to enqueue map view create job {:#x}", job);
                return false;
            }

            MAP_RENDER_TRIGGER_PENDING.store(true, Ordering::Relaxed);
            log::info!("PROCMAPENGINE: enqueued map view create job {:#x}", job);
            return true;
        }

        return false;
    }

    if !force_periodic {
        if MAP_INITIAL_RENDER_TRIGGERED.load(Ordering::Relaxed)
            || !MAP_RENDER_TRIGGER_PENDING.load(Ordering::Relaxed)
        {
            return false;
        }
    } else if !MAP_VIEW_READY.load(Ordering::Relaxed) {
        return false;
    }

    let map_state = read_u32_or_invalid(unicorn, base_address + MAP_ENGINE_STATE_VAR);
    if map_state == 0 || map_state == u32::MAX || map_state == 4 || map_state == 5 {
        log::warn!(
            "PROCMAPENGINE: skipped map render trigger; map engine state={:#x}",
            map_state
        );
        return false;
    }

    let is_initial = !force_periodic;
    if is_initial {
        MAP_INITIAL_RENDER_TRIGGERED.store(true, Ordering::Relaxed);
    }

    let function = base_address + MAP_TRIGGER_PIXMAP_RENDER;
    if !call_guest_function(
        unicorn,
        original_pc,
        function,
        [MAP_RENDER_VIEW_ID, 0, 0, 0],
    ) {
        log::warn!("PROCMAPENGINE: failed to queue map render job for view 1");
        return false;
    }

    let trace_count = MAP_RENDER_TRIGGER_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
    if is_initial || trace_count < 20 {
        log::info!(
            "PROCMAPENGINE: queued {} map render job for view {} with map engine state {:#x}",
            if is_initial { "initial" } else { "periodic" },
            MAP_RENDER_VIEW_ID,
            map_state
        );
    }

    true
}

fn add_render_thread_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let render_mainloop = base_address + RENDER_CONTROL_MAINLOOP_ENTRY;
    unicorn
        .add_code_hook(
            render_mainloop as u64,
            render_mainloop as u64,
            move |_, _, _| {
                let count = RENDER_CONTROL_MAINLOOP_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 10 {
                    log::info!(
                        "PROCMAPENGINE: render control mainloop entered count={}",
                        count + 1
                    );
                }
            },
        )
        .unwrap();

    let map_mainloop = base_address + MAP_VIEW_MAINLOOP_ENTRY;
    unicorn
        .add_code_hook(
            map_mainloop as u64,
            map_mainloop as u64,
            move |uc, _, _| {
                let view = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let count = MAP_VIEW_MAINLOOP_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 10 {
                    log::info!(
                        "PROCMAPENGINE: map view mainloop entered view={:#x} count={}",
                        view,
                        count + 1
                    );
                }
            },
        )
        .unwrap();

    for (offset, return_offset, name) in [
        (RENDER_CONTROL_WAIT_CALL, RENDER_CONTROL_WAIT_RETURN, "render control"),
        (MAP_VIEW_WAIT_CALL, MAP_VIEW_WAIT_RETURN, "map view"),
    ] {
        let call_addr = base_address + offset;
        let return_addr = base_address + return_offset;
        unicorn
            .add_code_hook(
                call_addr as u64,
                call_addr as u64,
                move |uc, _, _| {
                    let tick = MAP_JOB_WAIT_SKIP_TICK.fetch_add(1, Ordering::Relaxed) + 1;
                    if tick > 100 && tick % MAP_RENDER_TRIGGER_INTERVAL != 1 {
                        return;
                    }

                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                    let return_pc = if lr >= base_address && lr < base_address + 0x70_0000 {
                        lr
                    } else {
                        return_addr
                    };
                    let mut triggered = false;
                    if MAP_VIEW_READY.load(Ordering::Relaxed) {
                        // r648: force_periodic=false.  The periodic re-fire
                        // of the canned `MapEngine_TriggerPixmapViewRender`
                        // (113 hits in the first minute of r646) re-armed the
                        // Flint demo camera (its baked literals), and each
                        // off-centre rect made `PerformMapDataRequest` reset
                        // the whole request state machine (0x78fd48 -> 0,
                        // refetch counters, FreeFastMapChannel), so
                        // `GetNextDataHandledState` never reached 2,
                        // `GetLeftBlocks` never captured state 2 into the
                        // RCMOUPD job (+0x28), and the copy-to-active branch
                        // (roads -> layer lists) never ran.  The engine pumps
                        // its own draw jobs through the RndFI lifecycle and
                        // the renfi camera messages; keep only the initial
                        // trigger and let the request state machine sit in
                        // one rect long enough to advance.
                        triggered = trigger_pending_map_render(uc, return_pc, false);
                    }
                    if !triggered {
                        uc.reg_write(RegisterARM::R0, 0).unwrap_or_default();
                        uc.reg_write(RegisterARM::PC, return_addr as u64).unwrap_or_default();
                    }

                    let trace_count = MAP_JOB_WAIT_SKIP_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                    if trace_count < 50 {
                        log::info!(
                            "PROCMAPENGINE: skipped {} job queue wait tick={} return={:#x} triggered={}",
                            name,
                            tick,
                            return_addr,
                            triggered
                        );
                    }
                },
            )
            .unwrap();
    }
}

fn add_render_job_queue_wait_skip_hook(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let entry = base_address + RENDER_JOB_QUEUE_WAIT_ENABLED_ENTRY;
    let rc_return = base_address + RENDER_JOB_QUEUE_WAIT_RC_RETURN;
    let view_return = base_address + MAP_VIEW_JOB_QUEUE_WAIT_RETURN;

    unicorn
        .add_code_hook(entry as u64, entry as u64, move |uc, _, _| {
            let queue = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            if queue == 0 || queue == u32::MAX {
                return;
            }

            let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
            let (name, tick) = if lr == rc_return {
                (
                    "render control",
                    RENDER_JOB_WAIT_SKIP_TICK.fetch_add(1, Ordering::Relaxed) + 1,
                )
            } else if lr == view_return {
                (
                    "map view",
                    MAP_VIEW_JOB_WAIT_SKIP_TICK.fetch_add(1, Ordering::Relaxed) + 1,
                )
            } else {
                return;
            };

            if tick > 1 && tick % MAP_RENDER_TRIGGER_INTERVAL != 1 {
                return;
            }

            let queue_state = read_u32_or_invalid(uc, queue);
            uc.reg_write(RegisterARM::R0, queue_state as u64).unwrap_or_default();
            uc.reg_write(RegisterARM::PC, lr as u64).unwrap_or_default();

            let trace_count = RENDER_JOB_WAIT_SKIP_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if trace_count < 50 {
                log::info!(
                    "PROCMAPENGINE: skipped {} enabled semaphore wait tick={} queue={:#x} state={:#x} return={:#x}",
                    name,
                    tick,
                    queue,
                    queue_state,
                    lr
                );
            }
        })
        .unwrap();
}

fn add_natural_map_engine_start_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let call_addr = base_address + AIL_START_MAP_ENGINE_CALL;
    unicorn
        .add_code_hook(call_addr as u64, call_addr as u64, move |uc, _, _| {
            let control_state = read_u32_or_invalid(uc, base_address + MAP_ENGINE_CONTROL_STATE);
            let count = NATURAL_MAP_START_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 10 {
                log::info!(
                    "PROCMAPENGINE: natural map engine start call state={:#x}",
                    control_state
                );
            }

            if control_state == 0 {
                if write_u32(uc, base_address + MAP_ENGINE_CONTROL_STATE, 1) {
                    let trace_count =
                        NATURAL_MAP_START_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                    if trace_count < 10 {
                        log::info!(
                            "PROCMAPENGINE: forced uninitialized map engine state to stopped"
                        );
                    }
                } else {
                    log::warn!("PROCMAPENGINE: failed to force natural map engine start state");
                }
            } else if control_state == 2 {
                initialize_map_data_medium(uc, base_address);
            }
        })
        .unwrap();

    let result_addr = base_address + AIL_START_MAP_ENGINE_RESULT;
    unicorn
        .add_code_hook(result_addr as u64, result_addr as u64, move |uc, _, _| {
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let control_state = read_u32_or_invalid(uc, base_address + MAP_ENGINE_CONTROL_STATE);
            let count = NATURAL_MAP_START_RESULT_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if r0 == 1 {
                MAP_ENGINE_NATURALLY_STARTED.store(true, Ordering::Relaxed);
                MAP_VIEW_READY.store(true, Ordering::Relaxed);
            }
            if r0 == 1 || control_state == 2 {
                initialize_map_data_medium(uc, base_address);
            }
            if count < 10 {
                log::info!(
                    "PROCMAPENGINE: natural map engine start result r0={:#x} state={:#x}",
                    r0,
                    control_state
                );
            }
        })
        .unwrap();
}

fn add_forced_renderer_execute_hook(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let branch_addr = base_address + MAP_RENDERER_SKIP_BRANCH;
    let body_addr = base_address + MAP_RENDERER_EXECUTE_BODY;
    unicorn
        .add_code_hook(branch_addr as u64, branch_addr as u64, move |uc, _, _| {
            uc.reg_write(RegisterARM::PC, body_addr as u64).unwrap_or_default();
            let count = MAP_RENDERER_FORCE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 20 {
                log::info!(
                    "PROCMAPENGINE: forced renderer execute body count={} at {:#x}",
                    count + 1,
                    body_addr
                );
            }
        })
        .unwrap();

    let execute_entry = base_address + MAP_RENDERER_EXECUTE_ENTRY;
    unicorn
        .add_code_hook(
            execute_entry as u64,
            execute_entry as u64,
            move |uc, _, _| {
                let renderer = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let surface = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                let layer_list = read_u32_or_invalid(uc, sp + 0x18);
                let layer_count = read_u32_or_invalid(uc, sp + 0x1c) & 0xff;
                let map_engine = read_u32_or_invalid(uc, renderer + 0x18);
                let count = MAP_RENDERER_EXECUTE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 20 {
                    log::info!(
                        "PROCMAPENGINE: renderer Execute entry renderer={:#x} map_engine={:#x} surface={:#x} layers={:#x} count={:#x} trace={}",
                        renderer,
                        map_engine,
                        surface,
                        layer_list,
                        layer_count,
                        count + 1
                    );
                }
            },
        )
        .unwrap();

    let element_list_add = base_address + MAP_ELEMENT_LIST_ADD;
    unicorn
        .add_code_hook(
            element_list_add as u64,
            element_list_add as u64,
            move |uc, _, _| {
                let list = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let element = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let list_count = read_u32_or_invalid(uc, list + 10) & 0xffff;
                let list_array = read_u32_or_invalid(uc, list + 4);
                let element_vtable = read_u32_or_invalid(uc, element);
                let count =
                    MAP_ELEMENT_LIST_ADD_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 100 {
                    log::info!(
                        "PROCMAPENGINE: map element list Add list={:#x} element={:#x} vtable={:#x} list_count={} array={:#x} trace={}",
                        list,
                        element,
                        element_vtable,
                        list_count,
                        list_array,
                        count + 1
                    );
                }
            },
        )
        .unwrap();

    let draw_layer_add = base_address + MAP_DRAW_LAYER_LISTS_ADD_ELEMENT;
    unicorn
        .add_code_hook(
            draw_layer_add as u64,
            draw_layer_add as u64,
            move |uc, _, _| {
                let draw_layers = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let element = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let layer = (uc.reg_read(RegisterARM::R2).unwrap_or(0) & 0xff) as u32;
                let list = draw_layers.wrapping_add(layer * 0x10);
                let list_count = if list == 0 {
                    0xffff
                } else {
                    read_u32_or_invalid(uc, list + 10) & 0xffff
                };
                let element_vtable = read_u32_or_invalid(uc, element);
                let usage_addr = base_address + MAP_CURRENT_ARRAY_USAGE;
                let current_usage = read_u32_or_invalid(uc, usage_addr);
                let count = MAP_DRAW_LAYER_ADD_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 300 {
                    log::info!(
                        "PROCMAPENGINE: draw layer AddElement layers={:#x} element={:#x} vtable={:#x} layer={} list={:#x} list_count={} usage={} trace={}",
                        draw_layers,
                        element,
                        element_vtable,
                        layer,
                        list,
                        list_count,
                        current_usage,
                        count + 1
                    );
                }
            },
        )
        .unwrap();

    let draw_list_entry = base_address + MAP_RENDERER_DRAW_LIST_ENTRY;
    unicorn
        .add_code_hook(
            draw_list_entry as u64,
            draw_list_entry as u64,
            move |uc, _, _| {
                let renderer = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let list = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let surface = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let layer = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let list_count = if list == 0 {
                    0xffff
                } else {
                    read_u32_or_invalid(uc, list + 10) & 0xffff
                };
                let list_array = if list == 0 {
                    0xffff_ffff
                } else {
                    read_u32_or_invalid(uc, list + 4)
                };
                let first_element = if list_array == 0 || list_array == u32::MAX {
                    0xffff_ffff
                } else {
                    read_u32_or_invalid(uc, list_array)
                };
                let count = MAP_RENDERER_DRAW_LIST_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 200 {
                    log::info!(
                        "PROCMAPENGINE: renderer DrawList entry renderer={:#x} surface={:#x} layer={} list={:#x} count={} array={:#x} first={:#x} trace={}",
                        renderer,
                        surface,
                        layer,
                        list,
                        list_count,
                        list_array,
                        first_element,
                        count + 1
                    );
                }
            },
        )
        .unwrap();

    let draw_list_blit_target = base_address + MAP_RENDERER_DRAW_LIST_TARGET_3C;
    for (offset, slot, kind) in [
        (MAP_RENDERER_DRAW_LIST_VCALL_38, 0x38u32, "0x38"),
        (MAP_RENDERER_DRAW_LIST_VCALL_3C, 0x3cu32, "0x3c"),
    ] {
        let vcall_addr = base_address + offset;
        let draw_list_blit_target = draw_list_blit_target;
        unicorn
            .add_code_hook(vcall_addr as u64, vcall_addr as u64, move |uc, _, _| {
                let object = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let vtable = uc.reg_read(RegisterARM::R12).unwrap_or(0) as u32;
                let target = read_u32_or_invalid(uc, vtable + slot);
                let list = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                let layer = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let count = MAP_RENDERER_DRAW_LIST_VCALL_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 100 || count % 512 == 0 {
                    log::info!(
                        "PROCMAPENGINE: DrawList vcall {} t={}ms object={:#x} vtable={:#x} target={:#x} list={:#x} layer={:#x} trace={}",
                        kind,
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        object,
                        vtable,
                        target,
                        list,
                        layer,
                        count + 1
                    );
                }
                if std::env::var_os("EMU_PROCMAPENGINE_SYNTH_DRAWLIST").is_some()
                    && target == draw_list_blit_target
                    && list != 0
                    && list != u32::MAX
                {
                    let element_count = read_u32_or_invalid(uc, list + 10) & 0xffff;
                    if element_count != 0 {
                        gpu::request_draw_list(uc, element_count as i32);
                    }
                }
            })
            .unwrap();
    }

    let blit_entry = base_address + MAP_DRAW_BLIT_ICON_ENTRY;
    let blit_global = base_address + MAP_DRAW_BLIT_ICON_GLOBAL;
    let blit_table_global = base_address + MAP_DRAW_BLIT_ICON_TABLE_GLOBAL;
    let blit_global_end = base_address + MAP_DRAW_BLIT_ICON_GLOBAL_END;
    let blit_null_return = base_address + MAP_DRAW_BLIT_ICON_NULL_RETURN;
    let mut fake_blit_context = 0u32;
    unicorn
        .add_code_hook(blit_entry as u64, blit_entry as u64, move |uc, _, _| {
            let mut context_global = read_u32_or_invalid(uc, blit_global);
            let mut context = if context_global == 0 || context_global == u32::MAX {
                0
            } else {
                read_u32_or_invalid(uc, context_global)
            };

            if std::env::var_os("EMU_PROCMAPENGINE_FORCE_BLIT_CONTEXT").is_some()
                && context_global != 0
                && context_global != u32::MAX
                && context == 0
            {
                if fake_blit_context == 0 {
                    let mmu_arc = {
                        let data = uc.get_data();
                        data.mmu.clone()
                    };
                    fake_blit_context = mmu_arc.lock().unwrap().heap_alloc(
                        uc,
                        0x40000,
                        Prot::READ | Prot::WRITE,
                        "[procmap-blit-context]",
                    );
                    if fake_blit_context != 0 {
                        MAP_DRAW_BLIT_FAKE_CONTEXT.store(fake_blit_context, Ordering::Relaxed);
                        let fill = vec![(fake_blit_context + 4).to_le_bytes(); 0x40000 / 4].concat();
                        let _ = uc.mem_write(fake_blit_context as u64, &fill);

                        let mut global = blit_global;
                        while global < blit_global_end {
                            if read_u32_or_invalid(uc, global) == 0 {
                                let _ = uc.mem_write(global as u64, &fake_blit_context.to_le_bytes());
                            }
                            global = global.wrapping_add(4);
                        }
                    }
                }
                if fake_blit_context != 0 {
                    let _ = uc.mem_write(blit_global as u64, &fake_blit_context.to_le_bytes());
                    context_global = read_u32_or_invalid(uc, blit_global);
                    context = if context_global == 0 || context_global == u32::MAX {
                        0
                    } else {
                        read_u32_or_invalid(uc, context_global)
                    };
                }
            }

            let object = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let layer = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let mode = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let current_usage =
                read_u32_or_invalid(uc, base_address + MAP_CURRENT_ARRAY_USAGE);
            let count = MAP_DRAW_BLIT_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 40 {
                log::info!(
                    "PROCMAPENGINE: DrawBlit entry global={:#x} context={:#x} object={:#x} layer={:#x} mode={:#x} usage={} fake={:#x} trace={}",
                    context_global,
                    context,
                    object,
                    layer,
                    mode,
                    current_usage,
                    fake_blit_context,
                    count + 1
                );
            }
        })
        .unwrap();
    for (name, offset) in [
        ("return", MAP_DRAW_BLIT_ICON_NULL_RETURN),
        ("branch1", MAP_DRAW_BLIT_ICON_BRANCH_1),
        ("branch2", MAP_DRAW_BLIT_ICON_BRANCH_2),
        ("branch3", MAP_DRAW_BLIT_ICON_BRANCH_3),
        ("branch4", MAP_DRAW_BLIT_ICON_BRANCH_4),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                let r9 = uc.reg_read(RegisterARM::R9).unwrap_or(0) as u32;
                if name == "branch1" && r3 == 0 {
                    let fake = MAP_DRAW_BLIT_FAKE_CONTEXT.load(Ordering::Relaxed);
                    if fake != 0 && r9 != 0 && r9 != u32::MAX {
                        let slot = r9.wrapping_add(r4);
                        let page = slot & !0xfff;
                        let mmu_arc = {
                            let data = uc.get_data();
                            data.mmu.clone()
                        };
                        mmu_arc.lock().unwrap().mem_protect(
                            uc,
                            page,
                            0x1000,
                            Prot::READ | Prot::WRITE,
                        );
                        if uc.mem_write(slot as u64, &fake.to_le_bytes()).is_ok() {
                            let _ = uc.reg_write(RegisterARM::R3, fake as u64);
                        }
                    }
                }
                let count = MAP_DRAW_BLIT_BRANCH_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 80 {
                    log::info!(
                        "PROCMAPENGINE: DrawBlit {} at {:#x} r0={:#x} r3={:#x} r4={:#x} r7={:#x} r9={:#x} trace={}",
                        name,
                        addr,
                        r0,
                        r3,
                        r4,
                        r7,
                        r9,
                        count + 1
                    );
                }
            })
            .unwrap();
    }

    let _ = blit_null_return;

    let flip_entry = base_address + SURFACE_FLIP_ENTRY;
    unicorn
        .add_code_hook(flip_entry as u64, flip_entry as u64, move |uc, _, _| {
            let surface = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let count = SURFACE_FLIP_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 20 {
                log::info!(
                    "PROCMAPENGINE: surface Flip entry surface={:#x} count={}",
                    surface,
                    count + 1
                );
            }
        })
        .unwrap();
}

fn add_map_job_queue_add_element_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let entry = base_address + MAP_JOB_QUEUE_ADD_ELEMENT_ENTRY;
    unicorn
        .add_code_hook(entry as u64, entry as u64, move |uc, _, _| {
            let thread = uc.get_data().inner.thread_id();
            let queue = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let element = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let out_status = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let out_count = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
            let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
            let queue_state = read_u32_or_invalid(uc, queue);
            let message_handle = read_u32_or_invalid(uc, queue + 0x30);
            let count = MAP_JOB_QUEUE_ADD_ENTRY_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 300 {
                log::info!(
                    "PROCMAPENGINE: AddElement entry queue={:#x} state={:#x} msg={:#x} element={:#x} out_status={:#x} out_count={:#x} lr={:#x} sp={:#x} thread=[{}] trace={}",
                    queue,
                    queue_state,
                    message_handle,
                    element,
                    out_status,
                    out_count,
                    lr,
                    sp,
                    thread,
                    count + 1
                );
            }
        })
        .unwrap();

    let pre_post = base_address + MAP_JOB_QUEUE_ADD_ELEMENT_PRE_POST;
    unicorn
        .add_code_hook(pre_post as u64, pre_post as u64, move |uc, _, _| {
            save_map_job_callee_regs(uc);
        })
        .unwrap();

    let after_post = base_address + MAP_JOB_QUEUE_ADD_ELEMENT_AFTER_POST;
    unicorn
        .add_code_hook(after_post as u64, after_post as u64, move |uc, _, _| {
            let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
            restore_map_job_callee_regs(uc, sp);
            let thread = uc.get_data().inner.thread_id();
            let queue = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            let r9 = uc.reg_read(RegisterARM::R9).unwrap_or(0) as u32;
            let message_before_post = read_u32_or_invalid(uc, queue + 0x30);
            let message_current = read_u32_or_invalid(uc, r4);
            let suspicious = r4 == 0 || r4 != message_before_post;
            let count = MAP_JOB_QUEUE_AFTER_POST_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if suspicious || count < 20 {
                log::info!(
                    "PROCMAPENGINE: AddElement after OSAL Post queue={:#x} post_ret={:#x} r4={:#x} msg_in_queue={:#x} *r4={:#x} r9={:#x} sp={:#x} thread=[{}] trace={}",
                    queue,
                    r0,
                    r4,
                    message_before_post,
                    message_current,
                    r9,
                    sp,
                    thread,
                    count + 1
                );
            }
        })
        .unwrap();

    for offset in [MAP_JOB_QUEUE_ADD_ELEMENT_RETURN, MAP_JOB_QUEUE_ADD_ELEMENT_RETURN_ALT] {
        let return_addr = base_address + offset;
        unicorn
            .add_code_hook(return_addr as u64, return_addr as u64, move |_, _, _| {
                clear_map_job_saved_callee_regs();
            })
            .unwrap();
    }

    let b_post_ipc = base_address + (0x0066_0ea0u32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(b_post_ipc as u64, b_post_ipc as u64, move |uc, _, _| {
            let count = AIL_B_POST_IPC_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 24 && count % 20_000 != 0 {
                return;
            }
            let thread = uc.get_data().inner.thread_id();
            let svc = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xffff;
            let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
            log::info!(
                "PROCMAPENGINE: bPostIpcMessage svc={:#x} r2={:#x} msg={:#x} lr={:#x} thread=[{}] trace={}",
                svc,
                r2,
                r3,
                lr,
                thread,
                count + 1
            );
        })
        .unwrap();

    let ipc_post_trace = base_address + AIL_POST_IPC_MESSAGE;
    unicorn
        .add_code_hook(ipc_post_trace as u64, ipc_post_trace as u64, move |uc, _, _| {
            let count = AIL_IPC_POST_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 24 && count % 20_000 != 0 {
                return;
            }
            let thread = uc.get_data().inner.thread_id();
            let appid = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let queue = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let msgobj = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
            let handle = read_u32_or_invalid(uc, msgobj + 4);
            let msg_type = read_u32_or_invalid(uc, handle);
            let content = read_u32_or_invalid(uc, handle + 4);
            let mut words = Vec::new();
            if handle < 0xf000_0000 {
                for i in 0..14u32 {
                    words.push(format!("{:08x}", read_u32_or_invalid(uc, handle + i * 4)));
                }
            }
            log::info!(
                "PROCMAPENGINE: ail_bIpcMessagePost app={:#x} queue={:#x} lr={:#x} handle={:#x} type={} content={:#x} [{}] thread=[{}] trace={}",
                appid,
                queue,
                lr,
                handle,
                msg_type,
                content,
                words.join(" "),
                thread,
                count + 1
            );
        })
        .unwrap();

    // posfi NavPositionStatus consumption chain (observer probes): the
    // dispatcher hands svc 0x1e / fn 0x10 / op 6 messages to vHandleNavPosition,
    // which parses them, feeds the NavPropertyHandler (CCP) and re-arms the map
    // data request window.  Log each stage plus the parsed-object shape so a
    // fabricated provider message can be validated end to end.
    let vhandle_nav_position = base_address + NAVI_HANDLER_VHANDLE_NAV_POSITION;
    unicorn
        .add_code_hook(vhandle_nav_position as u64, vhandle_nav_position as u64, move |uc, _, _| {
            let count = NAV_POSITION_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 12 {
                return;
            }
            let thread = uc.get_data().inner.thread_id();
            let handler = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let message = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let handle = read_u32_or_invalid(uc, message + 4);
            log::info!(
                "PROCMAPENGINE: vHandleNavPosition msg={:#x} handle={:#x} opcode={:#x} seq={:#x} thread=[{}] trace={}",
                message,
                handle,
                read_u32_or_invalid(uc, handle + 0x1a) & 0xff,
                read_u32_or_invalid(uc, handle + 0x1c) & 0xffff,
                thread,
                count + 1
            );
            if count < 3 && (0x1000..0xf000_0000).contains(&handler) {
                // The window gate in the gate body compares the parsed
                // longitude/latitude against this handler's bounding box at
                // +0x70 (lon min) +0x74 (lon max) +0x78 (lat min) +0x7c
                // (lat max); log it so a silently pre-initialized window is
                // visible.
                log::info!(
                    "PROCMAPENGINE: vHandleNavPosition window-gate this={handler:#x} bbox=[{:08x} {:08x} {:08x} {:08x}]",
                    read_u32_or_invalid(uc, handler + 0x70),
                    read_u32_or_invalid(uc, handler + 0x74),
                    read_u32_or_invalid(uc, handler + 0x78),
                    read_u32_or_invalid(uc, handler + 0x7c)
                );
            }
        })
        .unwrap();

    let handle_service_data = base_address + AIL_HANDLE_MSG_SERVICE_DATA;
    unicorn
        .add_code_hook(handle_service_data as u64, handle_service_data as u64, move |uc, _, _| {
            // Watchdog: the engine's live registrations can detach the
            // fabricated posfi reference from the server-ref list; re-link
            // it before this message's claim walk runs (r429).
            {
                let dispatch = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                if dispatch < 0xf000_0000 {
                    let app_iface = read_u32_or_invalid(uc, dispatch);
                    if app_iface < 0xf000_0000 {
                        ensure_posfi_ref_linked(uc, app_iface);
                        ensure_dapi_map_service_reference(uc, app_iface);
                    }
                }
            }
            let count = HANDLE_SERVICE_DATA_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 400 {
                return;
            }
            let thread = uc.get_data().inner.thread_id();
            let message = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let handle = read_u32_or_invalid(uc, message + 4);
            // Dump the client-side service registry list (dispatch+0x58) once:
            // bIsDataSet matches entries' {+0x00, +0x02, +0x04, +0x08} against
            // the message tuple, so the entry contents tell which message
            // fields must carry what.
            let dispatch = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            if count == 0 && dispatch < 0xf000_0000 {
                // the claim tail calls (**(app_iface) + 0x20)(app_iface, msg);
                // log that virtual's target so its routing can be traced.
                let app_iface = read_u32_or_invalid(uc, dispatch);
                let vtable = read_u32_or_invalid(uc, app_iface);
                log::info!(
                    "PROCMAPENGINE: claim vtable trace: dispatch={dispatch:#x} app_iface={app_iface:#x} vtable={vtable:#x} +0x20->{:#x}",
                    read_u32_or_invalid(uc, vtable + 0x20)
                );
            }
            // Also dump on the rendererfi request (service 0x300): an opcode-2
            // request walks the *registry* list at app_iface+0x58 (`bIsServerMessage`
            // says "not from a server"), so its content decides whether the claim
            // tail-calls the port-control platform's `vOnNewMessage` or answers the
            // client with error 6 (unknown register-id).
            let request_service = read_u32_or_invalid(uc, handle + 0x14) & 0xffff;
            if (matches!(count, 0 | 6) || request_service == 0x0300) && dispatch < 0xf000_0000 {
                // app interface is dispatch+0x00; the AIL lists are
                // app_iface+0x4c (async service requests), +0x50 (server
                // service references) and +0x58 (client service registry).
                let app_iface = read_u32_or_invalid(uc, dispatch);
                for (list_off, list_name) in [
                    (0x4cu32, "requests"),
                    (0x50, "server-refs"),
                    (0x58, "client-registry"),
                    (0x70, "async-reqs"),
                ] {
                    let list_head = read_u32_or_invalid(uc, app_iface + list_off);
                    if list_off == 0x50 {
                        // Link-state diagnostics (r429): the engine's own
                        // svc 0x26 registration detached the host-linked
                        // posfi reference (first moved to the engine node),
                        // so log the raw list head and the posfi node's own
                        // prev/next to see how the chain is rebuilt.
                        let posfi = POSFI_REF_BLOB.load(Ordering::Relaxed);
                        log::info!(
                            "PROCMAPENGINE: server-refs list {list_head:#x} raw [{:08x} {:08x} {:08x} {:08x}] posfi_node={posfi:#x} prev={:08x} next={:08x}",
                            read_u32_or_invalid(uc, list_head),
                            read_u32_or_invalid(uc, list_head + 4),
                            read_u32_or_invalid(uc, list_head + 8),
                            read_u32_or_invalid(uc, list_head + 0xc),
                            read_u32_or_invalid(uc, posfi),
                            read_u32_or_invalid(uc, posfi + 4)
                        );
                    }
                    let mut node = read_u32_or_invalid(uc, list_head + 4);
                    let mut idx = 0u32;
                    while (0x1000..0xf000_0000).contains(&node) && idx < 12 {
                        let mut words = Vec::new();
                        for i in 0..8u32 {
                            words.push(format!("{:08x}", read_u32_or_invalid(uc, node + 8 + i * 4)));
                        }
                        log::info!(
                            "PROCMAPENGINE: {list_name} entry[{idx}] node={node:#x} [{}] trace={}",
                            words.join(" "),
                            count + 1
                        );
                        let next = read_u32_or_invalid(uc, node + 4);
                        if next == node {
                            break;
                        }
                        node = next;
                        idx += 1;
                    }
                }
            }
            log::info!(
                "PROCMAPENGINE: ail_bHandleMsgServiceData msg={:#x} handle={:#x} sub={:#x} class={:#x} regid={:#x} svc={:#x} fn={:#x} thread=[{}] trace={}",
                message,
                handle,
                read_u32_or_invalid(uc, handle) & 0xffff,
                read_u32_or_invalid(uc, handle + 0xb) & 0xff,
                read_u32_or_invalid(uc, handle + 0xe) & 0xffff,
                read_u32_or_invalid(uc, handle + 0x14) & 0xffff,
                read_u32_or_invalid(uc, handle + 0x18) & 0xffff,
                thread,
                count + 1
            );
        })
        .unwrap();

    // map_tclPortControl_Platform::vOnNewMessage: the client dispatch tail of
    // the claim path.  Logs the lookup key (message u16 at content+0x00) and
    // enumerates the client-handler map at platform+0xa0 so the correct key
    // for the posfi handler can be identified.
    let von_new_message = base_address + PORTCONTROL_VON_NEW_MESSAGE;
    unicorn
        .add_code_hook(von_new_message as u64, von_new_message as u64, move |uc, _, _| {
            let count = VON_NEW_MESSAGE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 12 {
                return;
            }
            let thread = uc.get_data().inner.thread_id();
            let platform = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let message = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let handle = read_u32_or_invalid(uc, message + 4);
            log::info!(
                "PROCMAPENGINE: vOnNewMessage platform={platform:#x} msg={message:#x} handle={handle:#x} sub={:#x} class={:#x} svc={:#x} fn={:#x} thread=[{thread}] trace={}",
                read_u32_or_invalid(uc, handle) & 0xffff,
                read_u32_or_invalid(uc, handle + 0xb) & 0xff,
                read_u32_or_invalid(uc, handle + 0x14) & 0xffff,
                read_u32_or_invalid(uc, handle + 0x18) & 0xffff,
                count + 1
            );
            if count < 3 && platform < 0xf000_0000 {
                // bpstl::map<u16, CCAClientHandlerBase*> at platform+0xa0.
                // Dump the header words, then walk the red-black tree with
                // both classic libstdc++ node layouts to enumerate keys.
                let map = platform + 0xa0;
                let mut words = Vec::new();
                for i in 0..6u32 {
                    words.push(format!("{:08x}", read_u32_or_invalid(uc, map + i * 4)));
                }
                log::info!("PROCMAPENGINE: client-handler map {map:#x} header [{map_words}]", map_words = words.join(" "));
                // map<unsigned_short,handler*>:: node layout read off
                // operator[] (0x39ef80): map+0 -> header node, header+4 =
                // root, node+8 = left, node+0xc = right, node+0x10 = key
                // (u16), node+0x14 = mapped handler pointer.
                let header = read_u32_or_invalid(uc, map);
                let root = read_u32_or_invalid(uc, header + 4);
                if (0x1000..0xf000_0000).contains(&root) {
                    let mut stack: Vec<u32> = Vec::new();
                    let mut node = root;
                    let mut visited = 0u32;
                    while visited < 16 {
                        while (0x1000..0xf000_0000).contains(&node) {
                            stack.push(node);
                            node = read_u32_or_invalid(uc, node + 8);
                        }
                        let Some(entry) = stack.pop() else { break };
                        log::info!(
                            "PROCMAPENGINE: handler-map node={entry:#x} key={:#x} value={:#x}",
                            read_u32_or_invalid(uc, entry + 0x10) & 0xffff,
                            read_u32_or_invalid(uc, entry + 0x14)
                        );
                        visited += 1;
                        node = read_u32_or_invalid(uc, entry + 0xc);
                    }
                }
            }
        })
        .unwrap();

    // ail_tclServiceReference::bIsDataSet - the claim walk's predicate (the
    // server-ref loop calls it with (svc, sub-id, reg-id, +0x0e) and 0xffff
    // wildcards).  Log every call's tuple, the entry's stored fields and the
    // caller LR so a non-match can be pinned to the mismatching slot.
    let ref_is_dataset = base_address + AIL_REF_IS_DATASET_FN;
    unicorn
        .add_code_hook(ref_is_dataset as u64, ref_is_dataset as u64, move |uc, _, _| {
            let count = REF_WALK_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 200 {
                return;
            }
            let thread = uc.get_data().inner.thread_id();
            let entry = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let p1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xffff;
            let p2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32 & 0xffff;
            let p3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32 & 0xffff;
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let p4 = read_u32_or_invalid(uc, sp) & 0xffff;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            // 0x66e3d0 compares p1..p3 against +0/+2/+4 (0xffff = wildcard) and
            // p4 against +8, so dump the four compared slots.
            log::info!(
                "PROCMAPENGINE: ref bIsDataSet entry={entry:#x} tuple=({p1:#x},{p2:#x},{p3:#x},{p4:#x}) compared=[{:#x},{:#x},{:#x},{:#x}] plus6={:#x} state={:#x} lr={lr:#x} thread=[{thread}] trace={}",
                read_u32_or_invalid(uc, entry) & 0xffff,
                (read_u32_or_invalid(uc, entry) >> 16) & 0xffff,
                read_u32_or_invalid(uc, entry + 4) & 0xffff,
                read_u32_or_invalid(uc, entry + 8) & 0xffff,
                (read_u32_or_invalid(uc, entry + 4) >> 16) & 0xffff,
                (read_u32_or_invalid(uc, entry + 4) >> 16) & 0xff,
                count + 1
            );
        })
        .unwrap();

    // ail_tclServiceReference ctor: shows what a register-confirmation actually
    // handed the engine for each service reference (the +8 unique id is the
    // slot later matched against every ServiceData message's unique id).
    let ref_ctor = base_address + AIL_REF_CTOR_FN;
    unicorn
        .add_code_hook(ref_ctor as u64, ref_ctor as u64, move |uc, _, _| {
            let count = REF_CTOR_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 40 {
                return;
            }
            let thread = uc.get_data().inner.thread_id();
            let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let app = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xffff;
            let service = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32 & 0xffff;
            let regid = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32 & 0xffff;
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let unique = read_u32_or_invalid(uc, sp) & 0xffff;
            let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
            log::info!(
                "PROCMAPENGINE: service-reference ctor this={this:#x} app={app:#x} service={service:#x} regid={regid:#x} unique={unique:#x} lr={lr:#x} thread=[{thread}] trace={}",
                count + 1
            );
        })
        .unwrap();

    let vprocess_nav_pos = base_address + NAV_HANDLER_VPROCESS_NAV_POS;    unicorn
        .add_code_hook(vprocess_nav_pos as u64, vprocess_nav_pos as u64, move |uc, _, _| {
            let count = NAV_PROCESS_POS_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 2 {
                return;
            }
            let thread = uc.get_data().inner.thread_id();
            let handler = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let object = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let mut words = Vec::new();
            if object < 0xf000_0000 {
                // Full posfi_tclMsgNavPositionStatus parse object (0x2bc
                // bytes): each embedded field slot carries its descriptor
                // cell pointer, so the whole tree plus the pattern payload
                // identifies every field's wire offset.
                for i in 0..176u32 {
                    words.push(format!("{:08x}", read_u32_or_invalid(uc, object + i * 4)));
                }
            }
            log::info!(
                "PROCMAPENGINE: vProcessNavPositionStatus handler={:#x} object={:#x} [{}] thread=[{}] trace={}",
                handler,
                object,
                words.join(" "),
                thread,
                count + 1
            );
        })
        .unwrap();

    let update_map_data_request = base_address + MAP_DATA_MANAGER_UPDATE_REQUEST;
    unicorn
        .add_code_hook(
            update_map_data_request as u64,
            update_map_data_request as u64,
            move |uc, _, _| {
                let count =
                    MAP_DATA_UPDATE_REQUEST_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                // r431: the posfi window call (r0=4) stayed invisible
                // because the flood of default r0=5 requests ate the whole
                // 24-entry budget first.  Log only a handful of those and
                // every other priority.
                let r0_now = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                if count >= 200 || (r0_now == 5 && count >= 8) {
                    return;
                }
                let thread = uc.get_data().inner.thread_id();
                let mut args = Vec::new();
                for reg in [
                    RegisterARM::R0,
                    RegisterARM::R1,
                    RegisterARM::R2,
                    RegisterARM::R3,
                ] {
                    args.push(format!("{:#x}", uc.reg_read(reg).unwrap_or(0) as u32));
                }
                let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                let stack: Vec<String> = (0..4)
                    .map(|i| format!("{:#x}", read_u32_or_invalid(uc, sp + i * 4)))
                    .collect();
                log::info!(
                    "PROCMAPENGINE: UpdateMapDataRequest r0-r3=[{}] stack=[{}] thread=[{}] trace={}",
                    args.join(" "),
                    stack.join(" "),
                    thread,
                    count + 1
                );
            },
        )
        .unwrap();

    // Cache the PortControl platform object for the Navi registration kick.
    let platform_ctor = base_address + PORTCONTROL_PLATFORM_CTOR;
    unicorn
        .add_code_hook(platform_ctor as u64, platform_ctor as u64, move |uc, _, _| {
            let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            PORTCONTROL_PLATFORM.store(this, Ordering::Relaxed);
            log::info!("PROCMAPENGINE: map_tclPortControl_Platform constructed at {:#x}", this);
        })
        .unwrap();

    let on_server_app_status = base_address + AIL_ON_SERVER_APP_STATUS;
    unicorn
        .add_code_hook(
            on_server_app_status as u64,
            on_server_app_status as u64,
            move |uc, _, _| {
                let count = AIL_SERVER_APP_STATUS_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 24 && count % 50_000 != 0 {
                    return;
                }
                let thread = uc.get_data().inner.thread_id();
                let svc = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xffff;
                let server_app = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32 & 0xffff;
                let state = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32 & 0xff;
                log::info!(
                    "PROCMAPENGINE: vOnServerApplicationStatus svc={:#x} server_app={:#x} state={:#x} thread=[{}] trace={}",
                    svc,
                    server_app,
                    state,
                    thread,
                    count + 1
                );
            },
        )
        .unwrap();

    let pre_crash = base_address + MAP_JOB_QUEUE_ADD_ELEMENT_PRE_CRASH;
    unicorn
        .add_code_hook(pre_crash as u64, pre_crash as u64, move |uc, _, _| {
            let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
            restore_map_job_callee_regs(uc, sp);
            let thread = uc.get_data().inner.thread_id();
            let queue = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
            let mut r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            let r9 = uc.reg_read(RegisterARM::R9).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
            let expected_message = read_u32_or_invalid(uc, queue + 0x30);
            let mut deref_r4 = read_u32_or_invalid(uc, r4);
            if (r4 == 0 || r4 == 0xffff_ffff || deref_r4 == 0xffff_ffff)
                && expected_message != 0
                && expected_message != 0xffff_ffff
            {
                uc.reg_write(RegisterARM::R4, expected_message as u64).unwrap();
                r4 = expected_message;
                deref_r4 = read_u32_or_invalid(uc, r4);
                log::info!(
                    "PROCMAPENGINE: AddElement repaired r4 before 0x4a4380 queue={:#x} expected_msg={:#x} thread=[{}]",
                    queue,
                    expected_message,
                    thread
                );
            }
            let suspicious =
                r4 == 0 || r4 != expected_message || deref_r4 == 0 || deref_r4 == 0xffff_ffff;
            let count = MAP_JOB_QUEUE_PRE_CRASH_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if suspicious || count < 20 {
                log::info!(
                    "PROCMAPENGINE: AddElement before 0x4a4380 ldr r0,[r4] queue={:#x} r0={:#x} r1={:#x} r2={:#x} r3={:#x} r4={:#x} expected_msg={:#x} *r4={:#x} r9={:#x} lr={:#x} sp={:#x} thread=[{}] trace={}",
                    queue,
                    r0,
                    r1,
                    r2,
                    r3,
                    r4,
                    expected_message,
                    deref_r4,
                    r9,
                    lr,
                    sp,
                    thread,
                    count + 1
                );
            }
        })
        .unwrap();
}

fn add_job_queue_wait_skip_hook(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    unicorn
        .add_code_hook(OSAL_THREAD_WAIT as u64, OSAL_THREAD_WAIT as u64, move |uc, _, _| {
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            if r0 != 1 {
                return;
            }

            let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
            if lr < base_address || lr >= base_address + 0x70_0000 {
                return;
            }

            let tick = MAP_OSAL_WAIT_SKIP_TICK.fetch_add(1, Ordering::Relaxed) + 1;
            if tick > 100 && tick % MAP_RENDER_TRIGGER_INTERVAL != 1 {
                return;
            }

            let mut triggered = false;
            if MAP_VIEW_READY.load(Ordering::Relaxed) {
                // r648: periodic Flint trigger disabled, see the
                // `add_render_job_queue_wait_skip_hook` comment above
                // (request-state-machine reset storm).
                triggered = trigger_pending_map_render(uc, lr, false);
            }
            if !triggered {
                uc.reg_write(RegisterARM::R0, 0).unwrap_or_default();
                uc.reg_write(RegisterARM::PC, lr as u64).unwrap_or_default();
            }

            let trace_count = MAP_OSAL_WAIT_SKIP_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if trace_count < 50 {
                log::info!(
                    "PROCMAPENGINE: skipped OSAL thread wait tick={} return={:#x} triggered={}",
                    tick,
                    lr,
                    triggered
                );
            }
        })
        .unwrap();
}

/// Trace the FastMap channel path, the gate that stands between the 34 block
/// ids DAPI answers and any block *content* ever arriving.
///
/// `map_tclJobMDMHandleGetBlockIDs::Execute` ends by calling
/// `map_tclMapDataManager::HandleMapDataRequest` (0x512108), which silently
/// returns unless `ms_u32DatasetID == ms_u32DatasetID_Validated != 0` *and*
/// `ms_bFastmapServiceAvailable == 1`; only then does it run
/// `IntHandleMapDataRequest`, which loads the wanted blocks through
/// `DAPI_LoadBlocks` over an open FastMap channel. The channel opens in
/// `OnUpdateServiceStateDAPIFastmap` (0x512400, reached via the CCA service
/// state machine `vOnServiceState` -> `map_tclClientServiceHandler::
/// vOnServiceStateChange`), which loops `ms_u32NumberOfFastMapChannels` times
/// over `ms_asFastMapChannelInfo` (registry
/// `PROCESS/MAP/MAPENGINE/APP_CONFIG/FASTMAP`: NUMBER default 3, NAME + SIZE
/// required) calling `MapData_OpenFastMapChannel` -> `DAPI_OpenFastMapChannel`
/// (needs `info + 4` shared memory != NULL) -> CCA fn 0x101 request. None of
/// these have ever been observed at runtime; these probes say exactly where the
/// chain stops.
fn add_fastmap_channel_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};

    // The MDM globals, at their Ghidra VAs relative to the image base.
    let global = |va: u32| base_address + (va - ORIGINAL_BASE);
    let stop_loading = global(0x0078_f7a0);
    let dataset_id = global(0x0078_f7f0);
    let fastmap_available = global(0x0078_f7f8);
    let channel_info = global(0x0078_f81c);
    let dataset_validated = global(0x0078_f874);
    let channel_number = global(0x0078_f878);

    // `map_tclClientServiceHandler::vOnServiceStateChange(uchar)`: the CCA
    // service state fan-in; `this + 4` is the u16 service id, `this + 0xe` the
    // stored state.
    let site = base_address + (0x0059_0390u32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(site as u64, site as u64, move |uc, _, _| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) >= 60 {
                return;
            }
            let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let svc = read_u16_or_invalid(uc, this + 4);
            if svc == 0x0026 {
                // A walk popped our directory publish and applied it to the
                // svc-0x26 client reference; the re-post driver can stop.
                FASTMAP_STATUS_APPLIED.store(true, Ordering::Relaxed);
            }
            log::info!(
                "PROCMAPENGINE fastmap trace vOnServiceStateChange t={}ms thread=[{}] handler={:#x} svc={:#x} old_state={:#x} new_state={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                this,
                svc,
                read_u8_or_invalid(uc, this + 0xe),
                (uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32) & 0xff,
            );
        })
        .unwrap();

    // `map_tclMapDataManager::OnUpdateServiceStateDAPIFastmap(uchar)`: opens a
    // channel per configured FastMap slot when it runs with state 1.
    let site = base_address + (0x0051_2400u32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(site as u64, site as u64, move |uc, _, _| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            let n = COUNT.fetch_add(1, Ordering::Relaxed);
            if n >= 20 {
                return;
            }
            log::info!(
                "PROCMAPENGINE fastmap trace OnUpdateServiceStateDAPIFastmap t={}ms thread=[{}] state={:#x} dataset={:#x} validated={:#x} fastmap_avail={:#x} channels={:#x} stop_loading={:#x} info[0,10,20]={:#x},{:#x},{:#x} lr={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                (uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32) & 0xff,
                read_u32_or_invalid(uc, dataset_id),
                read_u32_or_invalid(uc, dataset_validated),
                read_u8_or_invalid(uc, fastmap_available),
                read_u32_or_invalid(uc, channel_number),
                read_u32_or_invalid(uc, stop_loading),
                read_u32_or_invalid(uc, channel_info),
                read_u32_or_invalid(uc, channel_info + 0x10),
                read_u32_or_invalid(uc, channel_info + 0x20),
                ((uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1)
                    .wrapping_sub(base_address)
                    .wrapping_add(ORIGINAL_BASE),
            );
        })
        .unwrap();

    // `map_tclMapDataManager::HandleMapDataRequest(uchar)`: the gate itself.
    let site = base_address + (0x0051_2108u32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(site as u64, site as u64, move |uc, _, _| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            let n = COUNT.fetch_add(1, Ordering::Relaxed);
            if n >= 20 {
                return;
            }
            log::info!(
                "PROCMAPENGINE fastmap trace HandleMapDataRequest t={}ms thread=[{}] arg={:#x} dataset={:#x} validated={:#x} fastmap_avail={:#x} channels={:#x} stop_loading={:#x} lr={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                (uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32) & 0xff,
                read_u32_or_invalid(uc, dataset_id),
                read_u32_or_invalid(uc, dataset_validated),
                read_u8_or_invalid(uc, fastmap_available),
                read_u32_or_invalid(uc, channel_number),
                read_u32_or_invalid(uc, stop_loading),
                ((uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1)
                    .wrapping_sub(base_address)
                    .wrapping_add(ORIGINAL_BASE),
            );
        })
        .unwrap();

    // `map_tclPortControl::MapData_OpenFastMapChannel(u16, map_rFastMapChannelInfo&)`.
    let site = base_address + (0x0053_a298u32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(site as u64, site as u64, move |uc, _, _| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) >= 20 {
                return;
            }
            let channel = (uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32) & 0xffff;
            let info = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            log::info!(
                "PROCMAPENGINE fastmap trace MapData_OpenFastMapChannel t={}ms thread=[{}] channel={:#x} info={:#x} words=[{}]",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                channel,
                info,
                (0..4u32)
                    .map(|i| format!("{:#x}", read_u32_or_invalid(uc, info.wrapping_add(i * 4))))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        })
        .unwrap();

    // `PortControl_CCAClientHandler_DAPI::OpenFastMapChannel(u16, info&)`:
    // refuses with 0xffab2a while the client service handler
    // (`this + 0x24`) says `bIsServiceAvailable == 0`, so log its state.
    let site = base_address + (0x0053_d500u32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(site as u64, site as u64, move |uc, _, _| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) >= 20 {
                return;
            }
            let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let handler = read_u32_or_invalid(uc, this + 0x24);
            let (svc, state) = if handler != 0 && handler != u32::MAX {
                (
                    read_u16_or_invalid(uc, handler + 4),
                    read_u8_or_invalid(uc, handler + 0xe),
                )
            } else {
                (0xffff, 0xff)
            };
            let channel = (uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32) & 0xffff;
            let info = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let shm = read_u32_or_invalid(uc, info.wrapping_add(4));
            log::info!(
                "PROCMAPENGINE fastmap trace CCA OpenFastMapChannel t={}ms thread=[{}] handler={:#x} svc={:#x} state={:#x} channel={:#x} shm={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                handler,
                svc,
                state,
                channel,
                shm,
            );
        })
        .unwrap();

    // ---- property registration / service state fan-out (fastmap availability) ----
    // `PortControl_CCAClientHandlerBase::vOnServiceState(svc, reg, ...)`: logs
    // every arrival, whether a client handler matched or it traced "Unhandled
    // service state change".
    let site = base_address + (0x0059_2604u32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(site as u64, site as u64, move |uc, _, _| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) >= 40 {
                return;
            }
            let words = (0..5u32)
                .map(|i| {
                    let r = [RegisterARM::R0, RegisterARM::R1, RegisterARM::R2, RegisterARM::R3, RegisterARM::R4][i as usize];
                    format!("{:#x}", (uc.reg_read(r).unwrap_or(0) as u32))
                })
                .collect::<Vec<_>>()
                .join(",");
            log::info!(
                "PROCMAPENGINE fastmap trace vOnServiceState t={}ms thread=[{}] r0..r4=[{}] lr={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                words,
                ((uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1)
                    .wrapping_sub(base_address)
                    .wrapping_add(ORIGINAL_BASE),
            );
        })
        .unwrap();

    // AIL-side availability fan-out `vServiceAvailabilityChanged` (0x66434c):
    // reads the registry state; the only path that tells PortControl.
    let site = base_address + (0x0066_434cu32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(site as u64, site as u64, move |uc, _, _| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) >= 40 {
                return;
            }
            let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            log::info!(
                "PROCMAPENGINE fastmap trace vServiceAvailabilityChanged t={}ms thread=[{}] this={:#x} r1={:#x} lr={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                this,
                uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                ((uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1)
                    .wrapping_sub(base_address)
                    .wrapping_add(ORIGINAL_BASE),
            );
        })
        .unwrap();

    // Property subscription state machine: `vUpRegOrGet` (0x593580) sends the
    // UpReg/Get pair, `CheckSubscriptionState` (0x593218) and
    // `vCheckPropertyRegistration` (0x59327c) interpret the answers, and
    // `bSendServiceDataMsg` (0x593668) is the send itself. Log handler + args +
    // message header so the stalled step shows up.
    for (va, label) in [
        (0x0059_3580u32, "vUpRegOrGet(q,state)"),
        (0x0059_3218u32, "CheckSubscriptionState"),
        (0x0059_327cu32, "vCheckPropertyRegistration"),
        (0x0059_3668u32, "bSendServiceDataMsg"),
        (0x0059_31e0u32, "vServiceStateChanged(base)"),
    ] {
        let site = base_address + (va - ORIGINAL_BASE);
        unicorn
            .add_code_hook(site as u64, site as u64, move |uc, _, _| {
                static COUNT: AtomicU32 = AtomicU32::new(0);
                if COUNT.fetch_add(1, Ordering::Relaxed) >= 100 {
                    return;
                }
                let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let arg1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                // The message-taking sites pass an `amt_tclServiceData` whose
                // header pointer sits at +0x04; the others pass values.
                let (svc, reg, fn_id, opcode) = if va == 0x0059_3218 || va == 0x0059_327c {
                    let header = read_u32_or_invalid(uc, arg1.wrapping_add(4));
                    (
                        read_u16_or_invalid(uc, header.wrapping_add(0x14)),
                        read_u16_or_invalid(uc, header.wrapping_add(0x16)),
                        read_u16_or_invalid(uc, header.wrapping_add(0x18)),
                        read_u8_or_invalid(uc, header.wrapping_add(0x1a)),
                    )
                } else {
                    (0xffff, 0xffff, 0xffff, 0xff)
                };
                log::info!(
                    "PROCMAPENGINE fastmap trace {label} t={}ms thread=[{}] this={:#x} words=[{}] r1={:#x} svc={:#x} reg={:#x} fn={:#x} opcode={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    this,
                    (0..6u32)
                        .map(|i| format!("{:#x}", read_u32_or_invalid(uc, this.wrapping_add(i * 4))))
                        .collect::<Vec<_>>()
                        .join(","),
                    arg1,
                    svc,
                    reg,
                    fn_id,
                    opcode,
                );
            })
            .unwrap();
    }

    // `ail_tclServiceReference::bIsDataSet(svc, server_app, reg_id)` is called
    // once per client reference from the tail of
    // `ail_bExamineInfluenceToAdministration` to find the ref a ServiceStatus
    // applies to; the ServiceState only flips on a full 3-field match. Log
    // every candidate (stored fields + lookup tuple) so a near-miss - e.g. an
    // AIL-self-assigned register-id vs our conf id - shows up.
    let site = base_address + (0x0066_e394u32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(site as u64, site as u64, move |uc, _, _| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) >= 400 {
                return;
            }
            let reference = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let want = [RegisterARM::R1, RegisterARM::R2, RegisterARM::R3]
                .map(|r| (uc.reg_read(r).unwrap_or(0) as u32) & 0xffff);
            log::info!(
                "PROCMAPENGINE fastmap trace bIsDataSet t={}ms thread=[{}] ref={:#x} fields=[{}] want=[{:#x},{:#x},{:#x}] lr={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                reference,
                (0..4u32)
                    .map(|i| format!(
                        "{:#x}",
                        read_u32_or_invalid(uc, reference.wrapping_add(i * 4))
                    ))
                    .collect::<Vec<_>>()
                    .join(","),
                want[0],
                want[1],
                want[2],
                ((uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1)
                    .wrapping_sub(base_address)
                    .wrapping_add(ORIGINAL_BASE),
            );
        })
        .unwrap();

    // Instruction tracer over the AIL administration module, armed per
    // admin-type-2 arrival by the `ExamineInfluenceToAdministration` probe
    // (see EXAMINE_TRACE_STEPS). Logs the real fetch sequence so the actual
    // ServiceStatus code path is observable despite the Ghidra misdecode.
    {
        let lo = (base_address + (0x0066_0000u32 - ORIGINAL_BASE)) as u64;
        let hi = (base_address + (0x0067_0000u32 - ORIGINAL_BASE)) as u64;
        unicorn
            .add_code_hook(lo, hi, move |uc, address, _| {
                if EXAMINE_TRACE_STEPS.load(Ordering::Relaxed) == 0 {
                    return;
                }
                if uc.get_data().inner.thread_id()
                    != EXAMINE_TRACE_TID.load(Ordering::Relaxed)
                {
                    return;
                }
                let left = EXAMINE_TRACE_STEPS.fetch_sub(1, Ordering::Relaxed);
                if left <= 1 {
                    EXAMINE_TRACE_STEPS.store(0, Ordering::Relaxed);
                }
                log::info!(
                    "PROCMAPENGINE ail-step {:#x} (left {})",
                    ((address as u32) - base_address).wrapping_add(ORIGINAL_BASE),
                    left - 1,
                );
            })
            .unwrap();
    }

    // Write watchpoints on the three gate globals: whoever stores the dataset
    // id, the validated dataset id, or the fastmap availability byte tells us
    // which medium/CCA event the engine considers the trigger.
    for (va, name) in [
        (0x0078_f7f0u32, "ms_u32DatasetID"),
        (0x0078_f7f8u32, "ms_bFastmapServiceAvailable"),
        (0x0078_f874u32, "ms_u32DatasetID_Validated"),
    ] {
        use unicorn_engine::unicorn_const::HookType;
        let field = base_address + (va - ORIGINAL_BASE);
        unicorn
            .add_mem_hook(HookType::MEM_WRITE, field as u64, field as u64 + 3, move |uc, _t, address, size, value| {
                static COUNT: AtomicU32 = AtomicU32::new(0);
                if COUNT.fetch_add(1, Ordering::Relaxed) >= 60 {
                    return true;
                }
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                log::warn!(
                    "PROCMAPENGINE fastmap trace WRITE {name} t={}ms thread=[{}] size={} value={:#x} pc={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    size,
                    value,
                    pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE),
                );
                true
            })
            .unwrap();
    }

    // r611: the conf-triggered directory publish is popped by whichever engine
    // thread services `mbx_1024` first - r610 showed a pop by a consumer that
    // does not run the administration walk, so the pair was silently swallowed
    // and the fastmap gate (property 0x401 UpReg -> OpenFastMapChannel ->
    // channel handle in `ms_asFastMapChannelInfo`) never fired.  Re-post the
    // pair until the probe above reports an application.
    {
        let namespace = unicorn.get_data().namespace.clone();
        let spawned = std::thread::Builder::new()
            .name("fastmap-status".into())
            .spawn(move || {
                for round in 0..18u32 {
                    // guest-time pacing: the apply chain we wait for runs on
                    // the dilated clock (vtime).
                    crate::emulator::vtime::sleep_virtual(std::time::Duration::from_millis(
                        if round == 0 { 25_500 } else { 8_000 },
                    ));
                    if FASTMAP_STATUS_APPLIED.load(Ordering::Relaxed) {
                        break;
                    }
                    let (first, second) =
                        crate::libs::libosal_linux::message::post_engine_fastmap_status_pair(
                            &namespace,
                        );
                    log::info!(
                        "PROCMAPENGINE: re-posted directory ServiceStatus pair (state 1 then 0) into mbx_1024: {first} {second} round={round}"
                    );
                    if FASTMAP_STATUS_APPLIED.load(Ordering::Relaxed) {
                        break;
                    }
                }
            });
        if spawned.is_err() {
            log::warn!("PROCMAPENGINE: cannot start fastmap status re-post driver");
        }
    }
}

/// Follow DAPI's GetBlockIDs answer through the MDM job that applies it.
///
/// `PortControl_CCAClientHandler_DAPI::vHandleGetBlockIDs` (0x53e7ac) only
/// deserializes the answer when the ServiceData opcode is 7; opcode 8 is turned
/// into `amt_tclServiceDataError` and its code stored in the job, anything else
/// stores 0xffab26. The job's `Execute` (0x4b4028) then looks the requesting
/// map-data request up again by the MDM ref counter it stored at `job + 0x1e`
/// (`map_tclMapDataManager::GetDAPIRequestingMapDataRequest`, 0x5072b4) and, when
/// that lookup fails, throws the answer away with 0xffab14 at 0x4b4a98 - so
/// either step can silently swallow the block ids. These probes print the job
/// code, the ref counter, and the resolved request with its state/queue fields.
fn add_get_block_ids_answer_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static ANSWER_COUNT: AtomicU32 = AtomicU32::new(0);

    // `map_tclJobMDMHandleGetBlockIDs::Execute` entry.
    let execute = base_address + (0x004b_4028u32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(execute as u64, execute as u64, move |uc, _, _| {
            let count = ANSWER_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 60 {
                return;
            }
            let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            // The answer's block-id array (`fi_tclVisitorMessage::s32GetData` in
            // `vHandleGetBlockIDs` deserialized it just before this `Execute`): a
            // begin/end pointer pair at `job + 0x30`/`job + 0x34` with 12-byte
            // `fmp_tclUniqueID` elements (`(end - begin) >> 2 * -0x55555555` in
            // `Execute` is a divide by 12). `job + 0x24` is the array's vptr.
            let begin = read_u32_or_invalid(uc, this + 0x30);
            let end = read_u32_or_invalid(uc, this + 0x34);
            let count = if begin != u32::MAX && end != u32::MAX && end > begin && (end - begin) % 12 == 0 {
                ((end - begin) / 12) as u64
            } else {
                u64::from(u32::MAX)
            };
            let uid0 = if begin != u32::MAX && (0x10000..0x6000_0000).contains(&begin) {
                read_u32_or_invalid(uc, begin)
            } else {
                u32::MAX
            };
            let uid1 = if count > 1 {
                read_u32_or_invalid(uc, begin + 12)
            } else {
                0
            };
            log::info!(
                "PROCMAPENGINE map-data trace HandleGetBlockIDs::Execute at {:#x}: t={}ms thread=[{}] job={:#x} mdm_ref={:#x} answer_code={:#x} block_count={} uid0={:#x} uid1={:#x}",
                execute,
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                this,
                read_u16_or_invalid(uc, this + 0x1e),
                read_u32_or_invalid(uc, this + 0xc),
                count,
                uid0,
                uid1,
            );
        })
        .unwrap();

    // Result of `GetDAPIRequestingMapDataRequest`: `r0` is the request (0 means
    // the answer is dropped).
    let resolved = base_address + (0x004b_403cu32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(resolved as u64, resolved as u64, move |uc, _, _| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) >= 60 {
                return;
            }
            let request = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            log::info!(
                "PROCMAPENGINE map-data trace HandleGetBlockIDs request lookup at {:#x}: request={:#x} kind={:#x} state={:#x} refcounter={:#x} sentinel=[{:#x},{:#x}] fastmap_ch={:#x}",
                resolved,
                request,
                read_u32_or_invalid(uc, request),
                read_u32_or_invalid(uc, request + 0x74),
                read_u16_or_invalid(uc, request + 0x200),
                read_u32_or_invalid(uc, request + 0x1f4),
                read_u32_or_invalid(uc, request + 0x1f8),
                read_u8_or_invalid(uc, request + 0x1fc),
            );
        })
        .unwrap();

    // The drop path itself.
    let dropped = base_address + (0x004b_4a98u32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(dropped as u64, dropped as u64, move |uc, _, _| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) >= 60 {
                return;
            }
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let this = read_u32_or_invalid(uc, sp + 0x44);
            log::warn!(
                "PROCMAPENGINE map-data trace HandleGetBlockIDs DROPPED answer at {:#x}: job={:#x} mdm_ref={:#x} answer_code={:#x}",
                dropped,
                this,
                read_u16_or_invalid(uc, this + 0x1e),
                read_u32_or_invalid(uc, this + 0xc),
            );
        })
        .unwrap();

    // Every write to the MDM ref counter (`request + 0x200`, the u16
    // `mdm_sMapDataRequest::u16MDMRefCounter`) of the statically allocated
    // requests. `GetDAPIRequestingMapDataRequest` matches an incoming answer on
    // exactly this field, so a write that clears or changes it between the
    // request and its answer silently orphans the answer (0xffab14 path above).
    for (request, name) in [
        (0x0078_fcd4u32, "AcCam/view"),
        (0x0079_04e4u32, "car cursor"),
        (0x0079_06f4u32, "image block"),
    ] {
        use unicorn_engine::unicorn_const::HookType;
        let field = base_address + (request - ORIGINAL_BASE) + 0x200;
        unicorn
            .add_mem_hook(HookType::MEM_WRITE, field as u64, field as u64 + 1, move |uc, _t, address, size, value| {
                static COUNT: AtomicU32 = AtomicU32::new(0);
                if COUNT.fetch_add(1, Ordering::Relaxed) >= 200 {
                    return true;
                }
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                log::warn!(
                    "PROCMAPENGINE map-data trace refcounter WRITE {} t={}ms thread=[{}] field={:#x} size={} value={:#x} pc={:#x} request_state={:#x} sentinels=[{:#x},{:#x}]",
                    name,
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    address,
                    size,
                    value,
                    pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE),
                    read_u32_or_invalid(uc, (address - 0x200 + 0x74) as u32),
                    read_u32_or_invalid(uc, (address - 0x200 + 0x1f4) as u32),
                    read_u32_or_invalid(uc, (address - 0x200 + 0x1f8) as u32),
                );
                true
            })
            .unwrap();
    }

    // Who resets a live request: `PerformResetMapDataRequest` clears the request
    // state and the ref counter, orphaning the answer that is on its way. Its
    // callers are MDM jobs, `Stop`, `Init`, `OnUpdateDatasetId` and
    // `OnUpdateServiceStateDAPIFastmap` (a DAPI service-state change), so log the
    // caller and the value that triggered it.
    for (off, name) in [
        (0x0050_ea68u32, "PerformResetMapDataRequest"),
        (0x0051_2400u32, "OnUpdateServiceStateDAPIFastmap"),
        (0x0051_2230u32, "OnUpdateDatasetId"),
        (0x0050_f3acu32, "map_tclMapDataManager::Stop"),
    ] {
        let address = base_address + (off - ORIGINAL_BASE);
        let name: &'static str = name;
        unicorn
            .add_code_hook(address as u64, address as u64, move |uc, _, _| {
                static COUNT: AtomicU32 = AtomicU32::new(0);
                if COUNT.fetch_add(1, Ordering::Relaxed) >= 40 {
                    return;
                }
                log::warn!(
                    "PROCMAPENGINE map-data trace {} t={}ms thread=[{}] this={:#x} r1={:#x} lr={:#x}",
                    name,
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                    (uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1,
                );
            })
            .unwrap();
    }

    // The car-position channel: `vHandleExtrapolatedPositions` (client map entry
    // {svc 0x1e, fn 0x13} at 0x713f18) deserializes a
    // `posfi_tclMsgExtrapolatedPositionsStatus` into an RC job, whose `Execute`
    // (0x4d14cc) feeds `map_tclMapCCA::BeginUpdate` - that is what actually moves
    // the CCP the renderer draws and the camera follows. Without it the view stays
    // at the engine's power-on default (-83.0/42.5), which is outside the
    // (`sd2-GENJ_EU_I.A v2`) dataset on the card, so no real block can ever be
    // drawn. Log the deserialization result and the fields the RC job got.
    let extrap_deserialized = base_address + (0x0054_9bf4u32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(extrap_deserialized as u64, extrap_deserialized as u64, move |uc, _, _| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) >= 12 {
                return;
            }
            let result = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let job = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
            // The message object occupies job+0x20..+0x80; `Execute` reads the
            // POSIX time at +0x44/+0x48, a u16 at +0x50 and the position array as
            // [begin, end) at +0x58/+0x5c (88-byte elements).
            let mut words = Vec::with_capacity(24);
            for offset in (0x20..0x80u32).step_by(4) {
                words.push(read_u32_or_invalid(uc, job + offset));
            }
            log::warn!(
                "PROCMAPENGINE map-data trace ExtrapolatedPositions deserialized t={}ms thread=[{}] result={:#x} job={:#x} msg=[{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x}] lr={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                result,
                job,
                words[0],
                words[1],
                words[2],
                words[3],
                words[4],
                words[5],
                words[6],
                words[7],
                words[8],
                words[9],
                words[10],
                words[11],
                words[12],
                words[13],
                words[14],
                words[15],
                words[16],
                words[17],
                words[18],
                words[19],
                (uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1,
            );
            let begin = words[(0x58 - 0x20) / 4];
            let end = words[(0x5c - 0x20) / 4];
            if begin != 0 && end > begin {
                let mut element = Vec::with_capacity(0x58 / 4);
                for offset in (0..0x58u32).step_by(4) {
                    element.push(read_u32_or_invalid(uc, begin + offset));
                }
                log::warn!(
                    "PROCMAPENGINE map-data trace ExtrapolatedPositions first element at {:#x}: {:x?}",
                    begin,
                    element,
                );
            }
        })
        .unwrap();

    // Does the CCP really move? `map_tclJobRCHandleExtrapolatedPositions::Execute`
    // fills the CCA's interpolation array (`BeginUpdate` at 0x54d1b0 hands back the
    // array pointer, one `map_trCCP {x, y, heading}` per position) and then swaps
    // `m_u8CurrentCCAInstance`, which is what makes the renderer see it. Hook right
    // after `EndUpdate` (bl at 0x4d1738, `r7` = the CCA instance that was just
    // filled) and dump the instance plus the array it now publishes.
    let cca_filled = base_address + (0x004d_173cu32 - ORIGINAL_BASE);
    let cca_instance_global = base_address + (0x0078_f668u32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(cca_filled as u64, cca_filled as u64, move |uc, _, _| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) >= 8 {
                return;
            }
            let cca = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
            let start = read_u32_or_invalid(uc, cca);
            let end = read_u32_or_invalid(uc, cca + 4);
            let step = read_u32_or_invalid(uc, cca + 8);
            let array = read_u32_or_invalid(uc, cca + 0x10);
            let count = read_u32_or_invalid(uc, cca + 0x14);
            let mut entries = Vec::new();
            for index in 0..count.min(3) {
                for offset in [0u32, 4, 8] {
                    entries.push(read_u32_or_invalid(uc, array + index * 0xc + offset));
                }
            }
            log::warn!(
                "PROCMAPENGINE map-data trace CCP filled t={}ms thread=[{}] cca={:#x} ts=[{:#x}..{:#x}] step={:#x} count={:#x} array={:#x} entries=[{:x?}] instance={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                cca,
                start,
                end,
                step,
                count,
                array,
                entries,
                read_u8_or_invalid(uc, cca_instance_global),
            );
        })
        .unwrap();

    // `map_tclJobRCContinuousScrolling::Execute` (0x4c9898) - the job that turns
    // on "continuous scrolling", i.e. the map view following the CCP - refuses to
    // run (`0xffab00`) while `map_tclMapEngineControl::ms_eMapEngineState` is 0, 4
    // or 5. r458 proved the CCP itself is fine (it holds Krakow, 0xe29fa46 /
    // 0x23999b77) while the camera (`u32SetCameraParameters`) still asks for the
    // power-on rectangle, so watch every state transition with the writing site.
    {
        use unicorn_engine::unicorn_const::HookType;
        let state = base_address + (0x0079_07acu32 - ORIGINAL_BASE);
        unicorn
            .add_mem_hook(HookType::MEM_WRITE, state as u64, state as u64 + 3, move |uc, _t, address, size, value| {
                static COUNT: AtomicU32 = AtomicU32::new(0);
                if COUNT.fetch_add(1, Ordering::Relaxed) >= 60 {
                    return true;
                }
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE map-data trace MapEngineState WRITE t={}ms thread=[{}] size={} value={:#x} pc={:#x} lr={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    size,
                    value,
                    pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE),
                    (uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32)
                        .wrapping_sub(base_address)
                        .wrapping_add(ORIGINAL_BASE)
                        & !1,
                );
                true
            })
            .unwrap();
    }

    // The HMI controls the view through the engine's *server* side service
    // (`map_tclPortControl_Platform::vHandleRenFIMessages` 0x3a359c, called from
    // `vOnNewMessage`; rendererfi): continuous scrolling, camera, scale,
    // language... Log which method the HMI actually calls, so a missing
    // follow-the-car command is distinguishable from one the engine refused.
    let renfi = base_address + (0x003a_359cu32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(renfi as u64, renfi as u64, move |uc, _, _| {
            static SEEN: std::sync::OnceLock<std::sync::Mutex<Vec<[u32; 3]>>> =
                std::sync::OnceLock::new();
            let message = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            // `amt_tclBaseMessage`'s header lives behind the object's +0x04
            // pointer (cf. `*(char *)(*(int *)(param_2 + 4) + 0x1a)` in
            // `RndFI_HandleContinuousScrolling`).
            let header = read_u32_or_invalid(uc, message + 4);
            let key = [
                read_u16_or_invalid(uc, header + 0x14),
                read_u16_or_invalid(uc, header + 0x18),
                read_u8_or_invalid(uc, header + 0x1a) as u32,
            ];
            let is_new = {
                let mut seen = SEEN.get_or_init(Default::default).lock().unwrap();
                if seen.contains(&key) {
                    false
                } else {
                    if seen.len() < 64 {
                        seen.push(key);
                    }
                    true
                }
            };
            if !is_new {
                return;
            }
            log::info!(
                "PROCMAPENGINE map-data trace RenFI method t={}ms thread=[{}] svc={:#x} fn={:#x} opcode={:#x} message={:#x} lr={:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                key[0],
                key[1],
                key[2],
                message,
                (uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1,
            );
        })
        .unwrap();

    // `map_tclMapEngineControl`'s lifecycle state (`ms_eMapEngineState`, 0x7907ac) is what
    // decides whether a RenFI camera job survives: `RndFI_AddJobToQueue` (0x53ad90) hands
    // the job to the render control only while the state is 1/2/3 and otherwise returns
    // 0xffab60, which makes the caller destroy the job again.  The state is written by
    // exactly four functions - `Init` (0x51e000, -> 1 when it succeeds from 0), `Start`
    // (0x51da24, -> 2), `Stop` (0x51e97c, -> 3) and `Exit` (0x51ef54, -> 4/5) - and the
    // chain into them starts at the port-control platform's own `bOnInit` (0x390370 ->
    // `map_tclPortControl::Init` 0x3a46ac -> 0x53ae30).  Trace entries plus the state as
    // the gate sees it.
    {
        static HITS: AtomicU32 = AtomicU32::new(0);
        let state_at = base_address + (0x0079_07acu32 - ORIGINAL_BASE);
        for (offset, label) in [
            (0x0039_0370u32, "portControl_Platform::bOnInit"),
            (0x003a_46acu32, "map_tclPortControl::Init"),
            (0x0051_e000u32, "MapEngineControl::Init"),
            (0x0051_da24u32, "MapEngineControl::Start"),
            (0x0051_e97cu32, "MapEngineControl::Stop"),
            (0x0051_ef54u32, "MapEngineControl::Exit"),
            (0x0053_ad90u32, "RndFI_AddJobToQueue"),
        ] {
            let addr = base_address + (offset - ORIGINAL_BASE);
            let state = state_at;
            let label: &'static str = label;
            unicorn
                .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                    let hits = HITS.fetch_add(1, Ordering::Relaxed);
                    if hits >= 40 {
                        return;
                    }
                    let state_value = read_u32_or_invalid(uc, state);
                    log::info!(
                        "PROCMAPENGINE map-data trace engine lifecycle {label} t={}ms thread=[{}] state={state_value:#x} r0={:#x} lr={:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                        ((uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1)
                            .wrapping_sub(base_address)
                            .wrapping_add(ORIGINAL_BASE),
                    );
                })
                .unwrap();
        }
    }

    // With the lifecycle state at 2 the RenFI camera job *is* accepted into the render
    // control job queue, so the open question is why nothing ever executes it.  The pump
    // (`RenderControlMainLoop`, 0x3a8b6c) peeks the queue head with `PeekElement`
    // (0x4a52e0), asks the job's vtable slot +0x20 (`map_tclJobRC::IsExecutable`,
    // 0x4c5c50, answering `1 - render_control[0x1ad]` for camera jobs whose `job+0x10`
    // is not 0x8000), then removes it with `GetPeekedElement` (0x4a3d7c) and calls slot
    // +0x24 (`map_tclJobRCSetCameraParameters::Execute`, 0x4d74c4, which calls
    // `u32SetCameraParameters`).  Log the whole add/peek/gate/take/execute chain of the
    // queue (`ms_pkRenderControlJobQueue`, 0x715564) to find the step our job falls out
    // of - a non-executable head-of-line element parks every job behind it.
    {
        let rc_instance_global = base_address + (0x0071_5538u32 - ORIGINAL_BASE);
        let rc_queue_global = base_address + (0x0071_5564u32 - ORIGINAL_BASE);
        let instance_global = rc_instance_global;

        static ADD_LOGS: AtomicU32 = AtomicU32::new(0);
        let add = base_address + (0x003a_79b8u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(add as u64, add as u64, move |uc, _, _| {
                let logs = ADD_LOGS.fetch_add(1, Ordering::Relaxed);
                if logs >= 1500 {
                    return;
                }
                let job = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                // One call site floods the render control queue with a per-frame job;
                // keep the first few of those and spend the rest of the budget on the
                // other jobs, so a late MapObjectsUpdate job cannot be crowded out of
                // the log (r545 filled all 800 lines within 10 s).
                let caller = ((uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1)
                    .wrapping_sub(base_address)
                    .wrapping_add(ORIGINAL_BASE);
                if caller == 0x004c_7270 && logs >= 3 {
                    return;
                }
                let queue = read_u32_or_invalid(uc, rc_queue_global);
                // `map_tclJobQueue::AddElement` (0x4a4284) does not link the element: it
                // posts the element *pointer* into the queue's incoming-job OSAL message
                // queue (`queue + 0x30` -> OSALPointerQueue, whose +0x00 is the OSAL
                // handle and +0x04 the name), and `EnqueueElements` (0x4a4d7c) picks it
                // up again on the next peek.  Report that queue so the post can be
                // followed in the OSAL message-queue trace.
                let pointer_queue = read_u32_or_invalid(uc, queue.wrapping_add(0x30));
                let handle = read_u32_or_invalid(uc, pointer_queue);
                let name_at = read_u32_or_invalid(uc, pointer_queue.wrapping_add(4));
                let name: Vec<u8> = (0..12u32)
                    .map(|i| read_u8_or_invalid(uc, name_at.wrapping_add(i)) as u8)
                    .collect();
                let name = String::from_utf8_lossy(&name)
                    .chars()
                    .take_while(|c| *c != '\0')
                    .collect::<String>();
                let words: Vec<u32> = (0x20u32..0x40)
                    .step_by(4)
                    .map(|i| read_u32_or_invalid(uc, job.wrapping_add(i)))
                    .collect();
                log::info!(
                    "PROCMAPENGINE map-data trace render queue add t={}ms thread=[{}] job={:#x} vptr={:#x} queue={:#x} state={:#x} class={:#x} words=[{}] ijw={:#x} lr={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    job,
                    read_u32_or_invalid(uc, job),
                    queue,
                    read_u32_or_invalid(uc, queue),
                    read_u32_or_invalid(uc, job.wrapping_add(0x10)),
                    words
                        .iter()
                        .map(|w| format!("{w:#x}"))
                        .collect::<Vec<_>>()
                        .join(","),
                    handle,
                    ((uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1)
                        .wrapping_sub(base_address)
                        .wrapping_add(ORIGINAL_BASE),
                );
            })
            .unwrap();

        static POLL_LOGS: AtomicU32 = AtomicU32::new(0);
        static POLL_LAST: AtomicU32 = AtomicU32::new(0xffff_ffff);
        let poll = base_address + (0x004a_52e0u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(poll as u64, poll as u64, move |uc, _, _| {
                let logs = POLL_LOGS.fetch_add(1, Ordering::Relaxed);
                let queue = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let state = read_u32_or_invalid(uc, queue);
                let count = read_u32_or_invalid(uc, queue.wrapping_add(0x1c));
                // Only report a change of (queue, state, depth): the pump polls this in a
                // tight rotation, so a flat log cap hides everything that matters later.
                let signature = queue.rotate_left(4) ^ (state << 12) ^ count;
                if POLL_LAST.swap(signature, Ordering::Relaxed) == signature && logs >= 40 {
                    return;
                }
                log::info!(
                    "PROCMAPENGINE map-data trace render queue poll PeekElement t={}ms thread=[{}] queue={:#x} state={:#x} count={:#x} logs={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    queue,
                    state,
                    count,
                    logs + 1,
                );
            })
            .unwrap();

        static GATE_LOGS: AtomicU32 = AtomicU32::new(0);
        let gate = base_address + (0x004c_5c50u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(gate as u64, gate as u64, move |uc, _, _| {
                let logs = GATE_LOGS.fetch_add(1, Ordering::Relaxed);
                if logs >= 3000 {
                    return;
                }
                let job = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let instance = read_u32_or_invalid(uc, instance_global);
                log::info!(
                    "PROCMAPENGINE map-data trace render job gate IsExecutable t={}ms thread=[{}] job={:#x} vptr={:#x} mode={:#x} viewid={:#x} f1d={:#x} render_control={:#x} flag={:#x} lr={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    job,
                    read_u32_or_invalid(uc, job),
                    read_u32_or_invalid(uc, job.wrapping_add(0x10)),
                    read_u8_or_invalid(uc, job.wrapping_add(0x1c)),
                    read_u8_or_invalid(uc, job.wrapping_add(0x1d)),
                    instance,
                    if instance == 0 || instance == 0xffff_ffff {
                        0xffff_ffff
                    } else {
                        read_u8_or_invalid(uc, instance.wrapping_add(0x1ad))
                    },
                    ((uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1)
                        .wrapping_sub(base_address)
                        .wrapping_add(ORIGINAL_BASE),
                );
            })
            .unwrap();

        static TAKE_LOGS: AtomicU32 = AtomicU32::new(0);
        let take = base_address + (0x004a_3d7cu32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(take as u64, take as u64, move |uc, _, _| {
                let logs = TAKE_LOGS.fetch_add(1, Ordering::Relaxed);
                if logs >= 3000 {
                    return;
                }
                let element = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                // `GetPeekedElement` (0x4a3d7c) hands the element back only while the
                // queue's active list (`queue + 0x08`) is non-empty, otherwise the caller
                // is given a null element and the job is dropped without executing.
                let queue = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE map-data trace render queue take GetPeekedElement t={}ms thread=[{}] queue={:#x} element={:#x} vptr={:#x} state={:#x} count={:#x} active={:#x} class_list={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    queue,
                    element,
                    read_u32_or_invalid(uc, element),
                    read_u32_or_invalid(uc, queue),
                    read_u32_or_invalid(uc, queue.wrapping_add(4)),
                    read_u32_or_invalid(uc, queue.wrapping_add(8)),
                    read_u32_or_invalid(uc, queue.wrapping_add(0x10)),
                );
            })
            .unwrap();

        static REMOVED_LOGS: AtomicU32 = AtomicU32::new(0);
        let removed = base_address + (0x004a_3be4u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(removed as u64, removed as u64, move |uc, _, _| {
                let element = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let vptr = read_u32_or_invalid(uc, element);
                let logs = REMOVED_LOGS.fetch_add(1, Ordering::Relaxed);
                // `map_tclJobRCSetCameraParameters`' vtable; other job classes are only
                // reported a handful of times so the trace stays readable.
                if vptr != 0x006a_cb18 && logs >= 20 {
                    return;
                }
                // `RemoveElementFromIJQ` is the last statement of the success branch of
                // `GetPeekedElement`, so reaching it proves the element was handed back to
                // the main loop (which then dispatches the job's vtable slot +0x24).
                log::info!(
                    "PROCMAPENGINE map-data trace render queue taken RemoveElementFromIJQ t={}ms thread=[{}] queue={:#x} element={:#x} vptr={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                    element,
                    vptr,
                );
            })
            .unwrap();

        // 0x3a8df8 is the instruction right after `GetPeekedElement` returns in
        // `RenderControlMainLoop` and 0x3a8e00 the one that loads the vtable before
        // dispatching slot +0x24; both sit mid-TB, so they are watched with a page-range
        // hook dispatching on the fetched address (see docs/issues/design.md).
        {
            let page = base_address + (0x003a_8000u32 - ORIGINAL_BASE);
            unicorn
                .add_code_hook(page as u64, (page + 0xfff) as u64, move |uc, _, _| {
                    static DISPATCH_LOGS: AtomicU32 = AtomicU32::new(0);
                    let offset = (uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32)
                        .wrapping_sub(base_address)
                        .wrapping_add(ORIGINAL_BASE);
                    if offset != 0x003a_8df8 && offset != 0x003a_8e00 {
                        return;
                    }
                    let element = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let (vptr, execute) = if element == 0 {
                        (0, 0)
                    } else {
                        let vptr = read_u32_or_invalid(uc, element);
                        (vptr, read_u32_or_invalid(uc, vptr + 0x24))
                    };
                    let logs = DISPATCH_LOGS.fetch_add(1, Ordering::Relaxed);
                    if vptr != 0x006a_cb18 && logs >= 20 {
                        return;
                    }
                    log::info!(
                        "PROCMAPENGINE map-data trace render loop dispatch {offset:#x} t={}ms thread=[{}] element={:#x} vptr={:#x} execute={:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        element,
                        vptr,
                        execute,
                    );
                })
                .unwrap();
        }

        // `map_tclRenderControl::SetCameraParameters` has exactly two exits: 0x3a75f4
        // branches to `map_tclMapView::u32SetCameraParameters`, 0x3a7604 answers 0xffab48.
        // Both are branch targets, so they start their own translation block and a narrow
        // hook on them is dispatched (unlike a mid-block probe, see docs/issues/design.md).
        for (offset, label) in [
            (0x003a_75d4u32 - ORIGINAL_BASE, "view-check"),
            (0x003a_7604u32 - ORIGINAL_BASE, "rejected"),
        ] {
            static EXIT_LOGS: AtomicU32 = AtomicU32::new(0);
            let site = base_address + offset;
            unicorn
                .add_code_hook(site as u64, site as u64, move |uc, _, _| {
                    let logs = EXIT_LOGS.fetch_add(1, Ordering::Relaxed);
                    if logs >= 400 {
                        return;
                    }
                    let view_id = (uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32) & 0xff;
                    log::info!(
                        "PROCMAPENGINE map-data trace render control SetCameraParameters {label} t={}ms thread=[{}] view_id={:#x} view={:#x} lon={:#x} lat={:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        view_id,
                        read_u32_or_invalid(
                            uc,
                            base_address + (0x0071_5550u32 - ORIGINAL_BASE) + view_id * 4
                        ),
                        read_u32_or_invalid(
                            uc,
                            (uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32).wrapping_add(0x18),
                        ),
                        read_u32_or_invalid(
                            uc,
                            (uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32).wrapping_add(0x1c),
                        ),
                    );
                })
                .unwrap();
        }

        // The engine's DAPI `GetBlockIDs` requests stop being sent after the second one:
        // `ail_tclAppInterfaceRestricted::enPostMessage` (0x661a3c) answers 7 whenever
        // `ail_enCheckClientMailValidity` (0x66dd00) refuses the message, i.e. when the
        // client handler's service-reference list has no entry whose server app id and
        // register id match the message header (16), or when the entry's service state
        // does not allow the message opcode (2 for state 1 with an opcode that is neither
        // 4 nor 0xd, 17 for state 2).  Log header, list and verdict.
        {
            let site = base_address + (0x0066_dd00u32 - ORIGINAL_BASE);
            unicorn
                .add_code_hook(site as u64, site as u64, move |uc, _, _| {
                    static ENTRY_LOGS: AtomicU32 = AtomicU32::new(0);
                    let logs = ENTRY_LOGS.fetch_add(1, Ordering::Relaxed);
                    if logs >= 300 {
                        return;
                    }
                    let msg = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let list = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                    // `r3` is `ail_tclAppInterfaceRestricted + 0x50`, the client
                    // service-reference list this check walks.
                    ensure_dapi_map_service_reference(uc, list.wrapping_sub(0x50));
                    let header = read_u32_or_invalid(uc, msg.wrapping_add(4));
                    let src = read_u16_or_invalid(uc, header.wrapping_add(0x0));
                    let dst = read_u16_or_invalid(uc, header.wrapping_add(0x02));
                    let sub = read_u16_or_invalid(uc, header.wrapping_add(0x0c));
                    let service = read_u16_or_invalid(uc, header.wrapping_add(0x14));
                    let register_id = read_u16_or_invalid(uc, header.wrapping_add(0x16));
                    let function = read_u16_or_invalid(uc, header.wrapping_add(0x18));
                    let opcode = read_u8_or_invalid(uc, header.wrapping_add(0x1a));
                    // `list + 4` is the first node, `node + 4` the next one and the
                    // `ail_tclServiceReference` object starts at `node + 8`, where byte
                    // +0x06 is the service state.
                    let mut references = Vec::new();
                    let mut node = read_u32_or_invalid(uc, list.wrapping_add(4));
                    for _ in 0..8 {
                        if node == 0 || node == 0xffff_ffff {
                            break;
                        }
                        let reference = node.wrapping_add(8);
                        references.push(format!(
                            "state={:#x} words={}",
                            read_u8_or_invalid(uc, reference.wrapping_add(6)),
                            (0..7u32)
                                .map(|i| format!("{:#x}", read_u32_or_invalid(uc, reference.wrapping_add(i * 4))))
                                .collect::<Vec<_>>()
                                .join(","),
                        ));
                        node = read_u32_or_invalid(uc, node.wrapping_add(4));
                    }
                    log::info!(
                        "PROCMAPENGINE map-data trace client mail check t={}ms thread=[{}] app={:#x} msg={:#x} header={:#x} src={:#x} dst={:#x} sub={:#x} svc={:#x} reg={:#x} fn={:#x} opcode={:#x} list={:#x} refs=[{}] lr={:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        (uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32) & 0xffff,
                        msg,
                        header,
                        src,
                        dst,
                        sub,
                        service,
                        register_id,
                        function,
                        opcode,
                        list,
                        references.join(" | "),
                        ((uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1)
                            .wrapping_sub(base_address)
                            .wrapping_add(ORIGINAL_BASE),
                    );
                    // The unsolicited `fn=0x103` map-service requests (seen ~0.6 s
                    // after every `GetBlockIDs` answer) have no Ghidra-side handler
                    // name yet; dump their serialized body (block ids live past the
                    // 0x1c-byte header) so the method can be identified from its
                    // content.
                    if service == 0x26 && opcode == 2 && function != 2 {
                        static BODY_LOGS: AtomicU32 = AtomicU32::new(0);
                        if BODY_LOGS.fetch_add(1, Ordering::Relaxed) < 40 {
                            let mut bytes = [0u8; 128];
                            let body = match uc.mem_read((header + 0x1c) as u64, &mut bytes) {
                                Ok(()) => bytes
                                    .iter()
                                    .map(|b| format!("{b:02x}"))
                                    .collect::<Vec<_>>()
                                    .join(" "),
                                Err(_) => "<unreadable>".to_string(),
                            };
                            log::info!(
                                "PROCMAPENGINE map-data trace fn={:#x} request body at {:#x}: [{body}]",
                                function,
                                header + 0x1c,
                            );
                        }
                    }
                })
                .unwrap();
        }
        for (offset, label) in [
            (0x0066_de18u32 - ORIGINAL_BASE, "client mail check verdict"),
            (0x0066_de80u32 - ORIGINAL_BASE, "client mail check no-reference"),
            (0x0066_df04u32 - ORIGINAL_BASE, "client mail check state1"),
        ] {
            static VERDICT_LOGS: AtomicU32 = AtomicU32::new(0);
            let site = base_address + offset;
            unicorn
                .add_code_hook(site as u64, site as u64, move |uc, _, _| {
                    let logs = VERDICT_LOGS.fetch_add(1, Ordering::Relaxed);
                    if logs >= 300 {
                        return;
                    }
                    let stack = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE map-data trace {} t={}ms thread=[{}] code={:#x} state={:#x} opcode={:#x} reg={:#x}",
                        label,
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        (uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32) & 0xffff,
                        read_u32_or_invalid(uc, stack.wrapping_add(0x14)),
                        read_u32_or_invalid(uc, stack.wrapping_add(0x10)),
                        read_u32_or_invalid(uc, stack.wrapping_add(0xc)),
                    );
                })
                .unwrap();
        }

        // `ail_enCheckClientMailValidity` only lets a request (opcode 2) through while the
        // `ail_tclServiceReference` state is neither 1 nor 2, and the constructor leaves a
        // freshly created reference at 1 (`ail_tclServiceReference` 0x66e2ec).  State 0 is
        // reached through `vSetServiceState`, whose only caller is the administration
        // dispatch that turns `amt_tclServiceStatus` / `amt_tclServiceUnregister` messages
        // into state changes.  Log the references, who creates them and who changes their
        // state, so the "second GetBlockIDs is refused with 7" window has an owner.
        for (offset, label) in [
            (0x0066_e43cu32 - ORIGINAL_BASE, "service reference state set"),
            (0x0066_e2ecu32 - ORIGINAL_BASE, "service reference created"),
            (0x0066_82bcu32 - ORIGINAL_BASE, "MsgServiceRegisterConf"),
            (0x0066_963cu32 - ORIGINAL_BASE, "ExamineInfluenceToAdministration"),
        ] {
            static STATE_LOGS: AtomicU32 = AtomicU32::new(0);
            let site = base_address + offset;
            unicorn
                .add_code_hook(site as u64, site as u64, move |uc, _, _| {
                    let logs = STATE_LOGS.fetch_add(1, Ordering::Relaxed);
                    if logs >= 600 {
                        return;
                    }
                    let reference = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let message = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    // For the two `ail_tclServiceReference` sites r0 is the reference
                    // (+0x00 service, +0x02 server app, +0x04 register id, +0x06 state,
                    // +0x08 sub id); for the message sites r1 is the message whose header
                    // pointer sits at +0x04.
                    let admin = (uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32) & 0xff;
                    let state = if offset == 0x0066_e43c - ORIGINAL_BASE {
                        admin
                    } else {
                        read_u8_or_invalid(uc, reference.wrapping_add(6))
                    };
                    // `MsgServiceRegisterConf(this, msg)` and `vSetServiceState(this, value)`
                    // take their second argument in r1, `ExamineInfluenceToAdministration`
                    // takes the admin type in r1 and the message in r2.
                    let (message, admin) = if offset == 0x0066_963c - ORIGINAL_BASE {
                        (
                            uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32,
                            admin,
                        )
                    } else if offset == 0x0066_82bc - ORIGINAL_BASE {
                        (message, 0xff)
                    } else {
                        (0, admin)
                    };
                    let is_status = offset == 0x0066_963c - ORIGINAL_BASE && admin == 2;
                    let is_conf = offset == 0x0066_82bc - ORIGINAL_BASE;
                    let budget_ok = if is_conf {
                        CONF_TRACE_ARMS.fetch_add(1, Ordering::Relaxed) < 2
                    } else {
                        EXAMINE_TRACE_ARMS.fetch_add(1, Ordering::Relaxed) < 2
                    };
                    if (is_status && budget_ok) || (is_conf && budget_ok) {
                        EXAMINE_TRACE_TID.store(uc.get_data().inner.thread_id(), Ordering::Relaxed);
                        EXAMINE_TRACE_STEPS.store(500, Ordering::Relaxed);
                        if is_conf {
                            // Dump the sync-answer list (`*this + 0x5c`) that the
                            // conf handler gates the reference append on.
                            let thisp = read_u32_or_invalid(uc, reference);
                            let sync = read_u32_or_invalid(uc, thisp.wrapping_add(0x5c));
                            let refs = read_u32_or_invalid(uc, thisp.wrapping_add(0x50));
                            let refs_head = read_u32_or_invalid(uc, refs.wrapping_add(4));
                            log::info!(
                                "PROCMAPENGINE conf-entry t={}ms thread=[{}] app_iface={:#x} sync_list+0x5c={:#x} head+4={:#x} refs+0x50={:#x} head+4={:#x}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                uc.get_data().inner.thread_id(),
                                thisp,
                                sync,
                                read_u32_or_invalid(uc, sync.wrapping_add(4)),
                                read_u32_or_invalid(uc, thisp.wrapping_add(0x50)),
                                refs_head,
                            );
                        }
                    }
                    let header = read_u32_or_invalid(uc, message.wrapping_add(4));
                    log::info!(
                        "PROCMAPENGINE map-data trace {} t={}ms thread=[{}] ref={:#x} ref_words={:#x?} state={:#x} admin={:#x} header={:#x} src={:#x} dst={:#x} class={:#x} svc={:#x} reg={:#x} opcode={:#x} lr={:#x}",
                        label,
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        reference,
                        (0..9u32)
                            .map(|i| format!("{:#x}", read_u32_or_invalid(uc, reference.wrapping_add(i * 4))))
                            .collect::<Vec<_>>()
                            .join(","),
                        state,
                        admin,
                        header,
                        read_u16_or_invalid(uc, header),
                        read_u16_or_invalid(uc, header.wrapping_add(2)),
                        read_u16_or_invalid(uc, header.wrapping_add(0x0c)),
                        read_u16_or_invalid(uc, header.wrapping_add(0x14)),
                        read_u16_or_invalid(uc, header.wrapping_add(0x16)),
                        read_u8_or_invalid(uc, header.wrapping_add(0x1a)),
                        ((uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1)
                            .wrapping_sub(base_address)
                            .wrapping_add(ORIGINAL_BASE),
                    );
                })
                .unwrap();
        }

        // The narrow hooks on `ail_tclServiceReference`'s constructor (0x66e2ec, leaves the
        // service state at 1) and on `vSetServiceState` (0x66e43c) never fired although the
        // reference the client list holds for the DAPI map service (0x26) demonstrably
        // changed, so they are watched through their page instead (docs/issues/design.md).
        // `vReset` (0x66e4b4) and the setter at 0x66e504 are watched as well: they are the
        // other places that rewrite a reference's service / register / sub id.
        {
            let page = base_address + (0x0066_e000u32 - ORIGINAL_BASE);
            unicorn
                .add_code_hook(page as u64, (page + 0xfff) as u64, move |uc, _, _| {
                    static REF_LOGS: AtomicU32 = AtomicU32::new(0);
                    let offset = (uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32)
                        .wrapping_sub(base_address)
                        .wrapping_add(ORIGINAL_BASE);
                    let label = match offset {
                        0x0066_e2ec => "service reference created",
                        0x0066_e43c => "service reference state set",
                        0x0066_e4b4 => "service reference reset",
                        0x0066_e504 => "service reference ids set",
                        // liveness markers: both are called by the client mail check above,
                        // so a zero count here means the page hook itself never dispatches
                        0x0066_e444 => "service reference state read (liveness)",
                        0x0066_e3d0 => "service reference match test (liveness)",
                        _ => return,
                    };
                    let logs = REF_LOGS.fetch_add(1, Ordering::Relaxed);
                    if logs >= 400 {
                        return;
                    }
                    let reference = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE map-data trace {} t={}ms thread=[{}] ref={:#x} words={} r1={:#x} r2={:#x} r3={:#x} lr={:#x}",
                        label,
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        reference,
                        (0..9u32)
                            .map(|i| format!(
                                "{:#x}",
                                read_u32_or_invalid(uc, reference.wrapping_add(i * 4))
                            ))
                            .collect::<Vec<_>>()
                            .join(","),
                        (uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32) & 0xffff,
                        (uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32) & 0xffff,
                        (uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32) & 0xffff,
                        ((uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1)
                            .wrapping_sub(base_address)
                            .wrapping_add(ORIGINAL_BASE),
                    );
                })
                .unwrap();
        }

        static EXEC_LOGS: AtomicU32 = AtomicU32::new(0);
        let exec = base_address + (0x004d_74c4u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(exec as u64, exec as u64, move |uc, _, _| {
                let logs = EXEC_LOGS.fetch_add(1, Ordering::Relaxed);
                if logs >= 200 {
                    return;
                }
                let job = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                // `map_tclJobRCSetCameraParameters` keeps the camera parameter block it
                // hands to `u32SetCameraParameters` at `job + 0x20`, longitude at +0x18
                // and latitude at +0x1c of it.
                log::info!(
                    "PROCMAPENGINE map-data trace render job executed RCSetCameraParameters::Execute t={}ms thread=[{}] job={:#x} mode={:#x} state={:#x} lon={:#x} lat={:#x} scale={:#x} logs={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    job,
                    read_u32_or_invalid(uc, job.wrapping_add(0x10)),
                    read_u32_or_invalid(uc, base_address + (0x0079_07acu32 - ORIGINAL_BASE)),
                    read_u32_or_invalid(uc, job.wrapping_add(0x38)),
                    read_u32_or_invalid(uc, job.wrapping_add(0x3c)),
                    read_u32_or_invalid(uc, job.wrapping_add(0x24)),
                    logs + 1,
                );
            })
            .unwrap();

        static SET_LOGS: AtomicU32 = AtomicU32::new(0);
        let set = base_address + (0x003a_7594u32 - ORIGINAL_BASE);
        let views_global = base_address + (0x0071_5550u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(set as u64, set as u64, move |uc, _, _| {
                let logs = SET_LOGS.fetch_add(1, Ordering::Relaxed);
                if logs >= 600 {
                    return;
                }
                let view_id = (uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32) & 0xff;
                let params = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                // `map_tclRenderControl::SetCameraParameters` only forwards to
                // `map_tclMapView::u32SetCameraParameters` for view ids 1..4 whose slot
                // in `ms_apkOpenViews` is non-null; anything else answers 0xffab48, which
                // is why a RenFI request carrying view id 0 silently does nothing.
                let words: Vec<u32> =
                    (0..8u32).map(|i| read_u32_or_invalid(uc, params + i * 4)).collect();
                let views: Vec<u32> =
                    (0..6u32).map(|i| read_u32_or_invalid(uc, views_global + i * 4)).collect();
                let autozoom = read_u32_or_invalid(
                    uc,
                    base_address + (0x0071_5548u32 - ORIGINAL_BASE),
                );
                let (az_flags, az_words) = if autozoom == 0 || autozoom == 0xffff_ffff {
                    (0xffff_ffff, Vec::new())
                } else {
                    let flags = read_u32_or_invalid(uc, autozoom + 8);
                    let words = (0xcu32..0x2c)
                        .step_by(4)
                        .map(|i| read_u32_or_invalid(uc, autozoom + i))
                        .collect();
                    (flags, words)
                };
                log::info!(
                    "PROCMAPENGINE map-data trace render control SetCameraParameters t={}ms thread=[{}] view_id={:#x} params={:#x} words=[{}] autozoom={:#x} az_flags={:#x} az_words={} open_views={} lr={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    view_id,
                    params,
                    words
                        .iter()
                        .map(|w| format!("{w:#x}"))
                        .collect::<Vec<_>>()
                        .join(","),
                    autozoom,
                    az_flags,
                    az_words
                        .iter()
                        .map(|w| format!("{w:#x}"))
                        .collect::<Vec<_>>()
                        .join(","),
                    views
                        .iter()
                        .map(|w| format!("{w:#x}"))
                        .collect::<Vec<_>>()
                        .join(","),
                    ((uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1)
                        .wrapping_sub(base_address)
                        .wrapping_add(ORIGINAL_BASE),
                );
            })
            .unwrap();
    }

    // recognised SetCameraParameters method (0x5a) is dropped: 0x3a3244 is right after
    // `fi_tclVisitorMessage::s32GetData` (r0 == 0 means the payload deserialised, which
    // is the branch that calls `CreateRCJobSetCameraParametersSimple` at 0x4c5a84) and
    // 0x3a3338 is that accepted-payload branch.  0x3a3244 sits mid-TB, so this is hooked
    // as a page range dispatching on the fetched address (see docs/issues/design.md).
    {
        let page = base_address + (0x003a_3000u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(page as u64, (page + 0xfff) as u64, move |uc, _, _| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                let offset = (uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32)
                    .wrapping_sub(base_address)
                    .wrapping_add(ORIGINAL_BASE);
                // 0x3a344c is right after the `bl map_tclPortControl::RndFI_AddJobToQueue`
                // (0x53ad90): the queue call returns 0 when the element really went into
                // `ms_pkRenderControlJobQueue` and an error code otherwise, which sends the
                // handler to `DestroyRCJob` at 0x3a34d4 - so log both outcomes.
                if offset == 0x003a_344c || offset == 0x003a_34d4 {
                    static RESULT_LOGS: AtomicU32 = AtomicU32::new(0);
                    let logs = RESULT_LOGS.fetch_add(1, Ordering::Relaxed);
                    if logs >= 40 {
                        return;
                    }
                    log::info!(
                        "PROCMAPENGINE map-data trace RenFI camera add-result {offset:#x} t={}ms thread=[{}] job={:#x} add_result={:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        uc.reg_read(RegisterARM::R8).unwrap_or(0) as u32,
                        uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                    );
                    return;
                }
                if offset != 0x003a_3244 && offset != 0x003a_3338 {
                    return;
                }
                let hits = HITS.fetch_add(1, Ordering::Relaxed);
                if hits >= 24 {
                    return;
                }
                let stack = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                // `fi_tclVisitorMessage` lives at sp + 0xa4: +0x04 is the message
                // content, +0x18 the offset the payload starts at and +0x1c its
                // length; the deserialised field objects occupy sp + 0x30 .. 0xa0
                // (each slot that holds a vtable pointer identifies its field type).
                let visitor = stack + 0xa4;
                let content = read_u32_or_invalid(uc, visitor + 4);
                let payload = read_u32_or_invalid(uc, visitor + 0x18);
                let length = read_u32_or_invalid(uc, visitor + 0x1c);
                let fields: Vec<u32> = (0x30u32..0xa0)
                    .step_by(4)
                    .map(|i| read_u32_or_invalid(uc, stack + i))
                    .collect();
                let bytes: Vec<u32> = (0..14u32)
                    .map(|i| read_u32_or_invalid(uc, content.wrapping_add(payload).wrapping_add(i * 4)))
                    .collect();
                log::info!(
                    "PROCMAPENGINE map-data trace RenFI camera step {offset:#x} t={}ms thread=[{}] r0={:#x} r1={:#x} r2={:#x} r3={:#x} r9={:#x} content={:#x} payload={:#x} len={:#x}\n  frame: {fields:#x?}\n  wire : {bytes:#x?}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R9).unwrap_or(0) as u32,
                    content,
                    payload,
                    length,
                );
            })
            .unwrap();
        let simple = base_address + (0x004c_5a84u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(simple as u64, simple as u64, move |uc, _, _| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                let hits = HITS.fetch_add(1, Ordering::Relaxed);
                if hits >= 8 {
                    return;
                }
                let stack = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE map-data trace RenFI camera job created t={}ms thread=[{}] r0={:#x} r1={:#x} r2={:#x} r3={:#x} stack[0..0x28]={:x?}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32,
                    (0..10u32).map(|i| read_u32_or_invalid(uc, stack + i * 4)).collect::<Vec<_>>(),
                );
            })
            .unwrap();
    }

    // The engine's debug channel commands (`vChannelMgr` 0x3a4888, a switch over a
    // command record) are the other way to reach `TriggerPixmapViewRender`
    // (case 0xb), and its case 9 toggles the Navi stress-test ladder; both would
    // explain the camera being re-aimed from inside the map-data-manager loop.
    {
        let addr = base_address + 0x003a_4888u32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                let hits = HITS.fetch_add(1, Ordering::Relaxed);
                if hits >= 12 {
                    return;
                }
                let record = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let words: Vec<u32> =
                    (0..6u32).map(|i| read_u32_or_invalid(uc, record + i * 4)).collect();
                log::info!(
                    "PROCMAPENGINE map-data trace vChannelMgr t={}ms thread=[{}] record={:#x} words=[{}] lr={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    record,
                    words
                        .iter()
                        .enumerate()
                        .map(|(i, word)| format!("{:#x}:{:#010x}", i * 4, word))
                        .collect::<Vec<_>>()
                        .join(" "),
                    uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                );
            })
            .unwrap();
    }

    // Only four places call `MapEngine_TriggerPixmapViewRender`: the debug channel
    // (hooked above, never fires) and the three `MapEngine_Test*` helpers plus the
    // Navi handler's extrapolated-positions path, so watching all of them shows
    // which one re-aims the camera at the engine's built-in default position.
    for offset in [0x0053_b7c0u32, 0x0053_b984, 0x0053_bd68, 0x0054_98f8] {
        let addr = base_address + offset - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                // One counter per address: a shared static would let the
                // once-per-second posfi handler exhaust the budget and hide the
                // others.
                static HITS: [AtomicU32; 4] = [
                    AtomicU32::new(0),
                    AtomicU32::new(0),
                    AtomicU32::new(0),
                    AtomicU32::new(0),
                ];
                let index = match offset {
                    0x0053_b984 => 1,
                    0x0053_bd68 => 2,
                    0x0054_98f8 => 3,
                    _ => 0,
                };
                let hits = HITS[index].fetch_add(1, Ordering::Relaxed);
                if hits >= 10 {
                    return;
                }
                log::info!(
                    "PROCMAPENGINE map-data trace camera-reset candidate {:#x} t={}ms thread=[{}] arg={} lr={:#x}",
                    offset,
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32 & 0xff,
                    uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                );
            })
            .unwrap();
    }

    // Who triggers the pixmap-view render: it is the only remaining caller that
    // re-points the camera at the engine's hard-coded power-on position (the
    // coordinate pair lives in its literal pool at 0x53b7a8), and it is reached
    // either from the debug channel (`vChannelMgr` case 0xb) or from the Navi
    // handler's PC stress-test ladder, so the link register separates them.
    {
        let addr = base_address + 0x0053_b36cu32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                let hits = HITS.fetch_add(1, Ordering::Relaxed);
                if hits >= 400 {
                    return;
                }
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                if hits >= 4 {
                    // Beyond the first few, only the cadence matters.
                    log::info!(
                        "PROCMAPENGINE map-data trace TriggerPixmapViewRender t={}ms thread=[{}] hits={} view_id={} lr={:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        hits,
                        uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32 & 0xff,
                        lr,
                    );
                    return;
                }
                // The link register points outside the module here, so walk the
                // stack for return addresses that do live in the engine's text and
                // print them as the caller chain.
                let chain: Vec<u32> = (0..64u32)
                    .map(|i| read_u32_or_invalid(uc, sp + i * 4))
                    .filter(|word| word.wrapping_sub(base_address) < 0x00a0_0000)
                    .collect();
                // The link register points into the heap, so show what is actually
                // at that address: real generated code, or one of our own buffers
                // that the guest was tricked into executing.
                let around: Vec<u32> = (0..8u32)
                    .map(|i| read_u32_or_invalid(uc, lr.wrapping_sub(0x10) + i * 4))
                    .collect();
                log::info!(
                    "PROCMAPENGINE map-data trace TriggerPixmapViewRender t={}ms thread=[{}] view_id={} lr={:#x} sp={:#x} around=[{}] chain=[{}]",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32 & 0xff,
                    lr,
                    sp,
                    around
                        .iter()
                        .enumerate()
                        .map(|(i, word)| format!("{:#x}:{:#010x}", i as i32 * 4 - 0x10, word))
                        .collect::<Vec<_>>()
                        .join(" "),
                    chain
                        .iter()
                        .map(|word| format!("{:#x}", word))
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            })
            .unwrap();
    }

    // Whoever creates a camera job decides the view's position: the only callers
    // of `CreateRCJobSetCameraParameters` 0x4c5ac0 are `MDBC_CreateDefaultView`,
    // `MDBC_EarthJumpTo`, `vTriggerAutozoom` and the debug/stress-test helpers, so
    // logging the link register plus the parameters identifies both the creator and
    // the coordinate source behind the power-on camera that never moves.
    {
        let addr = base_address + 0x004c_5ac0u32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                static LRS: AtomicU32 = AtomicU32::new(0);
                let hits = HITS.fetch_add(1, Ordering::Relaxed);
                let lr = uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
                if hits >= 24 || LRS.swap(lr, Ordering::Relaxed) == lr {
                    return;
                }
                let params = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let words: Vec<u32> =
                    (0..8u32).map(|i| read_u32_or_invalid(uc, params + i * 4)).collect();
                log::info!(
                    "PROCMAPENGINE map-data trace CreateRCJobSetCameraParameters t={}ms thread=[{}] params={:#x} words=[{}] lr={:#x}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    params,
                    words
                        .iter()
                        .enumerate()
                        .map(|(i, word)| format!("{:#x}:{:#010x}", i * 4, word))
                        .collect::<Vec<_>>()
                        .join(" "),
                    lr,

                );
            })
            .unwrap();
    }

    // Steps between "AIL accepted the message" and "the RenFI method ran", so a
    // rejected camera request can be pinned to one of them: `vOnNewMessage` of the
    // port-control handler (claim vtable slot +0x20, 0x3a4048), the client/server
    // test it uses (`bIsClientMessage` 0x386c08) and the method's own entry
    // (`u32SetCameraParameters` 0x523eac).
    for (offset, label) in [
        (0x003a_4048u32 - ORIGINAL_BASE, "portControl::vOnNewMessage"),
        (0x0038_6c08u32 - ORIGINAL_BASE, "bIsClientMessage"),
        (0x0052_3eacu32 - ORIGINAL_BASE, "u32SetCameraParameters"),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                static HITS: [AtomicU32; 3] = [
                    AtomicU32::new(0),
                    AtomicU32::new(0),
                    AtomicU32::new(0),
                ];
                let index = if offset == 0x0038_6c08 - ORIGINAL_BASE {
                    1
                } else if offset == 0x0052_3eac - ORIGINAL_BASE {
                    2
                } else {
                    0
                };
                let hits = HITS[index].fetch_add(1, Ordering::Relaxed);
                // `vOnNewMessage` runs for every incoming service-data message, so a
                // small cap let the routine posfi/dapi traffic hide the one message
                // that matters here: count all of them and dump only the unusual ones.
                if hits >= if index == 0 { 6000 } else { 4000 } {
                    return;
                }
                if index == 0 {
                    let message = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let bases = [
                        ("via+0x00", read_u32_or_invalid(uc, message)),
                        ("via+0x04", read_u32_or_invalid(uc, message + 4)),
                        ("via+0x20", read_u32_or_invalid(uc, message + 0x20)),
                    ];
                    let interesting = bases.iter().any(|(_, base)| {
                        (0..20u32)
                            .any(|i| read_u32_or_invalid(uc, base + i * 4) & 0xffff == 0x300)
                    });
                    if !interesting {
                        if hits % 400 == 0 {
                            log::info!(
                                "PROCMAPENGINE map-data trace portControl::vOnNewMessage t={}ms thread=[{}] count={} (no 0x300 seen so far)",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                uc.get_data().inner.thread_id(),
                                hits,
                            );
                        }
                        return;
                    }
                    for (label, base) in bases {
                        let words: Vec<u32> =
                            (0..20u32).map(|i| read_u32_or_invalid(uc, base + i * 4)).collect();
                        if !words.iter().any(|word| *word & 0xffff == 0x300) {
                            continue;
                        }
                        let dumps = RENFI_HEADER_DUMPS.fetch_add(1, Ordering::Relaxed);
                        if dumps >= 8 {
                            continue;
                        }
                        log::info!(
                            "PROCMAPENGINE map-data trace RenFI buffer t={}ms thread=[{}] hits={} wrapper={:#x} {label}={:#x} words=[{}]",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            uc.get_data().inner.thread_id(),
                            hits,
                            message,
                            base,
                            words
                                .iter()
                                .enumerate()
                                .map(|(i, word)| format!("{:#x}:{:#010x}", i * 4, word))
                                .collect::<Vec<_>>()
                                .join(" "),
                        );
                    }
                    return;
                }
                if index == 2 {
                    // u32SetCameraParameters(camera_object, parameters): dump what
                    // the engine actually feeds the camera and what the two
                    // map-data request rectangles hold around the call, so "the
                    // command never arrived" is distinguishable from "the camera
                    // was set but the view rectangle refused to move".
                    let params = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    // r0 is `&view->u32SetCameraParameters`, i.e. the map view
                    // object + 8; the view's +0x10 holds the FIApplication
                    // instance the RenFI server compares payload+0x04 against
                    // (`getRenfIMapViewId` 0x3947cc), so this is where a
                    // fabricated camera request has to take its instance from.
                    let view = uc
                        .reg_read(RegisterARM::R0)
                        .unwrap_or(0)
                        .wrapping_sub(8) as u32;
                    let instance = read_u32_or_invalid(uc, view + 0x10);
                    if instance != 0 && instance != 0xffff_ffff {
                        RENFI_APPLICATION_INSTANCE.store(instance, Ordering::Relaxed);
                    }
                    let words: Vec<u32> =
                        (0..12u32).map(|i| read_u32_or_invalid(uc, params + i * 4)).collect();
                    let view_rect = [
                        read_u32_or_invalid(uc, base_address + (0x0078_fcd4u32 - ORIGINAL_BASE)),
                        read_u32_or_invalid(uc, base_address + (0x0078_fcd8u32 - ORIGINAL_BASE)),
                        read_u32_or_invalid(uc, base_address + (0x0078_fcdcu32 - ORIGINAL_BASE)),
                        read_u32_or_invalid(uc, base_address + (0x0078_fce0u32 - ORIGINAL_BASE)),
                    ];
                    let signature = words
                        .iter()
                        .fold(0u32, |acc, word| acc ^ word.rotate_left(7))
                        ^ view_rect[0];
                    static CALLS: AtomicU32 = AtomicU32::new(0);
                    let calls = CALLS.fetch_add(1, Ordering::Relaxed);
                    if calls >= 500 {
                        return;
                    }
                    let _ = signature;
                    log::info!(
                        "PROCMAPENGINE map-data trace u32SetCameraParameters t={}ms thread=[{}] obj={:#x} params={:#x} words=[{}] view_rect=[{:#x},{:#x},{:#x},{:#x}] lr={:#x}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        uc.reg_read(RegisterARM::R0).unwrap_or(0),
                        params,
                        words
                            .iter()
                            .map(|word| format!("{:#x}", word))
                            .collect::<Vec<_>>()
                            .join(","),
                        view_rect[0],
                        view_rect[1],
                        view_rect[2],
                        view_rect[3],
                        (uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32)
                            .wrapping_sub(base_address)
                            .wrapping_add(ORIGINAL_BASE)
                            & !1,
                    );
                    return;
                }
                let message = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let header = read_u32_or_invalid(uc, message + 4);
                log::info!(
                    "PROCMAPENGINE map-data trace {} t={}ms thread=[{}] hits={} r0={:#x} r1={:#x} header={:#x} svc={:#x} fn={:#x} opcode={:#x} lr={:#x}",
                    label,
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    hits,
                    uc.reg_read(RegisterARM::R0).unwrap_or(0),
                    message,
                    header,
                    read_u16_or_invalid(uc, header + 0x14),
                    read_u16_or_invalid(uc, header + 0x18),
                    read_u8_or_invalid(uc, header + 0x1a) as u32,
                    (uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32)
                        .wrapping_sub(base_address)
                        .wrapping_add(ORIGINAL_BASE)
                        & !1,
                );
            })
            .unwrap();
    }

    // Every re-arm of a map-data request: `UpdateMapDataRequest(prio, type,
    // scale, flags, rect)` - the rectangle arrives by value in the four stack
    // words above the return address - and `PerformMapDataRequest` tears the
    // pending request down (state 0, ref counter 0) whenever those parameters
    // differ from the ones in flight. Two callers with different rectangles - the
    // HMI's own view and the position feed - therefore take turns orphaning each
    // other's request, and no DAPI answer can be matched any more. Log who arms
    // what, and what the request currently holds.
    let update = base_address + (0x0050_494cu32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(update as u64, update as u64, move |uc, _, _| {
            static LAST: std::sync::OnceLock<std::sync::Mutex<[u32; 7]>> =
                std::sync::OnceLock::new();
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let prio = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let kind = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xff;
            let scale_bits = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            let stamp = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
            let rect = [
                read_u32_or_invalid(uc, sp),
                read_u32_or_invalid(uc, sp + 4),
                read_u32_or_invalid(uc, sp + 8),
                read_u32_or_invalid(uc, sp + 0xc),
            ];
            // Only log when the arm parameters actually change: the render loop
            // re-arms with the same rectangle on every frame and that flood hides
            // the changes that matter (the position feed taking over). The 4th
            // register (`stamp` = type * 1000 + OSAL_ClockGetElapsedTime()) is left
            // out of the comparison because it changes on every call.
            let changed = {
                let mut last = LAST.get_or_init(Default::default).lock().unwrap();
                let current = [prio, kind, scale_bits, rect[0], rect[1], rect[2], rect[3]];
                let changed = *last != current;
                if changed {
                    *last = current;
                }
                changed
            };
            if !changed {
                return;
            }
            log::info!(
                "PROCMAPENGINE map-data trace UpdateMapDataRequest at {:#x}: t={}ms thread=[{}] prio={:#x} type={:#x} scale={:.3} stamp={:#x} rect=[{:#x},{:#x},{:#x},{:#x}] lr={:#x}",
                update,
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                prio,
                kind,
                f32::from_bits(scale_bits),
                stamp,
                rect[0],
                rect[1],
                rect[2],
                rect[3],
                (uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1,
            );
        })
        .unwrap();
}

fn add_map_view_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let create_result = base_address + RENDER_CONTROL_CREATE_VIEW_RESULT;
    unicorn
        .add_code_hook(create_result as u64, create_result as u64, |uc, _, _| {
            if !MAP_VIEW_CREATE_MUTATING.swap(false, Ordering::Relaxed) {
                return;
            }

            let job = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
            if job == 0 || job == u32::MAX {
                MAP_VIEW_CREATE_PENDING.store(false, Ordering::Relaxed);
                log::warn!("PROCMAPENGINE: map view create job factory returned null");
                return;
            }

            if uc
                .mem_write((job + 0x1c) as u64, &[MAP_RENDER_VIEW_ID as u8])
                .is_err()
            {
                MAP_VIEW_CREATE_PENDING.store(false, Ordering::Relaxed);
                log::warn!(
                    "PROCMAPENGINE: failed to set view id on create job {:#x}",
                    job
                );
                return;
            }

            MAP_VIEW_CREATE_JOB_ADDRESS.store(job, Ordering::Relaxed);
            MAP_VIEW_CREATE_PENDING.store(false, Ordering::Relaxed);
            MAP_VIEW_CREATE_ENQUEUE_PENDING.store(true, Ordering::Relaxed);
            log::info!("PROCMAPENGINE: created map view {} job {:#x}", MAP_RENDER_VIEW_ID, job);
        })
        .unwrap();

    let get_view_result = base_address + RENDER_CONTROL_GET_VIEW_RESULT;
    unicorn
        .add_code_hook(get_view_result as u64, get_view_result as u64, |uc, _, _| {
            let view_id = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let view = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            if view_id != MAP_RENDER_VIEW_ID || view == 0 {
                return;
            }

            let surface_state = read_u32_or_invalid(uc, view + 0xf50);
            let surface = read_u32_or_invalid(uc, view + 0x40);
            let count = RENDER_CONTROL_GET_VIEW_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 20 {
                log::info!(
                    "PROCMAPENGINE: map view {} exists at {:#x} surface={:#x} surface_state={:#x}",
                    MAP_RENDER_VIEW_ID,
                    view,
                    surface,
                    surface_state
                );
            }

            if surface != 0 && surface != u32::MAX {
                if surface_state != 3 {
                    if uc
                        .mem_write((view + 0xf50) as u64, &3u32.to_le_bytes())
                        .is_err()
                    {
                        log::warn!(
                            "PROCMAPENGINE: failed to force surface state on view {:#x}",
                            view
                        );
                        return;
                    }
                }

                MAP_VIEW_READY.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
}

fn add_map_data_main_loop_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let queue_global = base_address + MAP_DATA_JOB_QUEUE_GLOBAL;
    let queue_state_addr = base_address + MAP_DATA_MAINLOOP_QUEUE_STATE;

    unicorn
        .add_code_hook(queue_state_addr as u64, queue_state_addr as u64, move |uc, _, _| {
            let queue = read_u32_or_invalid(uc, queue_global);
            let state = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let last = RENDER_LOOP_QUEUE_STATE_LAST.swap(state, Ordering::Relaxed);
            let count = MAP_DATA_MAINLOOP_STATE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if state != last || count < 50 {
                log::info!(
                    "PROCMAPENGINE map-data trace mainloop state at {:#x}: queue={:#x} state={:#x}",
                    queue_state_addr,
                    queue,
                    state
                );
            }
        })
        .unwrap();

    // AddJobToMapDataManagerQueue (0x503414) path probes (r433): the
    // fabricated posfi UpdateMapDataRequest(4,...) job is queued and the
    // NOIOSCM event posted, yet the mainloop's GetElement never hands it
    // out (no PerformMapDataRequest request=4).  Determine whether the
    // job took the AddElement path, the backup-list branch (DAT_0078f800
    // != 0 -> 0x5035c4), or the queue-null/semaphore early return
    // (0x50353c), and what fill state AddElement reported.
    {
        const ADDJOB_QUEUE_ENTRY: u32 = 0x0050_3414 - ORIGINAL_BASE;
        const ADDJOB_BACKUP_BRANCH: u32 = 0x0050_35c4 - ORIGINAL_BASE;
        const ADDJOB_EARLY_RETURN: u32 = 0x0050_353c - ORIGINAL_BASE;
        const ADDJOB_ADDELEMENT_RET: u32 = 0x0050_34c0 - ORIGINAL_BASE;
        const ADDJOB_DAT_0078F800: u32 = 0x0050_39a4 - ORIGINAL_BASE;
        let addjob_entry = base_address + ADDJOB_QUEUE_ENTRY;
        unicorn
            .add_code_hook(addjob_entry as u64, addjob_entry as u64, move |uc, _, _| {
                let count = MAP_DATA_ADDJOB_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                let job = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                if job >= 0xf000_0000 {
                    return;
                }
                let request = read_u32_or_invalid(uc, job + 0x24);
                if count >= 400 || (request == 5 && count % 50 != 0) {
                    return;
                }
                let thread = uc.get_data().inner.thread_id();
                let backup_got = read_u32_or_invalid(uc, base_address + ADDJOB_DAT_0078F800);
                log::info!(
                    "PROCMAPENGINE map-data trace AddJob entry at {addjob_entry:#x}: job={job:#x} type={:#x} request={request} param={:#x} backup_flag={:08x} thread=[{thread}] count={count}",
                    read_u32_or_invalid(uc, job + 8),
                    read_u32_or_invalid(uc, job + 0x28) & 0xff,
                    read_u32_or_invalid(uc, backup_got),
                );
            })
            .unwrap();
        for (name, offset) in [
            ("backup-list", ADDJOB_BACKUP_BRANCH),
            ("early-return0", ADDJOB_EARLY_RETURN),
        ] {
            let addr = base_address + offset;
            unicorn
                .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                    let count = MAP_DATA_ADDJOB_BRANCH_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                    if count >= 20 {
                        return;
                    }
                    let job = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                    let request = if job < 0xf000_0000 {
                        read_u32_or_invalid(uc, job + 0x24)
                    } else {
                        0xffff_ffff
                    };
                    log::info!(
                        "PROCMAPENGINE map-data trace AddJob {name} taken at {addr:#x}: job={job:#x} request={request} count={count}"
                    );
                })
                .unwrap();
        }
        let addelement_ret = base_address + ADDJOB_ADDELEMENT_RET;
        unicorn
            .add_code_hook(addelement_ret as u64, addelement_ret as u64, move |uc, _, _| {
                let count = MAP_DATA_ADDJOB_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                let job = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                if job >= 0xf000_0000 {
                    return;
                }
                let request = read_u32_or_invalid(uc, job + 0x24);
                if count >= 400 || (request == 5 && count % 50 != 0) {
                    return;
                }
                let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE map-data trace AddJob AddElement-> at {addelement_ret:#x}: job={job:#x} request={request} r0={:#x} w1c={:#x} w18={:#x}",
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                    read_u32_or_invalid(uc, sp + 0x1c),
                    read_u32_or_invalid(uc, sp + 0x18),
                );
            })
            .unwrap();
    }

    // CreateMDMJobMapDataRequest::Create (0x4b9d30) allocation outcome
    // (r434): the posfi UpdateMapDataRequest(4,...) never reached
    // AddJobToMapDataManagerQueue, so Create must have returned NULL -
    // heap-null branch (0x4b9df4) or _Internal_Alloc(0x44) failure is
    // caught right after the bl at 0x4b9d6c.
    {
        const MDMJOB_CREATE_ALLOC_RESULT: u32 = 0x004b_9d6c - ORIGINAL_BASE;
        const MDMJOB_CREATE_HEAP_NULL: u32 = 0x004b_9df4 - ORIGINAL_BASE;
        for (name, offset) in [
            ("alloc-result", MDMJOB_CREATE_ALLOC_RESULT),
            ("heap-null", MDMJOB_CREATE_HEAP_NULL),
        ] {
            let addr = base_address + offset;
            unicorn
                .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                    let prio = uc.reg_read(RegisterARM::R9).unwrap_or(0) as u32;
                    let job = if name == "heap-null" {
                        0
                    } else {
                        uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32
                    };
                    if prio == 4 && job != 0 {
                        MAP_DATA_POSFI_JOB.store(job, Ordering::Relaxed);
                    }
                    // Successful prio=5 (default view) creations are the
                    // flood; only failures and other priorities matter.
                    if job != 0 && prio == 5 {
                        return;
                    }
                    let count =
                        MAP_DATA_ADDJOB_BRANCH_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                    if count >= 40 {
                        return;
                    }
                    let thread = uc.get_data().inner.thread_id();
                    log::info!(
                        "PROCMAPENGINE map-data trace MDMJob Create {name} at {addr:#x}: prio={prio} job={job:#x} thread=[{thread}] count={count}"
                    );
                })
                .unwrap();
        }
    }

    // PerformMapDataRequest (0x50c93c) gate probes (r439): GetElement hands
    // the fabricated request=4 job to the mainloop (kernel-verified), yet no
    // DapiGetBlockIDs with the Krakow rect follows, so the request must stop
    // inside PerformMapDataRequest.  After the prologue the rect lives in
    // r10/r6/r11/r8 (stack args at sp+0x80..0x8c), the prio in r7 and the
    // scale in s16/r2; the gates return at 0x50c9b0 (width too small),
    // 0x50cb18 (height too small) and 0x50c9b8 (rect == 0x7fffffff), and the
    // priority dispatch at 0x50ca54 sends prio 5 to 0x50cf58 but prio 4 to
    // its own branch at 0x50cb48.
    {
        const PMDR_ENTRY: u32 = 0x0050_c978 - ORIGINAL_BASE;
        const PMDR_INVALID_RECT: u32 = 0x0050_c9b8 - ORIGINAL_BASE;
        const PMDR_WIDTH_SMALL: u32 = 0x0050_c9b0 - ORIGINAL_BASE;
        const PMDR_HEIGHT_SMALL: u32 = 0x0050_cb18 - ORIGINAL_BASE;
        const PMDR_PRIO4_PATH: u32 = 0x0050_cb48 - ORIGINAL_BASE;
        const PMDR_PRIO5_PATH: u32 = 0x0050_cf58 - ORIGINAL_BASE;

        let pmdr_entry = base_address + PMDR_ENTRY;
        unicorn
            .add_code_hook(pmdr_entry as u64, pmdr_entry as u64, move |uc, _, _| {
                let count = MAP_DATA_PMDR_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                let prio = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                if count >= 800 || (prio == 5 && count % 50 != 0) {
                    return;
                }
                let thread = uc.get_data().inner.thread_id();
                let r10 = uc.reg_read(RegisterARM::R10).unwrap_or(0) as u32;
                let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                let r11 = uc.reg_read(RegisterARM::R11).unwrap_or(0) as u32;
                let r8 = uc.reg_read(RegisterARM::R8).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE map-data trace PerformMapDataRequest at {pmdr_entry:#x}: prio={prio} param={:#x} scale={:#x} ts={:#x} rect=[{r10:#08x} {r6:#08x} {r11:#08x} {r8:#08x}] width={} height={} thread=[{thread}] count={count}",
                    uc.reg_read(RegisterARM::R9).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32,
                    (r6 as i32).wrapping_sub(r10 as i32),
                    (r8 as i32).wrapping_sub(r11 as i32),
                );
            })
            .unwrap();

        for (name, offset) in [
            ("invalid-rect-return", PMDR_INVALID_RECT),
            ("width-small-return", PMDR_WIDTH_SMALL),
            ("height-small-return", PMDR_HEIGHT_SMALL),
            ("prio4-path", PMDR_PRIO4_PATH),
            ("prio5-path", PMDR_PRIO5_PATH),
        ] {
            let addr = base_address + offset;
            unicorn
                .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                    let prio = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                    // the prio=5 default-view path is known good and floods
                    if prio == 5 {
                        return;
                    }
                    let count = MAP_DATA_PMDR_GATE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                    if count >= 60 {
                        return;
                    }
                    let thread = uc.get_data().inner.thread_id();
                    log::info!(
                        "PROCMAPENGINE map-data trace PerformMapDataRequest {name} at {addr:#x}: prio={prio} thread=[{thread}] count={count}"
                    );
                })
                .unwrap();
        }
    }

    // TriggerHandleMapDataRequest (0x504824): the prio-4 (car cursor) branch
    // of PerformMapDataRequest ends with TriggerHandleMapDataRequest(false),
    // which queues a "HandleMapDataRequestInvisibleData" MDM job - the job
    // that is supposed to turn the stored request into GetBlockIDs calls.
    {
        const TRIGGER_HANDLE_MAP_DATA_REQUEST: u32 = 0x0050_4824 - ORIGINAL_BASE;
        let addr = base_address + TRIGGER_HANDLE_MAP_DATA_REQUEST;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = MAP_DATA_TRIGGER_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                let visible = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32 & 0xff;
                if count >= 400 {
                    return;
                }
                let thread = uc.get_data().inner.thread_id();
                log::info!(
                    "PROCMAPENGINE map-data trace TriggerHandleMapDataRequest at {addr:#x}: visible={visible} thread=[{thread}] count={count}"
                );
            })
            .unwrap();
    }

    // Which test in `PerformMapDataRequest`'s prio-4 branch sends the car-cursor
    // record into the clearing path instead of the queueing one.  The code here is
    // Thumb and mid-TB, so hook the whole page and dispatch on the fetch address
    // (a narrow hook of this kind has repeatedly registered fine and never fired).
    {
        let page_begin = base_address + 0x0050_c000u32 - ORIGINAL_BASE;
        let page_end = base_address + 0x0050_d000u32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(page_begin as u64, page_end as u64, move |uc, _, size| {
                static HITS: AtomicU32 = AtomicU32::new(0);
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let offset = pc.wrapping_sub(base_address) + ORIGINAL_BASE;
                if !matches!(offset, 0x0050_cd24..=0x0050_cd2c) {
                    return;
                }
                let hits = HITS.fetch_add(1, Ordering::Relaxed);
                if hits >= 24 {
                    return;
                }
                let carsor = base_address + 0x0079_04e4u32 - ORIGINAL_BASE;
                let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                let words: Vec<u32> =
                    (0..24u32).map(|i| read_u32_or_invalid(uc, sp + i * 4)).collect();
                // Passes for the camera rectangle reach this code constantly; only the
                // ones carrying the posfi car box (longitude ~0x0e2xxxxx) matter here.
                if !words.iter().any(|word| (0xfff0_0000 & word) == 0x0e20_0000) {
                    return;
                }
                log::info!(
                    "PROCMAPENGINE map-data trace prio4-branch at {:#x} t={}ms thread=[{}] hits={} r0={:#x} r1={:#x} r4={:#x} r6={:#x} r9={:#x} counters[cam={} block={} carsor={}] f1f4={:#x} list=[{}] stack=[{}] size={}",
                    offset,
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    hits,
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R9).unwrap_or(0) as u32,
                    read_u32_or_invalid(
                        uc,
                        base_address + 0x0078_fcd4u32 - ORIGINAL_BASE + 0x74,
                    ),
                    read_u32_or_invalid(uc, base_address + 0x0079_0348u32 - ORIGINAL_BASE + 0x74),
                    read_u32_or_invalid(uc, carsor + 0x74),
                    read_u32_or_invalid(uc, carsor + 0x1f4),
                    (0..8u32)
                        .map(|i| {
                            format!(
                                "{:#x}:{:#x}",
                                0x713040 + i * 4,
                                read_u32_or_invalid(
                                    uc,
                                    base_address + 0x0071_3040u32 - ORIGINAL_BASE + i * 4
                                )
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                    (0..12u32)
                        .map(|i| format!("{:#x}", read_u32_or_invalid(uc, sp + i * 4)))
                        .collect::<Vec<_>>()
                        .join(" "),
                    size,
                );
            })
            .unwrap();
    }

    // IntHandleMapDataRequest (0x5116d4) request selection (r441): the
    // invisible-data job runs but never reaches GetBlockIDs for the car-cursor
    // request, so dump what GetHighestPriorityRequest (bl at 0x5116e0) actually
    // picked: kind (+0), data type (+4), handled-state (+0x74), the LeftToFetch
    // sentinels (+0x1f4/+0x1f8) and the rect, plus whether it is the static
    // ms_sMapDataReqCarsor (0x7904e4) the prio-4 branch had just updated.
    {
        const INT_HANDLE_GET_HIGHEST_PRIO_RESULT: u32 = 0x0051_16e4 - ORIGINAL_BASE;
        const MAP_DATA_REQ_CARSOR: u32 = 0x0079_04e4 - ORIGINAL_BASE;
        let addr = base_address + INT_HANDLE_GET_HIGHEST_PRIO_RESULT;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = MAP_DATA_TRIGGER_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 60 {
                    return;
                }
                let request = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                let visible_flag_ptr = read_u32_or_invalid(uc, sp + 0x38);
                let carsor = base_address + MAP_DATA_REQ_CARSOR;
                let thread = uc.get_data().inner.thread_id();
                log::info!(
                    "PROCMAPENGINE map-data trace IntHandleMapDataRequest picked at {addr:#x}: request={request:#x} carsor={carsor:#x} visible_flag={} kind={} data_type={:#x} state={} f1f4={:#x} f1f8={:#x} rect=[{:08x} {:08x} {:08x} {:08x}] thread=[{thread}] count={count}",
                    read_u32_or_invalid(uc, visible_flag_ptr) & 0xff,
                    read_u32_or_invalid(uc, request),
                    read_u32_or_invalid(uc, request + 4) & 0xff,
                    read_u32_or_invalid(uc, request + 0x74),
                    read_u32_or_invalid(uc, request + 500),
                    read_u32_or_invalid(uc, request + 0x1f8),
                    read_u32_or_invalid(uc, request + 0x60),
                    read_u32_or_invalid(uc, request + 0x64),
                    read_u32_or_invalid(uc, request + 0x68),
                    read_u32_or_invalid(uc, request + 0x6c),
                );
                // The arbitration only ever picks the camera record, so compare the
                // two static request records field by field: whichever field differs
                // is the reason `ms_sMapDataReqCarsor` is never a candidate.
                for (name, record) in [
                    ("camera", base_address + 0x0078_fcd4u32 - ORIGINAL_BASE),
                    ("carsor", carsor),
                ] {
                    static RECORD_DUMPS: AtomicU32 = AtomicU32::new(0);
                    if read_u32_or_invalid(uc, carsor + 0x60) == 0
                        || RECORD_DUMPS.fetch_add(1, Ordering::Relaxed) >= 12
                    {
                        continue;
                    }
                    log::info!(
                        "PROCMAPENGINE map-data trace request record {name} at {record:#x}: kind={} data_type={:#x} view={} state={} f1f4={:#x} f1f8={:#x} rect=[{:08x} {:08x} {:08x} {:08x}]",
                        read_u32_or_invalid(uc, record) & 0xff,
                        read_u32_or_invalid(uc, record + 4) & 0xff,
                        read_u32_or_invalid(uc, record + 0x200),
                        read_u32_or_invalid(uc, record + 0x74),
                        read_u32_or_invalid(uc, record + 0x1f4),
                        read_u32_or_invalid(uc, record + 0x1f8),
                        read_u32_or_invalid(uc, record + 0x60),
                        read_u32_or_invalid(uc, record + 0x64),
                        read_u32_or_invalid(uc, record + 0x68),
                        read_u32_or_invalid(uc, record + 0x6c),
                    );
                }
            })
            .unwrap();
    }

    let request_call_addr = base_address + MAP_DATA_JOB_MAP_DATA_REQUEST_CALL;
    unicorn
        .add_code_hook(
            request_call_addr as u64,
            request_call_addr as u64,
            move |uc, _, _| {
                let count = MAP_DATA_REQUEST_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                // Same lesson as the UpdateMapDataRequest probe (r431): the
                // request=5 default-window flood saturates the budget, so
                // throttle it and keep every other request id visible.
                let request = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                if count >= 600 || (request == 5 && count >= 10) {
                    return;
                }
                {
                    let priority = (uc.reg_read(RegisterARM::R1).unwrap_or(0) & 0xff) as u32;
                    let scale = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let data_type = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE map-data trace PerformMapDataRequest call at {:#x}: request={} priority={} scale={:#x} data_type={:#x} rect=[{:#x},{:#x},{:#x},{:#x}]",
                        request_call_addr,
                        request,
                        priority,
                        scale,
                        data_type,
                        read_u32_or_invalid(uc, sp),
                        read_u32_or_invalid(uc, sp + 4),
                        read_u32_or_invalid(uc, sp + 8),
                        read_u32_or_invalid(uc, sp + 12)
                    );
                }
            },
        )
        .unwrap();

    let request_result_addr = base_address + MAP_DATA_JOB_MAP_DATA_REQUEST_RESULT;
    unicorn
        .add_code_hook(
            request_result_addr as u64,
            request_result_addr as u64,
            move |uc, _, _| {
                let count = MAP_DATA_REQUEST_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 200 {
                    let result = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE map-data trace PerformMapDataRequest result at {:#x}: r0={:#x}",
                        request_result_addr,
                        result
                    );
                }
            },
        )
        .unwrap();

    let handle_result_addr = base_address + MAP_DATA_JOB_HANDLE_REQUEST_RESULT;
    unicorn
        .add_code_hook(
            handle_result_addr as u64,
            handle_result_addr as u64,
            move |uc, _, _| {
                let count = MAP_DATA_HANDLE_REQUEST_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 200 {
                    let result = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE map-data trace HandleMapDataRequest result at {:#x}: r0={:#x}",
                        handle_result_addr,
                        result
                    );
                }
            },
        )
        .unwrap();

    let dapi_get_block_ids_entry = base_address + MAP_DATA_DAPI_GET_BLOCK_IDS_ENTRY;
    unicorn
        .add_code_hook(
            dapi_get_block_ids_entry as u64,
            dapi_get_block_ids_entry as u64,
            move |uc, _, _| {
                let count = MAP_DATA_DAPI_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 200 {
                    let platform = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let handler = read_u32_or_invalid(uc, platform + 0xb0);
                    let service_handler = if handler == 0 || handler == u32::MAX {
                        0xffff_ffff
                    } else {
                        read_u32_or_invalid(uc, handler + 0x24)
                    };
                    let service_state_before = if service_handler == 0 || service_handler == u32::MAX
                    {
                        0xffff_ffff
                    } else {
                        let state = read_u32_or_invalid(uc, service_handler + 0xe) & 0xff;
                        let _ = write_u8(uc, service_handler + 0xe, 0);
                        state
                    };
                    let rect = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE map-data trace DapiGetBlockIDs entry at {:#x}: platform={:#x} handler={:#x} service_handler={:#x} service_state_before={:#x} rect={:#x} rect=[{:#x},{:#x},{:#x},{:#x}]",
                        dapi_get_block_ids_entry,
                        platform,
                        handler,
                        service_handler,
                        service_state_before,
                        rect,
                        read_u32_or_invalid(uc, rect),
                        read_u32_or_invalid(uc, rect + 4),
                        read_u32_or_invalid(uc, rect + 8),
                        read_u32_or_invalid(uc, rect + 12)
                    );
                }
            },
        )
        .unwrap();

    let cca_get_block_ids_vcall = base_address + MAP_DATA_CCA_GET_BLOCK_IDS_VCALL;
    unicorn
        .add_code_hook(
            cca_get_block_ids_vcall as u64,
            cca_get_block_ids_vcall as u64,
            move |uc, _, _| {
                let count = MAP_DATA_DAPI_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 200 {
                    let service = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let request = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let refcounter = uc.reg_read(RegisterARM::R2).unwrap_or(0) & 0xffff;
                    let vtable = read_u32_or_invalid(uc, service);
                    let target = read_u32_or_invalid(uc, vtable + 0x10);
                    log::info!(
                        "PROCMAPENGINE map-data trace CcaGetBlockIDs vcall at {:#x}: service={:#x} request={:#x} refcounter={:#x} vtable={:#x} target={:#x}",
                        cca_get_block_ids_vcall,
                        service,
                        request,
                        refcounter,
                        vtable,
                        target
                    );
                }
            },
        )
        .unwrap();
    let send_request_entry = base_address + MAP_DATA_CCA_SEND_REQUEST_ENTRY;
    add_dapi_register_diag_hooks(unicorn, base_address);
    unicorn
        .add_code_hook(send_request_entry as u64, send_request_entry as u64, move |uc, _, _| {
            // The unit's CCA directory (TE side) never answers the register
            // handshake, so the map engine's DAPI client handler stays
            // unregistered and every ServiceData request is rejected with
            // error id 6. Kick the natural registration once per un-
            // registered state: vRegisterService posts the REGISTER to mbx_7
            // and the reply's RegisterConf fills in the handler register-id.
            let handler = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let handler_service = read_u16_or_invalid(uc, handler + 4) & 0xffff;
            let register_id = read_u16_or_invalid(uc, handler + 0xc) & 0xffff;
            let mut register_fired = false;
            if handler_service == MAP_DATA_DAPI_SERVICE_ID {
                if register_id != 0xffff {
                    MAP_DATA_DAPI_REGISTER_PENDING.store(false, Ordering::Relaxed);
                } else if !MAP_DATA_DAPI_REGISTER_PENDING.swap(true, Ordering::Relaxed) {
                    let function = base_address + MAP_DATA_CLIENT_VREGISTER_SERVICE;
                    if call_guest_function(uc, send_request_entry, function, [handler, 0, 0, 0]) {
                        log::info!(
                            "PROCMAPENGINE: fired DAPI client service registration for handler {:#x}",
                            handler
                        );
                        register_fired = true;
                    } else {
                        MAP_DATA_DAPI_REGISTER_PENDING.store(false, Ordering::Relaxed);
                    }
                }
            }
            // Navi client handler (posfi 0x1e consumer): its bInit ran at
            // startup and created the four client service handlers, but the
            // registration pass lives in vOnNewAppState, which only fires when
            // the AIL app state transitions to 3/4 - and the PortControl
            // platform never gets that notification here.  Run the same
            // registration loop host-side once: walk the platform's handler
            // vector for the Navi vtable and vRegisterService each pending
            // client service handler (svc at +4, register-id sentinel 0xffff
            // at +0xc, exactly as vOnNewAppState iterates them).
            //
            // Serialization (r428): call_guest_function only saves the caller
            // registers once (LR = restore stub), so a second call queued
            // while the first still runs corrupts both frames - the engine
            // then resumed the GetBlockIDs job with r1=0 and died in
            // bSendCCARequest (READ_UNMAPPED 0x591b00), killing the map-data
            // mainloop (tid63 exited(1)) in every kick-era run.  Fire at most
            // one guest call per hook event; the posfi kick runs on the next
            // entry to this hook.
            let platform = PORTCONTROL_PLATFORM.load(Ordering::Relaxed);
            if !register_fired && platform != 0 && platform < 0xf000_0000 {
                // Handler vector of PortControl_CCAClientHandlerBase* (4-byte
                // elements) at platform +0x94..+0x9c.
                let begin = read_u32_or_invalid(uc, platform + 0x94);
                let end = read_u32_or_invalid(uc, platform + 0x9c);
                let span = end.saturating_sub(begin);
                if begin < 0xf000_0000 && span > 0 && span <= 8 * 4 {
                    let mut offset = 0u32;
                    while offset < span {
                        let candidate = read_u32_or_invalid(uc, begin.saturating_add(offset));
                        if read_u32_or_invalid(uc, candidate) == NAVI_CLIENT_HANDLER_VTABLE {
                            if NAVI_REGISTER_KICK_PENDING
                                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::Relaxed)
                                .is_err()
                            {
                                break;
                            }
                            // vOnNewAppState's loop: client service handler
                            // vector at handler +0xc..+0x10, vRegisterService
                            // on every entry (bInit already populated it).
                            let svc_begin = read_u32_or_invalid(uc, candidate + 0xc);
                            let svc_end = read_u32_or_invalid(uc, candidate + 0x10);
                            let svc_span = svc_end.saturating_sub(svc_begin);
                            let mut registered = 0u32;
                            if svc_begin < 0xf000_0000 && svc_span > 0 && svc_span <= 16 * 4 {
                                let mut svc_off = 0u32;
                                while svc_off < svc_span {
                                    let service_handler = read_u32_or_invalid(
                                        uc,
                                        svc_begin.saturating_add(svc_off),
                                    );
                                    let service_id =
                                        read_u16_or_invalid(uc, service_handler + 4) & 0xffff;
                                    let reg_id =
                                        read_u16_or_invalid(uc, service_handler + 0xc) & 0xffff;
                                    // Only register svc 0x1e (posfi): that is
                                    // the one service the fabricated provider
                                    // feeds.  Registering the sibling Navi
                                    // services (0x1f/0x2c/0x37, r412-r427)
                                    // made DAPI's ADM remember requests whose
                                    // engine-side message slots stayed empty,
                                    // and the later tail-call entered
                                    // bSendCCARequest (0x591af0) with
                                    // message=NULL -> READ_UNMAPPED at
                                    // 0x591b00, silently killing the map-data
                                    // mainloop thread (tid63 exited(1)).
                                    if reg_id == 0xffff && service_id == 0x1e {
                                        let function =
                                            base_address + MAP_DATA_CLIENT_VREGISTER_SERVICE;
                                        if call_guest_function(
                                            uc,
                                            send_request_entry,
                                            function,
                                            [service_handler, 0, 0, 0],
                                        ) {
                                            registered += 1;
                                            log::info!(
                                                "PROCMAPENGINE: fired Navi client service registration for svc {:#x} handler {:#x}",
                                                service_id,
                                                service_handler
                                            );
                                            // posfi has no live provider here
                                            // (on the unit PROCNAV's posfi
                                            // server answers the REGISTER), so
                                            // DAPI's ADM never confirms svc
                                            // 0x1e and the dispatcher never
                                            // claims inbound NavPositionStatus.
                                            // Feed the engine the RegisterConf
                                            // it would get from the unit's ADM:
                                            // class-0x43 envelope whose sub-id
                                            // slot (+0x00) is the *server app
                                            // id*, +0x0e the target sub-id,
                                            // +0x14 the service, +0x16 the
                                            // assigned register-id, +0x18 the
                                            // status (1 = OK, which makes
                                            // vUnknownRegisterConf insert the
                                            // ServiceReference the claim walk
                                            // matches).  Server app and client
                                            // sub-id come from the handler
                                            // object itself so the async
                                            // request lookup matches.
                                            if service_id == 0x1e {
                                                let server_app = read_u16_or_invalid(
                                                    uc,
                                                    service_handler + 6,
                                                ) & 0xffff;
                                                let client_sub = read_u16_or_invalid(
                                                    uc,
                                                    service_handler + 0xa,
                                                ) & 0xffff;
                                                log::info!(
                                                    "PROCMAPENGINE: posfi handler {:#x} server_app={:#x} sub8={:#x} client_sub={:#x}",
                                                    service_handler,
                                                    server_app,
                                                    read_u16_or_invalid(uc, service_handler + 8)
                                                        & 0xffff,
                                                    client_sub
                                                );
                                                post_posfi_register_conf(
                                                    uc,
                                                    server_app,
                                                    client_sub,
                                                );
                                                insert_posfi_service_reference(
                                                    uc, send_request_entry,
                                                );
                                            }
                                        }
                                    }
                                    svc_off += 4;
                                }
                            }
                            log::info!(
                                "PROCMAPENGINE: Navi client handler {:#x} registration pass done ({} services fired)",
                                candidate,
                                registered
                            );
                            break;
                        }
                        offset += 4;
                    }
                }
            }
            let count = MAP_DATA_DAPI_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 200 && !register_fired {
                let handler = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let message = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let message_vtable = read_u32_or_invalid(uc, message);
                let (vt4, vt8, vtc, vt10) = if message_vtable < 0xf000_0000 {
                    (
                        read_u32_or_invalid(uc, message_vtable + 4),
                        read_u32_or_invalid(uc, message_vtable + 8),
                        read_u32_or_invalid(uc, message_vtable + 0xc),
                        read_u32_or_invalid(uc, message_vtable + 0x10),
                    )
                } else {
                    (0, 0, 0, 0)
                };
                log::info!(
                    "PROCMAPENGINE map-data trace SendCCARequest entry at {:#x}: handler={:#x} message={:#x} message_vtable={:#x} handler_service={:#x} handler_function={:#x} vt[4]={:#x} vt[8]={:#x} vt[c]={:#x} vt[10]={:#x}",
                    send_request_entry,
                    handler,
                    message,
                    message_vtable,
                    read_u32_or_invalid(uc, handler.saturating_add(4)) & 0xffff,
                    read_u32_or_invalid(uc, handler.saturating_add(6)) & 0xffff,
                    vt4,
                    vt8,
                    vtc,
                    vt10
                );
            }
        })
        .unwrap();

    let request_ctor_result = base_address + MAP_DATA_SEND_REQUEST_CONSTRUCTOR_RESULT;
    unicorn
        .add_code_hook(
            request_ctor_result as u64,
            request_ctor_result as u64,
            move |uc, _, _| {
                let count = MAP_DATA_DAPI_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 200 {
                    let message_object = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let base_message = read_u32_or_invalid(uc, message_object + 4);
                    log::info!(
                        "PROCMAPENGINE map-data trace request ctor result at {:#x}: object={:#x} base_message={:#x} valid={:#x}",
                        request_ctor_result,
                        message_object,
                        base_message,
                        read_u32_or_invalid(uc, message_object + 0x20) & 0xff
                    );
                }
            },
        )
        .unwrap();

    let send_service_vcall = base_address + MAP_DATA_SEND_SERVICE_VCALL;
    unicorn
        .add_code_hook(send_service_vcall as u64, send_service_vcall as u64, move |uc, _, _| {
            let count = MAP_DATA_DAPI_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 200 {
                let client = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let data = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let vtable = read_u32_or_invalid(uc, client);
                let target = read_u32_or_invalid(uc, vtable + 0x48);
                log::info!(
                    "PROCMAPENGINE map-data trace SendServiceDataMsg vcall at {:#x}: client={:#x} data={:#x} vtable={:#x} target={:#x}",
                    send_service_vcall,
                    client,
                    data,
                    vtable,
                    target
                );
            }
        })
        .unwrap();
    let send_service_result = base_address + MAP_DATA_SEND_SERVICE_RESULT;
    unicorn
        .add_code_hook(send_service_result as u64, send_service_result as u64, move |uc, _, _| {
            let count = MAP_DATA_DAPI_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 200 {
                let result = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE map-data trace SendServiceDataMsg result at {:#x}: r0={:#x}",
                    send_service_result,
                    result
                );
            }
        })
        .unwrap();

    // Who asks for a registration, does the 9 s retry timer fire, and does the
    // conf handling run: procmapengine sends ServiceRegister for svc 0x26 twice
    // and DAPIAPP drops the client entry when it replaces it.
    for (offset, label) in [
        (AIL_EN_REGISTER_ASYNC_EXECUTE, "bRegisterAsyncExecute"),
        (AIL_EN_ASYNC_REGISTER_TOUT, "vTriggerAsyncRegisterTout"),
        (AIL_EN_HANDLE_ASYNC_REGISTER_CONF, "vHandleAsyncRegisterConf"),
    ] {
        let entry = base_address + offset;
        unicorn
            .add_code_hook(entry as u64, entry as u64, move |uc, _, _| {
                static REG_TRACE_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if REG_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) >= 40 {
                    return;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                let this = read(RegisterARM::R0);
                let r1 = read(RegisterARM::R1);
                let r2 = read(RegisterARM::R2);
                let r3 = read(RegisterARM::R3);
                let sp = read(RegisterARM::SP);
                let lr = read(RegisterARM::LR)
                    .wrapping_sub(base_address)
                    .wrapping_add(ORIGINAL_BASE);
                // Parameters 5..7 live on the stack; the retry timer callback
                // only gets its owner as argument.
                log::info!(
                    "PROCMAPENGINE ail trace {label} at {:#x}: this={this:#x} r1={r1:#x} r2={r2:#x} r3={r3:#x} stack=[{:#x},{:#x},{:#x}] app_list={:#x} timer={:#x} lr={lr:#x}",
                    entry,
                    read_u32_or_invalid(uc, sp),
                    read_u32_or_invalid(uc, sp + 4),
                    read_u32_or_invalid(uc, sp + 8),
                    read_u32_or_invalid(uc, this + 0x70),
                    read_u32_or_invalid(uc, this + 0x74),
                );
            })
            .unwrap();
    }

    let post_message_entry = base_address + AIL_EN_POST_MESSAGE_ENTRY;
    unicorn
        .add_code_hook(post_message_entry as u64, post_message_entry as u64, move |uc, _, _| {
            let count = AIL_MAIL_VALIDITY_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 20 {
                log::info!(
                    "PROCMAPENGINE ail trace PostMessage entry at {:#x}: r0={:#x} r1={:#x} r2={:#x} r3={:#x} app_id={:#x} service_refs={:#x}",
                    post_message_entry,
                    uc.reg_read(RegisterARM::R0).unwrap_or(0),
                    uc.reg_read(RegisterARM::R1).unwrap_or(0),
                    uc.reg_read(RegisterARM::R2).unwrap_or(0),
                    uc.reg_read(RegisterARM::R3).unwrap_or(0),
                    read_u16_or_invalid(uc, uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32 + 0x10),
                    read_u32_or_invalid(uc, uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32 + 0x50)
                );
            }
        })
        .unwrap();

    let mail_validity_entry = base_address + AIL_EN_CHECK_CLIENT_MAIL_VALIDITY;
    unicorn
        .add_code_hook(mail_validity_entry as u64, mail_validity_entry as u64, move |uc, _, _| {
            let count = AIL_MAIL_VALIDITY_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 20 {
                let app_id = (uc.reg_read(RegisterARM::R0).unwrap_or(0) & 0xffff) as u32;
                let semaphore = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let message = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let service_refs = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let header = read_u32_or_invalid(uc, message + 4);
                synthesize_client_service_reference(uc, message, service_refs);
                let first_node = read_u32_or_invalid(uc, service_refs + 4);
                log::info!(
                    "PROCMAPENGINE ail trace CheckClientMailValidity entry at {:#x}: app_id={:#x} semaphore={:#x} message={:#x} service_refs={:#x} header={:#x} source={:#x} target={:#x} service={:#x} dataset={:#x} register={:#x} msg_type={:#x} first_node={:#x} node_service={:#x} node_server={:#x} node_register={:#x} node_state={:#x}",
                    mail_validity_entry,
                    app_id,
                    semaphore,
                    message,
                    service_refs,
                    header,
                    read_u16_or_invalid(uc, header),
                    read_u16_or_invalid(uc, header + 2),
                    read_u16_or_invalid(uc, header + 12),
                    read_u16_or_invalid(uc, header + 20),
                    read_u16_or_invalid(uc, header + 22),
                    read_u16_or_invalid(uc, header + 26),
                    first_node,
                    read_u16_or_invalid(uc, first_node + 8),
                    read_u16_or_invalid(uc, first_node + 10),
                    read_u16_or_invalid(uc, first_node + 12),
                    read_u16_or_invalid(uc, first_node + 14) & 0xff
                );
            }
        })
        .unwrap();

    let mail_validity_epilogue = base_address + AIL_EN_CHECK_CLIENT_MAIL_VALIDITY_EPILOGUE;
    unicorn
        .add_code_hook(
            mail_validity_epilogue as u64,
            mail_validity_epilogue as u64,
            move |uc, _, _| {
                let count = AIL_MAIL_VALIDITY_RESULT_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 20 {
                    log::info!(
                        "PROCMAPENGINE ail trace CheckClientMailValidity epilogue at {:#x}: r4={:#x}",
                        mail_validity_epilogue,
                        uc.reg_read(RegisterARM::R4).unwrap_or(0)
                    );
                }
            },
        )
        .unwrap();

    for (name, offset) in [
        ("after validity", AIL_EN_POST_MESSAGE_AFTER_VALIDITY),
        ("success path", AIL_EN_POST_MESSAGE_SUCCESS_PATH),
        ("receiver queue call", AIL_EN_POST_MESSAGE_RECEIVER_QUEUE_CALL),
        ("post result", AIL_EN_POST_MESSAGE_POST_RESULT),
        ("delete path", AIL_EN_POST_MESSAGE_DELETE_PATH),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = AIL_POST_MESSAGE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 40 {
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE ail trace PostMessage {} at {:#x}: sp={:#x} result={:#x} r0={:#x} r1={:#x} r2={:#x} r3={:#x} lr={:#x}",
                        name,
                        addr,
                        sp,
                        read_u32_or_invalid(uc, sp + 0x2c),
                        uc.reg_read(RegisterARM::R0).unwrap_or(0),
                        uc.reg_read(RegisterARM::R1).unwrap_or(0),
                        uc.reg_read(RegisterARM::R2).unwrap_or(0),
                        uc.reg_read(RegisterARM::R3).unwrap_or(0),
                        uc.reg_read(RegisterARM::LR).unwrap_or(0),
                    );
                }
            })
            .unwrap();
    }

    let cca_get_block_ids_result = base_address + MAP_DATA_CCA_GET_BLOCK_IDS_RESULT;
    unicorn
        .add_code_hook(
            cca_get_block_ids_result as u64,
            cca_get_block_ids_result as u64,
            move |uc, _, _| {
                let count = MAP_DATA_DAPI_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 200 {
                    let result = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE map-data trace CcaGetBlockIDs result at {:#x}: r0={:#x}",
                        cca_get_block_ids_result,
                        result
                    );
                }
            },
        )
        .unwrap();

    for (name, offset) in [
        (
            "vHandleOpenFastMapChannel",
            MAP_DATA_VHANDLE_OPEN_FMC_ENTRY,
        ),
        ("vHandleGetBlockIDs", MAP_DATA_VHANDLE_GET_BLOCK_IDS_ENTRY),
        ("vHandleDefaultDataSet", MAP_DATA_VHANDLE_DEFAULT_DATASET_ENTRY),
        ("vHandleLoadBlocks", MAP_DATA_VHANDLE_LOAD_BLOCKS_ENTRY),
        (
            "vHandleDatablocksChanged",
            MAP_DATA_VHANDLE_DATABLOCKS_CHANGED_ENTRY,
        ),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = MAP_DATA_DAPI_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 200 {
                    let service_data = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let inner = read_u32_or_invalid(uc, service_data + 4);
                    let opcode = read_u32_or_invalid(uc, inner + 0x1a) & 0xff;
                    let mdm_ref = read_u16_or_invalid(uc, inner + 0x1c);
                    log::info!(
                        "PROCMAPENGINE map-data trace {} entry at {:#x}: service_data={:#x} inner={:#x} opcode={:#x} mdm_ref={:#x}",
                        name,
                        addr,
                        service_data,
                        inner,
                        opcode,
                        mdm_ref
                    );
                }
            })
            .unwrap();
    }

    // Job-queue tail of the LoadBlocks answer path: vHandleLoadBlocks creates
    // `map_tclJobMDMHandleLoadBlocks` (Create thunk 0x4a59f8 -> Create 0x4b60b4,
    // vtable 0x6a3b48) and queues it via map_tclPortControl::MapData_AddJobToQueue
    // (0x53a094); the MDM thread then runs Execute (0x4b5420, vtable+0x24) which
    // maps the channel shared memory, hands the decoded blocks to the render
    // control and releases the channel slot. r578 proved the handler runs but
    // the job never reaches any queue (no log line carries vtable 0x6a3b48), so
    // watch entry of queue-add and of Execute to find where they vanish.
    {
        let statics = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let queued = std::sync::Arc::clone(&statics);
        let addr = base_address + 0x0053_a094u32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = queued.fetch_add(1, Ordering::Relaxed);
                if count < 200 {
                    let job = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let vtable = read_u32_or_invalid(uc, job);
                    log::info!(
                        "PROCMAPENGINE map-data trace AddJobToQueue entry at {:#x}: job={:#x} vtable={:#x} lr={:#x}",
                        addr,
                        job,
                        vtable,
                        uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32
                    );
                }
            })
            .unwrap();
        let executed = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let addr = base_address + 0x004b_5420u32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = executed.fetch_add(1, Ordering::Relaxed);
                if count < 200 {
                    let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let mdm_ref = read_u16_or_invalid(uc, this + 0x1e);
                    let begin = read_u32_or_invalid(uc, this + 0x34);
                    let end = read_u32_or_invalid(uc, this + 0x38);
                    log::info!(
                        "PROCMAPENGINE map-data trace MDMHandleLoadBlocks::Execute entry at {:#x}: this={:#x} mdm_ref={:#x} result_blocks={} lr={:#x}",
                        addr,
                        this,
                        mdm_ref,
                        end.saturating_sub(begin) / 0x18,
                        uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32
                    );
                }
            })
            .unwrap();
    }

    // r579: `map_tclJobRCMapObjectsUpdate` (vtable 0x6abe80) reached the render
    // queue and passed the IsExecutable gate 24x, so the MDM->RC handoff works.
    // Its Execute (0x4d37e0, vtable+0x2c) forwards each map object to
    // `DTM_FBO_Handler::bCheckForTextureUpdateBasedOnNewMO` (0x556170, texture
    // cache invalidation) and to `map_tclSceneManager::MapObjectsUpdate`
    // (0x4357c8, scene graph).  Watch all three: if the FBO handler is null or
    // the object loop never runs, the MAP surface stays the flat background.
    {
        let mo_exec = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let addr = base_address + 0x004d_37e0u32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = mo_exec.fetch_add(1, Ordering::Relaxed);
                if count >= 100 {
                    return;
                }
                let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let vec = read_u32_or_invalid(uc, this + 0x54);
                let n = read_u32_or_invalid(uc, vec);
                let view = read_u32_or_invalid(uc, this + 0x1c) & 0xff;
                log::info!(
                    "PROCMAPENGINE render trace RCMapObjectsUpdate::Execute t entry at {:#x}: this={:#x} mo_vec={:#x} count={:#x} view={:#x}",
                    addr,
                    this,
                    vec,
                    n,
                    view
                );
            })
            .unwrap();
        let tex = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let addr = base_address + 0x0055_6170u32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = tex.fetch_add(1, Ordering::Relaxed);
                if count >= 60 {
                    return;
                }
                let mo = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let fbo = read_u32_or_invalid(uc, base_address + (0x0071_ae94u32 - ORIGINAL_BASE));
                log::info!(
                    "PROCMAPENGINE render trace bCheckForTextureUpdateBasedOnNewMO entry at {:#x}: map_object={:#x} fbo_handler={:#x}",
                    addr,
                    mo,
                    fbo
                );
            })
            .unwrap();
        let scene = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let addr = base_address + 0x0043_57c8u32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = scene.fetch_add(1, Ordering::Relaxed);
                if count >= 100 {
                    return;
                }
                let job = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE render trace SceneManager::MapObjectsUpdate entry at {:#x}: job={:#x} scene_mgr={:#x} type={:#x} reqstate={:#x} view={:#x} lod={:#x} a20={:#x} mos={:#x}",
                    addr,
                    job,
                    read_u32_or_invalid(uc, base_address + (0x0079_0788u32 - ORIGINAL_BASE)),
                    read_u32_or_invalid(uc, job + 0x24),
                    read_u32_or_invalid(uc, job + 0x28),
                    read_u32_or_invalid(uc, job + 0x1c) & 0xff,
                    read_u32_or_invalid(uc, job + 0x2c),
                    read_u32_or_invalid(uc, job + 0x20),
                    read_u32_or_invalid(uc, job + 0x54)
                );
            })
            .unwrap();
    }

    // r580: procmapengine's GL traffic is shader/state setup plus exactly the
    // per-frame `glClear(ccced9)` - the view render runs but no texture/FBO
    // work.  Inside `RCMapObjectsUpdate::Execute` the per-object branch points
    // decide that: 0x4d3870 sees `vcall+0x138` (r0: 1 = "new MO relevant for
    // this view"?), 0x4d3888 sees `vcall+0x1a8` (0 = skip texture update), and
    // 0x4d38a0 is the texture-update call itself.  Page-watch the function's
    // 4 KiB block (mid-TB narrow hooks lie, AGENTS.md) and log r0 + the object
    // each time, plus `DTM_Texture_Helper::bInit_Textures` (0x55817c) to see
    // whether the texture subsystem ever initialized.
    {
        let branch = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let page_lo = base_address + (0x004d_3000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                let label = match original {
                    0x004d_3870 => "after MO vcall+0x138 (1=relevant)",
                    0x004d_3888 => "after MO vcall+0x1a8 (0=skip)",
                    0x004d_38a0 => "GO texture update (bl bCheckForTextureUpdateBasedOnNewMO)",
                    0x004d_383c => "skip-object tail (vcall+0x1ac only)",
                    _ => return,
                };
                let count = branch.fetch_add(1, Ordering::Relaxed);
                if count >= 120 {
                    return;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE render trace MO-loop {label} at {original:#x}: r0={r0:#x} mo={r4:#x}"
                );
            })
            .unwrap();
        let texinit = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let addr = base_address + 0x0055_817cu32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = texinit.fetch_add(1, Ordering::Relaxed);
                if count >= 8 {
                    return;
                }
                log::info!(
                    "PROCMAPENGINE render trace DTM_Texture_Helper::bInit_Textures entry at {:#x}: this={:#x} lr={:#x}",
                    addr,
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32
                );
            })
            .unwrap();
    }

    // r582 (EMU_GPU_TRACE_ALL): procmapengine's complete GL vocabulary is
    // shader/state/texture-creation + `glClear(ccced9)` - no FBO binding and no
    // draw call ever, so the render-to-texture path is dead before GL.
    // `map_tclRenderer::PreRender` (0x52f184) is the gate: it returns 0 when
    // `DAT_00790820` (0x790820) - the per-view count from
    // `MasterHeightDataManager::ms_alstActiveNodes` (0x71aec8, 5 x 16-byte
    // {count, ?, ?, nodes-array} entries) - is zero, and only the non-zero
    // path calls s32SetStatesForRenderToTexture (0x52cb88) and draws.  Log the
    // view id (byte at mcp+5, r2) and both counts each PreRender run.
    {
        let pre = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let page_lo = base_address + (0x0052_f000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original != 0x0052_f184 {
                    return;
                }
                let count = pre.fetch_add(1, Ordering::Relaxed);
                if count >= 60 {
                    return;
                }
                let mcp = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let view = read_u32_or_invalid(uc, mcp.wrapping_add(5)) & 0xff;
                let dat = read_u32_or_invalid(uc, base_address + (0x0079_0820u32 - ORIGINAL_BASE));
                let entry = base_address + (0x0071_aec8u32 - ORIGINAL_BASE) + view * 0x10;
                let active = read_u32_or_invalid(uc, entry);
                let nodes = read_u32_or_invalid(uc, entry + 0xc);
                log::info!(
                    "PROCMAPENGINE render trace PreRender entry at {:#x}: view={:#x} nodes_to_process={:#x} alstActive.count={:#x} nodes_arr={:#x}",
                    original,
                    view,
                    dat,
                    active,
                    nodes
                );
            })
            .unwrap();
    }

    // r583: PreRender sees alstActive.count=0 every frame.  Its list-builder
    // `map_tclSceneManager::UpdateDrawLayerListsForDTM` (0x42a8ec) returns 0
    // immediately when the per-view scene root `scene_mgr + 0x44 + view*4` is
    // null; otherwise it walks the quadtree and fills the DTM node arrays.
    // Dump the gate value, the query rect (param_2 -> 4 ints) and the first
    // root words to tell "no scene root" from "empty tree".
    {
        let upd = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let addr = base_address + 0x0042_a8ecu32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = upd.fetch_add(1, Ordering::Relaxed);
                if count >= 40 {
                    return;
                }
                let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let mcp = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let rect = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let view = read_u32_or_invalid(uc, mcp.wrapping_add(5)) & 0xff;
                let root = read_u32_or_invalid(uc, this + 0x44 + view * 4);
                let rw = read_u32_or_invalid(uc, root);
                let r0 = read_u32_or_invalid(uc, rect);
                let r1 = read_u32_or_invalid(uc, rect + 4);
                let r2 = read_u32_or_invalid(uc, rect + 8);
                let r3 = read_u32_or_invalid(uc, rect + 0xc);
                log::info!(
                    "PROCMAPENGINE render trace UpdateDrawLayerListsForDTM entry at {:#x}: scene_mgr={:#x} view={:#x} root={:#x} root[0]={:#x} rect=[{:#x},{:#x},{:#x},{:#x}]",
                    addr,
                    this,
                    view,
                    root,
                    rw,
                    r0,
                    r1,
                    r2,
                    r3
                );
            })
            .unwrap();
    }

    // Zero r584 hits on UpdateDrawLayerListsForDTM: PreRender's DAT-gate skips
    // it, so `ms_alstActiveNodes` (0x71aec8) is only ever filled elsewhere -
    // candidate readers are `bUpdateLOD` (0x3bd940) and
    // `map_tclMapElm_DTMTextureDummy::Draw` (0x3cef80), and the DTM texture
    // node registration happens per texture creation.  Watch every write into
    // the 80-byte array (dispatch on the fetch address, per AGENTS.md a page
    // hook is the reliable form) and log those two entries.
    {
        // r603: the alstActiveNodes write-watch was removed to keep the host
        // address-space section budget down (phys_section_add assert at
        // r603); its question is answered (only _INIT_8 zeroes it).
        let upd = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let addr = base_address + 0x003b_d940u32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = upd.fetch_add(1, Ordering::Relaxed);
                if count >= 30 {
                    return;
                }
                let mcp = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let view = read_u32_or_invalid(uc, mcp.wrapping_add(5)) & 0xff;
                log::info!(
                    "PROCMAPENGINE render trace bUpdateLOD entry at {:#x}: mcp={:#x} view={:#x} lr={:#x}",
                    addr,
                    mcp,
                    view,
                    uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32
                );
            })
            .unwrap();
        let drw = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let addr = base_address + 0x003c_ef80u32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = drw.fetch_add(1, Ordering::Relaxed);
                if count >= 30 {
                    return;
                }
                log::info!(
                    "PROCMAPENGINE render trace DTMTextureDummy::Draw entry at {:#x}: this={:#x} r1={:#x} lr={:#x}",
                    addr,
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32
                );
            })
            .unwrap();
    }

    // r584/r585: PreRender returns 0 every frame (DTM node lists never
    // written since the _INIT_8 zeroing), `bUpdateLOD` and
    // `map_tclMapElm_DTMTextureDummy::Draw` never run, and
    // UpdateDrawLayerListsForDTM is only called from PreRender's
    // non-zero branch.  ViewRenderThread (0x530238) then computes
    // bSkipFrame = (PreProcess | PickFrame | PreRender) == 0 and only calls
    // map_tclRenderer::Execute (0x52d618, bl at 0x5304c0) plus
    // rl_tclSurface::Flip (0x5520d8) when a frame is NOT skipped.  Watch all
    // three to see whether the draw body runs at all and on which surface
    // (surface = renderer +0x40, r0 of Flip).
    {
        let exec = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let addr = base_address + 0x0052_d618u32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = exec.fetch_add(1, Ordering::Relaxed);
                if count >= 40 {
                    return;
                }
                let renderer = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let renderer_obj = read_u32_or_invalid(uc, renderer.wrapping_add(0x18));
                // ViewRenderThread passes layers = renderer + 0x14a (u8 layer
                // ids) and count = byte at renderer + 0x279; the per-layer
                // `map_tclMapElementList` table is at Execute's this + 8.
                let n_layers = read_u32_or_invalid(uc, renderer_obj.wrapping_add(0x279)) & 0xff;
                let layers = read_u32_or_invalid(uc, renderer_obj.wrapping_add(0x14a));
                let table = read_u32_or_invalid(uc, renderer.wrapping_add(8));
                log::info!(
                    "PROCMAPENGINE render trace Renderer::Execute entry at {:#x}: viewctl={:#x} renderer={:#x} surface={:#x} n_layers={:#x} layers={:#010x} elem_table={:#x} count={:#x}",
                    addr,
                    renderer,
                    renderer_obj,
                    read_u32_or_invalid(uc, renderer_obj.wrapping_add(0x40)),
                    n_layers,
                    layers,
                    table,
                    count
                );
            })
            .unwrap();
        let flip = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let addr = base_address + 0x0055_20d8u32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = flip.fetch_add(1, Ordering::Relaxed);
                if count >= 40 {
                    return;
                }
                log::info!(
                    "PROCMAPENGINE render trace Surface::Flip entry at {:#x}: surface={:#x} count={:#x}",
                    addr,
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                    count
                );
            })
            .unwrap();
        let dl = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let addr = base_address + 0x0052_ccc8u32 - ORIGINAL_BASE;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                // r630b: the flat 120-cap was exhausted within the first
                // few frames (43 layers per pass), hiding the tail where the
                // harvest fills lists.  Log only NON-EMPTY lists (plus a
                // sparse heartbeat of empty ones).
                let count = dl.fetch_add(1, Ordering::Relaxed);
                let list = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let layer = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let w0 = read_u32_or_invalid(uc, list);
                let w1 = read_u32_or_invalid(uc, list + 4);
                let w2 = read_u32_or_invalid(uc, list + 8);
                // DrawList iterates *(list+4) [count = u16 at list+0xa] and
                // calls vtable+0x38 per element - the element count is the
                // real emptiness test.
                let cnt = read_u32_or_invalid(uc, list + 0xa) & 0xffff;
                let vtbl = if w1 == 0 { 0 } else { read_u32_or_invalid(uc, w1) };
                let non_empty = cnt != 0;
                if !non_empty && (count % 4300 != 0 || count >= 47_300) {
                    return;
                }
                if non_empty && count >= 100_000 {
                    return;
                }
                log::info!(
                    "PROCMAPENGINE render trace DrawList entry at {:#x}: layer={:#x} elem_list={:#x} w0={:#x} w1={:#x} w2={:#x} count={:#x} t={}ms elems={} vtbl={:#x}",
                    addr,
                    layer,
                    list,
                    w0,
                    w1,
                    w2,
                    count,
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    cnt,
                    vtbl
                );
            })
            .unwrap();
    }

    // r637: the per-element draw call of the DrawList loop (verified ARM
    // disasm): 0x52ccc8 takes list=r1 (-> r6); the loop is gated by a global
    // `[0x52ce10]-referred word at +0x1c` being <= 7 (`bhi 0x52cda8` skips
    // every element - a candidate bail for the zero-GL mystery), then per
    // iteration loads r12 = [r0] / [r12] (r0 = *(list+4)) and tail-calls
    // [r12 + 0x38] at 0x52cd40 with r0 = *(list+4), r1 = r7 (3rd arg),
    // r2 = r8 (stack arg +0x28), r3 = r10 (2nd arg), r4 = index, r5 = stack
    // arg +0x2c (per-frame cursor the callee advances).  Mid-TB fetch address
    // -> page-range hook (unicorn bounded-hook quirk, see AGENTS.md).
    {
        let drawn = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let page_lo = base_address + (0x0052_c000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original != 0x0052_cd40u32 {
                    return;
                }
                let count = drawn.fetch_add(1, Ordering::Relaxed);
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                // 0x52cd18 `ldr r12,[r0]`, 0x52cd28 `cpy r0,r12`, 0x52cd34
                // `ldr r12,[r12]`: at 0x52cd40 r0 is the mid object and r12
                // its vtable, so the callee is `[[r0] + 0x38]`, not
                // `[r0 + 0x38]` (the r637 target=0xe1510003 reading was the
                // wrong slot).
                let vtbl = if r0 == 0 {
                    0
                } else {
                    read_u32_or_invalid(uc, r0)
                };
                let target = if vtbl == 0 || vtbl == u32::MAX {
                    0
                } else {
                    read_u32_or_invalid(uc, vtbl.wrapping_add(0x38))
                };
                if count >= 60 {
                    return;
                }
                log::info!(
                    "PROCMAPENGINE render trace MOUpd elem-draw t={}ms this={:#x} vtbl={:#x} a_r1={:#x} a_r2={:#x} a_r3={:#x} idx={} iter={:#x} target={:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    r0,
                    vtbl,
                    uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32,
                    target,
                    count
                );
            })
            .unwrap();
    }

    // r587: 43 layers iterate, every per-layer element list is empty
    // ({vtable, 0, 0}).  Those lists are filled per frame by
    // `map_tclSceneManager::DetermineMapObjects` (0x430f34), called from
    // PreProcess at 0x52e340/0x52e870, which also pushes map objects into the
    // DTM grid via `MasterHeightDataManager::s32InsertMO` (0x43cad8).
    // Watch the call sites (were they reached at all?), the entry (rect +
    // flag), and the insert count.
    {
        let det = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let page_lo = base_address + (0x0052_e000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original != 0x0052_e340 && original != 0x0052_e870 {
                    return;
                }
                let count = det.fetch_add(1, Ordering::Relaxed);
                if count >= 20 {
                    return;
                }
                let scene = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE render trace PreProcess->DetermineMapObjects call site {:#x}: scene={:#x} mcp={:#x} rect={:#x} lists={:#x} f8={:#x} f10={:#x} f14={:#x} f1c={:#x} f20={:#x} f30={:#x} f34={:#x} f38={:#x} roots=[{:#x},{:#x},{:#x},{:#x},{:#x}] rectv=[{:#x},{:#x},{:#x},{:#x}]",
                    original,
                    scene,
                    uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32,
                    read_u32_or_invalid(uc, scene + 8),
                    read_u32_or_invalid(uc, scene + 0x10),
                    read_u32_or_invalid(uc, scene + 0x14),
                    read_u32_or_invalid(uc, scene + 0x1c),
                    read_u32_or_invalid(uc, scene + 0x20),
                    read_u32_or_invalid(uc, scene + 0x30),
                    read_u32_or_invalid(uc, scene + 0x34),
                    read_u32_or_invalid(uc, scene + 0x38),
                    read_u32_or_invalid(uc, scene + 0x44),
                    read_u32_or_invalid(uc, scene + 0x48),
                    read_u32_or_invalid(uc, scene + 0x4c),
                    read_u32_or_invalid(uc, scene + 0x50),
                    read_u32_or_invalid(uc, scene + 0x54),
                    read_u32_or_invalid(uc, uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32),
                    read_u32_or_invalid(uc, uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32 + 4),
                    read_u32_or_invalid(uc, uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32 + 8),
                    read_u32_or_invalid(uc, uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32 + 0xc)
                );
                // r733: the `s32InsertMO` loop at 0x431884 walks
                // `[scene+0x30+bank*4] -> (count @ +0, mo** @ +0xc)` and is
                // skipped when the count is 0 (r732: zero insertions in
                // every run).  Log the per-bank cell-array object and its
                // count at call time so the empty-feed can be pinned to a
                // bank.
                for bank in 0..3u32 {
                    let cell_obj =
                        read_u32_or_invalid(uc, scene.wrapping_add(0x30 + bank * 4));
                    log::info!(
                        "PROCMAPENGINE render trace DetermineMapObjects bank={} cell_obj={:#x} count={:#x} arr={:#x}",
                        bank,
                        cell_obj,
                        read_u32_or_invalid(uc, cell_obj),
                        read_u32_or_invalid(uc, cell_obj.wrapping_add(0xc)),
                    );
                }
            })
            .unwrap();
        let ins = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        // r741: page dispatch (narrow entry hooks under-count, AGENTS.md).
        let page_lo = base_address + (0x0043_c000u32 - ORIGINAL_BASE);
        let addr = base_address + (0x0043_cad8u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(page_lo as u64, page_lo as u64 + 0xfff, move |uc, _, _| {
                if uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32 != addr {
                    return;
                }
                let count = ins.fetch_add(1, Ordering::Relaxed);
                if count >= 40 {
                    return;
                }
                log::info!(
                    "PROCMAPENGINE render trace s32InsertMO entry at {:#x}: map_object={:#x} count={:#x}",
                    addr,
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                    count
                );
            })
            .unwrap();
        // `DetermineMapObjects` only inserts MOs into the DTM grid when the
        // per-view cell array (scene+0x30+view*4, `*count` there) is non-zero;
        // those arrays are fed by the raw data blocks created from the
        // LoadBlocks shared-memory payload:
        // `map_tclRawMapDataCache::CreateNewRawDataBlock(uniqueID, type)`
        // (0x5166dc) + `AddNewRawDataBlock` (0x5164e0).  If these never run,
        // the 2 `result_blocks` per LoadBlocks answer die in shm and no map
        // element ever exists.
        {
            let create = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let addr = base_address + 0x0051_66dcu32 - ORIGINAL_BASE;
            unicorn
                .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                    let count = create.fetch_add(1, Ordering::Relaxed);
                    if count >= 40 {
                        return;
                    }
                    let uid = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let kind = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    // r735: which converter is feeding the cache?  34 blocks
                    // are created but CreateMapObjectsFromRawMapDataBlock ran
                    // once, so the callers of these 34 differ from the
                    // LoadBlocks job; log LR (as original VA) to pin them.
                    let lr = (uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32)
                        .wrapping_sub(base_address)
                        .wrapping_add(ORIGINAL_BASE);
                    let w0 = read_u32_or_invalid(uc, uid);
                    let w1 = read_u32_or_invalid(uc, uid + 4);
                    let w2 = read_u32_or_invalid(uc, uid + 8);
                    log::info!(
                        "PROCMAPENGINE render trace CreateNewRawDataBlock entry at {:#x}: uid={:#x} [{:#x},{:#x},{:#x}] type={:#x} count={:#x} lr={:#x}",
                        addr,
                        uid,
                        w0,
                        w1,
                        w2,
                        kind,
                        count,
                        lr
                    );
                })
                .unwrap();
            let add = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let addr = base_address + 0x0051_64e0u32 - ORIGINAL_BASE;
            unicorn
                .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                    let count = add.fetch_add(1, Ordering::Relaxed);
                    if count >= 40 {
                        return;
                    }
                    log::info!(
                        "PROCMAPENGINE render trace AddNewRawDataBlock entry at {:#x}: block={:#x} count={:#x}",
                        addr,
                        uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                        count
                    );
                })
                .unwrap();
        }
        // r590: raw data blocks ARE created/added (24x), yet the per-view
        // active-MO array stays empty.  `PerformLocalUpdateOfMapObjects`
        // (0x42c49c) is what refills it: it reads the per-[lod][idx] RC
        // MapObjectsUpdate job from scene+0x84+(idx+lod*4)*4 (second array at
        // +0xd4) and only fills when `job != null && job.cell_count(+0x28)
        // >= 2` (else trace "CentreRect loading INcomplete"), then calls
        // FillActiveMOArrayFromRCMapObjectsRequest.  Watch the entry with the
        // job lookup, and watch writes to both job-array rows.
        {
            let plc = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let addr = base_address + 0x0042_c49cu32 - ORIGINAL_BASE;
            unicorn
                .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                    let count = plc.fetch_add(1, Ordering::Relaxed);
                    if count >= 30 {
                        return;
                    }
                    let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let view = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xff;
                    let lod = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32 & 0xff;
                    let bitmask = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32 & 0xffff;
                    // 0x42c4e0: slot = this + 0x84 + (view*4 + lod)*4 (r3 is a
                    // bitmask, NOT part of the index).
                    let job_a = if lod < 4 && view < 5 {
                        read_u32_or_invalid(uc, this + 0x84 + (view * 4 + lod) * 4)
                    } else {
                        0xdead_beee
                    };
                    let cells_a = read_u32_or_invalid(uc, job_a.wrapping_add(0x28));
                    let job_b = if lod < 4 && view < 5 {
                        read_u32_or_invalid(uc, this + 0xd4 + (view * 4 + lod) * 4)
                    } else {
                        0xdead_beee
                    };
                    log::info!(
                        "PROCMAPENGINE render trace PerformLocalUpdate entry at {:#x}: scene={:#x} view={} lod={} bitmask={:#x} jobA={:#x} cellsA={:#x} jobB={:#x} count={:#x}",
                        addr,
                        this,
                        view,
                        lod,
                        bitmask,
                        job_a,
                        cells_a,
                        job_b,
                        count
                    );
                })
                .unwrap();
            // r603: the jobArray A/B/C write-watches were removed (the
            // SetCameraViewPointMOs entry probe shows the slot contents
            // directly) to keep the host section budget down.
            let setp = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let addr = base_address + 0x0043_2b44u32 - ORIGINAL_BASE;
            unicorn
                .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                    let count = setp.fetch_add(1, Ordering::Relaxed);
                    if count >= 40 {
                        return;
                    }
                    let array = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let job = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let view = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32 & 0xff;
                    // lod arrives on the stack (str r12,[sp] at 0x435920): r12.
                    // Thumb BL pushes nothing; read the arg via the caller's sp.
                    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    let lod = read_u32_or_invalid(uc, sp) & 0xffff_ffff;
                    let off = (lod + view * 4) * 4;
                    let slot = read_u32_or_invalid(uc, array.wrapping_add(off));
                    log::info!(
                        "PROCMAPENGINE render trace SetCameraViewPointMOs entry at {:#x}: array={:#x} job={:#x} view={} lod={:#x} slot={:#x} count={:#x}",
                        addr,
                        array,
                        job,
                        view,
                        lod,
                        slot,
                        count
                    );
                })
                .unwrap();
            // Else leg 0x435d78 (view>4 || lod>3) - page hook, dispatch on pc.
            // r613: extend the dispatch with the two render-tail gates found
            // in `MapObjectsUpdate` (0x4357c8).  0x435940 (`LAB_00435940`) runs
            // after `Combine(stored_job, new_job)` succeeded with r5 = stored
            // job: dump its +0x28 (request state) and +0x50 (block-load
            // complete flag, set by the answer leg at 0x4b4e18 region).
            // 0x436014 is the `state == 2` copy branch - the point where the
            // stored job's MO list is finally copied into the per-view active
            // "inside" arrays that `DetermineMapObjects` walks.
            let els = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            // r613: `FillActiveMOArrayFromRCMapObjectsRequest` (0x428eec) only
            // harvests when the stored job rect (+0x40..+0x4c centre, or
            // +0x30..+0x3c surround) covers the requested centre rect
            // (param_4) and its MO list is non-empty.  Dump those inputs at
            // entry (function entry - a reliable TB-start hook).
            {
                let fa = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                let addr = base_address + 0x0042_8eecu32 - ORIGINAL_BASE;
                unicorn
                    .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                        let job = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                        if job == 0 {
                            // PLU also calls for (view,lod) combos whose slot
                            // is empty; those trivially bail. Not interesting.
                            return;
                        }
                        let count = fa.fetch_add(1, Ordering::Relaxed);
                        if count >= 300 {
                            return;
                        }
                        let rect3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                        let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                        let rect4 = read_u32_or_invalid(uc, sp);
                        log::info!(
                            "PROCMAPENGINE render trace FillActiveMO entry t={}ms job={:#x} mask={:#x} mos={} jobrect=[{:#x},{:#x},{:#x},{:#x}] surround=[{:#x},{:#x}] centre=[{:#x},{:#x},{:#x},{:#x}] count={:#x}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            job,
                            (uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32) & 0xffff,
                            {
                                let arr = read_u32_or_invalid(uc, job + 0x54);
                                if arr == 0 {
                                    0
                                } else {
                                    read_u32_or_invalid(uc, arr)
                                }
                            },
                            if job == 0 { 0 } else { read_u32_or_invalid(uc, job + 0x40) },
                            if job == 0 { 0 } else { read_u32_or_invalid(uc, job + 0x44) },
                            if job == 0 { 0 } else { read_u32_or_invalid(uc, job + 0x48) },
                            if job == 0 { 0 } else { read_u32_or_invalid(uc, job + 0x4c) },
                            if rect3 == 0 { 0 } else { read_u32_or_invalid(uc, rect3) },
                            if rect3 == 0 { 0 } else { read_u32_or_invalid(uc, rect3 + 4) },
                            read_u32_or_invalid(uc, rect4),
                            read_u32_or_invalid(uc, rect4 + 4),
                            read_u32_or_invalid(uc, rect4 + 8),
                            read_u32_or_invalid(uc, rect4 + 0xc),
                            count
                        );
                    })
                    .unwrap();
            }
            // r622: three-way visibility into the same function's rect gate:
            // 0x4291b0 = centre rect (param_4) covered, 0x429014 = surround
            // rect (param_3/param_5) covered - both reach the MO loop - and
            // 0x428fd8 = the bail exit reached when no rect covers.  At the
            // bail r7 still carries the job and r10 param_3; param_4/param_5
            // live at [sp,#0x40]/[sp,#0x44].
            {
                let gate = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                let page_lo = base_address + (0x0042_8000u32 - ORIGINAL_BASE);
                let page_hi = page_lo + 0x1fff;
                let gt = gate.clone();
                unicorn
                    .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                        let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                        let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                        let tag = match original {
                            0x0042_91b0u32 => "fill-gate1",
                            0x0042_9014u32 => "fill-gate23",
                            0x0042_8fd8u32 => "fill-bail",
                            _ => return,
                        };
                        let n = gt.fetch_add(1, Ordering::Relaxed);
                        if n >= 60 {
                            return;
                        }
                        let job = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                        let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                        let rect3 = uc.reg_read(RegisterARM::R10).unwrap_or(0) as u32;
                        let rect4 = read_u32_or_invalid(uc, sp.wrapping_add(0x40));
                        let rect5 = read_u32_or_invalid(uc, sp.wrapping_add(0x44));
                        log::info!(
                            "PROCMAPENGINE render trace {} t={}ms job={:#x} jobc=[{:#x},{:#x},{:#x},{:#x}] jobs=[{:#x},{:#x},{:#x},{:#x}] p3=[{:#x},{:#x},{:#x},{:#x}] p4=[{:#x},{:#x},{:#x},{:#x}] p5=[{:#x},{:#x},{:#x},{:#x}] n={}",
                            tag,
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            job,
                            read_u32_or_invalid(uc, job + 0x40),
                            read_u32_or_invalid(uc, job + 0x44),
                            read_u32_or_invalid(uc, job + 0x48),
                            read_u32_or_invalid(uc, job + 0x4c),
                            read_u32_or_invalid(uc, job + 0x30),
                            read_u32_or_invalid(uc, job + 0x34),
                            read_u32_or_invalid(uc, job + 0x38),
                            read_u32_or_invalid(uc, job + 0x3c),
                            read_u32_or_invalid(uc, rect3),
                            read_u32_or_invalid(uc, rect3 + 4),
                            read_u32_or_invalid(uc, rect3 + 8),
                            read_u32_or_invalid(uc, rect3 + 0xc),
                            read_u32_or_invalid(uc, rect4),
                            read_u32_or_invalid(uc, rect4 + 4),
                            read_u32_or_invalid(uc, rect4 + 8),
                            read_u32_or_invalid(uc, rect4 + 0xc),
                            read_u32_or_invalid(uc, rect5),
                            read_u32_or_invalid(uc, rect5 + 4),
                            read_u32_or_invalid(uc, rect5 + 8),
                            read_u32_or_invalid(uc, rect5 + 0xc),
                            n
                        );
                    })
                    .unwrap();
            }
            // r623/r626: `GlobalViewRect_2D` builds the per-frame DOM rect
            // as corner offsets (mcp+0x14..+0x20 fixed-point corners times a
            // scale matrix) plus the absolute centre pair at mcp+0x38 (x) /
            // mcp+0x3c (y) - the r625 scan confirms which fields hold the
            // 0xc4f4xxxx stale values.  Those stayed frozen at the boot
            // default while the view's own camera copy (view+0x84/+0x88)
            // holds the renfi values; watch the centre pair (mcp = view
            // 0xda0bc4 + 0x280 = 0xda0e44) to find who must move it.
            {
                use unicorn_engine::unicorn_const::HookType;
                let mcp_c = 0xda0e78u32; // 0xda0bc4 + 0x280 + 0x38 (rect centre x/y)
                let _ = unicorn.add_mem_hook(
                    HookType::MEM_WRITE,
                    mcp_c as u64,
                    mcp_c as u64 + 7,
                    move |uc, _t, address, size, value| {
                        static COUNT: AtomicU32 = AtomicU32::new(0);
                        static LAST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                        // r623: the r623 cap of 60 was exhausted by the init
                        // chatter (screen-size writes below 0x10000) before
                        // the interesting window; skip the small values.
                        // r626b: r626's 300-cap was eaten by the identical
                        // every-frame stale writes - log only value CHANGES.
                        if value <= 0x1_0000 {
                            return true;
                        }
                        let changed = LAST.swap(value as u32, Ordering::Relaxed) != value as u32;
                        if !changed {
                            return true;
                        }
                        if COUNT.fetch_add(1, Ordering::Relaxed) >= 300 {
                            return true;
                        }
                        let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                        let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                        log::warn!(
                            "PROCMAPENGINE render trace MCP-centre WRITE t={}ms thread=[{}] off=+{:#x} size={} value={:#x} pc={:#x} lr={:#x}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            uc.get_data().inner.thread_id(),
                            address.wrapping_sub(mcp_c as u64),
                            size,
                            value,
                            pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE),
                            lr.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE) & !1,
                        );
                        true
                    },
                );
                // r626b: the Render commit `view+0x84/0x88 -> mcp+0x38/0x3c`
                // (the camera copy, 0x528e3c) runs only when the float at
                // view+0x2e0 is non-zero (guard `vcmp s15,#0 / beq 0x529914`
                // at 0x528e14).  Watch it to learn what makes it live.
                let _ = unicorn.add_mem_hook(
                    HookType::MEM_WRITE,
                    0xda0ea4u64, // 0xda0bc4 + 0x2e0
                    0xda0ea4u64 + 3,
                    move |uc, _t, _address, _size, value| {
                        static COUNT: AtomicU32 = AtomicU32::new(0);
                        static LAST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                        if LAST.swap(value as u32, Ordering::Relaxed) == value as u32 {
                            return true;
                        }
                        if COUNT.fetch_add(1, Ordering::Relaxed) >= 200 {
                            return true;
                        }
                        let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                        let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                        log::warn!(
                            "PROCMAPENGINE render trace VIEW-2e0 WRITE t={}ms thread=[{}] value={:#x} pc={:#x} lr={:#x}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            uc.get_data().inner.thread_id(),
                            value,
                            pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE),
                            lr.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE) & !1,
                        );
                        true
                    },
                );
                // r629: watch view+0x84 (the camera longitude copy the
                // Render commit reads) - the Milwaukee value keeps creeping
                // back between renfi commits; catch the writer red-handed.
                let _ = unicorn.add_mem_hook(
                    HookType::MEM_WRITE,
                    0xda0c48u64, // 0xda0bc4 + 0x84
                    0xda0c48u64 + 3,
                    move |uc, _t, _address, _size, value| {
                        // r639: stamp every Milwaukee restoration for the
                        // renfi echo-back (see CAM_MILWAUKEE_STAMP).
                        if value as u32 == 0xc4f4_5dfb {
                            CAM_MILWAUKEE_STAMP
                                .store(crate::libs::dapi::emu_t0_elapsed_ms() as u64, Ordering::Relaxed);
                        }
                        static COUNT: AtomicU32 = AtomicU32::new(0);
                        static LAST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                        if LAST.swap(value as u32, Ordering::Relaxed) == value as u32 {
                            return true;
                        }
                        if COUNT.fetch_add(1, Ordering::Relaxed) >= 200 {
                            return true;
                        }
                        let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                        let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                        log::warn!(
                            "PROCMAPENGINE render trace VIEW-84 WRITE t={}ms thread=[{}] value={:#x} pc={:#x} lr={:#x}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            uc.get_data().inner.thread_id(),
                            value,
                            pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE),
                            lr.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE) & !1,
                        );
                        true
                    },
                );
            }
            // r625: `map_tclMapMathUtils::GlobalViewRect_2D(rect, mcp, m2x2)`
            // (0x44b14c) builds the per-frame DOM rect from the true mcp's
            // fixed-point centre/corners at mcp+0x14..+0x20.  Dump its
            // arguments and that field block at entry to identify the real
            // mcp and where its centre comes from (the DOM-leg "mcp" turned
            // out to be a scene object carrying only 180/400/240 there).
            {
                let gr = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                let addr = base_address + 0x0044_b14cu32 - ORIGINAL_BASE;
                unicorn
                    .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                        let count = gr.fetch_add(1, Ordering::Relaxed);
                        if count >= 12 {
                            return;
                        }
                        let rect = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                        let mcp = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                        log::info!(
                            "PROCMAPENGINE render trace GlobalViewRect t={}ms thread=[{}] rect={:#x} mcp={:#x} f10={:#x} f14={:#x} f18={:#x} f1c={:#x} f20={:#x} lr={:#x} n={}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            uc.get_data().inner.thread_id(),
                            rect,
                            mcp,
                            read_u32_or_invalid(uc, mcp + 0x10),
                            read_u32_or_invalid(uc, mcp + 0x14),
                            read_u32_or_invalid(uc, mcp + 0x18),
                            read_u32_or_invalid(uc, mcp + 0x1c),
                            read_u32_or_invalid(uc, mcp + 0x20),
                            {
                                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                                lr.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE) & !1
                            },
                            count
                        );
                        // r625b: locate where the 0xc4f4xxxx centre ints
                        // actually live inside this mcp.
                        log::info!(
                            "PROCMAPENGINE render trace GlobalViewRect-scan t={}ms mcp={:#x} b24=[{:#x},{:#x},{:#x},{:#x}] b34=[{:#x},{:#x},{:#x},{:#x}] b44=[{:#x},{:#x},{:#x},{:#x}] b54=[{:#x},{:#x},{:#x},{:#x}]",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            mcp,
                            read_u32_or_invalid(uc, mcp + 0x24),
                            read_u32_or_invalid(uc, mcp + 0x28),
                            read_u32_or_invalid(uc, mcp + 0x2c),
                            read_u32_or_invalid(uc, mcp + 0x30),
                            read_u32_or_invalid(uc, mcp + 0x34),
                            read_u32_or_invalid(uc, mcp + 0x38),
                            read_u32_or_invalid(uc, mcp + 0x3c),
                            read_u32_or_invalid(uc, mcp + 0x40),
                            read_u32_or_invalid(uc, mcp + 0x44),
                            read_u32_or_invalid(uc, mcp + 0x48),
                            read_u32_or_invalid(uc, mcp + 0x4c),
                            read_u32_or_invalid(uc, mcp + 0x50),
                            read_u32_or_invalid(uc, mcp + 0x54),
                            read_u32_or_invalid(uc, mcp + 0x58),
                            read_u32_or_invalid(uc, mcp + 0x5c),
                            read_u32_or_invalid(uc, mcp + 0x60)
                        );
                    })
                    .unwrap();
            }
            let lab = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let cp2 = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let page_lo = base_address + (0x0043_5000u32 - ORIGINAL_BASE);
            // r614: the copy branch target 0x436014 sits on the NEXT page -
            // cover 0x435000..0x436fff so its dispatch fires too.
            let page_hi = page_lo + 0x1fff;
            let lab_c = lab.clone();
            let cp2_c = cp2.clone();
            let g590 = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let g591 = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let g592 = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let g590 = g590.clone();
            let g591 = g591.clone();
            let g592 = g592.clone();
            // r638: r627's copy fired because the INCOMING job lived long
            // enough for its completion flag (+0x50) to be set and forwarded
            // by Combine.  Since r635 each renfi drift rotates in a fresh
            // incoming job and new50 stays 0 - so watch +0x50 of every
            // incoming job we have ever seen (armed from the Combine probe)
            // and catch who does (or does not) raise it.
            let armed: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<u32>>> =
                std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
            let armed_c = armed.clone();
            unicorn
                .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                    let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                    let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                    match original {
                        0x0043_5d78u32 => {
                            let count = els.fetch_add(1, Ordering::Relaxed);
                            if count >= 20 {
                                return;
                            }
                            log::info!(
                                "PROCMAPENGINE render trace MapObjectsUpdate-else-leg 0x435d78 hit at {:#x} count={:#x}",
                                original,
                                count
                            );
                        }
                        0x0043_5db0u32 => {
                            let count = lab_c.fetch_add(1, Ordering::Relaxed);
                            // Right after `bl Combine`: r5 = stored (slot)
                            // job, r8 = incoming job; the reads below observe
                            // the forwarded state/flag (`Combine` writes
                            // stored+0x28/+0x50 from the incoming job before
                            // returning).
                            let job = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                            let newjob = uc.reg_read(RegisterARM::R8).unwrap_or(0) as u32;
                            let res = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                            // Arm a write watch on this incoming job's
                            // completion flag (once per object).
                            if newjob != 0 && (newjob >> 28) == 9 {
                                use unicorn_engine::unicorn_const::HookType;
                                let mut guard = armed_c.lock().unwrap();
                                if guard.len() < 48 && guard.insert(newjob) {
                                    drop(guard);
                                    let _ = uc.add_mem_hook(
                                        HookType::MEM_WRITE,
                                        newjob as u64 + 0x50,
                                        newjob as u64 + 0x53,
                                        move |uc2, _t, _a, size, value| {
                                            static FC: AtomicU32 = AtomicU32::new(0);
                                            if FC.fetch_add(1, Ordering::Relaxed) >= 80 {
                                                return true;
                                            }
                                            let pc = uc2.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                                            let lr = uc2.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                                            log::warn!(
                                                "PROCMAPENGINE render trace MOUpd flag50 WRITE t={}ms thread=[{}] job={:#x} size={} value={:#x} pc={:#x} lr={:#x}",
                                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                                uc2.get_data().inner.thread_id(),
                                                newjob,
                                                size,
                                                value,
                                                pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE),
                                                lr.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE) & !1,
                                            );
                                            true
                                        },
                                    );
                                }
                            }
                            if count >= 60 {
                                return;
                            }
                            log::info!(
                                "PROCMAPENGINE render trace MOUpd combined t={}ms stored={:#x} stored28={:#x} stored50={:#x} newjob={:#x} new28={:#x} new50={:#x} res={:#x} count={:#x}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                job,
                                read_u32_or_invalid(uc, job + 0x28),
                                read_u8_or_invalid(uc, job + 0x50),
                                newjob,
                                read_u32_or_invalid(uc, newjob + 0x28),
                                read_u8_or_invalid(uc, newjob + 0x50),
                                res,
                                count
                            );
                        }
                        0x0043_5940u32 => {}
                        // The three checks between `LAB_00435940` and the
                        // flag gate at 0x435978, any of which returns before
                        // the copy branch: GetView result, view->scale (the
                        // +0x304 slot) and `GetDetailLevel(scale) ==
                        // stored+0x2c`.  r5 = stored job on this leg.
                        0x0043_5950u32 => {
                            let n = g590.fetch_add(1, Ordering::Relaxed);
                            if n < 12 {
                                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                                log::info!("PROCMAPENGINE render trace MOUpd gate-view t={}ms view={:#x} n={}", crate::libs::dapi::emu_t0_elapsed_ms(), r0, n);
                            }
                        }
                        0x0043_595cu32 => {
                            let n = g591.fetch_add(1, Ordering::Relaxed);
                            if n < 12 {
                                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                                log::info!("PROCMAPENGINE render trace MOUpd gate-scale t={}ms scale_slot={:#x} n={}", crate::libs::dapi::emu_t0_elapsed_ms(), r0, n);
                            }
                        }
                        0x0043_5974u32 => {
                            let n = g592.fetch_add(1, Ordering::Relaxed);
                            if n < 40 {
                                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                                let job = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                                log::info!(
                                    "PROCMAPENGINE render trace MOUpd gate-lod t={}ms detail={} joblod={} stored50={:#x} stored28={:#x} n={}",
                                    crate::libs::dapi::emu_t0_elapsed_ms(),
                                    r0,
                                    read_u32_or_invalid(uc, job + 0x2c),
                                    read_u8_or_invalid(uc, job + 0x50),
                                    read_u32_or_invalid(uc, job + 0x28),
                                    n
                                );
                            }
                        }
                        0x0043_6014u32 => {
                            let count = cp2_c.fetch_add(1, Ordering::Relaxed);
                            if count >= 150 {
                                return;
                            }
                            let job = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                            let arr = read_u32_or_invalid(uc, job + 0x54);
                            let mos = if arr == 0 {
                                0
                            } else {
                                read_u32_or_invalid(uc, arr)
                            };
                            if mos > 0 {
                                MAP_MO_COPY_DONE.store(true, Ordering::Relaxed);
                            }
                            log::info!(
                                "PROCMAPENGINE render trace MOUpd copy-active t={}ms stored={:#x} stored28={:#x} mos={} count={:#x}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                job,
                                read_u32_or_invalid(uc, job + 0x28),
                                mos,
                                count
                            );
                        }
                        _ => {}
                    }
                })
                .unwrap();
            // r616: `DetermineMapObjects` (0x430f34) calls
            // `PerformLocalUpdateOfMapObjects` at 0x4317b8 - the harvest step
            // that feeds the per-frame element lists - but five branches skip
            // it: `mcp[0xaa] == 0` (update-request leg, lands at 0x4328b4),
            // the two active-inside arrays being NULL (0x432a44 / 0x432a50)
            // and the visible/POI arrays being NULL (0x43186c, shared exit).
            // 0x431718 marks the leg that proceeds towards the PLU call.
            // One hook spans both pages (0x431000..0x432fff).
            {
                let domskip = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                let page_lo = base_address + (0x0043_1000u32 - ORIGINAL_BASE);
                let page_hi = page_lo + 0x1fff;
                let sk = domskip.clone();
                unicorn
                    .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                        let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                        let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                        if !matches!(
                            original,
                            0x0043_1718u32
                                | 0x0043_17bcu32
                                | 0x0043_186cu32
                                | 0x0043_28b4u32
                                | 0x0043_2a44u32
                                | 0x0043_2a50u32
                        ) {
                            return;
                        }
                        let n = sk.fetch_add(1, Ordering::Relaxed);
                        if n >= 200 {
                            return;
                        }
                        let mcp = uc.reg_read(RegisterARM::R10).unwrap_or(0) as u32;
                        let view = read_u8_or_invalid(uc, mcp + 5);
                        if original == 0x0043_1718u32 {
                            // r624: GlobalViewRect_2D centres on mcp+0x1c/
                            // +0x20; read them live here so we see what the
                            // per-frame rect is built from.
                            let cx = read_u32_or_invalid(uc, mcp + 0x1c);
                            let cy = read_u32_or_invalid(uc, mcp + 0x20);
                            log::info!(
                                "PROCMAPENGINE render trace DOM-mcp-centre t={}ms mcp={:#x} centre=({:#x},{:#x})",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                mcp,
                                cx,
                                cy
                            );
                        }
                        if original == 0x0043_17bcu32 {
                            // Right after `bl PerformLocalUpdateOfMapObjects`:
                            // r0 = harvested element count.
                            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                            log::info!(
                                "PROCMAPENGINE render trace DOM-plu-ret t={}ms view={} elements={}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                view,
                                r0
                            );
                            return;
                        }
                        log::info!(
                            "PROCMAPENGINE render trace DOM-leg t={}ms leg={:#x} mcp={:#x} mcpaa={:#x} view={} n={}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            original,
                            mcp,
                            read_u8_or_invalid(uc, mcp + 0xaa),
                            view,
                            n
                        );
                    })
                    .unwrap();
            }
            // `FillActiveMOArrayFromRCMapObjectsRequest` (which alone fills the
            // per-view active-MO array DetermineMapObjects walks) runs only
            // when the stored job's +0x28 reqstate >= 2; every observed job
            // stores state 1 ("CentreRect loading INcomplete").  The state
            // comes from the MDM request at call time:
            // CreateRCJobMapObjectsUpdate(view_ptr, prio_ptr, state_ptr) -
            // dump the dereferenced values, plus GetLeftBlocks (0x5054f4, the
            // only other creator, called from IntHandleMapDataRequest) to see
            // whether the surround-block round ever runs.
            {
                let cj = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                let addr = base_address + 0x004c_5bacu32 - ORIGINAL_BASE;
                unicorn
                    .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                        let count = cj.fetch_add(1, Ordering::Relaxed);
                        if count >= 60 {
                            return;
                        }
                        let vp = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                        let pp = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                        let sp = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                        log::info!(
                            "PROCMAPENGINE render trace CreateRCJobMOUpd entry at {:#x}: req={:#x} view={:#x} prio={:#x} state={:#x} lr={:#x} count={:#x}",
                            addr,
                            vp.wrapping_sub(4),
                            read_u32_or_invalid(uc, vp) & 0xff,
                            read_u32_or_invalid(uc, pp) & 0xff,
                            read_u32_or_invalid(uc, sp) & 0xffff_ffff,
                            uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                            count
                        );
                    })
                    .unwrap();
                let glb = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                let addr = base_address + 0x0050_54f4u32 - ORIGINAL_BASE;
                unicorn
                    .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                        let count = glb.fetch_add(1, Ordering::Relaxed);
                        if count >= 30 {
                            return;
                        }
                        let req = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                        let a1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                        let a2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                        log::info!(
                            "PROCMAPENGINE render trace GetLeftBlocks entry at {:#x}: req={:#x} type={:#x} state74={:#x} out1={:#x} c1={:#x} out2={:#x} c2={:#x} count={:#x}",
                            addr,
                            req,
                            read_u32_or_invalid(uc, req) & 0xff,
                            read_u32_or_invalid(uc, req + 0x74),
                            a1,
                            read_u32_or_invalid(uc, a1),
                            a2,
                            read_u32_or_invalid(uc, a2),
                            count
                        );
                    })
                    .unwrap();
                // The camera-viewpoint request is a static at 0x78fcd4 (its
                // +0x74 state stays 1 forever -> every RC job is created with
                // reqstate 1 -> FillActiveMOArrayFromRCMapObjectsRequest never
                // runs, gate "CentreRect loading INcomplete" wants >= 2).
                // Watch the state dword and a window around it to find the
                // promotion writer (or prove there is none in this state).
                {
                    use unicorn_engine::unicorn_const::HookType;
                    let lo = 0x0078_fd3cu32 as u64;
                    let hi = lo + 0x20;
                    let st = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                    let res = unicorn.add_mem_hook(
                        HookType::MEM_WRITE,
                        lo,
                        hi,
                        move |uc, _t, address, _size, value| {
                            let count = st.fetch_add(1, Ordering::Relaxed);
                            if count >= 300 {
                                return true;
                            }
                            let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                            let original =
                                pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                            log::info!(
                                "PROCMAPENGINE render trace req-state WRITE t={}ms addr={:#x} value={:#x} pc={:#x} lr={:#x}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                address,
                                value,
                                original,
                                uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32
                            );
                            true
                        },
                    );
                    if res.is_err() {
                        log::warn!("PROCMAPENGINE: req-state write watch failed");
                    }
                }
                // Block bookkeeping window: +0x1f4/+0x1f8 (LeftToFetch, must
                // drain back to -1 for the 1->2 promotion) and +0x200
                // (MDMRefCtr, u16, gate `== 0`) of the static request.
                {
                    use unicorn_engine::unicorn_const::HookType;
                    let lo = 0x0078_fec4u32 as u64;
                    let hi = lo + 0x14;
                    let st = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                    let res = unicorn.add_mem_hook(
                        HookType::MEM_WRITE,
                        lo,
                        hi,
                        move |uc, _t, address, _size, value| {
                            let count = st.fetch_add(1, Ordering::Relaxed);
                            if count >= 400 {
                                return true;
                            }
                            let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                            let original =
                                pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                            log::info!(
                                "PROCMAPENGINE render trace req-fetch WRITE t={}ms addr={:#x} value={:#x} pc={:#x} lr={:#x}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                address,
                                value,
                                original,
                                uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32
                            );
                            true
                        },
                    );
                    if res.is_err() {
                        log::warn!("PROCMAPENGINE: req-fetch write watch failed");
                    }
                }
                // r610: `LockFreeFastMapChannel` (0x504418) hands out a slot of
                // `ms_asFastMapChannelInfo` (0x78f81c, 3 entries, stride 0x10,
                // busy word at +0xc, channel u16 at +0x8, tag u16 at +0xa) and
                // returns 0xff once all three are busy.  r609b proved the three
                // slots leak - by the time the block-id round completed
                // (t=90.6 s) every slot was stuck and the LoadBlocks leg died
                // with "GetBlockIDs pending Job not finished" forever.  Watch
                // every write to the table to find the occupation sites and the
                // missing release.
                {
                    use unicorn_engine::unicorn_const::HookType;
                    let lo = 0x0078_f81cu32 as u64;
                    let hi = lo + 0x2f;
                    let st = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                    let res = unicorn.add_mem_hook(
                        HookType::MEM_WRITE,
                        lo,
                        hi,
                        move |uc, _t, address, _size, value| {
                            let count = st.fetch_add(1, Ordering::Relaxed);
                            if count >= 300 {
                                return true;
                            }
                            let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                            let original =
                                pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                            log::info!(
                                "PROCMAPENGINE render trace fmchan WRITE t={}ms addr={:#x} value={:#x} pc={:#x} lr={:#x} count={}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                address,
                                value,
                                original,
                                uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                                count
                            );
                            true
                        },
                    );
                    if res.is_err() {
                        log::warn!("PROCMAPENGINE: fmchan write watch failed");
                    }
                }
                // r598: after the promotion marker (0xefffffff) neither
                // +0x1f4 nor +0x1f8 ever receives a block count - the block-id
                // round inside `map_tclMapDataManager::GetBlockIDs` (0x507de0,
                // IntHandleMapDataRequest line 123) is not completing.  Probe
                // entry and the DAPI-call return value.
                {
                    let gbids = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                    let page_lo = base_address + (0x0050_7000u32 - ORIGINAL_BASE);
                    let page_hi = page_lo + 0xfff;
                    {
                        let lo = page_lo;
                        let cnt = std::sync::Arc::clone(&gbids);
                        unicorn
                            .add_code_hook(lo as u64, (page_hi) as u64, move |uc, _, _| {
                                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                                let original =
                                    pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                                if original != 0x0050_7de0u32 {
                                    return;
                                }
                                let count = cnt.fetch_add(1, Ordering::Relaxed);
                                if count >= 40 {
                                    return;
                                }
                                let req = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                                log::info!(
                                    "PROCMAPENGINE render trace MDM::GetBlockIDs entry at {:#x}: req={:#x} state74={:#x} f1f4={:#x} f1f8={:#x} f200={:#x} lr={:#x} count={:#x}",
                                    original,
                                    req,
                                    read_u32_or_invalid(uc, req + 0x74),
                                    read_u32_or_invalid(uc, req + 0x1f4),
                                    read_u32_or_invalid(uc, req + 0x1f8),
                                    read_u32_or_invalid(uc, req + 0x200),
                                    uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                                    count
                                );
                                let req = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                                let lodslot = read_u32_or_invalid(uc, req + 0x74);
                                let n = if lodslot < 8 { lodslot } else { 0 };
                                let cnt114 = read_u32_or_invalid(uc, req + n * 0x10 + 0x114);
                                let arr = read_u32_or_invalid(uc, req + n * 0x10 + 0x120);
                                let id0 = read_u32_or_invalid(uc, arr);
                                let id0b = read_u32_or_invalid(uc, arr.wrapping_add(4));
                                let id1 = read_u32_or_invalid(uc, arr.wrapping_add(8));
                                let id1b = read_u32_or_invalid(uc, arr.wrapping_add(0xc));
                                let f1f8 = read_u32_or_invalid(uc, req + 0x1f8);
                                let cache = read_u32_or_invalid(
                                    uc,
                                    base_address + (0x0079_0704u32 - ORIGINAL_BASE),
                                );
                                let ccount = read_u32_or_invalid(uc, cache);
                                log::info!(
                                    "PROCMAPENGINE render trace MDM::GetBlockIDs rows lod={} cnt114={:#x} arr120={:#x} id0={:#x},{:#x} id1={:#x},{:#x} rawcache={:#x} rawcount={:#x} f1f8={:#x}",
                                    n,
                                    cnt114,
                                    arr,
                                    id0,
                                    id0b,
                                    id1,
                                    id1b,
                                    cache,
                                    ccount,
                                    f1f8,
                                );
                            })
                            .unwrap();
                    }
                    // Return site (bl at 0x511fd8 -> 0x511fdc) and a one-shot
                    // armed trace of the very first GetBlockIDs body to see
                    // which error leg it leaves through.
                    {
                        let ret = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                        let page_lo = base_address + (0x0051_1000u32 - ORIGINAL_BASE);
                        let page_hi = page_lo + 0xfff;
                        unicorn
                            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                                let original =
                                    pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                                if original != 0x0051_1fdcu32 {
                                    return;
                                }
                                let count = ret.fetch_add(1, Ordering::Relaxed);
                                if count >= 40 {
                                    return;
                                }
                                log::info!(
                                    "PROCMAPENGINE render trace MDM::GetBlockIDs return r0={:#x} count={:#x}",
                                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                                    count
                                );
                            })
                            .unwrap();
                    }
                    let armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let steps = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                    // The synch-data response join job: DAPI's fn 0x103 answer
                    // must lead through `map_tclJobSDMHandleGetBlockIDs::
                    // Execute` (0x4bfd08) - it decrements req+0x1f8 per block
                    // (0x4c0184) and sets it to -1 when complete (0x4c040c),
                    // which arms the 1->2 state promotion.
                    {
                        let sdm = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                        let addr = base_address + 0x004b_fd08u32 - ORIGINAL_BASE;
                        unicorn
                            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                                let count = sdm.fetch_add(1, Ordering::Relaxed);
                                if count >= 40 {
                                    return;
                                }
                                let thisp = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                                log::info!(
                                    "PROCMAPENGINE render trace SDMHandleGetBlockIDs::Execute entry at {:#x}: job={:#x} r4={:#x} count={:#x}",
                                    addr,
                                    thisp,
                                    read_u32_or_invalid(uc, thisp + 4),
                                    count
                                );
                            })
                            .unwrap();
                    }
                    let page_lo = base_address + (0x0050_7000u32 - ORIGINAL_BASE);
                    let page_hi = page_lo + 0xfff;
                    let page_lo2 = base_address + (0x0050_8000u32 - ORIGINAL_BASE);
                    let page_hi2 = page_lo2 + 0xfff;
                    for (lo, hi) in [(page_lo, page_hi), (page_lo2, page_hi2)] {
                        let armed = std::sync::Arc::clone(&armed);
                        let steps = std::sync::Arc::clone(&steps);
                        let start = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                        let _start = start;
                        unicorn
                            .add_code_hook(lo as u64, hi as u64, move |uc, _, _| {
                                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                                let original =
                                    pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                                if original == 0x0050_7de0u32 {
                                    // arm only for the first invocation
                                    if steps.load(Ordering::Relaxed) == 0 {
                                        armed.store(true, Ordering::Relaxed);
                                    }
                                    return;
                                }
                                if !armed.load(Ordering::Relaxed) {
                                    return;
                                }
                                let n = steps.fetch_add(1, Ordering::Relaxed);
                                if n >= 700 {
                                    armed.store(false, Ordering::Relaxed);
                                    return;
                                }
                                if original < 0x0050_7de0u32 || original > 0x0050_8490u32 {
                                    armed.store(false, Ordering::Relaxed);
                                    return;
                                }
                                log::info!(
                                    "PROCMAPENGINE gbids-step {:#010x} (left {})",
                                    original,
                                    700 - n
                                );
                            })
                            .unwrap();
                    }
                }
                // The request static (0x78fcd4) tracks outstanding work at
                // +0x1f4/+0x1f8 ("LeftToFetch: %d/%d") and per-state block
                // rows at +state*0x10+0x114.  Dump them on every
                // IntHandleMapDataRequest to see whether LeftToFetch ever
                // drains to 0 (which should promote +0x74 past 1).
                {
                    let ihm = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                    let page_lo = base_address + (0x0051_1000u32 - ORIGINAL_BASE);
                    let page_hi = page_lo + 0xfff;
                    unicorn
                        .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                            let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                            let original =
                                pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                            if original != 0x0051_16d4u32 {
                                return;
                            }
                            let count = ihm.fetch_add(1, Ordering::Relaxed);
                            if count >= 80 {
                                return;
                            }
                            // The camera-viewpoint request is the static at
                            // 0x78fcd4; dump it directly (r0 is a byte arg).
                            let req_static = 0x0078_fcd4u32;
                            log::info!(
                                "PROCMAPENGINE render trace IntHandleMapDataRequest entry r0={:#x} state74={:#x} f114={:#x} f118={:#x} f1f4={:#x} f1f8={:#x} f200={:#x} count={:#x}",
                                uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                                read_u32_or_invalid(uc, req_static + 0x74),
                                read_u32_or_invalid(uc, req_static + 0x114),
                                read_u32_or_invalid(uc, req_static + 0x118),
                                read_u32_or_invalid(uc, req_static + 0x1f4),
                                read_u32_or_invalid(uc, req_static + 0x1f8),
                                read_u32_or_invalid(uc, req_static + 0x200),
                                count
                            );
                        })
                        .unwrap();
                }
                // r606: promotion 1->2 requires BOTH LeftToFetch fields back to
                // -1.  +0x1f4 (visible) completes, but +0x1f8 (synch/elevation)
                // is only ever armed through `map_tclSynchDataManager::
                // s32GetBlockIDs` (0x519548): node -> m_apoDataLoaders[node]
                // -> vtable+0x10 (PyramidLoader::s32GetBlockIDs 0x58bf6c),
                // which is disabled unless its file probe in bInit succeeded
                // (`this[4] == 1`).  Dump node, loader, enable flag, device
                // string, the vtable targets and the return value.
                {
                    let syn = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                    let ret = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                    let page_lo = base_address + (0x0051_9000u32 - ORIGINAL_BASE);
                    let page_hi = page_lo + 0xfff;
                    let loaders = base_address + (0x0079_0744u32 - ORIGINAL_BASE);
                    unicorn
                        .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                            let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                            let original =
                                pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                            if original == 0x0051_9548u32 {
                                let count = syn.fetch_add(1, Ordering::Relaxed);
                                if count >= 12 {
                                    return;
                                }
                                let np = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                                let node = read_u32_or_invalid(uc, np) & 0xff;
                                let loader =
                                    read_u32_or_invalid(uc, loaders.wrapping_add(node * 4));
                                let vptr = read_u32_or_invalid(uc, loader);
                                let en = read_u32_or_invalid(uc, loader.wrapping_add(4)) & 0xff;
                                let devp = read_u32_or_invalid(uc, loader.wrapping_add(8));
                                let f10 = read_u32_or_invalid(uc, vptr.wrapping_add(0x10));
                                let f2c = read_u32_or_invalid(uc, vptr.wrapping_add(0x2c));
                                let f30 = read_u32_or_invalid(uc, vptr.wrapping_add(0x30));
                                log::info!(
                                    "PROCMAPENGINE render trace SynchGetBlockIDs entry node={node} loader={loader:#x} enabled={en} device={devp:#x} vtable={vptr:#x} vt+0x10={f10:#x} vt+0x2c={f2c:#x} vt+0x30={f30:#x} count={count}",
                                );
                            } else if original == 0x0051_9578u32 || original == 0x0051_957cu32 {
                                let count = ret.fetch_add(1, Ordering::Relaxed);
                                if count >= 12 {
                                    return;
                                }
                                log::info!(
                                    "PROCMAPENGINE render trace SynchGetBlockIDs return site={original:#x} r0={:#x} count={count}",
                                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                                );
                            }
                        })
                        .unwrap();
                }
                // r608: the request-state zeroing at 0x50ccf0 (inside
                // PerformMapDataRequest) destroys accepted 34-block rounds
                // (row + +0x1f4 written, RCMOUS queued) one second after they
                // land.  One-shot armed backtrace: the first time the reset's
                // caller landing pad 0x50ce0c is reached, log the fetch
                // sequence backwards through PerformMapDataRequest's callers
                // (return-address chain via the frames' saved lr is hard, so
                // just dump the step trace backwards is impossible - instead
                // arm a step tracer over 0x50c000-0x50dfff and stop at the
                // first fetch outside, whose lr is the culprit).
                {
                    let armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let steps = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                    for page in [0x0050_c000u32, 0x0050_d000u32] {
                        let lo = base_address + (page - ORIGINAL_BASE);
                        let armed = std::sync::Arc::clone(&armed);
                        let steps = std::sync::Arc::clone(&steps);
                        unicorn
                            .add_code_hook(lo as u64, (lo + 0xfff) as u64, move |uc, _, _| {
                                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                                let original =
                                    pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                                if original == 0x0050_ccf0u32 && steps.load(Ordering::Relaxed) == 0
                                {
                                    armed.store(true, Ordering::Relaxed);
                                    log::info!(
                                        "PROCMAPENGINE render trace req-state reset executing t={}ms lr={:#x}",
                                        crate::libs::dapi::emu_t0_elapsed_ms(),
                                        uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32
                                    );
                                    return;
                                }
                                if !armed.load(Ordering::Relaxed) {
                                    return;
                                }
                                let n = steps.fetch_add(1, Ordering::Relaxed);
                                if n >= 400 {
                                    armed.store(false, Ordering::Relaxed);
                                    return;
                                }
                                if original < 0x0050_c000u32 || original > 0x0050_dfffu32 {
                                    armed.store(false, Ordering::Relaxed);
                                    log::info!(
                                        "PROCMAPENGINE reset-step EXIT {:#010x} lr={:#x}",
                                        original,
                                        uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32
                                    );
                                    return;
                                }
                                log::info!(
                                    "PROCMAPENGINE reset-step {:#010x} lr={:#x} (left {})",
                                    original,
                                    uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                                    400 - n
                                );
                            })
                            .unwrap();
                    }
                }
                // r606: the request state climbs 0->1 then is reset to 0
                // (PerformMapDataRequest 0x50ccf0 / PerformResetMapDataRequest
                // 0x50ec0c) over and over - never reaches 2 which
                // FillActiveMOArrayFromRCMapObjectsRequest requires.  Both are
                // fed by UpdateMapDataRequest (0x50494c), called per nav
                // position (vHandleNavPosition), per camera change and from
                // Render/DetermineMapObjects.  Page-dispatch both entry points
                // with args to see what re-arms (and resets) the request.
                {
                    let upd = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                    let page_lo = base_address + (0x0050_4000u32 - ORIGINAL_BASE);
                    let page_hi = page_lo + 0xfff;
                    unicorn
                        .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                            let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                            let original =
                                pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                            if original != 0x0050_494cu32 {
                                return;
                            }
                            let count = upd.fetch_add(1, Ordering::Relaxed);
                            if count >= 160 {
                                return;
                            }
                            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                            log::info!(
                                "PROCMAPENGINE render trace UpdateMapDataRequest entry t={}ms prio={:#x} view={:#x} scale_bits={:#x} r3={:#x} rect=[{:#x},{:#x},{:#x},{:#x}] lr={:#x} count={:#x}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                                uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xff,
                                uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32,
                                uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32,
                                read_u32_or_invalid(uc, sp + 4),
                                read_u32_or_invalid(uc, sp + 8),
                                read_u32_or_invalid(uc, sp + 0xc),
                                read_u32_or_invalid(uc, sp + 0xc),
                                uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                                count
                            );
                        })
                        .unwrap();
                    let perf = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                    let page_lo = base_address + (0x0050_c000u32 - ORIGINAL_BASE);
                    let page_hi = page_lo + 0xfff;
                    unicorn
                        .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                            let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                            let original =
                                pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                            if original != 0x0050_c93cu32 {
                                return;
                            }
                            let count = perf.fetch_add(1, Ordering::Relaxed);
                            if count >= 160 {
                                return;
                            }
                            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                            log::info!(
                                "PROCMAPENGINE render trace PerformMapDataRequest entry t={}ms prio={:#x} view={:#x} scale_bits={:#x} r3={:#x} rect=[{:#x},{:#x},{:#x},{:#x}] lr={:#x} count={:#x}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                                uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32 & 0xff,
                                uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32,
                                uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32,
                                read_u32_or_invalid(uc, sp + 4),
                                read_u32_or_invalid(uc, sp + 8),
                                read_u32_or_invalid(uc, sp + 0xc),
                                read_u32_or_invalid(uc, sp + 0xc),
                                uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
                                count
                            );
                        })
                        .unwrap();
                }
            }
        }
    }

    // Message-map router `PortControl_CCAClientHandlerBase::vDispatchMessage`
    // (0x592454): keys are (u16 svc @inner+0x14, u16 fn @inner+0+0x18) pairs
    // (the map's static seed table at 0x713ea8 holds {0x70006, 0x140006,
    // 0x1010026 -> vHandleOpenFastMapChannel, 0x1030026 -> vHandleGetBlockIDs,
    // 0x1040026 -> vHandleLoadBlocks, 0x201/0x2020026, 0x4010026 ->
    // vHandleDatablocksChanged}). r578 saw 25 fn=0x104 answers arrive in
    // ail_bHandleMsgServiceData but only 2 vHandleLoadBlocks runs, so log every
    // dispatch with a svc-0x26 key plus the unmatched (empty-map-walk) exit.
    {
        let dispatch_watch = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let page_lo = base_address + (0x0059_2000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                let entry_hit = original == 0x0059_2454;
                // vOnUnhandledMessage (0x5923e8): reached from the walk's
                // empty-tree arm (0x5924e8) - the message is deleted here.
                let unhandled_hit = original == 0x0059_23e8;
                if !entry_hit && !unhandled_hit {
                    return;
                }
                let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let msg = if entry_hit {
                    uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32
                } else {
                    // vOnUnhandledMessage(&copy): same r1 slot.
                    uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32
                };
                let inner = read_u32_or_invalid(uc, msg.wrapping_add(4));
                let svc = read_u16_or_invalid(uc, inner.wrapping_add(0x14));
                let fnid = read_u16_or_invalid(uc, inner.wrapping_add(0x18));
                if svc != 0x26 {
                    return;
                }
                let count = dispatch_watch.fetch_add(1, Ordering::Relaxed);
                if count >= 300 {
                    return;
                }
                let opcode = read_u32_or_invalid(uc, inner.wrapping_add(0x1a)) & 0xff;
                let mdm_ref = read_u16_or_invalid(uc, inner.wrapping_add(0x1c));
                log::info!(
                    "PROCMAPENGINE map-data trace vDispatchMessage {} at {:#x}: this={:#x} svc=0x{:x} fn=0x{:x} opcode={:#x} mdm_ref={:#x}",
                    if entry_hit { "entry" } else { "UNHANDLED" },
                    original,
                    this,
                    svc,
                    fnid,
                    opcode,
                    mdm_ref
                );
            })
            .unwrap();
    }

    for (name, call_offset, result_offset) in [
        (
            "IntGetBlockIDs",
            MAP_DATA_INT_GET_BLOCK_IDS_CALL,
            MAP_DATA_INT_GET_BLOCK_IDS_RESULT,
        ),
        (
            "DapiGetBlockIDs",
            MAP_DATA_DAPI_GET_BLOCK_IDS_CALL,
            MAP_DATA_DAPI_GET_BLOCK_IDS_RESULT,
        ),
        (
            "DapiLoadBlocks",
            MAP_DATA_DAPI_LOAD_BLOCKS_CALL,
            MAP_DATA_DAPI_LOAD_BLOCKS_RESULT,
        ),
    ] {
        let call_addr = base_address + call_offset;
        let result_addr = base_address + result_offset;
        unicorn
            .add_code_hook(call_addr as u64, call_addr as u64, move |uc, _, _| {
                let count = MAP_DATA_DAPI_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 200 {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE map-data trace {} call at {:#x}: r0={:#x} r1={:#x} r2={:#x} r3={:#x}",
                        name,
                        call_addr,
                        r0,
                        r1,
                        r2,
                        r3
                    );
                }
            })
            .unwrap();
        unicorn
            .add_code_hook(result_addr as u64, result_addr as u64, move |uc, _, _| {
                let count = MAP_DATA_DAPI_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 200 {
                    let result = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE map-data trace {} result at {:#x}: r0={:#x}",
                        name,
                        result_addr,
                        result
                    );
                }
            })
            .unwrap();
    }

    for (name, offset) in [
        ("direct", MAP_DATA_MAINLOOP_GET_ELEMENT_RESULT),
        ("wait", MAP_DATA_MAINLOOP_WAIT_GET_ELEMENT_RESULT),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = MAP_DATA_MAINLOOP_STATE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                let job = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                // r438: the request=4 posfi job is delivered to the mainloop's
                // EnqueueElements (kernel-verified) but never executed, and the
                // plain 100-entry budget went blind long before the fabricated
                // position request arrives - track the posfi pointer itself and
                // sample the rest.
                let posfi = MAP_DATA_POSFI_JOB.load(Ordering::Relaxed);
                let job_type_now = read_u32_or_invalid(uc, job + 8);
                let interesting = job_type_now != 0x45 && job_type_now != 0x83;
                if !(posfi != 0 && job == posfi) && !interesting && count % 50 != 0 {
                    return;
                }
                if job_type_now == 0x45 && count % 50 != 0 && !(posfi != 0 && job == posfi) {
                    return;
                }
                let state = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                let thread = uc.get_data().inner.thread_id();
                let job_type = read_u32_or_invalid(uc, job + 8);
                let queue_slot = read_u32_or_invalid(uc, base_address + 0x0050_399c - ORIGINAL_BASE);
                let queue = read_u32_or_invalid(uc, queue_slot);
                log::info!(
                    "PROCMAPENGINE map-data trace {} GetElement at {:#x}: job={:#x} queue_state={:#x} job_type={:#x} thread=[{}] count={count} queue={:#x} qstate={} qcount={} head={:#x} posfi={posfi:#x}",
                    name,
                    addr,
                    job,
                    state,
                    job_type,
                    thread,
                    queue,
                    read_u32_or_invalid(uc, queue),
                    read_u32_or_invalid(uc, queue + 4),
                    read_u32_or_invalid(uc, queue + 8),
                );
            })
            .unwrap();
    }

    for (name, offset) in [
        ("direct", MAP_DATA_MAINLOOP_JOB_CAN_EXECUTE),
        ("wait", MAP_DATA_MAINLOOP_WAIT_JOB_CAN_EXECUTE),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = MAP_DATA_MAINLOOP_JOB_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 100 {
                    let job = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    let vtable = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                    let target = read_u32_or_invalid(uc, vtable + 0x24);
                    log::info!(
                        "PROCMAPENGINE map-data trace {} job can-execute at {:#x}: job={:#x} vtable={:#x} target={:#x}",
                        name,
                        addr,
                        job,
                        vtable,
                        target
                    );
                }
            })
            .unwrap();
    }
}

fn add_render_control_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let queue_state_addr = base_address + RENDER_CONTROL_QUEUE_STATE;
    let add_job_addr = base_address + RENDER_CONTROL_ADD_JOB;
    unicorn
        .add_code_hook(add_job_addr as u64, add_job_addr as u64, move |uc, _, _| {
            let count = RENDER_CONTROL_ADD_JOB_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 50 {
                let job = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let vtable = read_u32_or_invalid(uc, job);
                log::info!(
                    "PROCMAPENGINE render-control trace AddJob at {:#x}: job={:#x} vtable={:#x}",
                    add_job_addr,
                    job,
                    vtable
                );
            }
        })
        .unwrap();

    for (name, offset) in [
        ("JobRCStartQueue::Execute", RC_JOB_START_QUEUE_EXECUTE),
        ("JobRCStartQueueAndRenderView::Execute", RC_JOB_START_AND_RENDER_EXECUTE),
        ("JobRCRenderPixmapView::Execute", RC_JOB_RENDER_PIXMAP_EXECUTE),
        ("JobRCCreateView::Execute", RC_JOB_CREATE_VIEW_EXECUTE),
        ("JobRCRenderView::Execute", RC_JOB_RENDER_VIEW_EXECUTE),
    ] {
        let job_addr = base_address + offset;
        unicorn
            .add_code_hook(job_addr as u64, job_addr as u64, move |uc, _, _| {
                let count = RC_JOB_EXECUTE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 100 {
                    let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE render-control trace {} at {:#x}: this={:#x}",
                        name,
                        job_addr,
                        this
                    );
                }
            })
            .unwrap();
    }

    for (name, offset) in [
        (
            "RenderPixmapView instance result",
            RC_JOB_RENDER_PIXMAP_INSTANCE_RESULT,
        ),
        ("RenderPixmapView view result", RC_JOB_RENDER_PIXMAP_VIEW_RESULT),
    ] {
        let step_addr = base_address + offset;
        unicorn
            .add_code_hook(step_addr as u64, step_addr as u64, move |uc, _, _| {
                let count = RC_JOB_EXECUTE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 100 {
                    let value = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE render-control trace {} at {:#x}: r0={:#x}",
                        name,
                        step_addr,
                        value
                    );
                }
            })
            .unwrap();
    }

    let create_execute_result = base_address + RC_JOB_CREATE_VIEW_FINAL;
    unicorn
        .add_code_hook(
            create_execute_result as u64,
            create_execute_result as u64,
            move |uc, _, _| {
                let count = RC_JOB_EXECUTE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 100 {
                    let job = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                    let result = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    let view_id = read_u32_or_invalid(uc, job + 0x1c) & 0xff;
                    let create_state = read_u32_or_invalid(uc, job + 0x20);
                    log::info!(
                        "PROCMAPENGINE render-control trace CreateView result at {:#x}: job={:#x} view={} state={:#x} result={:#x}",
                        create_execute_result,
                        job,
                        view_id,
                        create_state,
                        result
                    );
                }
            },
        )
        .unwrap();

    let view_state_addr = base_address + RC_JOB_RENDER_PIXMAP_VIEW_STATE;
    unicorn
        .add_code_hook(view_state_addr as u64, view_state_addr as u64, move |uc, _, _| {
            let count = RC_JOB_EXECUTE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 100 {
                let state = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let view = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE render-control trace RenderPixmapView view state at {:#x}: view={:#x} state={:#x}",
                    view_state_addr,
                    view,
                    state
                );
            }
        })
        .unwrap();

    let map_view_render_addr = base_address + MAP_VIEW_RENDER_ENTRY;
    unicorn
        .add_code_hook(
            map_view_render_addr as u64,
            map_view_render_addr as u64,
            move |uc, _, _| {
                let count = RC_JOB_EXECUTE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 100 {
                    let view = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE render-control trace MapView::Render at {:#x}: view={:#x}",
                        map_view_render_addr,
                        view
                    );
                }
            },
        )
        .unwrap();

    let view_render_thread_addr = base_address + VIEW_RENDER_THREAD_ENTRY;
    unicorn
        .add_code_hook(
            view_render_thread_addr as u64,
            view_render_thread_addr as u64,
            move |uc, _, _| {
                let count = RC_JOB_EXECUTE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 100 {
                    let renderer = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE render-control trace ViewRenderThread at {:#x}: renderer={:#x}",
                        view_render_thread_addr,
                        renderer
                    );
                }
            },
        )
        .unwrap();

    let peek_addr = base_address + RENDER_CONTROL_PEEK_RESULT;
    unicorn
        .add_code_hook(peek_addr as u64, peek_addr as u64, move |uc, _, _| {
            let count = RENDER_CONTROL_PEEK_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 50 {
                let queue = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE render-control trace PeekElement result at {:#x}: job={:#x}",
                    peek_addr,
                    queue
                );
            }
        })
        .unwrap();

    let can_execute_addr = base_address + RENDER_CONTROL_JOB_CAN_EXECUTE_RESULT;
    unicorn
        .add_code_hook(
            can_execute_addr as u64,
            can_execute_addr as u64,
            move |uc, _, _| {
                let count = RENDER_CONTROL_JOB_CAN_EXECUTE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 100 {
                    let result = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let job = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE render-control trace job can-execute result at {:#x}: job={:#x} result={:#x}",
                        can_execute_addr,
                        job,
                        result
                    );
                }
            },
        )
        .unwrap();

    for (name, offset) in [
        ("GetPeekedElement result", RENDER_CONTROL_JOB_RESULT),
        ("second virtual result", RENDER_CONTROL_SECOND_VIRTUAL_RESULT),
    ] {
        let step_addr = base_address + offset;
        unicorn
            .add_code_hook(step_addr as u64, step_addr as u64, move |uc, _, _| {
                let count = RENDER_CONTROL_JOB_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 100 {
                    let value = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE render-control trace {} at {:#x}: r0={:#x}",
                        name,
                        step_addr,
                        value
                    );
                }
            })
            .unwrap();
    }

    let execute_addr = base_address + RENDER_CONTROL_EXECUTE_RESULT;
    unicorn
        .add_code_hook(execute_addr as u64, execute_addr as u64, move |uc, _, _| {
            let count = RENDER_CONTROL_EXECUTE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 100 {
                let job = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                let vtable = read_u32_or_invalid(uc, job);
                log::info!(
                    "PROCMAPENGINE render-control trace Execute result at {:#x}: job={:#x} vtable={:#x}",
                    execute_addr,
                    job,
                    vtable
                );
            }
        })
        .unwrap();

    let render_view_addr = base_address + RENDER_CONTROL_RENDER_VIEW;
    unicorn
        .add_code_hook(
            render_view_addr as u64,
            render_view_addr as u64,
            move |uc, _, _| {
                let count = RENDER_CONTROL_RENDER_VIEW_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 50 {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!(
                        "PROCMAPENGINE render-control trace RenderView at {:#x}: r0={:#x}",
                        render_view_addr,
                        r0
                    );
                }
            },
        )
        .unwrap();

    unicorn
        .add_code_hook(
            queue_state_addr as u64,
            queue_state_addr as u64,
            move |uc, _, _| {
                let queue = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let state = read_u32_or_invalid(uc, queue);
                let last = RENDER_LOOP_QUEUE_STATE_LAST.swap(state, Ordering::Relaxed);
                let count = RENDER_LOOP_QUEUE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if state != last || count < 20 {
                    log::info!(
                        "PROCMAPENGINE render-control trace queue state at {:#x}: queue={:#x} state={:#x}",
                        queue_state_addr,
                        queue,
                        state
                    );
                }
            },
        )
        .unwrap();
}

fn add_b_create_window_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    static B_CREATE_WINDOW_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
    let entry_addr = base_address + B_CREATE_WINDOW_ENTRY;
    unicorn
        .add_code_hook(entry_addr as u64, entry_addr as u64, move |uc, _, _| {
            let count = B_CREATE_WINDOW_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 20 {
                return;
            }
            let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let settings = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let view_type = read_u32_or_invalid(uc, settings);
            let w = read_u32_or_invalid(uc, settings + 8);
            let h = read_u32_or_invalid(uc, settings + 0xc);
            log::info!(
                "PROCMAPENGINE bCreateWindow trace entry at {:#x}: this={:#x} view_type={:#x} settings_w={} settings_h={}",
                entry_addr,
                this,
                view_type,
                w,
                h
            );
        })
        .unwrap();

    for (name, offset, store_off) in [
        (
            "svgCreateResourceSurface #1 -> this+0x70",
            B_CREATE_WINDOW_RES_SURFACE_1_RESULT,
            0x70u32,
        ),
        (
            "svgCreateResourceSurface #2 -> this+0x74",
            B_CREATE_WINDOW_RES_SURFACE_2_RESULT,
            0x74,
        ),
        (
            "svgCreateResourceSurface #3 -> this+0x78",
            B_CREATE_WINDOW_RES_SURFACE_3_RESULT,
            0x78,
        ),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = B_CREATE_WINDOW_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 60 {
                    return;
                }
                let this = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let w = read_u32_or_invalid(uc, this + 8);
                let h = read_u32_or_invalid(uc, this + 0xc);
                log::info!(
                    "PROCMAPENGINE bCreateWindow trace {} at {:#x}: this={:#x} r0={:#x} w={} h={}",
                    name,
                    addr,
                    this,
                    r0,
                    w,
                    h
                );
                let _ = store_off;
            })
            .unwrap();
    }

    let layer_result = base_address + B_CREATE_WINDOW_LAYER_TRIPLE_RESULT;
    unicorn
        .add_code_hook(layer_result as u64, layer_result as u64, move |uc, _, _| {
            let count = B_CREATE_WINDOW_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 60 {
                return;
            }
            let this = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let w = read_u32_or_invalid(uc, this + 8);
            let h = read_u32_or_invalid(uc, this + 0xc);
            log::info!(
                "PROCMAPENGINE bCreateWindow trace svgCreateLayerContextTriple at {:#x}: this={:#x} layer={:#x} w={} h={}",
                layer_result,
                this,
                r0,
                w,
                h
            );
        })
        .unwrap();

    let egl_window_addr = base_address + B_CREATE_WINDOW_EGL_WINDOW_SURFACE_ENTRY;
    unicorn
        .add_code_hook(egl_window_addr as u64, egl_window_addr as u64, move |uc, _, _| {
            let count = B_CREATE_WINDOW_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 40 {
                return;
            }
            let this = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
            let display = read_u32_or_invalid(uc, this + 0x4c);
            let config = read_u32_or_invalid(uc, this + 0x50);
            let layer = read_u32_or_invalid(uc, this + 0x7c);
            let w = read_u32_or_invalid(uc, this + 8);
            let h = read_u32_or_invalid(uc, this + 0xc);
            log::info!(
                "PROCMAPENGINE bCreateWindow trace eglCreateWindowSurface branch at {:#x}: display={:#x} config={:#x} layer={:#x} w={} h={}",
                egl_window_addr,
                display,
                config,
                layer,
                w,
                h
            );
        })
        .unwrap();

    let apply_addr = base_address + B_CREATE_WINDOW_APPLY_IN_SYNC_RESULT;
    unicorn
        .add_code_hook(apply_addr as u64, apply_addr as u64, move |uc, _, _| {
            let count = B_CREATE_WINDOW_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 40 {
                return;
            }
            let this = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
            log::info!(
                "PROCMAPENGINE bCreateWindow trace svgApplyLayerInSync returned at {:#x}: this={:#x} layer={:#x}",
                apply_addr,
                this,
                read_u32_or_invalid(uc, this + 0x7c)
            );
        })
        .unwrap();
}

fn add_ail_power_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    for (name, original_addr, kind) in [
        ("hGetLpmInQueue result", AIL_HGET_LPM_IN_QUEUE_RESULT, 0u8),
        ("bSendCCAPowerMsg", AIL_SEND_CCA_POWER_MSG, 1u8),
        ("bSendCCAPowerMsg result", AIL_SEND_CCA_POWER_MSG_RESULT, 2u8),
    ] {
        let addr = base_address + original_addr;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = AIL_POWER_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 100 {
                    return;
                }

                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;

                match kind {
                    0 => log::info!(
                        "PROCMAPENGINE power trace {} at {:#x}: queue={:#x}",
                        name,
                        addr,
                        r0
                    ),
                    2 => log::info!(
                        "PROCMAPENGINE power trace {} at {:#x}: result={:#x}",
                        name,
                        addr,
                        r4
                    ),
                    _ => log::info!(
                        "PROCMAPENGINE power trace {} at {:#x}: r0={:#x} r1={:#x} r2={:#x} r3={:#x}",
                        name,
                        addr,
                        r0,
                        r1,
                        r2,
                        r3
                    ),
                }
            })
            .unwrap();
    }
}

fn add_ail_power_dispatch_state_hook(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let addr = base_address + (0x0066_7a8c - ORIGINAL_BASE);
    unicorn
        .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
            let count = AIL_POWER_DISPATCH_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 200 {
                return;
            }
            let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
            let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
            let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
            let object = read_u32_or_invalid(uc, r7);
            let state = read_u32_or_invalid(uc, object + APP_STATE_OFFSET);
            log::info!(
                "PROCMAPENGINE power dispatch trace at {:#x}: object={:#x} state={:#x} type={:#x} new_state(data1@0x18)={:#x} data2@0x1c={:#x}",
                addr,
                object,
                state,
                r5,
                r6,
                r4
            );
        })
        .unwrap();
}

#[allow(dead_code)]
fn force_ail_ipc_post_success(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let addr = base_address + AIL_POST_IPC_MESSAGE;
    unicorn
        .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
            let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
            if uc.reg_write(RegisterARM::R0, 0x72000_u64).is_err()
                || uc.reg_write(RegisterARM::PC, lr as u64).is_err()
            {
                return;
            }
            log::info!(
                "PROCMAPENGINE stubbed bPostIpcMessage at {:#x} -> 0x72000",
                addr
            );
        })
        .unwrap();
}

fn suppress_duplicate_procmap_start_conf(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let addr = base_address + (0x0066_7cd8u32 - ORIGINAL_BASE);
    let exit_addr = base_address + (0x0066_7c5cu32 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
            let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
            let object = read_u32_or_invalid(uc, r7);
            let state = read_u32_or_invalid(uc, object + APP_STATE_OFFSET);
            if state <= 2 {
                return;
            }

            if uc.reg_write(RegisterARM::PC, exit_addr as u64).is_err() {
                return;
            }

            let count = DUPLICATE_PROCMAP_START_CONF_SUPPRESSED.fetch_add(1, Ordering::Relaxed);
            if count < 20 {
                log::info!(
                    "PROCMAPENGINE: suppressed duplicate PWR_PROXY_START_CONF for map app 0x{:x} in state 0x{:x}",
                    object,
                    state
                );
            }
        })
        .unwrap();
}

fn add_procmap_switch_trace_hook(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    for (name, offset) in [
        ("switch decision", 0x0066_7a94u32),
        ("default path", 0x0066_7c5cu32),
        ("type3 path", 0x0066_7cd8u32),
    ] {
        let addr = base_address + (offset - ORIGINAL_BASE);
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = AIL_POWER_SWITCH_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= AIL_POWER_SWITCH_TRACE_LIMIT {
                    return;
                }
                let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                let r3 = if name == "switch decision" {
                    r5.wrapping_sub(3)
                } else {
                    0
                };
                let object = read_u32_or_invalid(uc, r7);
                log::info!(
                    "PROCMAPENGINE switch trace {} at {:#x}: object={:#x} state={:#x} r3={:#x} r4={:#x} r5={:#x} r6={:#x}",
                    name,
                    addr,
                    object,
                    read_u32_or_invalid(uc, object + APP_STATE_OFFSET),
                    r3,
                    r4,
                    r5,
                    r6
                );
            })
            .unwrap();
    }
}

fn add_procmap_type3_state_hook(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    for (name, offset) in [
        ("bOnInit vcall result", 0x0066_7d00u32),
        ("after state update", 0x0066_7d18u32),
    ] {
        let addr = base_address + (offset - ORIGINAL_BASE);
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                let object = read_u32_or_invalid(uc, r7);
                log::info!(
                    "PROCMAPENGINE type3 trace {} at {:#x}: r0={:#x} object={:#x} state={:#x} state4={:#x}",
                    name,
                    addr,
                    r0,
                    object,
                    read_u32_or_invalid(uc, object + APP_STATE_OFFSET),
                    read_u32_or_invalid(uc, object + 0x34)
                );
            })
            .unwrap();
    }
}

fn add_cca_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    for (name, offset) in [
        ("cca-dispatch", CCA_DISPATCH),
        ("cca-power-handler", CCA_POWER_HANDLER),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = CCA_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 500 {
                    return;
                }

                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let app = read_u32_or_invalid(uc, r0);
                let app_state = read_u32_or_invalid(uc, app + 0x30);
                let app_target = read_u32_or_invalid(uc, app + 0x10);
                let allow_power = read_u32_or_invalid(uc, app + 0x6c);
                let message_content = read_u32_or_invalid(uc, r1 + 4);
                let message_header = [
                    read_u32_or_invalid(uc, message_content),
                    read_u32_or_invalid(uc, message_content + 8),
                    read_u32_or_invalid(uc, message_content + 0xc),
                ];

                log::info!(
                    "PROCMAPENGINE cca trace {} at {:#x}: dispatch=0x{:x} app=0x{:x} app_state={:#x} app_target={:#x} allow_power={:#x} param3={:#x} msg=0x{:x} content=0x{:x} header=[{:#x}, {:#x}, {:#x}]",
                    name,
                    addr,
                    r0,
                    app,
                    app_state,
                    app_target,
                    allow_power,
                    r2,
                    r1,
                    message_content,
                    message_header[0],
                    message_header[1],
                    message_header[2]
                );
            })
            .unwrap();
    }
}

fn ensure_guest_call_stub(unicorn: &mut Unicorn<'_, Context>) -> Option<u32> {
    let current = PROCMAP_GUEST_CALL_STUB.load(Ordering::Relaxed);
    if current != 0 {
        return Some(current);
    }

    let mmu_arc = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    let addr = mmu_arc.lock().unwrap().heap_alloc(
        unicorn,
        GUEST_CALL_STUB_SIZE,
        Prot::READ | Prot::WRITE | Prot::EXEC,
        "[procmap-call-stub]",
    );

    if addr == 0 || unicorn.mem_write(addr as u64, &[0x0f, 0xc0, 0xbd, 0xe8]).is_err() {
        log::warn!("PROCMAPENGINE: failed to allocate ARM pop{{r0-r3,lr,pc}} guest-call stub");
        return None;
    }

    PROCMAP_GUEST_CALL_STUB.store(addr, Ordering::Relaxed);
    log::info!("PROCMAPENGINE: allocated guest-call stub at {:#x}", addr);
    Some(addr)
}

/// Traces every early-return decision of
/// `ail_tclAppInterfaceRestricted::u16RegisterService` so a DAPI client
/// registration that silently fails can be pinned to its bail-out branch.
fn add_dapi_register_diag_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::AtomicU32 as AU32;
    static DIAG_COUNT: AU32 = AU32::new(0);

    let sites: [(u32, &str); 9] = [
        (0x0059_0668 - ORIGINAL_BASE, "vRegisterService-entry (r0=handler)"),
        (0x0059_06e4 - ORIGINAL_BASE, "vRegisterService-registering (lr=appintf r5=p5)"),
        (0x0066_2d24 - ORIGINAL_BASE, "u16RegisterService-entry (r9=svc r7=server)"),
        (0x0066_2d4c - ORIGINAL_BASE, "after-WhoAmI (r4=owner r0=whoami r7=server)"),
        (0x0066_2d54 - ORIGINAL_BASE, "sync-list-nonnull (r3=[+0x5c])"),
        (0x0066_2dc0 - ORIGINAL_BASE, "serverid-0xffff-resolve"),
        (0x0066_2dd8 - ORIGINAL_BASE, "after-GetAppIdFromServiceId (r0 r7)"),
        (0x0066_2d84 - ORIGINAL_BASE, "after-semaphore (r0)"),
        (0x0066_2e10 - ORIGINAL_BASE, "posting-path (r2=sync-answer r9=svc r7=server)"),
    ];
    for (offset, label) in sites {
        let address = base_address + offset;
        unicorn
            .add_code_hook(address as u64, address as u64, move |uc, _, _| {
                let count = DIAG_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 30 {
                    return;
                }
                let handler = if label.contains("entry") {
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32
                } else {
                    uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32
                };
                if label.contains("registering") || label.contains("u16RegisterService-entry") {
                    // bRegisterAsync defers the REGISTER until the TE-side
                    // directory reports the server app as running - which it
                    // never does here. Feed it the ApplicationInfoStatus so
                    // the deferred execute posts the real REGISTER to mbx_7.
                    // Injecting at the first u16RegisterService entry (init
                    // time, not only the map-data one) matters: the deferred
                    // execute runs when the AE thread next waits on its
                    // mailbox, and if the REGISTER reaches DAPIAPP after the
                    // first GetBlockIDs request, DAPIAPP answers error id 6,
                    // procmap's error path posts UNREGISTER, and the freshly
                    // created client entry is torn down before it is usable.
                    crate::libs::libosal_linux::message::post_app_info_status(uc, 7);
                }
                let app_interface = read_u32_or_invalid(uc, handler.wrapping_add(0x10));
                let vtable = read_u32_or_invalid(uc, app_interface);
                let target = read_u32_or_invalid(uc, vtable.wrapping_add(0x58));
                let regid = read_u16_or_invalid(uc, handler.wrapping_add(0xc)) & 0xffff;
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
                let r9 = uc.reg_read(RegisterARM::R9).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE DAPI-register diag [{}] {} r0={:#x} r2={:#x} r3={:#x} r4={:#x} r6(handler)={:#x} regid={:#x} appintf={:#x} vtable={:#x} vt58={:#x} r7={:#x} r9={:#x} lr={:#x}",
                    uc.get_data().inner.thread_id(),
                    label,
                    r0,
                    r2,
                    r3,
                    r4,
                    handler,
                    regid,
                    app_interface,
                    vtable,
                    target,
                    r7,
                    r9,
                    lr
                );
            })
            .unwrap();
    }
}

/// Stand in for the ADM's RegisterConf for the posfi client registration
/// (service 0x1e): on the unit PROCNAV's posfi server registers the service
/// and DAPI's ADM confirms client registrations against it; neither the
/// server nor its service directory entry exists here, so the engine's Navi
/// client handler would never receive its register-id and inbound
/// NavPositionStatus messages could not be routed.  Field layout per
/// ail_tclOneThreadAppInterface::vUnknownRegisterConf (0x665d40): u16@+0x00
/// = server app id (the ADM routes the conf by it), u16@+0x0e = target
/// sub-id, u16@+0x14 = service id, u16@+0x16 = assigned register-id,
/// u8@+0x18 = status (1 = OK; only status 1 inserts the ServiceReference
/// into app_iface+0x50 that ail_bHandleMsgServiceData's claim walk matches)
/// and {+0x1a, +0x1c, +0x1e} = service version triple.  Server app id and
/// client sub-id are taken from the handler object so the async-request
/// lookup (bIsAnswer) finds the pending registration.
fn post_posfi_register_conf(unicorn: &mut Unicorn<'_, Context>, server_app: u32, target_sub: u32) {
    const POSFI_REGISTER_ID: u16 = 0x17;
    let mut content = vec![0u8; 0x20];
    content[0x00..0x04]
        .copy_from_slice(&(((0x0400u32 << 16) | server_app & 0xffff) as u32).to_le_bytes());
    content[0x04..0x08].copy_from_slice(&(0x20u32).to_le_bytes());
    content[0x08] = 0x02; // source app: DAPI (the ADM)
    content[0x0b] = 0x43; // class: RegisterConf
    content[0x0e..0x10].copy_from_slice(&(target_sub as u16).to_le_bytes());
    content[0x10..0x14].copy_from_slice(&0x0000_0fa0u32.to_le_bytes());
    content[0x14..0x16].copy_from_slice(&0x001eu16.to_le_bytes()); // service id
    content[0x16..0x18].copy_from_slice(&POSFI_REGISTER_ID.to_le_bytes()); // assigned register-id
    content[0x18] = 0x01; // status: OK
    content[0x1a..0x1c].copy_from_slice(&0x0001u16.to_le_bytes()); // version major
    content[0x1c..0x1e].copy_from_slice(&0x0001u16.to_le_bytes()); // version middle
    content[0x1e..0x20].copy_from_slice(&0x0001u16.to_le_bytes()); // version minor
    let mut blob = Vec::with_capacity(8 + content.len());
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&(content.len() as u32).to_le_bytes());
    blob.extend_from_slice(&content);
    let namespace = unicorn.get_data().namespace.clone();
    let mut guard = namespace.lock().unwrap();
    crate::common::osal_queues::OsalQueueService::ensure_queue(&mut guard.mq, "mbx_1024");
    let posted = crate::common::osal_queues::OsalQueueService::guest_post(
        &mut guard.mq,
        "mbx_1024",
        blob,
        0,
    );
    if posted {
        guard.notify_waiters();
    }
    drop(guard);
    log::info!(
        "PROCMAPENGINE: injected posfi RegisterConf (server_app={server_app:#x} target_sub={target_sub:#x} reg-id {POSFI_REGISTER_ID:#x} status OK) into mbx_1024 (posted={posted})"
    );
}

static POSFI_REF_BLOB: AtomicU32 = AtomicU32::new(0);
static POSFI_REF_INSERTED: AtomicU32 = AtomicU32::new(0);

/// Stand in for the ServiceReference a status-OK RegisterConf would create:
/// the engine's client-claim walk in ail_bHandleMsgServiceData matches
/// inbound service data against the list at app_iface+0x50, but that list is
/// only fed through the *asynchronous* registration path (async request at
/// app_iface+0x70 + status-1 conf); the engine's Navi client handlers
/// register synchronously, so the fabricated conf is dropped with
/// "asynchronous answer not awaited" and no reference ever appears.
///
/// Adding the reference goes through fwl_List::vAdd, whose operator_new
/// silently fails on this emulated process (no working plain malloc from the
/// hook context), so mirror the engine's own list-linking instead: nodes are
/// 24 bytes {+0 prev, +4 next, +8 16-byte item} hung on a head of
/// {+4 first, +8 last, +0xc count}.  Item fields follow
/// ail_tclServiceReference's ctor as read by the walk's bIsDataSet:
/// {+0x00 service, +0x02 server app, +0x04 register-id, +0x06 state}
/// against message {+0x14, +0x00, +0x16, +0x0e} (0xffff = wildcard), with
/// state 0 = AVAILABLE required by the claim.
// Keep the fabricated posfi service reference linked into the engine's
// server-reference list (app_iface+0x50).  The engine's own svc 0x26
// registration (r429, live once DAPI answers the register handshake)
// rebuilds that list and detaches the host-linked node, after which no
// NavPositionStatus message can claim.  Re-link at the tail whenever the
// watchdog walk fails to find the node; the node body (+8..+20) already
// carries the tuple fields, so only the prev/next links and list tail are
// rewritten.  Safe to run from any hook: the emulator is serialized.
fn ensure_posfi_ref_linked(unicorn: &mut Unicorn<'_, Context>, app_iface: u32) {
    let node = POSFI_REF_BLOB.load(Ordering::Relaxed);
    if node == 0 || node >= 0xf000_0000 {
        return;
    }
    let list = read_u32_or_invalid(unicorn, app_iface + 0x50);
    if list >= 0xf000_0000 {
        return;
    }
    let mut walk = read_u32_or_invalid(unicorn, list + 4);
    let mut steps = 0u32;
    while (0x1000..0xf000_0000).contains(&walk) && walk != list && steps < 16 {
        if walk == node {
            return;
        }
        walk = read_u32_or_invalid(unicorn, walk + 4);
        steps += 1;
    }
    let last = read_u32_or_invalid(unicorn, list + 8);
    if (0x1000..0xf000_0000).contains(&last) && last != list {
        if !write_u32(unicorn, node, last) {
            return;
        }
        if !write_u32(unicorn, last + 4, node) {
            return;
        }
    } else if !write_u32(unicorn, node, 0) {
        return;
    } else if !write_u32(unicorn, list + 4, node) {
        return;
    }
    if !write_u32(unicorn, node + 4, 0) {
        return;
    }
    if !write_u32(unicorn, list + 8, node) {
        return;
    }
    let count = read_u32_or_invalid(unicorn, list + 0xc);
    if count < 0x1000_0000 {
        write_u32(unicorn, list + 0xc, count + 1);
    }
    log::info!("PROCMAPENGINE: re-linked posfi service reference into list {list:#x}");
}

/// Keep a client service reference for the DAPI map service in the engine's AIL list.
///
/// Whether the engine's registration of the DAPI map service (0x26) takes the
/// synchronous path - `ail_bHandleMsgServiceRegisterConf` (0x6682bc) stores the
/// reference the client's later messages are checked against - or the asynchronous one
/// - `vHandleAsyncRegisterConf` (0x665720) only flags the pending request and calls back
/// into the client handler - is a race between the engine's register call and the conf.
/// With the asynchronous result `ail_enCheckClientMailValidity` (0x66dd00) finds no
/// entry for (0x26, DAPI) and refuses every `GetBlockIDs` request with 0x10, which
/// `enPostMessage` (0x661a3c) reports as 7, so no map data is ever asked for (r561);
/// the synchronous result works (r557).  This registration bookkeeping is already
/// emulator-owned - the posfi entry in this very list is fabricated the same way - so
/// make the entry the conf implies exist either way: the service id, DAPI's app id, the
/// register id from the conf, service state available and unique id 0, which is what the
/// engine's own requests carry and, after the transport cleared `AIL_NO_UNIQUE_ID` from
/// DAPI's answers (`stamp_unique_id_for_receiver` in libosal), what its answers carry
/// too.  Note that the unique id must *not* be set to 0xfffe: the claim walk compares
/// that slot against the message and the engine's own requests carry 0, so doing it here
/// instead of in the messages blocks the requests (r551).
fn ensure_dapi_map_service_reference(unicorn: &mut Unicorn<'_, Context>, app_iface: u32) {
    /// DAPI's application id, the server of the map service.
    const DAPI_APP_ID: u32 = 0x7;

    let register_id = MAP_DATA_REGISTER_ID.load(Ordering::Relaxed);
    if register_id == 0xffff {
        // No conf seen yet: there is nothing a client reference could point at, and an
        // entry with a register id DAPI never handed out gets "unknown register" back.
        return;
    }
    let list = read_u32_or_invalid(unicorn, app_iface + 0x50);
    if list >= 0xf000_0000 {
        return;
    }
    let count = read_u32_or_invalid(unicorn, list + 0xc);
    if count >= 0x0010_0000 {
        return;
    }
    let mut last = read_u32_or_invalid(unicorn, list + 8);
    let mut node = read_u32_or_invalid(unicorn, list + 4);
    let mut steps = 0u32;
    while (0x1000..0xf000_0000).contains(&node) && node != list && steps < 16 {
        let item = node + 8;
        if read_u16_or_invalid(unicorn, item) == MAP_DATA_DAPI_SERVICE_ID
            && read_u16_or_invalid(unicorn, item + 2) == DAPI_APP_ID
        {
            return;
        }
        last = node;
        node = read_u32_or_invalid(unicorn, node + 4);
        steps += 1;
    }
    let mmu_arc = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    let node = mmu_arc.lock().unwrap().heap_alloc(
        unicorn,
        32,
        Prot::READ | Prot::WRITE,
        "[dapi-map-svc-ref]",
    );
    if node == 0 {
        log::warn!("PROCMAPENGINE: no guest memory for the DAPI map service reference");
        return;
    }
    // Node layout as `vAdd` builds it: +0x00 previous, +0x04 next, +0x08 the
    // `ail_tclServiceReference` itself (+0x00 service, +0x02 server app, +0x04 register
    // id, +0x06 state, +0x08 unique id, then the version wildcards).
    let mut bytes = vec![0u8; 24];
    bytes[0x08..0x0a].copy_from_slice(&(MAP_DATA_DAPI_SERVICE_ID as u16).to_le_bytes());
    bytes[0x0a..0x0c].copy_from_slice(&(DAPI_APP_ID as u16).to_le_bytes());
    bytes[0x0c..0x0e].copy_from_slice(&(register_id as u16).to_le_bytes());
    bytes[0x12..0x18].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    if count == 0 {
        if !write_u32(unicorn, node, 0) || !write_u32(unicorn, node + 4, 0) {
            return;
        }
        if !write_u32(unicorn, list + 4, node) || !write_u32(unicorn, list + 8, node) {
            return;
        }
    } else if last < 0xf000_0000 {
        bytes[0x00..0x04].copy_from_slice(&last.to_le_bytes());
        if !write_u32(unicorn, last + 4, node) || !write_u32(unicorn, list + 8, node) {
            return;
        }
    } else {
        return;
    }
    if unicorn.mem_write(node as u64, &bytes).is_err() {
        log::warn!("PROCMAPENGINE: failed to write the DAPI map service reference at {node:#x}");
        return;
    }
    if !write_u32(unicorn, list + 0xc, count + 1) {
        return;
    }
    log::info!(
        "PROCMAPENGINE: linked client service reference {{svc {MAP_DATA_DAPI_SERVICE_ID:#x}, app {DAPI_APP_ID:#x}, reg-id {register_id:#x}, state AVAILABLE, unique id 0}} into list {list:#x} (node {node:#x}, count {count} -> {})",
        count + 1
    );
}

fn insert_posfi_service_reference(unicorn: &mut Unicorn<'_, Context>, _original_pc: u32) {    if POSFI_REF_INSERTED.load(Ordering::Relaxed) != 0 {
        return;
    }
    let base_address = PROCMAP_BASE.load(Ordering::Relaxed);
    let app = read_u32_or_invalid(unicorn, base_address + MAP_ENGINE_GLOBAL);
    let list = read_u32_or_invalid(unicorn, app + 0x50);
    if app >= 0xf000_0000 || list >= 0xf000_0000 {
        log::warn!("PROCMAPENGINE: posfi service-reference insert skipped (app={app:#x} list={list:#x})");
        return;
    }
    let mut node = POSFI_REF_BLOB.load(Ordering::Relaxed);
    if node == 0 {
        let mmu_arc = {
            let data = unicorn.get_data();
            data.mmu.clone()
        };
        let addr =
            mmu_arc.lock().unwrap().heap_alloc(unicorn, 32, Prot::READ | Prot::WRITE, "[posfi-ref]");
        if addr == 0 {
            log::warn!("PROCMAPENGINE: no guest memory for posfi service reference");
            return;
        }
        node = addr;
        POSFI_REF_BLOB.store(addr, Ordering::Relaxed);
    }
    let count = read_u32_or_invalid(unicorn, list + 0xc);
    if count >= 0x0010_0000 {
        log::warn!("PROCMAPENGINE: posfi service-reference insert skipped (list {list:#x} count {count:#x})");
        return;
    }
    let last = read_u32_or_invalid(unicorn, list + 8);
    let mut bytes = vec![0u8; 24];
    bytes[0x08] = 0x1e; // item service id: posfi
    // The claim walk compares the message's u16 at +0x00 against this slot
    // (bIsDataSet param_2, observed live at 0x66c580).  That field must
    // carry the sub-id the Navi client handler is registered under (0x16,
    // engine ctor 0x39f268), matching the sub-id the provider stamps into
    // the message, so the walk and the platform+0xa0 handler lookup agree.
    bytes[0x0a] = 0x16;
    bytes[0x0c] = 0x17; // item register-id (matches provider messages)
    bytes[0x0e] = 0x00; // item service state: AVAILABLE
    bytes[0x10..0x18].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    if count == 0 {
        // first = last = node, prev = next = 0, count = 1
        if !write_u32(unicorn, node, 0) || !write_u32(unicorn, node + 4, 0) {
            return;
        }
        if !write_u32(unicorn, list + 4, node) || !write_u32(unicorn, list + 8, node) {
            return;
        }
    } else if last < 0xf000_0000 {
        // append behind the current last node (mirrors vAdd's second branch)
        bytes[0x00..0x04].copy_from_slice(&last.to_le_bytes());
        if !write_u32(unicorn, last + 4, node) {
            return;
        }
        if !write_u32(unicorn, list + 8, node) {
            return;
        }
    } else {
        return;
    }
    if unicorn.mem_write(node as u64, &bytes).is_err() {
        log::warn!("PROCMAPENGINE: failed to write posfi service reference node at {node:#x}");
        return;
    }
    if !write_u32(unicorn, list + 0xc, count + 1) {
        return;
    }
    POSFI_REF_INSERTED.store(1, Ordering::Relaxed);
    log::info!(
        "PROCMAPENGINE: linked posfi service reference {{svc 0x1e, reg-id 0x17, state AVAILABLE}} into list {list:#x} (node {node:#x}, count {} -> {})",
        count,
        count + 1
    );

    // The body thread's very first receive (wait + u32GetFromMessageQueue on
    // the MQB queue) only completes when a real message arrives: without one
    // it strands in the IOSC event wait and never reaches its second wait
    // iteration - where it would re-read the repaired handle (app+0x78) and
    // join the entry thread on the claiming pass of mbx_1024 (r410).  Post
    // one posfi message into MQB_1024: it doubles as the first claim test
    // against the freshly linked reference.
    let blob = crate::libs::procnav::nav_position_status_message(0x7ff0);
    let namespace = unicorn.get_data().namespace.clone();
    let threads = unicorn.get_data().threads.clone();
    let mut guard = namespace.lock().unwrap();
    crate::common::osal_queues::OsalQueueService::ensure_queue(&mut guard.mq, "MQB_1024");
    let posted = crate::common::osal_queues::OsalQueueService::guest_post(
        &mut guard.mq,
        "MQB_1024",
        blob,
        0,
    );
    if posted {
        // libosal's producer-side post sets the consumer's registered IOSC
        // event; a host-side post must do the same or the body thread's
        // u32GetFromMessageQueue waits forever on it (r410).  Set every IOSC
        // event one of this process' threads is currently blocked on - the
        // thread list is per process, so this touches no other app.
        let mut ids = Vec::new();
        for thread in threads.lock().unwrap().iter() {
            if let crate::emulator::thread::ThreadStatus::Blocked(
                crate::emulator::thread::BlockReason::IoscEvent { id, .. },
            ) = thread.status
            {
                ids.push(id);
            }
        }
        let signalled = crate::os::dev::iosc::signal_events(&mut guard, ids);
        guard.notify_waiters();
        if !signalled.is_empty() {
            log::info!(
                "PROCMAPENGINE: signalled IOSC events {signalled:?} for the body thread's queue receive"
            );
        }
    }
    drop(guard);
    log::info!("PROCMAPENGINE: posted posfi wake message into MQB_1024 (posted={posted})");
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

    let r0 = unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    let r1 = unicorn.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
    let r2 = unicorn.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
    let r3 = unicorn.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
    let lr = unicorn.reg_read(RegisterARM::R14).unwrap_or(0) as u32;
    let sp = unicorn.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
    if sp < 0x1000 {
        log::warn!("PROCMAPENGINE: refusing guest call from invalid SP {:#x}", sp);
        return false;
    }

    let new_sp = sp.wrapping_sub(24);
    let saved = [r0, r1, r2, r3, lr, original_pc];
    if !saved
        .iter()
        .enumerate()
        .all(|(index, value)| write_u32(unicorn, new_sp + (index as u32 * 4), *value))
    {
        log::warn!(
            "PROCMAPENGINE: failed to save caller state at {:#x} before guest call {:#x}",
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

fn read_cstring(unicorn: &mut Unicorn<'_, Context>, address: u32) -> String {
    if address < 0x1000 || address == u32::MAX {
        return "<invalid>".to_string();
    }

    let mut bytes = [0u8; 256];
    if unicorn.mem_read(address as u64, &mut bytes).is_err() {
        return "<unreadable>".to_string();
    }

    match bytes.iter().position(|byte| *byte == 0) {
        Some(len) => String::from_utf8_lossy(&bytes[..len]).into_owned(),
        None => String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// r703: private guest arena backing the `vAddTLParams` multi-draw arrays
/// (the engine would allocate them with `map_malloc`, which a host-side hook
/// cannot call).  2 KiB per slot array x 10 slots fits in 2 x 0x800 pages.
/// r721: bit `n` set once `emulate_add_tl_params` stashed a live
/// translation vector for bank `n` at `arena + 0x1800 + n * 12`.
static TL_VEC_VALID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// r723: set once the `vFinishCombinedDrawing` entry stamp owned
/// `poTransPos3D`; the street pass then ignores the per-bank stash.
static VF_VEC_OWNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// r724: the largest-|x| (view-centring pan) stamp seen at `vfin` entry,
/// stored as f32 bit patterns; `u32::MAX` marks "none yet".
static VF_BEST_X: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(u32::MAX);
static VF_BEST_VEC: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static VF_BEST_Z: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn ensure_tl_arena(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    static TL_ARENA: AtomicU32 = AtomicU32::new(0);
    let cached = TL_ARENA.load(Ordering::Relaxed);
    if cached != 0 {
        return if cached == u32::MAX { 0 } else { cached };
    }
    for candidate in [0x5a00_0000u32, 0x5c00_0000, 0x4a00_0000, 0x4c00_0000] {
        if unicorn
            .mem_map(u64::from(candidate), 0x2000, Prot::READ | Prot::WRITE)
            .is_ok()
        {
            let _ = unicorn.mem_write(u64::from(candidate), &[0u8; 0x2000]);
            TL_ARENA.store(candidate, Ordering::Relaxed);
            log::info!(
                "PROCMAPENGINE street-coord arena t={}ms TL multi-draw arena mapped at {candidate:#x}",
                crate::libs::dapi::emu_t0_elapsed_ms()
            );
            return candidate;
        }
    }
    TL_ARENA.store(u32::MAX, Ordering::Relaxed);
    log::warn!("PROCMAPENGINE street-coord arena: no candidate address could be mapped");
    0
}

/// r703: stand-in for `map_tclMapElm_Street::vAddTLParams` (0x40f730).
///
/// The combined-streets pass (`vDrawCombinedStreets`, 0x409250) draws the
/// road banks with `glMultiDrawArraysEXT(5, ms_apu32First[b], ms_apu32Count[b],
/// ms_au32PrimCount[b])` and early-exits unless `ms_au32PrimCount[b] != 0` and
/// `ms_apoVBOResource[b]` (with a live GL id at `+0xc`) are set.  Those
/// globals are ONLY ever filled by `vAddTLParams`, which the real device calls
/// from `map_tclMapElm_Street::Draw` (0x40fc64, the plain-street TL modes
/// 0x36..0x55).  The emulator's render-control jobs never dispatch that pass
/// (r702: `Draw` entry has zero hits while `vFinishCombinedDrawing` runs per
/// DTM street element), so primCount stays 0, the multi-draw is never armed,
/// and roads never rasterize (r700/r701).  The same gap also explains
/// `ms_apoVBOResourceZValues = [0,0]`: `vAddTLParams` is its only writer.
///
/// This function performs the identical bookkeeping from the
/// `vFinishCombinedDrawing` entry hook (r0 = street element, r1 = mode,
/// r2 = MCP - the exact values the real `Draw` call site passes), reading
/// every value out of the engine's own line-config / `uti_DTM_StreetCoord_Buffer`
/// objects.  It fabricates no geometry: the (first, count) pairs, resources,
/// color arrays and z/texcoord resources all come straight from the scb the
/// element built while uploading its real road vertices.
fn emulate_add_tl_params(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    // `map_tclMapElm_Street` static arrays (10 slots of 4 bytes unless noted).
    const MS_APU32_FIRST: u32 = 0x0073_c150;
    const MS_APO_VBO_RESOURCE: u32 = 0x0073_c178;
    const MS_IN_OUT_INDEX: u32 = 0x0073_c1a0; // 1 byte
    const MS_PRIM_COUNT: u32 = 0x0073_c1a4;
    const MS_SIZE_OF_MULDRAW: u32 = 0x0073_c1f4;
    const MS_CURR_SCB: u32 = 0x0073_c220;
    const MS_VBO_TEXCOORD: u32 = 0x0073_c248;
    const MS_LINE_CFG_OFFSET: u32 = 0x0073_c270; // 10 x u16
    const MS_COLOR_ARRAY_SIZE: u32 = 0x0073_c284;
    const MS_VBO_Z_VALUES: u32 = 0x0073_c2cc;
    const MS_APU32_COUNT: u32 = 0x0073_c2f4;
    const MS_COLOR_ARRAY: u32 = 0x0073_c320;
    const MS_APO_IMAGE: u32 = 0x0073_c348;

    let mode = unicorn.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
    if mode.wrapping_sub(0x36) >= 0x20 {
        // `vFinishCombinedDrawing` only draws (and the real `Draw` only adds
        // params) for the TL modes.
        return;
    }
    let thisx = unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    let mcp = unicorn.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
    if thisx < 0x1000 || mcp < 0x1000 {
        return;
    }
    // The plain `map_tclMapElm_Street` keeps its line config at `+0x17c`
    // (holding the scb at `+0x40`, the config offset at `+0x44` and the
    // color-array index at `+0x46`).  `map_tclMapElm_DTMStreet` (all elements
    // seen in the emulator) instead stores the scb directly at `+0x40`
    // (proven by the `vPrepareStreetCoordBuffer` probe), so fall back to that
    // with the neutral slot 0 the `vDrawCombinedStreets` 0..9 loop reads.
    let settings_raw = read_u32_or_invalid(unicorn, thisx.wrapping_add(0x17c));
    let direct_scb = read_u32_or_invalid(unicorn, thisx.wrapping_add(0x40));
    let (scb, slot, cfg_offset, node) = if settings_raw >= 0x1000 {
        (
            read_u32_or_invalid(unicorn, settings_raw.wrapping_add(0x40)),
            read_u8_or_invalid(unicorn, settings_raw.wrapping_add(0x46)),
            read_u16_or_invalid(unicorn, settings_raw.wrapping_add(0x44)) as u16,
            read_u16_or_invalid(unicorn, thisx.wrapping_add(0x180)).wrapping_sub(1),
        )
    } else {
        (direct_scb, 0u32, 0u16, 0u32)
    };
    static GATE_LOGS: AtomicU32 = AtomicU32::new(0);
    if GATE_LOGS.fetch_add(1, Ordering::Relaxed) < 8 {
        log::info!(
            "PROCMAPENGINE street-coord probe t={}ms vAddTLParams-emu gate mode={:#x} this={:#x} mcp={:#x} settings={:#x} scb_direct={:#x} scb={:#x} slot={} node={} n={}",
            crate::libs::dapi::emu_t0_elapsed_ms(),
            mode,
            thisx,
            mcp,
            settings_raw,
            direct_scb,
            scb,
            slot,
            node,
            GATE_LOGS.load(Ordering::Relaxed)
        );
    }
    if scb < 0x1000 || slot >= 10 {
        return;
    }
    let bank = read_u8_or_invalid(unicorn, mcp.wrapping_add(5));
    // r713: the real `Street::Draw` (0x40fbf8) stamps `poTransPos3D`
    // (0x73c31c) with the element's tile translation, `dapi + 0x8c +
    // bank*12` inside its DAPI buffer (`this+0xc`).  That code never
    // runs here (only DTMStreet::Draw does, and it leaves the global
    // alone), so the combined pass' `vLoadModelViewMatrix` stored our
    // r709 zero vector: `t=(0,0,0)` and every stroke pinned to the top
    // right clip border (r712).  Mirror the real write so the genuine
    // SD-backed pan flows into the composed modelview; the r709 arena
    // fallback in the `vpcs` entry probe still covers elements whose
    // dapi buffer lacks a mapped vector.
    let dapi = read_u32_or_invalid(unicorn, thisx.wrapping_add(0xc));
    let trans = dapi.wrapping_add(0x8c + u32::from(bank) * 12);
    // r721: do NOT write the shared `poTransPos3D` global from here.  Many
    // element families arm through this function and the modelview is
    // composed once per `vpcs` pass from whichever vector was stamped last
    // (r718: a foreign tile's (1200, 0, -3030) poisoned the street pass
    // while the street element's own (-4490, 0, -1253) - matching the
    // render layer's t=(-4272, -952, +1250) - sat unused; r719's scb gate
    // never fired because `ms_apoCurrDTMStreetCoordBuffer` is still unset
    // at arm time).  Instead copy the vector into a per-bank slot in the
    // TL arena and let the `vpcs` entry probe - which knows its own bank -
    // point the pointer at its own element's vector.
    if dapi >= 0x1000 && bank < 48 && unicorn.mem_read(trans as u64, &mut [0u8; 12]).is_ok() {
        let arena = ensure_tl_arena(unicorn);
        if arena != 0 {
            let mut vec12 = [0u8; 12];
            let _ = unicorn.mem_read(trans as u64, &mut vec12);
            let stash = arena + 0x1800 + u32::from(bank) * 12;
            if unicorn.mem_write(stash as u64, &vec12).is_ok() {
                TL_VEC_VALID.fetch_or(1u64 << bank, Ordering::Relaxed);
                static TRANS_LOGS: AtomicU32 = AtomicU32::new(0);
                if TRANS_LOGS.fetch_add(1, Ordering::Relaxed) < 6 {
                    let x = f32::from_le_bytes([vec12[0], vec12[1], vec12[2], vec12[3]]);
                    let y = f32::from_le_bytes([vec12[4], vec12[5], vec12[6], vec12[7]]);
                    let z = f32::from_le_bytes([vec12[8], vec12[9], vec12[10], vec12[11]]);
                    log::info!(
                        "PROCMAPENGINE street-coord probe t={}ms poTransPos3D-vec bank={} dapi {:#x}+{:#x} -> ({x:.1}, {y:.1}, {z:.1}) n={}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        bank,
                        dapi,
                        0x8c + u32::from(bank) * 12,
                        TRANS_LOGS.load(Ordering::Relaxed)
                    );
                }
            }
        }
    }
    let inout = read_u8_or_invalid(
        unicorn,
        base_address.wrapping_add(MS_IN_OUT_INDEX - ORIGINAL_BASE),
    );
    let global = |off: u32| {
        base_address
            .wrapping_add(off - ORIGINAL_BASE)
            .wrapping_add(slot * 4)
    };

    let mut first_ptr = read_u32_or_invalid(unicorn, global(MS_APU32_FIRST));
    let mut count_ptr = read_u32_or_invalid(unicorn, global(MS_APU32_COUNT));
    if first_ptr < 0x1000 || count_ptr < 0x1000 {
        let arena = ensure_tl_arena(unicorn);
        if arena == 0 {
            return;
        }
        first_ptr = arena + slot * 200;
        count_ptr = arena + 2000 + slot * 200;
        write_u32(unicorn, global(MS_APU32_FIRST), first_ptr);
        write_u32(unicorn, global(MS_APU32_COUNT), count_ptr);
        write_u32(unicorn, global(MS_SIZE_OF_MULDRAW), 0x32);
    }
    let capacity = read_u32_or_invalid(unicorn, global(MS_SIZE_OF_MULDRAW)).max(1);
    let prim = read_u32_or_invalid(unicorn, global(MS_PRIM_COUNT));
    let write_entry = |uc: &mut Unicorn<'_, Context>, base: u32, value: u32| {
        let addr = base.wrapping_add(prim.min(capacity - 1) * 4);
        let _ = uc.mem_write(addr as u64, &value.to_le_bytes());
    };
    let node_map = read_u32_or_invalid(unicorn, scb.wrapping_add(0x16c));
    let starts = read_u32_or_invalid(unicorn, scb.wrapping_add(0x160));
    let counts = read_u32_or_invalid(unicorn, scb.wrapping_add(0x164));
    if prim < capacity && node_map >= 0x1000 && starts >= 0x1000 && counts >= 0x1000 {
        let section = read_u16_or_invalid(unicorn, node_map.wrapping_add(node * 2));
        let first_val = read_u32_or_invalid(unicorn, starts.wrapping_add(section * 4));
        let count_val = read_u32_or_invalid(unicorn, counts.wrapping_add(section * 4));
        if first_val != 0xffff_ffff && (1..0x100_0000).contains(&count_val) {
            write_entry(unicorn, first_ptr, first_val);
            write_entry(unicorn, count_ptr, count_val);
            write_u32(unicorn, global(MS_PRIM_COUNT), prim + 1);
        }
    } else if prim < capacity {
        // r707: the per-node section arrays (`+0x160/0x164/0x16c`) stay NULL
        // in the emulator (write-watch r705 saw no writer), but the whole
        // uploaded section is real geometry: `scb+0x2c` is the vertex count
        // the engine itself uploaded to this element's VBO (`vDrawCombinedStreets`
        // feeds exactly that count back as the dir-section offset), so one
        // (0, sections) strip covers the element's real road line.
        let sections = read_u32_or_invalid(unicorn, scb.wrapping_add(0x2c));
        if (1..0x100_0000).contains(&sections) {
            write_entry(unicorn, first_ptr, 0);
            write_entry(unicorn, count_ptr, sections);
            write_u32(unicorn, global(MS_PRIM_COUNT), prim + 1);
        }
    }
    // Per-slot stamping: the engine re-reads these every combined draw, so
    // refresh them from the current element's scb on every call (mirrors
    // `vAddTLParams` running once per element draw).
    let vbo_res = read_u32_or_invalid(unicorn, scb.wrapping_add(0x118));
    write_u32(unicorn, global(MS_APO_VBO_RESOURCE), vbo_res);
    write_u16(
        unicorn,
        base_address
            .wrapping_add(MS_LINE_CFG_OFFSET - ORIGINAL_BASE)
            .wrapping_add(slot * 2),
        cfg_offset,
    );
    let z_res = read_u32_or_invalid(unicorn, scb.wrapping_add(bank * 4 + 0x134));
    write_u32(unicorn, global(MS_VBO_Z_VALUES), z_res);
    let color_size = read_u32_or_invalid(unicorn, scb.wrapping_add(0x14c));
    write_u32(unicorn, global(MS_COLOR_ARRAY_SIZE), color_size);
    let tex_res = read_u32_or_invalid(unicorn, scb.wrapping_add(bank * 4 + 0x174));
    write_u32(unicorn, global(MS_VBO_TEXCOORD), tex_res);
    let image = read_u32_or_invalid(unicorn, scb.wrapping_add((inout + bank * 2 + 0x62) * 4));
    write_u32(unicorn, global(MS_APO_IMAGE), image);
    let color_array = read_u32_or_invalid(unicorn, scb.wrapping_add(0x148));
    write_u32(unicorn, global(MS_COLOR_ARRAY), color_array);
    write_u32(unicorn, global(MS_CURR_SCB), scb);
    static STAMP_LOGS: AtomicU32 = AtomicU32::new(0);
    if STAMP_LOGS.fetch_add(1, Ordering::Relaxed) < 12 {
        log::info!(
            "PROCMAPENGINE street-coord probe t={}ms vAddTLParams-emu mode={:#x} slot={} bank={} scb={:#x} node={} prim {}->{:#x} first={:#x} count={:#x} vbo_res={:#x}",
            crate::libs::dapi::emu_t0_elapsed_ms(),
            mode,
            slot,
            bank,
            scb,
            node,
            prim,
            read_u32_or_invalid(unicorn, global(MS_PRIM_COUNT)),
            read_u32_or_invalid(unicorn, first_ptr),
            read_u32_or_invalid(unicorn, count_ptr),
            read_u32_or_invalid(unicorn, global(MS_APO_VBO_RESOURCE)),
        );
    }
}

fn read_u32_or_invalid(unicorn: &mut Unicorn<'_, Context>, address: u32) -> u32 {
    let mut bytes = [0u8; 4];
    match unicorn.mem_read(address as u64, &mut bytes) {
        Ok(()) => u32::from_le_bytes(bytes),
        Err(_) => 0xffff_ffff,
    }
}

fn read_u16_or_invalid(unicorn: &mut Unicorn<'_, Context>, address: u32) -> u32 {
    let mut bytes = [0u8; 2];
    match unicorn.mem_read(address as u64, &mut bytes) {
        Ok(()) => u16::from_le_bytes(bytes) as u32,
        Err(_) => 0xffff_ffff,
    }
}

fn read_u8_or_invalid(unicorn: &mut Unicorn<'_, Context>, address: u32) -> u32 {
    let mut bytes = [0u8; 1];
    match unicorn.mem_read(address as u64, &mut bytes) {
        Ok(()) => bytes[0] as u32,
        Err(_) => 0xffff_ffff,
    }
}

fn write_u32(unicorn: &mut Unicorn<'_, Context>, address: u32, value: u32) -> bool {
    unicorn
        .mem_write(address as u64, &value.to_le_bytes())
        .is_ok()
}

fn write_u16(unicorn: &mut Unicorn<'_, Context>, address: u32, value: u16) -> bool {
    unicorn
        .mem_write(address as u64, &value.to_le_bytes())
        .is_ok()
}

fn write_u8(unicorn: &mut Unicorn<'_, Context>, address: u32, value: u8) -> bool {
    unicorn.mem_write(address as u64, &[value]).is_ok()
}

fn synthesize_client_service_reference(
    unicorn: &mut Unicorn<'_, Context>,
    message: u32,
    service_refs: u32,
) -> Option<u32> {
    if message == 0 || message == u32::MAX || service_refs == 0 || service_refs == u32::MAX {
        return None;
    }

    let existing = read_u32_or_invalid(unicorn, service_refs + 4);
    let header = read_u32_or_invalid(unicorn, message + 4);
    if header == 0 || header == u32::MAX {
        return (existing != 0).then_some(existing);
    }

    let service_id = (read_u16_or_invalid(unicorn, header + 20) & 0xffff) as u16;
    let server_app_id = (read_u16_or_invalid(unicorn, header + 2) & 0xffff) as u16;
    let dataset_id = (read_u16_or_invalid(unicorn, header + 12) & 0xffff) as u16;

    // The register-id DAPIAPP handed out in its ServiceRegister conf. Its map
    // worker (`dap_map_tclWorker::enProcessJob`) refuses the wildcard with
    // error 6 "unknown register", so stamp it into the reference every time we
    // are asked for it - exactly what the real ail client library keeps from
    // the conf it received.
    let conf_register_id = (MAP_DATA_REGISTER_ID.load(Ordering::Relaxed) & 0xffff) as u16;

    // A client only holds a reference to a service once the server has
    // confirmed the registration. While no conf is in yet there is nothing to
    // look up, so the mail check must fail: the request then fails inside
    // procmapengine and is retried, instead of being sent with the wildcard
    // register-id, which the server answers with an error that also tears the
    // registration down again.
    if service_id as u32 == MAP_DATA_DAPI_SERVICE_ID && conf_register_id == 0xffff {
        if existing != 0 {
            write_u32(unicorn, service_refs + 4, 0);
            write_u32(unicorn, service_refs + 8, 0);
            write_u32(unicorn, service_refs + 12, 0);
        }
        log::info!(
            "PROCMAPENGINE: no client reference for svc {service_id:#06x} yet (conf outstanding)"
        );
        return None;
    }

    if existing != 0 {
        if conf_register_id != 0xffff {
            write_u16(unicorn, existing + 12, conf_register_id);
        }
        return Some(existing);
    }

    let node = MAP_DATA_CLIENT_SERVICE_REF_NODE.load(Ordering::Relaxed);
    let node = if node != 0 {
        node
    } else {
        let mmu_arc = {
            let data = unicorn.get_data();
            data.mmu.clone()
        };
        let node = mmu_arc.lock().unwrap().heap_alloc(
            unicorn,
            0x18,
            Prot::READ | Prot::WRITE,
            "[procmap-svc-ref]",
        );
        if node == 0 {
            log::warn!("PROCMAPENGINE: failed to allocate synthetic client service reference");
            return None;
        }
        MAP_DATA_CLIENT_SERVICE_REF_NODE.store(node, Ordering::Relaxed);
        node
    };

    write_u32(unicorn, node, 0);
    write_u32(unicorn, node + 4, 0);
    write_u16(unicorn, node + 8, service_id);
    write_u16(unicorn, node + 10, server_app_id);
    // Register-id from the conf (wildcard only until the conf arrives).
    write_u16(unicorn, node + 12, conf_register_id);
    write_u8(unicorn, node + 14, 0);
    write_u8(unicorn, node + 15, 0);
    write_u16(unicorn, node + 16, dataset_id);
    write_u16(unicorn, node + 18, 0xffff);
    write_u16(unicorn, node + 20, 0xffff);
    write_u16(unicorn, node + 22, 0xffff);

    write_u32(unicorn, service_refs + 4, node);
    write_u32(unicorn, service_refs + 8, node);
    write_u32(unicorn, service_refs + 12, 1);

    log::info!(
        "PROCMAPENGINE: synthesized DAPI client service ref node={:#x} list={:#x} service={:#x} server={:#x} dataset={:#x} register={:#x}",
        node,
        service_refs,
        service_id,
        server_app_id,
        dataset_id,
        conf_register_id,
    );

    Some(node)
}

fn initialize_map_data_medium(unicorn: &mut Unicorn<'_, Context>, base_address: u32) -> bool {
    if MAP_DATA_MEDIUM_INITIALIZED.load(Ordering::Relaxed) {
        return true;
    }

    let ok = write_u32(
        unicorn,
        base_address + MAP_DATA_STOP_LOADING_GLOBAL,
        0,
    ) && write_u8(unicorn, base_address + MAP_DATA_FASTMAP_AVAILABLE_GLOBAL, 1)
        && write_u32(
            unicorn,
            base_address + MAP_DATA_DATASET_ID_GLOBAL,
            MAP_DATA_FAKE_DATASET_ID,
        )
        && write_u32(
            unicorn,
            base_address + MAP_DATA_DATASET_ID_VALIDATED_GLOBAL,
            MAP_DATA_FAKE_DATASET_ID,
        );

    if ok {
        MAP_DATA_MEDIUM_INITIALIZED.store(true, Ordering::Relaxed);
        log::info!(
            "PROCMAPENGINE: initialized synthetic map medium dataset_id={:#x} fastmap_available=1 stop_loading=0",
            MAP_DATA_FAKE_DATASET_ID
        );
    } else if !MAP_DATA_MEDIUM_FAILURE_LOGGED.swap(true, Ordering::Relaxed) {
        log::warn!("PROCMAPENGINE: failed to initialize synthetic map medium globals");
    }

    ok
}
/// r689: watch the street geometry upload chain.  Static analysis (design.md)
/// shows `map_tclMapElm_DTMStreet::vPrepareStreetCoordBuffer` (0x55ae60) is
/// the ONLY path that fills the road resource VBOs: it pulls the DTM coord
/// container from the DAPI object (`this+0xc` vtable+0xdc) and hands it to
/// `uti_DTM_StreetCoord_Buffer::bProcessData` (0x448e38), which calls
/// `vUpdateVBO` -> `rl_tclTextureManager::u32UpdateVBO` -> `glBufferSubData`
/// for the pos (`buffer+0x30`) and dir (`buffer+0x34`) arrays.  Every run so
/// far logged zero `glBufferSubData`, so either the prepare pass never runs,
/// its time-slice/DPM gate skips it, or the DAPI container comes back NULL.
/// All four watch points live inside page 0x55a000 (a page-range hook, per
/// the narrow-hook false-negative rule).
thread_local! {
    /// scb objects whose `+0x28..0x3c` window is already under a write-watch.
    static WATCHED_SCBS: std::cell::RefCell<std::collections::HashSet<u32>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

fn add_street_coord_buffer_probe_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;
    let entry_hits = Arc::new(AtomicU32::new(0));
    let process_calls = Arc::new(AtomicU32::new(0));
    let process_results = Arc::new(AtomicU32::new(0));
    let skip_hits = Arc::new(AtomicU32::new(0));
    let page_lo = base_address + (0x0055_a000u32 - ORIGINAL_BASE);
    let page_hi = page_lo + 0xfff;
    unicorn
        .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
            let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
            let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
            if original == 0x0055_adf0u32 {
                // `vFinishCombinedDrawing` entry (r0 = street element,
                // r1 = mode, r2 = MCP): arm the multi-draw arrays the very
                // soon-as `vDrawCombinedStreets` requires (r703).
                static VF_IN: AtomicU32 = AtomicU32::new(0);
                let hits = VF_IN.fetch_add(1, Ordering::Relaxed);
                if hits < 8 {
                    log::info!(
                        "PROCMAPENGINE street-coord probe t={}ms vFinishCombinedDrawing entry r0={:#x} r1={:#x} r2={:#x} r3={:#x} lr={:#x} n={}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        uc.reg_read(RegisterARM::R0).unwrap_or(0),
                        uc.reg_read(RegisterARM::R1).unwrap_or(0),
                        uc.reg_read(RegisterARM::R2).unwrap_or(0),
                        uc.reg_read(RegisterARM::R3).unwrap_or(0),
                        uc.reg_read(RegisterARM::LR)
                            .unwrap_or(0)
                            .wrapping_sub(u64::from(base_address))
                            .wrapping_add(u64::from(ORIGINAL_BASE)) as u32,
                        hits
                    );
                }
                emulate_add_tl_params(uc, base_address);
                // r722: `rl_tclOpenGLShader::vUpdateShaderUniforms` is
                // called unconditionally by `vDraw` (0x55a064, this page)
                // but re-uploads the program's uniform block - which
                // includes `u_matModelView` from
                // `rl_tclRenderLayer3D::m_aflMdlMatrix`, refreshed each
                // pass by `vLoadModelViewMatrix` from the `poTransPos3D`
                // vector - only when the view's
                // `rl_tclTextureManager::ms_au32CurrentFrameNumber` entry
                // (0x71adc0) changed.  The real unit bumps it once per
                // rendered frame; the emulator never runs the frame loop,
                // so the gate stayed shut since the first upload and every
                // pass drew with a stale modelview.  Bump all view entries
                // per finish-drawing pass so the engine re-uploads by
                // itself.
                let frame_addr = base_address.wrapping_add(0x0071_adecu32 - ORIGINAL_BASE);
                for view in 0..4u32 {
                    let addr = frame_addr.wrapping_add(view * 4);
                    let value = read_u32_or_invalid(uc, addr);
                    let _ = uc.mem_write(addr as u64, &value.wrapping_add(1).to_le_bytes());
                }
                // r723: this entry IS the street pass - `r0` is the DTM
                // street element whose DAPI buffer (`this+0xc`) carries the
                // map-centre translation (r713: dapi 0x1044248 -> (-4490.6,
                // 0, -1253.3), matching the render layer's own pan
                // t=(-4272, -952, +1250)).  Stamp `poTransPos3D` here, once
                // per pass, exactly like the real unit's per-element
                // `Street::Draw` write at 0x40fbf8: the per-bank stash from
                // `emulate_add_tl_params` could not keep the street vector,
                // because the (dead-simple) route/overlay elements arm
                // through the same bank and stashed theirs last (r721/r722:
                // every drawn pass still used (1200, 0, -3030)).
                let vf_this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let vf_mcp = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let vf_bank = read_u8_or_invalid(uc, vf_mcp.wrapping_add(5));
                let vf_dapi = read_u32_or_invalid(uc, vf_this.wrapping_add(0xc));
                let vf_trans = vf_dapi.wrapping_add(0x8c + u32::from(vf_bank) * 12);
                if vf_dapi >= 0x1000 && uc.mem_read(vf_trans as u64, &mut [0u8; 12]).is_ok() {
                    // r724: `vFinishCombinedDrawing` runs for several
                    // element families and each family's `Street::Draw`-
                    // style stamp would overwrite the previous one (route
                    // overlay (1219, 0, -3030) vs street centre (-4472, 0,
                    // -1253)).  The pan stamp that centres the view is
                    // `-block_centre * scale` and is by far the largest in
                    // magnitude (real SD-backed data: 0x1044248 carries the
                    // car's Krakow block), so keep the stamp with the
                    // largest |x| and hand it to every drawn pass.
                    let x_bits = read_u32_or_invalid(uc, vf_trans);
                    let y_bits = read_u32_or_invalid(uc, vf_trans.wrapping_add(4));
                    let z_bits = read_u32_or_invalid(uc, vf_trans.wrapping_add(8));
                    let x = f32::from_bits(x_bits);
                    let best_bits = VF_BEST_X.load(Ordering::Relaxed);
                    if best_bits == u32::MAX
                        || x.abs() > f32::from_bits(best_bits).abs()
                    {
                        let _ = VF_BEST_X
                            .store(x.to_bits(), Ordering::Relaxed);
                        VF_BEST_VEC.store(
                            (u64::from(x_bits))
                                | (u64::from(y_bits) << 32),
                            Ordering::Relaxed,
                        );
                        VF_BEST_Z.store(z_bits, Ordering::Relaxed);
                    }
                    let pp_addr = base_address.wrapping_add(0x0073_c31cu32 - ORIGINAL_BASE);
                    let stash_addr = {
                        let arena = ensure_tl_arena(uc);
                        arena + 0x1c00
                    };
                    let best_x = f32::from_bits(VF_BEST_X.load(Ordering::Relaxed));
                    let best_xy = VF_BEST_VEC.load(Ordering::Relaxed);
                    let best_z = VF_BEST_Z.load(Ordering::Relaxed);
                    let bytes: [u8; 12] = [
                        (best_xy & 0xff) as u8,
                        ((best_xy >> 8) & 0xff) as u8,
                        ((best_xy >> 16) & 0xff) as u8,
                        ((best_xy >> 24) & 0xff) as u8,
                        ((best_xy >> 32) & 0xff) as u8,
                        ((best_xy >> 40) & 0xff) as u8,
                        ((best_xy >> 48) & 0xff) as u8,
                        ((best_xy >> 56) & 0xff) as u8,
                        (best_z & 0xff) as u8,
                        ((best_z >> 8) & 0xff) as u8,
                        ((best_z >> 16) & 0xff) as u8,
                        ((best_z >> 24) & 0xff) as u8,
                    ];
                    if uc.mem_write(stash_addr as u64, &bytes).is_ok()
                        && uc.mem_write(pp_addr as u64, &stash_addr.to_le_bytes()).is_ok()
                    {
                        VF_VEC_OWNED.store(true, Ordering::Relaxed);
                        static VF_LOGS: AtomicU32 = AtomicU32::new(0);
                        if VF_LOGS.fetch_add(1, Ordering::Relaxed) < 6 {
                            log::info!(
                                "PROCMAPENGINE street-coord probe t={}ms poTransPos3D best=({:.1}, {:.1}, {:.1}) (this-pass {x:.1}, {:.1}, {:.1}) dapi {:#x} n={}",
                                crate::libs::dapi::emu_t0_elapsed_ms(),
                                best_x,
                                f32::from_bits(((best_xy >> 32) & 0xffff_ffff) as u32),
                                f32::from_bits(best_z),
                                f32::from_bits(y_bits),
                                f32::from_bits(z_bits),
                                vf_dapi,
                                VF_LOGS.load(Ordering::Relaxed)
                            );
                        }
                    }
                }
            } else if original == 0x0055_ae30u32 || original == 0x0055_ae54u32 {
                // r708: `vDrawCombinedStreets` return sites (both vpcs calls
                // in `vFinishCombinedDrawing`, r705 entry-probe lr).  r707's
                // decision-PC trace inside the `vpcs` page logged nothing for
                // the ARMED pass although the primcount gate opened - either
                // the bounded-hook checks are missing from those already
                // translated TBs (unicorn 2.1.5 quirk) or the function left
                // before the loop.  Read the primcount/first arrays right
                // after the return from this page-proven hook: a consumed
                // primcount proves the loop ran (and the draw path was taken
                // or skipped internally), a surviving one proves it left
                // before the reset.
                static RET_TRACE: AtomicU32 = AtomicU32::new(0);
                let hits = RET_TRACE.fetch_add(1, Ordering::Relaxed);
                if hits < 24 {
                    let pc0 = read_u32_or_invalid(
                        uc,
                        base_address.wrapping_add(0x0073_c1a4u32 - ORIGINAL_BASE),
                    );
                    let pc1 = read_u32_or_invalid(
                        uc,
                        base_address.wrapping_add(0x0073_c1a8u32 - ORIGINAL_BASE),
                    );
                    log::info!(
                        "PROCMAPENGINE street-coord probe t={}ms vpcs-return at {:#x} primcount=[{},{}] n={}",
                        crate::libs::dapi::emu_t0_elapsed_ms(),
                        original,
                        pc0,
                        pc1,
                        hits
                    );
                }
            } else if original == 0x0055_ae60u32 {
                // vPrepareStreetCoordBuffer entry: r0=this, r1=MCP.
                let count = entry_hits.fetch_add(1, Ordering::Relaxed);
                if count >= 24 {
                    return;
                }
                let thisx = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let mcp = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let dapi = read_u32_or_invalid(uc, thisx.wrapping_add(0xc));
                let scb = read_u32_or_invalid(uc, thisx.wrapping_add(0x40));
                let pos_src = read_u32_or_invalid(uc, scb.wrapping_add(0x30));
                let dir_src = read_u32_or_invalid(uc, scb.wrapping_add(0x34));
                let dir_res = read_u32_or_invalid(uc, scb.wrapping_add(0x118));
                let bank = (read_u32_or_invalid(uc, mcp.wrapping_add(4)) >> 8) & 0xff;
                let coord_count = read_u32_or_invalid(uc, scb.wrapping_add(0x114)) & 0xffff;
                let node_arr = read_u32_or_invalid(uc, scb.wrapping_add(0x154));
                log::info!(
                    "PROCMAPENGINE street-coord probe t={}ms thread=[{}] enter bank={} this={:#x} dapi={:#x} scb={:#x} pos_src={:#x} dir_src={:#x} dir_res={:#x} coord_count={} node_arr={:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    bank,
                    thisx,
                    dapi,
                    scb,
                    pos_src,
                    dir_src,
                    dir_res,
                    coord_count,
                    node_arr,
                    count
                );
                // r690: pos_src/dir_src (scb+0x30/0x34) are NULL in every
                // observed run, so `vUpdateVBO`'s gate never opens and the
                // road VBOs stay zero-filled.  Watch who (if anybody) writes
                // those fields: register a write-watch for the first few
                // distinct scb objects and log writer PCs.
                let watched = WATCHED_SCBS.with(|set| {
                    let mut set = set.borrow_mut();
                    if set.len() >= 6 || !set.insert(scb) {
                        false
                    } else {
                        true
                    }
                });
                if watched && scb > 0x1000 {
                    for (lo_off, hi_off) in [
                        (0x28u64, 0x3cu64),
                        (0x114, 0x11c),
                        (0x150, 0x158),
                        (0x108, 0x120),
                        (0x134, 0x144),
                        (0x15c, 0x168),
                    ] {
                        let watch_count = Arc::new(AtomicU32::new(0));
                        let res = uc.add_mem_hook(
                            unicorn_engine::unicorn_const::HookType::MEM_WRITE,
                            u64::from(scb) + lo_off,
                            u64::from(scb) + hi_off,
                            move |writer: &mut Unicorn<'_, Context>, _kind, address, size, value| {
                                if watch_count.fetch_add(1, Ordering::Relaxed) >= 40 {
                                    return true;
                                }
                                let pc = writer.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                                log::info!(
                                    "PROCMAPENGINE street-coord watch t={}ms WRITE scb+{:#x}={:#x} size={} pc={:#x}",
                                    crate::libs::dapi::emu_t0_elapsed_ms(),
                                    address.wrapping_sub(u64::from(scb)),
                                    value,
                                    size,
                                    pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE)
                                );
                                true
                            },
                        );
                        log::info!(
                            "PROCMAPENGINE street-coord watch install scb={:#x} window={:#x}..{:#x} ok={}",
                            scb,
                            lo_off,
                            hi_off,
                            res.is_ok()
                        );
                        std::mem::forget(res);
                    }
                }
            } else if original == 0x0055_af54u32 {
                // `bl uti_DTM_StreetCoord_Buffer::bProcessData`: the upload
                // path IS reached (container was non-NULL).
                let count = process_calls.fetch_add(1, Ordering::Relaxed);
                if count >= 16 {
                    return;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE street-coord probe t={}ms bProcessData scb={:#x} container={:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    r0,
                    r2,
                    count
                );
            } else if original == 0x0055_af58u32 {
                // right after that `bl`: r0 = bProcessData return.
                let count = process_results.fetch_add(1, Ordering::Relaxed);
                if count >= 16 {
                    return;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                log::info!(
                    "PROCMAPENGINE street-coord probe t={}ms bProcessData returned {:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    r0,
                    count
                );
            } else if original == 0x0055_af9cu32 {
                // LAB_0055af9c: the time-slice / DPM / NULL-container skip leg.
                let count = skip_hits.fetch_add(1, Ordering::Relaxed);
                if count >= 8 {
                    return;
                }
                log::info!(
                    "PROCMAPENGINE street-coord probe t={}ms skipped (timeslice/dpm/null) n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    count
                );
            }
        })
        .unwrap();

    // r691: does `bProcessData` reach the per-node `bl vUpdateVBO` site at
    // 0x449118 (inside LAB_00449168, right after processThickLine)?
    {
        let call_hits = Arc::new(AtomicU32::new(0));
        let page_lo = base_address + (0x0044_9000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original != 0x0044_9118u32 {
                    return;
                }
                let count = call_hits.fetch_add(1, Ordering::Relaxed);
                if count >= 16 {
                    return;
                }
                let scb = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let pos_src = read_u32_or_invalid(uc, scb.wrapping_add(0x30));
                let dir_src = read_u32_or_invalid(uc, scb.wrapping_add(0x34));
                let node = (uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32) & 0xffff;
                log::info!(
                    "PROCMAPENGINE street-coord probe t={}ms vUpdateVBO-call scb={:#x} node={} pos_src={:#x} dir_src={:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    scb,
                    node,
                    pos_src,
                    dir_src,
                    count
                );
            })
            .unwrap();
    }

    // r691: `uti_DTM_StreetCoord_Buffer::vUpdateVBO` entry - dumps the exact
    // gate values (`param_4 == 0`, `+0x30 != 0`, `+0x34 != 0`, `+0x118 != 0`)
    // that decide whether the `glBufferSubData` uploads run.
    {
        let entry_hits = Arc::new(AtomicU32::new(0));
        let page_lo = base_address + (0x0044_6000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original != 0x0044_6f9cu32 {
                    return;
                }
                let count = entry_hits.fetch_add(1, Ordering::Relaxed);
                if count >= 16 {
                    return;
                }
                let scb = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                let param4 = read_u32_or_invalid(uc, sp);
                let pos_src = read_u32_or_invalid(uc, scb.wrapping_add(0x30));
                let dir_src = read_u32_or_invalid(uc, scb.wrapping_add(0x34));
                let dir_res = read_u32_or_invalid(uc, scb.wrapping_add(0x118));
                let dir_res_id = read_u32_or_invalid(uc, dir_res.wrapping_add(0xc));
                // r692: upload size = `piVar2[node] << 3` with piVar10/2 =
                // `[scb+0x160]/[scb+0x164]`; `u32UpdateVBO` silently returns
                // when size == 0, the only remaining explanation for the zero
                // `glBufferSubData` count with every other gate open.
                let sections = read_u32_or_invalid(uc, scb.wrapping_add(0x2c));
                let starts = read_u32_or_invalid(uc, scb.wrapping_add(0x160));
                let counts = read_u32_or_invalid(uc, scb.wrapping_add(0x164));
                let first_start = read_u32_or_invalid(uc, starts);
                let first_count = read_u32_or_invalid(uc, counts);
                log::info!(
                    "PROCMAPENGINE street-coord probe t={}ms vUpdateVBO scb={:#x} param3={:#x} param4={:#x} pos_src={:#x} dir_src={:#x} dir_res={:#x} dir_res_id={:#x} sections={sections} starts={starts:#x} counts={counts:#x} first=({first_start},{first_count}) n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    scb,
                    r3,
                    param4,
                    pos_src,
                    dir_src,
                    dir_res,
                    dir_res_id,
                    count
                );
                crate::gpu::note_road_geometry_uploaded();
            })
            .unwrap();
    }

    // r694: `map_tclMapElm_Street::vDrawCombinedStreets` entry (0x409250).
    // The whole visible-road pipeline ends here: it draws the street banks
    // with `glMultiDrawArraysEXT` but first gates on the DTM z-values branch,
    // which needs `map_tclMapMCP::bIsDTMEnabled()` = feature-bit AND
    // `map_tclPyramidLoaderUtil::ms_bPydFileExists` (0x713d33).  With DTM off
    // the engine feeds a_VertexZ = 0 as a constant, which the RH streets
    // projection (`w = -z_mv`, camera at z_mv = +1253) rejects entirely -
    // zero visible road pixels even with real geometry uploaded.  Log entry,
    // the flag and the z-resource array to see which half is missing.
    {
        let entry_hits = Arc::new(AtomicU32::new(0));
        let page_lo = base_address + (0x0040_9000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original != 0x0040_9250u32 {
                    // r707: the arming works (r706: primcount[0]=1, first0 =
                    // arena) yet neither the `glMultiDrawArraysEXT` stub
                    // (wrapper 0x552edc resolves through eglGetProcAddress
                    // into our svc stub) nor its NULL-fallback `glDrawArrays`
                    // loop ever fires, so the slot loop diverges before the
                    // call.  Trace every decision PC of the armed branch to
                    // find where it leaves.
                    let (site, log_slot) = match original {
                        0x0040_92b4 => ("loop-head", true),
                        0x0040_9508 => ("skip-resetslot", true),
                        0x0040_9514 => ("slot-continue", true),
                        0x0040_96f0 => ("post-flags", true),
                        0x0040_9828 => ("legs-entry", true),
                        0x0040_9884 => ("z-vbo-check", true),
                        0x0040_98b4 => ("z-const-path", true),
                        0x0040_98ec => ("multidraw-call", true),
                        0x0040_996c => ("image-gate", true),
                        0x0040_99dc => ("color-gate", true),
                        0x0040_9b1c => ("mv-compose", false),
                        _ => return,
                    };
                    static SITES: [std::sync::OnceLock<std::sync::Mutex<u64>>; 11] = [
                        std::sync::OnceLock::new(),
                        std::sync::OnceLock::new(),
                        std::sync::OnceLock::new(),
                        std::sync::OnceLock::new(),
                        std::sync::OnceLock::new(),
                        std::sync::OnceLock::new(),
                        std::sync::OnceLock::new(),
                        std::sync::OnceLock::new(),
                        std::sync::OnceLock::new(),
                        std::sync::OnceLock::new(),
                        std::sync::OnceLock::new(),
                    ];
                    let idx: usize = match original {
                        0x0040_92b4 => 0,
                        0x0040_9508 => 1,
                        0x0040_9514 => 2,
                        0x0040_96f0 => 3,
                        0x0040_9828 => 4,
                        0x0040_9884 => 5,
                        0x0040_98b4 => 6,
                        0x0040_98ec => 7,
                        0x0040_996c => 8,
                        0x0040_99dc => 9,
                        _ => 10,
                    };
                    let slot = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32 & 0xff;
                    let cell = SITES[idx].get_or_init(|| std::sync::Mutex::new(0));
                    let n = {
                        let mut g = cell.lock().unwrap();
                        let n = *g;
                        *g += 1;
                        n
                    };
                    if n < 16 {
                        log::info!(
                            "PROCMAPENGINE street-coord probe t={}ms vpcs trace {} slot={} n={}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            if log_slot { site } else { site },
                            slot,
                            n
                        );
                    }
                    return;
                }
                // r698: with DTM enabled and z-resources empty the combined
                // streets pass jumps to the exit label and never draws.  Clear
                // the pyramid flag (medium-level fact we fabricate anyway) so
                // `bIsDTMEnabled()` is false, the engine takes the constant-z
                // leg and issues `glMultiDrawArraysEXT`.  Runs on every entry,
                // before the logging cap.
                let pyd_addr = base_address.wrapping_add(0x0071_3d33u32 - ORIGINAL_BASE);
                if read_u32_or_invalid(uc, pyd_addr) & 0xff != 0 {
                    let _ = uc.mem_write(pyd_addr as u64, &[0u8; 1]);
                }
                // r709: an ARMED pass also executes `rl_tclRenderLayer3D::
                // vLoadModelViewMatrix` (0x3b20d4 via 0x409598, reached once
                // the slot's shader program handle is loaded) which
                // dereferences `*0x73c31c` - the `poTransPos3D` pointer the
                // real unit only fills for tilted/DTM views.  It stays NULL
                // in the emulator, so since the first successful arming the
                // render thread died with READ_UNMAPPED at 0x3b20dc inside
                // it (r706-r708: `[69] Execution error: READ_UNMAPPED`,
                // `thread exited with code 1`).  Point the pointer at a
                // zeroed 3-float vector in the TL arena: the top-down view
                // wants a neutral translation, and `vLoadModelViewMatrix`
                // then just stores (0, 0, -0) into `m_aflMdlMatrix[2]`.
                let pp_addr = base_address.wrapping_add(0x0073_c31cu32 - ORIGINAL_BASE);
                let pos_ptr = read_u32_or_invalid(uc, pp_addr);
                let mapped = pos_ptr >= 0x1000 && uc.mem_read(pos_ptr as u64, &mut [0u8; 12]).is_ok();
                // r721: prefer the vector the arm of THIS pass' bank stashed
                // in the TL arena (see `emulate_add_tl_params`): a shared
                // global lets foreign tiles poison the composed modelview,
                // the per-bank stash keeps each `vpcs` pass on its own
                // element's genuine SD-backed translation.
                let entry_mcp = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let entry_bank = ((read_u32_or_invalid(uc, entry_mcp.wrapping_add(4)) >> 8) & 0xff) as u32;
                let stashed = !VF_VEC_OWNED.load(Ordering::Relaxed)
                    && entry_bank < 48
                    && TL_VEC_VALID.load(Ordering::Relaxed) & (1u64 << entry_bank) != 0;
                let arena = ensure_tl_arena(uc);
                if stashed && arena != 0 {
                    let stash = arena + 0x1800 + entry_bank * 12;
                    let _ = uc.mem_write(pp_addr as u64, &stash.to_le_bytes());
                    static STASH_LOGS: AtomicU32 = AtomicU32::new(0);
                    if STASH_LOGS.fetch_add(1, Ordering::Relaxed) < 6 {
                        let mut vec12 = [0u8; 12];
                        let _ = uc.mem_read(stash as u64, &mut vec12);
                        let x = f32::from_le_bytes([vec12[0], vec12[1], vec12[2], vec12[3]]);
                        let y = f32::from_le_bytes([vec12[4], vec12[5], vec12[6], vec12[7]]);
                        let z = f32::from_le_bytes([vec12[8], vec12[9], vec12[10], vec12[11]]);
                        log::info!(
                            "PROCMAPENGINE street-coord probe t={}ms poTransPos3D bank={} -> stash {:#x} ({x:.1}, {y:.1}, {z:.1}) n={}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            entry_bank,
                            stash,
                            STASH_LOGS.load(Ordering::Relaxed)
                        );
                    }
                } else if !mapped && arena != 0 {
                    let _ = uc
                        .mem_write(pp_addr as u64, &(arena + 0x1000).to_le_bytes());
                    static PT_LOGS: AtomicU32 = AtomicU32::new(0);
                    if PT_LOGS.fetch_add(1, Ordering::Relaxed) < 4 {
                        log::info!(
                            "PROCMAPENGINE street-coord probe t={}ms poTransPos3D {:#x} -> arena {:#x} (neutral translation) n={}",
                            crate::libs::dapi::emu_t0_elapsed_ms(),
                            pos_ptr,
                            arena + 0x1000,
                            PT_LOGS.load(Ordering::Relaxed)
                        );
                    }
                }
                let count = entry_hits.fetch_add(1, Ordering::Relaxed);
                if count >= 40 && count % 2048 != 0 {
                    return;
                }
                let mcp = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let bank = (read_u32_or_invalid(uc, mcp.wrapping_add(4)) >> 8) & 0xff;
                let pyd = 0;
                let z0 = read_u32_or_invalid(
                    uc,
                    base_address.wrapping_add(0x0073_c2ccu32 - ORIGINAL_BASE),
                );
                let z1 = read_u32_or_invalid(
                    uc,
                    base_address.wrapping_add(0x0073_c2d0u32 - ORIGINAL_BASE),
                );
                // r700: `glMultiDrawArraysEXT` is issued only when
                // `ms_au32PrimCount[bank] != 0` (0x73c1a4, filled by
                // `vAddTLParams` at the END of the previous Street Draw).
                let pc0 = read_u32_or_invalid(
                    uc,
                    base_address.wrapping_add(0x0073_c1a4u32 - ORIGINAL_BASE),
                );
                let pc1 = read_u32_or_invalid(
                    uc,
                    base_address.wrapping_add(0x0073_c1a8u32 - ORIGINAL_BASE),
                );
                let lr = uc
                    .reg_read(RegisterARM::LR)
                    .unwrap_or(0)
                    .wrapping_sub(u64::from(base_address))
                    .wrapping_add(u64::from(ORIGINAL_BASE)) as u32;
                let first0 = read_u32_or_invalid(
                    uc,
                    base_address.wrapping_add(0x0073_c150u32 - ORIGINAL_BASE),
                );
                log::info!(
                    "PROCMAPENGINE street-coord probe t={}ms vDrawCombinedStreets bank={} pyd_exists={} z_res=[{:#x},{:#x}] primcount=[{},{}] first0={:#x} lr={:#x} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    bank,
                    pyd,
                    z0,
                    z1,
                    pc0,
                    pc1,
                    first0,
                    lr,
                    count
                );
            })
            .unwrap();
    }

    // r701: `vAddTLParams` (0x40f730, called at the END of
    // `map_tclMapElm_Street::Draw`) is what fills `ms_apu32First`/
    // `ms_apu32Count`/`ms_au32PrimCount` for the next
    // `vDrawCombinedStreets`; `multi-draw` never fired and the entry probe
    // shows `primcount=[0,0]`, so either it never runs or its node-count
    // argument (`Draw` passes `this[0x180] - 1`, the TL node count) is empty.
    {
        let entry_hits = Arc::new(AtomicU32::new(0));
        let page_lo = base_address + (0x0040_f000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original != 0x0040_f730u32 {
                    return;
                }
                let count = entry_hits.fetch_add(1, Ordering::Relaxed);
                if count >= 24 && count % 1024 != 0 {
                    return;
                }
                let thisx = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let nodes = (uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32) & 0xffff;
                let bank = (uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32) & 0xff;
                let count180 = read_u32_or_invalid(uc, thisx.wrapping_add(0x180)) & 0xffff;
                log::info!(
                    "PROCMAPENGINE street-coord probe t={}ms vAddTLParams this={:#x} nodes={} this[0x180]={} bank={} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    thisx,
                    nodes,
                    count180,
                    bank,
                    count
                );
            })
            .unwrap();
    }

    // r702: `map_tclMapElm_Street::Draw` (0x40fb18, __thiscall:
    // r1 = draw mode) only calls `vAddTLParams` when `mode - 0x36 < 0x20`
    // (the TL/3D street modes); `vAddTLParams` never runs, so sample the
    // mode values the element dispatch actually uses.
    {
        let mode_hits = Arc::new(AtomicU32::new(0));
        let page_lo = base_address + (0x0040_f000u32 - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        unicorn
            .add_code_hook(page_lo as u64, page_hi as u64, move |uc, _, _| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                if original != 0x0040_fb18u32 {
                    return;
                }
                let count = mode_hits.fetch_add(1, Ordering::Relaxed);
                if count >= 48 && count % 512 != 0 {
                    return;
                }
                let mode = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let mcp = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let bank = (read_u32_or_invalid(uc, mcp.wrapping_add(4)) >> 8) & 0xff;
                log::info!(
                    "PROCMAPENGINE street-coord probe t={}ms Street::Draw mode={:#x} bank={} n={}",
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                    mode,
                    bank,
                    count
                );
            })
            .unwrap();
    }
}
