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
    add_ail_power_dispatch_state_hook(unicorn, base_address);
    add_procmap_switch_trace_hook(unicorn, base_address);
    suppress_duplicate_procmap_start_conf(unicorn, base_address);
    add_procmap_type3_state_hook(unicorn, base_address);

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
    add_map_data_main_loop_trace_hooks(unicorn, base_address);
    add_get_block_ids_answer_trace_hooks(unicorn, base_address);

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

fn trigger_pending_map_render(
    unicorn: &mut Unicorn<'_, Context>,
    original_pc: u32,
    force_periodic: bool,
) -> bool {
    let base_address = PROCMAP_BASE.load(Ordering::Relaxed);
    if base_address == 0 {
        return false;
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
                        triggered = trigger_pending_map_render(uc, return_pc, true);
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
                if count < 120 {
                    log::info!(
                        "PROCMAPENGINE: DrawList vcall {} object={:#x} vtable={:#x} target={:#x} list={:#x} layer={:#x} trace={}",
                        kind,
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
                        ensure_service_reference_unique_ids(uc, app_iface);
                    }
                }
            }
            let count = HANDLE_SERVICE_DATA_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 12 {
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
            if matches!(count, 0 | 6) && dispatch < 0xf000_0000 {
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
                triggered = trigger_pending_map_render(uc, lr, true);
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
            log::info!(
                "PROCMAPENGINE map-data trace HandleGetBlockIDs::Execute at {:#x}: t={}ms thread=[{}] job={:#x} mdm_ref={:#x} answer_code={:#x} block_count={}",
                execute,
                crate::libs::dapi::emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                this,
                read_u16_or_invalid(uc, this + 0x1e),
                read_u32_or_invalid(uc, this + 0xc),
                read_u32_or_invalid(uc, this + 0x24),
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
                if hits >= if index == 0 { 6000 } else { 60 } {
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
                    static LAST: AtomicU32 = AtomicU32::new(0xffff_ffff);
                    if LAST.swap(signature, Ordering::Relaxed) == signature {
                        return;
                    }
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

/// Keep the engine's service-reference records claimable by DAPI.
///
/// The claim walk (`ail_tclServiceReference::bIsDataSet`, 0x66e3d0) matches an
/// incoming ServiceData against a reference on (service id, server app,
/// register id, unique id).  Its 0xffff wildcard is implemented only for the
/// message arguments (`param == 0xffff || this[slot] == param`), so the stored
/// slot has to compare equal to what the peer really sends.  The async register
/// path never assigns that slot: `bRegisterAsyncExecute` (0x660f74) builds the
/// outgoing ServiceRegister with zeros in its trailing words (the log shows
/// `sub12=0xfffe sub14=0x0000`) and `vHandleAsyncRegisterConf` (0x665720) only
/// flags the request entry and calls back into the client handler, so the
/// reference DAPI's answer is matched against holds unique id 0 while every
/// ServiceData DAPI sends carries `AIL_NO_UNIQUE_ID` (0xfffe).  Each map-data
/// answer was therefore dropped as unclaimable and the engine answered with a
/// ServiceDataError instead of handing the block ids to the request (r443:
/// `tuple=(0x26,0x7,0x1,0xfffe)` vs `compared=[0x26,0x7,0x1,0x0]`, GetBlockIDs
/// stuck at state 1, which in turn starves the car-cursor request).
///
/// Only unassigned references of the DAPI application are touched and only with
/// the sentinel that peer actually uses; no claim that could already match
/// changes (no message in a run carries unique id 0), so no existing route can
/// regress.  This maintains registration bookkeeping the emulator already owns
/// (the same list the fabricated posfi reference lives in), never the apps.
fn ensure_service_reference_unique_ids(unicorn: &mut Unicorn<'_, Context>, app_iface: u32) {
    /// DAPI's application id: the peer whose ServiceData answers carry 0xfffe.
    const DAPI_APP_ID: u32 = 0x7;

    let list = read_u32_or_invalid(unicorn, app_iface + 0x50);
    if list >= 0xf000_0000 {
        return;
    }
    let mut node = read_u32_or_invalid(unicorn, list + 4);
    let mut steps = 0u32;
    while (0x1000..0xf000_0000).contains(&node) && node != list && steps < 16 {
        let item = node + 8;
        let service = read_u32_or_invalid(unicorn, item) & 0xffff;
        let server_app = read_u32_or_invalid(unicorn, item + 2) & 0xffff;
        let unique = read_u32_or_invalid(unicorn, item + 8) & 0xffff;
        if service != 0
            && service != 0xffff
            && server_app == DAPI_APP_ID
            && unique == 0
            && write_u16(unicorn, item + 8, 0xfffe)
        {
            log::info!(
                "PROCMAPENGINE: service reference {item:#x} (service {service:#x}, DAPI) had no unique id, set 0xfffe to match DAPI's messages"
            );
        }
        node = read_u32_or_invalid(unicorn, node + 4);
        steps += 1;
    }
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