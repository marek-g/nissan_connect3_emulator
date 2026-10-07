use crate::emulator::context::Context;
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
const RENDER_CONTROL_START: u32 = 0x003a_95ec - ORIGINAL_BASE;
const RENDER_CONTROL_CREATE_VIEW_JOB: u32 = 0x004c_a9e0 - ORIGINAL_BASE;
const RENDER_CONTROL_CREATE_VIEW_RESULT: u32 = 0x004c_aacc - ORIGINAL_BASE;
const RENDER_CONTROL_GET_VIEW_RESULT: u32 = 0x003a_5d48 - ORIGINAL_BASE;
const RENDER_CONTROL_QUEUE_STATE: u32 = 0x003a_8bec - ORIGINAL_BASE;
const RENDER_CONTROL_ADD_JOB: u32 = 0x003a_79b8 - ORIGINAL_BASE;
const RENDER_CONTROL_RENDER_VIEW: u32 = 0x003a_7f78 - ORIGINAL_BASE;
const RC_JOB_START_QUEUE_EXECUTE: u32 = 0x0058_6318 - ORIGINAL_BASE;
const RC_JOB_START_AND_RENDER_EXECUTE: u32 = 0x0058_69e8 - ORIGINAL_BASE;
const RC_JOB_RENDER_PIXMAP_EXECUTE: u32 = 0x004d_61a4 - ORIGINAL_BASE;
const RC_JOB_CREATE_VIEW_EXECUTE: u32 = 0x004c_a6f4 - ORIGINAL_BASE;
const RC_JOB_RENDER_VIEW_EXECUTE: u32 = 0x004d_6b24 - ORIGINAL_BASE;
const MAP_VIEW_RENDER_ENTRY: u32 = 0x0052_87f8 - ORIGINAL_BASE;
const VIEW_RENDER_THREAD_ENTRY: u32 = 0x0053_0264 - ORIGINAL_BASE;
const RC_JOB_CREATE_VIEW_FINAL: u32 = 0x004c_a780 - ORIGINAL_BASE;
const RC_JOB_RENDER_PIXMAP_INSTANCE_RESULT: u32 = 0x004d_62e8 - ORIGINAL_BASE;
const RC_JOB_RENDER_PIXMAP_VIEW_RESULT: u32 = 0x004d_62fc - ORIGINAL_BASE;
const RC_JOB_RENDER_PIXMAP_VIEW_STATE: u32 = 0x004d_625c - ORIGINAL_BASE;
const RENDER_CONTROL_PEEK_RESULT: u32 = 0x003a_8c20 - ORIGINAL_BASE;
const RENDER_CONTROL_JOB_CAN_EXECUTE_RESULT: u32 = 0x003a_8c34 - ORIGINAL_BASE;
const RENDER_CONTROL_JOB_RESULT: u32 = 0x003a_8df8 - ORIGINAL_BASE;
const RENDER_CONTROL_SECOND_VIRTUAL_RESULT: u32 = 0x003a_8e0c - ORIGINAL_BASE;
const RENDER_CONTROL_EXECUTE_RESULT: u32 = 0x003a_8e9c - ORIGINAL_BASE;
const MAP_TRIGGER_PIXMAP_RENDER: u32 = 0x0053_b36c - ORIGINAL_BASE;
const MAP_RENDER_VIEW_ID: u32 = 1;

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
static CCA_BODY_FORWARD_SUPPRESSED: AtomicU32 = AtomicU32::new(0);
static INIT_MAP_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static CCA_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static PORTCONTROL_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static MAP_INIT_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static AIL_POWER_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

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
    CCA_BODY_FORWARD_SUPPRESSED.store(0, Ordering::Relaxed);
    PROCMAP_GUEST_CALL_STUB.store(0, Ordering::Relaxed);
    INIT_MAP_TRACE_COUNT.store(0, Ordering::Relaxed);
    CCA_TRACE_COUNT.store(0, Ordering::Relaxed);
    PORTCONTROL_TRACE_COUNT.store(0, Ordering::Relaxed);
    MAP_INIT_TRACE_COUNT.store(0, Ordering::Relaxed);
    AIL_POWER_TRACE_COUNT.store(0, Ordering::Relaxed);

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

    let repair_addr = base_address + PORTCONTROL_INIT_RESULT;
    let activate_after_init = std::env::var_os("EMU_PROCMAPENGINE_SKIP_INITIAL_RENDER_TRIGGER")
        .is_none_or(|value| value.is_empty() || value == "0");
    unicorn
        .add_code_hook(repair_addr as u64, repair_addr as u64, move |uc, _, _| {
            if uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32 != 0 {
                repair_map_ipc_wait_param(uc);
                if activate_after_init {
                    start_map_engine_after_init(uc, repair_addr);
                }
            }
        })
        .unwrap();

    let handler_hook_addr = base_address + PORTCONTROL_HANDLER_RESULT;
    unicorn
        .add_code_hook(
            handler_hook_addr as u64,
            handler_hook_addr as u64,
            move |uc, _, _| {
                if uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32 != 0 {
                    trigger_pending_map_render(uc, handler_hook_addr);
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
                    if MAP_VIEW_READY.load(Ordering::Relaxed) {
                        trigger_pending_map_render(uc, render_hook_addr);
                    }
                },
            )
            .unwrap();
    }

    add_map_view_hooks(unicorn, base_address);

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
                        "PROCMAPENGINE errmem trace {} at {:#x}: r0={:#x} msg={}",
                        name,
                        addr,
                        r0,
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
    if map_state == 0 || map_state == u32::MAX || map_state == 4 || map_state == 5 {
        log::warn!(
            "PROCMAPENGINE: skipped map engine Start after PortControl init; state={:#x}",
            map_state
        );
        MAP_ENGINE_STARTED_AFTER_INIT.store(false, Ordering::Relaxed);
        return false;
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

fn trigger_pending_map_render(unicorn: &mut Unicorn<'_, Context>, original_pc: u32) -> bool {
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

    if MAP_INITIAL_RENDER_TRIGGERED.load(Ordering::Relaxed)
        || !MAP_RENDER_TRIGGER_PENDING.load(Ordering::Relaxed)
    {
        return false;
    }

    let map_state = read_u32_or_invalid(unicorn, base_address + MAP_ENGINE_STATE_VAR);
    if map_state == 0 || map_state == u32::MAX || map_state == 4 || map_state == 5 {
        log::warn!(
            "PROCMAPENGINE: skipped initial map render trigger; map engine state={:#x}",
            map_state
        );
        return false;
    }

    MAP_INITIAL_RENDER_TRIGGERED.store(true, Ordering::Relaxed);
    let function = base_address + MAP_TRIGGER_PIXMAP_RENDER;
    if !call_guest_function(
        unicorn,
        original_pc,
        function,
        [MAP_RENDER_VIEW_ID, 0, 0, 0],
    ) {
        log::warn!("PROCMAPENGINE: failed to queue initial map render job for view 1");
        return false;
    }

    log::info!(
        "PROCMAPENGINE: queued initial map render job for view {} with map engine state {:#x}",
        MAP_RENDER_VIEW_ID,
        map_state
    );
    true
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
        ("bPostIpcMessage", AIL_POST_IPC_MESSAGE, 3u8),
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

fn add_cca_trace_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    for (name, offset) in [
        ("cca-dispatch", CCA_DISPATCH),
        ("cca-power-handler", CCA_POWER_HANDLER),
    ] {
        let addr = base_address + offset;
        unicorn
            .add_code_hook(addr as u64, addr as u64, move |uc, _, _| {
                let count = CCA_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 50 {
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

fn write_u32(unicorn: &mut Unicorn<'_, Context>, address: u32, value: u32) -> bool {
    unicorn
        .mem_write(address as u64, &value.to_le_bytes())
        .is_ok()
}