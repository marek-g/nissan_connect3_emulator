use crate::emulator::context::Context;
use crate::gpu;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use unicorn_engine::unicorn_const::Prot;
use unicorn_engine::{RegisterARM, Unicorn};

const ORIGINAL_BASE: u32 = 0x0000_8000;
const AIL_VSTART_APP_ENTRY: u32 = 0x0065_d664 - ORIGINAL_BASE;
const APP_NEW_STATE: u32 = 0x0039_0064 - ORIGINAL_BASE;
const CREATE_DEFAULT_VIEW_FLAG: u32 = 0x0071_5519 - ORIGINAL_BASE;
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
const MAP_DATA_SEND_REQUEST_CONSTRUCTOR_RESULT: u32 = 0x0059_1b68 - ORIGINAL_BASE;
const MAP_DATA_SEND_SERVICE_VCALL: u32 = 0x0059_05ec - ORIGINAL_BASE;
const MAP_DATA_SEND_SERVICE_RESULT: u32 = 0x0059_05f0 - ORIGINAL_BASE;
const MAP_DATA_CCA_GET_BLOCK_IDS_RESULT: u32 = 0x0053_ddcc - ORIGINAL_BASE;
const MAP_DATA_DAPI_GET_BLOCK_IDS_RESULT: u32 = 0x0053_a278 - ORIGINAL_BASE;
const MAP_DATA_DAPI_LOAD_BLOCKS_CALL: u32 = 0x0053_a230 - ORIGINAL_BASE;
const MAP_DATA_DAPI_LOAD_BLOCKS_RESULT: u32 = 0x0053_a234 - ORIGINAL_BASE;
const MAP_DATA_DATASET_ID_GLOBAL: u32 = 0x0078_f7f0 - ORIGINAL_BASE;
const MAP_DATA_DATASET_ID_VALIDATED_GLOBAL: u32 = 0x0078_f874 - ORIGINAL_BASE;
const MAP_DATA_FASTMAP_AVAILABLE_GLOBAL: u32 = 0x0078_f7f8 - ORIGINAL_BASE;
const MAP_DATA_STOP_LOADING_GLOBAL: u32 = 0x0078_f7a0 - ORIGINAL_BASE;
const MAP_DATA_FAKE_DATASET_ID: u32 = 1;
const MAP_DATA_JOB_QUEUE_GLOBAL: u32 = 0x0050_c08c - ORIGINAL_BASE;
const MAP_DATA_MAINLOOP_QUEUE_STATE: u32 = 0x0050_be18 - ORIGINAL_BASE;
const MAP_DATA_MAINLOOP_GET_ELEMENT_RESULT: u32 = 0x0050_be64 - ORIGINAL_BASE;
const MAP_DATA_MAINLOOP_JOB_CAN_EXECUTE: u32 = 0x0050_be94 - ORIGINAL_BASE;
const MAP_DATA_MAINLOOP_WAIT_GET_ELEMENT_RESULT: u32 = 0x0050_bf00 - ORIGINAL_BASE;
const MAP_DATA_MAINLOOP_WAIT_JOB_CAN_EXECUTE: u32 = 0x0050_bf30 - ORIGINAL_BASE;
const AIL_EN_CHECK_CLIENT_MAIL_VALIDITY: u32 = 0x0066_dd00 - ORIGINAL_BASE;
const AIL_EN_CHECK_CLIENT_MAIL_VALIDITY_EPILOGUE: u32 = 0x0066_de18 - ORIGINAL_BASE;
const AIL_EN_POST_MESSAGE_ENTRY: u32 = 0x0066_1a3c - ORIGINAL_BASE;
const AIL_EN_POST_MESSAGE_AFTER_VALIDITY: u32 = 0x0066_1b08 - ORIGINAL_BASE;
const AIL_EN_POST_MESSAGE_SUCCESS_PATH: u32 = 0x0066_1bb8 - ORIGINAL_BASE;
const AIL_EN_POST_MESSAGE_RECEIVER_QUEUE_CALL: u32 = 0x0066_1c2c - ORIGINAL_BASE;
const AIL_EN_POST_MESSAGE_POST_RESULT: u32 = 0x0066_1c48 - ORIGINAL_BASE;
const AIL_EN_POST_MESSAGE_DELETE_PATH: u32 = 0x0066_1b9c - ORIGINAL_BASE;
const MAP_TRIGGER_PIXMAP_RENDER: u32 = 0x0053_b36c - ORIGINAL_BASE;
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
static MAP_DATA_HANDLE_REQUEST_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_DAPI_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static AIL_MAIL_VALIDITY_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static AIL_MAIL_VALIDITY_RESULT_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static AIL_POST_MESSAGE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_CLIENT_SERVICE_REF_NODE: AtomicU32 = AtomicU32::new(0);
static MAP_DATA_MEDIUM_INITIALIZED: AtomicBool = AtomicBool::new(false);
static MAP_DATA_MEDIUM_FAILURE_LOGGED: AtomicBool = AtomicBool::new(false);
static CCA_BODY_FORWARD_SUPPRESSED: AtomicU32 = AtomicU32::new(0);
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
    CCA_BODY_FORWARD_SUPPRESSED.store(0, Ordering::Relaxed);
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
        .add_code_hook(cca_dispatch_addr as u64, cca_dispatch_addr as u64, |uc, _, _| {
            let queue = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
            if queue == u32::MAX {
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

            if !std::env::var_os("EMU_PROCMAPENGINE_CREATE_DEFAULT_VIEW")
                .is_some_and(|value| value != "0" && !value.is_empty())
            {
                let flag = base_address + CREATE_DEFAULT_VIEW_FLAG;
                if uc.mem_write(flag as u64, &[0u8]).is_ok() {
                    log::info!(
                        "PROCMAPENGINE: cleared g_bCreateDefaultView at {:#x} before map startup",
                        flag
                    );
                } else {
                    log::warn!(
                        "PROCMAPENGINE: failed to clear g_bCreateDefaultView at {:#x}",
                        flag
                    );
                }
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

    let request_call_addr = base_address + MAP_DATA_JOB_MAP_DATA_REQUEST_CALL;
    unicorn
        .add_code_hook(
            request_call_addr as u64,
            request_call_addr as u64,
            move |uc, _, _| {
                let count = MAP_DATA_REQUEST_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 200 {
                    let request = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
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
    unicorn
        .add_code_hook(send_request_entry as u64, send_request_entry as u64, move |uc, _, _| {
            let count = MAP_DATA_DAPI_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 200 {
                let handler = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let message = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let message_vtable = read_u32_or_invalid(uc, message);
                log::info!(
                    "PROCMAPENGINE map-data trace SendCCARequest entry at {:#x}: handler={:#x} message={:#x} message_vtable={:#x} handler_service={:#x} handler_function={:#x} vt[4]={:#x} vt[8]={:#x} vt[c]={:#x} vt[10]={:#x}",
                    send_request_entry,
                    handler,
                    message,
                    message_vtable,
                    read_u32_or_invalid(uc, handler + 4) & 0xffff,
                    read_u32_or_invalid(uc, handler + 6) & 0xffff,
                    read_u32_or_invalid(uc, message_vtable + 4),
                    read_u32_or_invalid(uc, message_vtable + 8),
                    read_u32_or_invalid(uc, message_vtable + 0xc),
                    read_u32_or_invalid(uc, message_vtable + 0x10)
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
                if count < 100 {
                    let job = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let state = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                    let job_type = read_u32_or_invalid(uc, job + 8);
                    log::info!(
                        "PROCMAPENGINE map-data trace {} GetElement at {:#x}: job={:#x} queue_state={:#x} job_type={:#x}",
                        name,
                        addr,
                        job,
                        state,
                        job_type
                    );
                }
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
    if existing != 0 {
        return Some(existing);
    }

    let header = read_u32_or_invalid(unicorn, message + 4);
    if header == 0 || header == u32::MAX {
        return None;
    }

    let service_id = (read_u16_or_invalid(unicorn, header + 20) & 0xffff) as u16;
    let server_app_id = (read_u16_or_invalid(unicorn, header + 2) & 0xffff) as u16;
    let dataset_id = (read_u16_or_invalid(unicorn, header + 12) & 0xffff) as u16;

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
    write_u16(unicorn, node + 12, 0);
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
        "PROCMAPENGINE: synthesized DAPI client service ref node={:#x} list={:#x} service={:#x} server={:#x} dataset={:#x}",
        node,
        service_refs,
        service_id,
        server_app_id,
        dataset_id
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