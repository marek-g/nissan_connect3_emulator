use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

const ORIGINAL_BASE: u32 = 0x0000_8000;

fn hook_entry(
    unicorn: &mut Unicorn<'_, Context>,
    base_address: u32,
    original_address: u32,
    name: &'static str,
) {
    let address = base_address + (original_address - ORIGINAL_BASE);
    
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:14", move |uc: &mut Unicorn<'_, Context>| {
            let addr = address as u64;
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
            log::warn!(
                "DAPI {} [{}] {} addr={:#x} r0={:#x} r1={:#x} lr={:#x}",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                name,
                addr,
                r0,
                r1,
                lr
            );
        
            0
        });
}

fn hook_result(
    unicorn: &mut Unicorn<'_, Context>,
    base_address: u32,
    original_address: u32,
    name: &'static str,
) {
    let address = base_address + (original_address - ORIGINAL_BASE);
    
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:40", move |uc: &mut Unicorn<'_, Context>| {
            let addr = address as u64;
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            log::warn!(
                "DAPI {} [{}] {} result addr={:#x} r0={:#x}",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                name,
                addr,
                r0
            );
        
            0
        });
}

#[allow(dead_code)]
fn hook_force_return(
    unicorn: &mut Unicorn<'_, Context>,
    base_address: u32,
    original_address: u32,
    name: &'static str,
    value: u32,
) {
    let address = base_address + (original_address - ORIGINAL_BASE);
    // Trap-site flavor: the dispatcher treats a non-zero handler return as the
    // new PC, which reproduces the old `PC = LR` function skip exactly (and
    // unlike a legacy hook it cannot be lost to the narrow-hook translation
    // quirk when neighbouring sites on the same page carry svc patches).
    crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:70", move |uc: &mut Unicorn<'_, Context>| {
            let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
            uc.reg_write(RegisterARM::R0, value as u64).unwrap();
            log::warn!(
                "DAPI {} [{}] {} stubbed addr={:#x} -> {:#x}",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                name,
                address,
                value
            );
            lr
        });
}

fn hook_power_dispatch_state(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let address = base_address + (0x00b4_2098 - ORIGINAL_BASE);
    
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:83", move |uc: &mut Unicorn<'_, Context>| {
            let addr = address as u64;
            let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
            let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
            let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
            let r8 = uc.reg_read(RegisterARM::R8).unwrap_or(0) as u32;
            let mut buf = [0u8; 4];
            let mut read_u32 = |address: u32| {
                if uc.mem_read(address as u64, &mut buf).is_ok() {
                    u32::from_le_bytes(buf)
                } else {
                    0
                }
            };
            let object = read_u32(r6);
            let state = read_u32(object + 0x30);
            log::warn!(
                "DAPI {} [{}] power dispatch state addr={:#x} msg={:#x} object={:#x} appid={:#x} state={:#x} type={:#x} data1={:#x} data2={:#x}",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                addr,
                r6,
                object,
                read_u32(object + 8) & 0xffff,
                state,
                r5,
                r8,
                r7
            );
        
            0
        });
}

/// Documented exception to the "never patch the Bosch applications" rule, for
/// the CCA registration stage only.
///
/// Inside the DAPI message handler the branch at `0xb40418` tests two values
/// collected for the incoming request (`[sp,#0x4c]` and `[sp,#0x50]`). When the
/// first one is set, the handler takes the path that answers the client with
/// `ServiceDataError(6)` and removes the client's registry entry
/// (`ail_vPostServiceDataError6AndUnregister`, which is exactly the code that
/// traces "ADMIN_OPERATION_LOCKED" and "InterfaceState!=INITIALIZED"). On the
/// head unit that path is unreachable for a running map card, because the
/// interface is already initialized when the map engine registers; here the
/// startup ordering is compressed, so the very first request of procmapengine
/// destroys its own registration and every later request then legitimately
/// reports an unknown register-id.
///
/// Clearing those two values makes the handler take the normal path. Only the
/// decision is influenced - the reply construction, the registry and the worker
/// stay untouched. Disable with `EMU_PATCH_DAPI_ADMIN_LOCK=0`.
fn hook_clear_admin_lock_branch(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let address = base_address + (0x00b4_0420 - ORIGINAL_BASE);
    
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:136", move |uc: &mut Unicorn<'_, Context>| {
            let addr = address as u64;
            let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
            let mut buf = [0u8; 4];
            let mut word = |offset: u32| {
                if uc
                    .mem_read(sp.wrapping_add(offset) as u64, &mut buf)
                    .is_ok()
                {
                    u32::from_le_bytes(buf)
                } else {
                    0
                }
            };
            let request = word(0x4c);
            let state = word(0x50);
            if request != 0 || state != 0 {
                uc.mem_write(sp.wrapping_add(0x4c) as u64, &[0u8; 4])
                    .unwrap();
                uc.mem_write(sp.wrapping_add(0x50) as u64, &[0u8; 4])
                    .unwrap();
                log::warn!(
                    "DAPI {} [{}] admin-lock branch cleared addr={:#x} sp={:#x} request={:#x} state={:#x}",
                    uc.get_data().elf_path,
                    uc.get_data().inner.thread_id(),
                    addr,
                    sp,
                    request,
                    state
                );
            }
        
            0
        });
}

fn hook_power_ack_call(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let address = base_address + (0x00b4_23b0 - ORIGINAL_BASE);
    
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:173", move |uc: &mut Unicorn<'_, Context>| {
            let addr = address as u64;
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let mut buf = [0u8; 4];
            let mut read_u32 = |address: u32| {
                if uc.mem_read(address as u64, &mut buf).is_ok() {
                    u32::from_le_bytes(buf)
                } else {
                    0
                }
            };
            let object = r0;
            let vtable = read_u32(object);
            let target = read_u32(vtable.wrapping_add(0x74));
            log::warn!(
                "DAPI {} [{}] power ack indirect addr={:#x} r0={:#x} r1={:#x} object={:#x} vtable={:#x} target={:#x}",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                addr,
                r0,
                r1,
                object,
                vtable,
                target
            );
        
            0
        });
}

pub fn dapiapp_add_code_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    for (address, name) in [
        (0x0081_37e0, "vStartApp"),
        (0x0081_8170, "scd_init"),
        (0x0081_3b94, "amt_bInit"),
        (0x0081_f1e8, "s32InitApp07"),
        (0x0081_f108, "vExitApp07"),
        (0x0081_37e4, "bLBS_OnlyInitForTEngineRh"),
        (0x0092_3a44, "s32InitAppFc_lbsm"),
        (0x008b_4c40, "bRegPRMNotifications"),
        (0x008b_7d20, "vPRMCallBackMediaState"),
        (0x0082_35a8, "dap_tclDapiApp::bOnInit"),
        (0x0082_2dec, "dap_tclDapiApp::bGetConfigFromReg"),
        (0x0082_2264, "dap_tclDapiApp::bGetCommonConfig"),
        (0x0082_2b6c, "dap_tclDapiApp::bGetDeviceConfig"),
        (0x0082_0aa0, "dap_tclDapiApp::bGetRNWConfig"),
        (0x0082_08d8, "dap_tclDapiApp::bGetTMCConfig"),
        (0x0082_0178, "dap_tclDapiApp::bGetMapConfig"),
        (0x0081_f8c0, "dap_tclDapiApp::bGetRegionSelectionConfig"),
        (0x0082_128c, "dap_tclDapiApp::bGetLisaConfig"),
        (0x0082_2c1c, "dap_tclDapiApp::bInitThread"),
        (0x0082_5d10, "dap_tclDataServer::u16Init"),
        (0x0082_8260, "dap_tclDataManager::u16Init"),
        (0x00b3_9b6c, "ail::bSendCCAPowerMsg"),
        (0x00b3_7ab0, "ail::bPostIpcMessage"),

        (0x00b3_5508, "ail::vOnAppStateAckFailed"),
        (0x00b3_a6c4, "ail::vApplicationRequestErrorEnd"),
        (0x0081_da14, "amt::prGetOSALMsgHandle"),
        (0x0081_de78, "amt::bAllocateMessage"),
    ] {
        hook_entry(unicorn, base_address, address, name);
    }
    for (address, name) in [
        (0x0082_308c, "bGetConfigFromReg::lisa result"),
        (0x0082_30a8, "bInitThread::DAPDATAS result"),
        (0x0082_30c4, "bInitThread::DAPDATAM result"),
        (0x0082_30e0, "bInitThread::DAPDEVM result"),
        (0x0082_3100, "bInitThread::DYN1 result"),
        (0x0082_3120, "bInitThread::DYN2 result"),
        (0x0082_3140, "bInitThread::DYN3 result"),
        (0x0082_3160, "bInitThread::DYN4 result"),
        (0x0082_3180, "bInitThread::DYN5 result"),
        (0x0082_31a0, "bInitThread::DYN6 result"),
        (0x0082_31c0, "bInitThread::DYN7 result"),
        (0x0082_323c, "bGetConfigFromReg final result"),
        (0x0082_3740, "bOnInit::DataServer vtable+8 result"),
        (0x0082_3810, "bOnInit return result"),
        (0x00b3_9bf0, "bSendCCAPowerMsg::bPostIpcMessage result"),
        (0x00b3_7b08, "bPostIpcMessage::ail_bIpcMessagePost result"),
        (0x00b4_2170, "power type3 bSendCCAPowerMsg result"),
    ] {
        hook_result(unicorn, base_address, address, name);
    }
    hook_power_dispatch_state(unicorn, base_address);
    hook_power_ack_call(unicorn, base_address);
    hook_service_registration(unicorn, base_address);
    hook_service_register_handler(unicorn, base_address);
    hook_service_data_errors(unicorn, base_address);
    hook_service_data_scan(unicorn, base_address);
    if crate::libs::procmapengine::render_trace_enabled() {
    hook_fastmap_worker_steps(unicorn, base_address);
    }
    hook_service_state_setter(unicorn, base_address);
    hook_registry_guard(unicorn, base_address);
    hook_error_job_origin(unicorn, base_address);
    hook_service_status_handler(unicorn, base_address);
    hook_srvctrl_worker_lookup(unicorn, base_address);
    if crate::libs::procmapengine::render_trace_enabled() {
    hook_rs_region_outlines_gate(unicorn, base_address);
    }
    hook_rs_broadcasts(unicorn, base_address);
    if crate::libs::procmapengine::render_trace_enabled() {
    hook_rs_media_info(unicorn, base_address);
    }
    hook_media_info_vector(unicorn, base_address);
    hook_dapi_device_config(unicorn, base_address);
    if crate::libs::procmapengine::render_trace_enabled() {
    hook_dapi_file_lookup(unicorn, base_address);
    }
    hook_device_manager_init(unicorn, base_address);
    hook_device_manager_flow(unicorn, base_address);
    if std::env::var("EMU_PATCH_DAPI_ADMIN_LOCK")
        .map(|value| value != "0")
        .unwrap_or(true)
    {
        hook_clear_admin_lock_branch(unicorn, base_address);
    }
}

/// Coarse trace of the DAPDEVM task once it is running: which messages the
/// device manager receives/dispatches and how far the device/medium
/// evaluation and the config/resource-notification jobs of its scheduler
/// get, to find where the PRM device registration chain stops.
fn hook_device_manager_flow(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

    for (address, name) in [
        (0x0082_d6a4, "DeviceManager::vReceiveMsg"),
        (0x0082_db58, "DeviceManager::vDispatchSystemMsg"),
        (0x0082_cd38, "DeviceManager::vDispatchFunctionalityMsg"),
        (0x0082_d02c, "DeviceManager::vProcessingAvailChangeJob"),
        (0x0082_d78c, "DeviceManager::vInitDevices"),
        (0x0082_e1a4, "DeviceManager::C1"),
        (0x0083_1710, "Scheduler::vProcessJob"),
        (0x0083_1560, "Scheduler::vProcessDeviceJob"),
        (0x0083_0598, "Scheduler::vProcessInfoJob"),
        (0x0082_f570, "Scheduler::vProcessConfigEvaluation"),
        (0x0082_f1dc, "Scheduler::vProcessConfigUpdate"),
        (0x0082_f81c, "Scheduler::vProcessResourceNotific"),
        (0x0083_0d40, "Scheduler::vProcessDeviceResult"),
        (0x0083_17cc, "Scheduler::vInitializeDevHandler"),
        (0x008b_4c40, "DeviceTableWorker::bRegPRMNotifications"),
    ] {
        let address = base_address + (address - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:319", move |uc: &mut Unicorn<'_, Context>| {
                let addr = address as u64;
                let count = TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 800 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                log::warn!(
                    "DAPI-flow {} [{}] {} addr={:#x} r0={:#x} r1={:#x} r2={:#x}",
                    uc.get_data().elf_path,
                    uc.get_data().inner.thread_id(),
                    name,
                    addr,
                    r0,
                    r1,
                    r2,
                );
            
                0
            });
    }

    // bRegPRMNotifications media-status ioctl: at 0x8b4d7c r5 holds the fd
    // returned by OSAL_IOOpen(ctrl-dev) and r1=0x7ffffffc; 0x8b4d84 receives
    // the returned status bit-mask (or -1). Log both to see whether the
    // control device open succeeded and what status the emulator gave back.
    // OSAL_IOOpen wrapper 0x813144 (bRegPRMNotifications calls it via bl):
    // log path+flags at entry, then tap the return address (lr) once to
    // capture the returned fd.
    {
        let open_entry = base_address + (0x0081_3180u32 - ORIGINAL_BASE);
        crate::os::code_stub::trap_site(unicorn, open_entry, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:364", move |uc: &mut Unicorn<'_, Context>| {
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                let path = crate::emulator::utils::read_string(uc, r0);
                log::warn!(
                    "DAPI-flow {} [{}] OSAL_IOOpen enter path={} flags={:#x} lr={:#x}",
                    uc.get_data().elf_path,
                    uc.get_data().inner.thread_id(),
                    path,
                    r1,
                    lr
                );
                0
        });
    }

    for (address, name) in [
        (0x008b_4d7c, "bRegPRM ioctl-before (fd in r5)"),
        (0x008b_4d84, "bRegPRM ioctl-after (r0=result)"),
        (0x008b_5094, "bRegPRM open-failed path (800)"),
        (0x008b_50c0, "bRegPRM empty-path path (0x218)"),
    ] {
        let address = base_address + (address - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:381", move |uc: &mut Unicorn<'_, Context>| {
                let addr = address as u64;
                let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                log::warn!(
                    "DAPI-flow {} [{}] {} addr={:#x} r5={:#x} r0={:#x}",
                    uc.get_data().elf_path,
                    uc.get_data().inner.thread_id(),
                    name,
                    addr,
                    r5,
                    r0,
                );
            
                0
            });
    }
}

/// ServiceStatus consumer (unanalyzed region, entered at 0xb3a448):
/// (app, regId, new_state) -> scans the app's registry and applies the
/// message's state byte to the entry with a matching register-id. Trace its
/// parameters so the synthesized AVAILABLE status can be validated.
fn hook_service_status_handler(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

    for (address, name) in [
        (0x00b3_a448, "status-consumer entry"),
        (0x00b3_a4b0, "status-consumer matched-set"),
        (0x00b3_a4c8, "status-consumer second-branch"),
    ] {
        let address = base_address + (address - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:413", move |uc: &mut Unicorn<'_, Context>| {
                let addr = address as u64;
                let count = TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 4000 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let r8 = uc.reg_read(RegisterARM::R8).unwrap_or(0) as u32;
                let r10 = uc.reg_read(RegisterARM::R10).unwrap_or(0) as u32;
                log::warn!(
                    "DAPI {} [{}] {} addr={:#x} r0={:#x} r1={:#x} r2={:#x} r8={:#x} r10={:#x}",
                    uc.get_data().elf_path,
                    uc.get_data().inner.thread_id(),
                    name,
                    addr,
                    r0,
                    r1,
                    r2,
                    r8,
                    r10,
                );
            
                0
            });
    }
}

/// `dap_dev_tclDeviceManager::u16Init` registers the "DAP_DEVICEMANAGER_THREAD"
/// notification client (commcon vtable+0x14, regId lands in object+0xc8) and
/// then subscribes to event 8 (media state, vtable+0x10). Its return value is
/// what makes `dap_tclDataServer::u16Init` fail with 0xffff, which keeps the
/// map medium permanently unavailable. Trace both call results.
fn hook_device_manager_init(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::AtomicBool;
    static PRM_INFO_SENT: AtomicBool = AtomicBool::new(false);

    for (address, name) in [
        (0x0082_de04, "DeviceManager register-notification result (r0=regid/0xfffe)"),
        (0x0082_de54, "DeviceManager subscribe-event8 result"),
    ] {
        let address = base_address + (address - ORIGINAL_BASE);
        let notify_at = address;
        
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:456", move |uc: &mut Unicorn<'_, Context>| {
                let addr = address as u64;
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32;
                let mut buf = [0u8; 4];
                let target_app = if uc.mem_read((r5 + 0x1c) as u32 as u64, &mut buf).is_ok() {
                    u32::from_le_bytes(buf)
                } else {
                    0
                };
                log::warn!(
                    "DAPI {} [{}] {} addr={:#x} r0={:#x} r1={:#x} obj={:#x} target_app={:#x}",
                    uc.get_data().elf_path,
                    uc.get_data().inner.thread_id(),
                    name,
                    addr,
                    r0,
                    r1,
                    r5,
                    target_app,
                );
                // The notification registration defers inside bRegisterAsync
                // waiting for an ApplicationInfoStatus that would announce
                // the PRM server (app 0x0a, the peripheral manager inside
                // procbaselx). Feed it to DAPI's own mailbox so the deferred
                // registration runs, bRegPRMNotifications executes and the
                // media-state event 8 subscription becomes live.
                if addr == notify_at as u64 && r0 != 0xfffe {
                    crate::libs::libosal_linux::message::post_app_info_status_to(
                        uc,
                        0x000a,
                        7,
                        &PRM_INFO_SENT,
                    );
                }
            
                0
            });
    }
}

/// Trace every write to a service-registry entry's state byte (entry+0x8):
/// the ServiceData accept gate rejects opcode-2 requests while the matched
/// client entry is not ACTIVE (0), so we need to know who leaves it at 1.
/// True once DAPIAPP's own (internal, regid 0xffff) svc-0x26 registry entry
/// has been set ACTIVE by the medium-up path (DAPDEVM thread). The real unit
/// orders process startup so the map engine's service registration happens
/// after this; procmap's client entry copies this state at REGISTER time,
/// and a copied 1 makes every later request fail with error 0xb.
pub static MAP_MEDIUM_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Wall-clock origin for the boot-timing traces (first use, i.e. early boot).
pub static EMU_T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub fn emu_t0_elapsed_ms() -> u128 {
    (*EMU_T0.get_or_init(std::time::Instant::now)).elapsed().as_millis()
}

fn hook_service_state_setter(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::Ordering;
    use std::sync::atomic::AtomicU32;
    static SET_COUNT: AtomicU32 = AtomicU32::new(0);

    let address = base_address + (0x00b4_6228 - ORIGINAL_BASE);
    
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:521", move |uc: &mut Unicorn<'_, Context>| {
            let count = SET_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 300 {
                return 0;
            }
            let read = |reg| uc.reg_read(reg).unwrap_or(0) as u32;
            // Watch for the internal svc-0x26 entry going ACTIVE (state 0).
            let entry = read(RegisterARM::R0);
            let new_state = read(RegisterARM::R1) & 0xff;
            let mut key = [0u8; 4];
            if new_state == 0
                && uc.mem_read(entry as u64, &mut key).is_ok()
                && u16::from_le_bytes([key[0], key[1]]) == 0xffff
                && u16::from_le_bytes([key[2], key[3]]) == 0x26
            {
                MAP_MEDIUM_ACTIVE.store(true, Ordering::SeqCst);
                log::info!(
                    "DAPI map-data medium ACTIVE at t={}ms (internal svc-0x26 entry {:#x})",
                    emu_t0_elapsed_ms(),
                    entry
                );
            }
            log::warn!(
                "DAPI {} [{}] registry vSetServiceState entry={:#x} new_state={:#x} lr={:#x}",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                read(RegisterARM::R0),
                read(RegisterARM::R1) & 0xff,
                read(RegisterARM::LR),
            );
        
            0
        });
}

/// Traces DAPIAPP's ServiceRegister handler: the service-conf lookup result
/// (vt+0xd8) and the register-id assignment result (vt+0x24), to verify the
/// client's own registrations reach and pass the handler.
fn hook_registry_guard(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    // Register-handler conf-getter (vt+0xd8) result at the
    // `cmp r0,#0` - shows whether a REGISTER passes the service-conf lookup.
    for (off, tag) in [(0x00b4_3a70u32, "conf-result"), (0x00b4_3ad0u32, "assign-result")] {
        let address = base_address + (off - ORIGINAL_BASE);
        let tag: &'static str = tag;
        
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:565", move |uc: &mut Unicorn<'_, Context>| {
                static HOOK_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if HOOK_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 40 {
                    return 0;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                log::warn!(
                    "DAPI reg-handler {} r0={:#x} r9={:#x} r10={:#x} r11={:#x}",
                    tag,
                    read(RegisterARM::R0),
                    read(RegisterARM::R9),
                    read(RegisterARM::R10),
                    read(RegisterARM::R11),
                );
            
                0
            });
    }

    // Service-data success path: after the registry scan + state gate passes,
    // `ail_bHandleMsgServiceData` tail-calls the app vtable slot +0x20 at
    // 0xb4357c. Log the resolved handler target, and also the return-0
    // fall-through at 0xb43518 (dispatch skipped).
    {
        let dispatch = base_address + (0x00b4_3578u32 - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, dispatch, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:591", move |uc: &mut Unicorn<'_, Context>| {
                static D_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if D_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 40 {
                    return 0;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                let r0 = read(RegisterARM::R0);
                let r1 = read(RegisterARM::R1);
                let vtable = read(RegisterARM::R3);
                let mut buf = [0u8; 4];
                let target = match uc.mem_read((vtable + 0x20) as u64, &mut buf) {
                    Ok(()) => u32::from_le_bytes(buf),
                    Err(_) => 0,
                };
                log::warn!(
                    "DAPI svcdata-DISPATCH addr={:#x} r0(obj)={:#x} r1(msg)={:#x} vtable={:#x} target(vt+0x20)={:#x} lr={:#x}",
                    dispatch,
                    r0,
                    r1,
                    vtable,
                    target,
                    read(RegisterARM::LR),
                );
            
                0
            });

        // The dispatch decision itself: `cmp r9,#0xfffe` at 0xb434e0 picks
        // between the generic service handler (vtable+0x20) and the
        // app-info/no-op fall-through.
        let decide = base_address + (0x00b4_34d4u32 - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, decide, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:623", move |uc: &mut Unicorn<'_, Context>| {
                static P_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if P_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 40 {
                    return 0;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                log::warn!(
                    "DAPI svcdata-PATH r7(msg)={:#x} r9={:#x} r10={:#x}",
                    read(RegisterARM::R7),
                    read(RegisterARM::R9),
                    read(RegisterARM::R10),
                );
            
                0
            });

        let skip = base_address + (0x00b4_3518u32 - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, skip, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:641", move |uc: &mut Unicorn<'_, Context>| {
                static S_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if S_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 40 {
                    return 0;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                log::warn!(
                    "DAPI svcdata-SKIP-DISPATCH r9={:#x} r10={:#x} r4={:#x}",
                    read(RegisterARM::R9),
                    read(RegisterARM::R10),
                    read(RegisterARM::R4),
                );
            
                0
            });

        // `dap_tclCommunicationContext::vSendToTask`: a request whose target
        // task index is 0 or whose connection-table entry is missing is
        // dropped with status 0x213 and the client never gets an answer.
        let send = base_address + (0x00b5_1ca8u32 - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, send, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:662", move |uc: &mut Unicorn<'_, Context>| {
                static T_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if T_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 4000 {
                    return 0;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                let jobpp = read(RegisterARM::R4);
                let mut buf = [0u8; 4];
                let job = uc
                    .mem_read(jobpp as u64, &mut buf)
                    .map(|_| u32::from_le_bytes(buf))
                    .unwrap_or(0);
                let job_type = uc
                    .mem_read((job + 0x20) as u64, &mut buf)
                    .map(|_| buf[0])
                    .unwrap_or(0xff);
                // Job type 4 is the CCA map-data service (svc 0x26); the rest
                // is startup noise that would flood the log.
                if job_type != 4 {
                    return 0;
                }
                log::warn!(
                    "DAPI vSendToTask jobtype={} thread-idx={} task={:#x} job={:#x}",
                    job_type,
                    read(RegisterARM::R7),
                    read(RegisterARM::R0),
                    job,
                );
            
                0
            });

        let drop = base_address + (0x00b5_1cc8u32 - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, drop, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:696", move |uc: &mut Unicorn<'_, Context>| {
                static X_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if X_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 40 {
                    return 0;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                log::warn!(
                    "DAPI vSendToTask DROPPED (status 0x213) thread-idx={} job={:#x} lr={:#x}",
                    read(RegisterARM::R7),
                    read(RegisterARM::R4),
                    read(RegisterARM::LR),
                );
            
                0
            });

        // `dap_tclJob::u16AddAction` -> `dap_tclActionList::bSetBlock`: an
        // external CCA request is turned into a job here; when the action
        // block cannot be opened the job is discarded and the client never
        // hears back. Log the list state (action number via [this+4], see
        // `cou16GetActionNumber` 0xb6b2f4; +0x34 = descriptor array) so the
        // failure mode is visible.
        // `dap_map_tclWorker::vReportError(code, file, line, func)`: every
        // failure of the CCA map-data worker ends here.
        let report = base_address + (0x0084_4e78u32 - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, report, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:721", move |uc: &mut Unicorn<'_, Context>| {
                static R_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if R_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 40 {
                    return 0;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                // vReportError(this, code, text, line, func): the text/func
                // arguments are strings, so resolve whatever looks like one.
                let text = |value: u32| -> String {
                    if value < 0x00e0_0000 || value > 0x00f0_0000 {
                        return String::from("-");
                    }
                    let mut byte = [0u8; 1];
                    let mut out = String::new();
                    for offset in 0..64u32 {
                        if uc.mem_read((value + offset) as u64, &mut byte).is_err() || byte[0] == 0
                        {
                            break;
                        }
                        out.push(byte[0] as char);
                    }
                    out
                };
                log::warn!(
                    "DAPI map-worker vReportError this={:#x} code={:#x} r2={:#x} \"{}\" line={} r4={:#x} \"{}\" lr={:#x}",
                    read(RegisterARM::R0),
                    read(RegisterARM::R1),
                    read(RegisterARM::R2),
                    text(read(RegisterARM::R2)),
                    read(RegisterARM::R3),
                    read(RegisterARM::R4),
                    text(read(RegisterARM::R4)),
                    read(RegisterARM::LR),
                );
            
                0
            });

        // `dap_tclAbstractWorker::u16SendResponse(comm, message, size, ...)`:
        // logs the map worker sending its answer so the send can be put next to
        // the error answer seen by the client.
        let send_response = base_address + (0x00b6_3388u32 - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, send_response, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:764", move |uc: &mut Unicorn<'_, Context>| {
                static S_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if S_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 20 {
                    return 0;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                log::warn!(
                    "DAPI worker u16SendResponse this={:#x} comm={:#x} msg={:#x} r3={:#x} lr={:#x}",
                    read(RegisterARM::R0),
                    read(RegisterARM::R1),
                    read(RegisterARM::R2),
                    read(RegisterARM::R3),
                    read(RegisterARM::LR),
                );
            
                0
            });

        // `amt_tclServiceDataError::vSetErrorData(u16)`: every CCA error answer
        // carries its code through here, so logging the code and the caller
        // shows which layer decided to answer a request with an error.
        let set_error = base_address + (0x0081_98ecu32 - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, set_error, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:787", move |uc: &mut Unicorn<'_, Context>| {
                static E_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if E_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 30 {
                    return 0;
                }
                log::warn!(
                    "DAPI ServiceDataError code={:#x} lr={:#x}",
                    (uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32) & 0xffff,
                    uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32,
                );
            
                0
            });

        // `dap_map_tclWorker::enProcessJob` epilogue: `r8` is the out-parameter
        // holding the result code (0xffff means success) and `r0` the return
        // value. Logging both shows what the map worker decided even when it
        // does not report an error through `vReportError`.
        let job_result = base_address + (0x0084_ba90u32 - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, job_result, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:807", move |uc: &mut Unicorn<'_, Context>| {
                static J_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if J_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 40 {
                    return 0;
                }
                let r8 = uc.reg_read(RegisterARM::R8).unwrap_or(0) as u32;
                let mut buf = [0u8; 2];
                let result = uc
                    .mem_read(r8 as u64, &mut buf)
                    .map(|_| u16::from_le_bytes(buf))
                    .unwrap_or(0xffff);
                log::warn!(
                    "DAPI map-worker enProcessJob result={:#x} this={:#x} lr={:#x}",
                    result,
                    uc.reg_read(RegisterARM::R5).unwrap_or(0) as u32,
                    uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32,
                );
            
                0
            });

        // `ActionList::bSetBlock` entry logging is installed below.
        let setblock = base_address + (0x00b6_b610u32 - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, setblock, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:831", move |uc: &mut Unicorn<'_, Context>| {
                static A_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if A_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 60 {
                    return 0;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                let this = read(RegisterARM::R0);
                let mut buf = [0u8; 4];
                // `cou16GetActionNumber` (`0xb6b2f4`) reads the u16 action
                // number through `[this+4]`, and `bSetBlock` fails when that
                // number is 0 or `idx` exceeds it; the array pointer at +0x34
                // only says whether the ctor allocated descriptor storage.
                let array = uc
                    .mem_read((this + 4) as u64, &mut buf)
                    .map(|_| u32::from_le_bytes(buf))
                    .unwrap_or(0);
                let cap = if array != 0 {
                    uc.mem_read(array as u64, &mut buf)
                        .map(|_| u16::from_le_bytes([buf[0], buf[1]]))
                        .unwrap_or(0xffff)
                } else {
                    0
                };
                let blocks = uc
                    .mem_read((this + 0x34) as u64, &mut buf)
                    .map(|_| u32::from_le_bytes(buf))
                    .unwrap_or(0);
                log::warn!(
                    "DAPI ActionList::bSetBlock this={:#x} name={:#x} size={:#x} idx={} actionnumber={} array={:#x} blocks={:#x} lr={:#x}",
                    this,
                    read(RegisterARM::R1),
                    read(RegisterARM::R2),
                    read(RegisterARM::R3),
                    cap,
                    array,
                    blocks,
                    read(RegisterARM::LR),
                );
            
                0
            });
    }
}

/// Where the map-data worker decides to answer with an error instead of data.
///
/// Every DAPI error answer is built on the same path:
/// `LISA_tclBasicWorker::vSetAndCreateDapiErrorJob(&result, code)` ->
/// `vTraceAndSetErrorJob(&result, code)` ->
/// `u16TransformDapLiFiErrorCodeToDapiErrorCode(code)` ->
/// `dap_tclJob::bInitErrorResponse(job, request, dapi_code)`, and
/// `dap_tclCommunicationContext::bReceiveInternalMsg` then turns that error job
/// into the `amt_tclServiceDataError` the client receives. The transform shows
/// where a client-visible code comes from (DAPI 0x305 <- LiFi 0x3001/0x3002 and
/// 0x3008/0x3009, 0x215 <- 0x1004/0x100c/0x100d, 0x218 <- 0x1002..0x1003 and
/// >0x22f, 0x204 <- >0x2000, else 0xd), so logging the *input* code together
/// with the failing call site names the data-access check that refused to
/// answer procmapengine's GetBlockIDs request.
fn hook_error_job_origin(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};

    for (off, tag) in [
        (0x00be_a5a8u32, "vSetAndCreateDapiErrorJob"),
        (0x00be_a554u32, "vTraceAndSetErrorJob"),
    ] {
        let address = base_address + (off - ORIGINAL_BASE);
        let tag: &'static str = tag;
        
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:887", move |uc: &mut Unicorn<'_, Context>| {
                static COUNT: AtomicU32 = AtomicU32::new(0);
                if COUNT.fetch_add(1, Ordering::Relaxed) >= 60 {
                    return 0;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                let word = |addr: u32| -> u32 {
                    let mut buf = [0u8; 4];
                    uc.mem_read(addr as u64, &mut buf)
                        .map(|_| u32::from_le_bytes(buf))
                        .unwrap_or(0)
                };
                let this = read(RegisterARM::R0);
                // `this + 0x76c` is the job the worker answers, `this + 8` the
                // request it came from; job+0x20 is the job type (4 = CCA
                // map-data service) and job+0x12 the u16 CCA service id.
                let answer_job = word(this + 0x76c);
                let request_job = word(this + 8);
                log::warn!(
                    "DAPI error-job {} worker={:#x} vtable={:#x} code={:#x} answer_job={:#x}(type={:#x} svc={:#x}) request_job={:#x}(type={:#x} svc={:#x}) lr={:#x}",
                    tag,
                    this,
                    word(this),
                    read(RegisterARM::R2) & 0xffff,
                    answer_job,
                    word(answer_job + 0x20),
                    word(answer_job + 0x12) & 0xffff,
                    request_job,
                    word(request_job + 0x20),
                    word(request_job + 0x12) & 0xffff,
                    read(RegisterARM::LR) & !1,
                );
            
                0
            });
    }

    // `dap_tclJob::poCreateErrorResult(request, code)`: the generic factory that
    // turns a failed request into the error answer (the code is passed to
    // `bInitErrorResponse` unchanged, so it is already a DAPI code, not a LiFi
    // one). Its caller is the layer that decided to fail the request, which is
    // what the LR names here.
    let create_error = base_address + (0x00b5_5d00u32 - ORIGINAL_BASE);
    
crate::os::code_stub::trap_site(unicorn, create_error, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:930", move |uc: &mut Unicorn<'_, Context>| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) >= 60 {
                return 0;
            }
            let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
            let mut buf = [0u8; 4];
            let request = read(RegisterARM::R0);
            let request_type = match uc.mem_read((request + 0x20) as u64, &mut buf) {
                Ok(()) => u32::from_le_bytes(buf),
                Err(_) => 0xffff_ffff,
            };
            let request_svc = match uc.mem_read((request + 0x12) as u64, &mut buf) {
                Ok(()) => u32::from_le_bytes(buf) & 0xffff,
                Err(_) => 0xffff,
            };
            log::warn!(
                "DAPI poCreateErrorResult request={:#x} request_type={:#x} request_svc={:#x} code={:#x} lr={:#x}",
                request,
                request_type,
                request_svc,
                read(RegisterARM::R1) & 0xffff,
                read(RegisterARM::LR) & !1,
            );
        
            0
        });

    // The translated code that actually reaches the client, plus its setter.
    let init_error = base_address + (0x00b5_4878u32 - ORIGINAL_BASE);
    
crate::os::code_stub::trap_site(unicorn, init_error, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:960", move |uc: &mut Unicorn<'_, Context>| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) >= 60 {
                return 0;
            }
            let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
            let mut buf = [0u8; 4];
            let job = read(RegisterARM::R0);
            let job_type = uc
                .mem_read((job + 0x20) as u64, &mut buf)
                .map(|_| u32::from_le_bytes(buf))
                .unwrap_or(0xffff_ffff);
            log::warn!(
                "DAPI bInitErrorResponse job={:#x} type={:#x} dapi_code={:#x} lr={:#x}",
                job,
                job_type,
                read(RegisterARM::R2) & 0xffff,
                read(RegisterARM::LR) & !1,
            );
        
            0
        });

    // `dap_map_tclWorker::u16SendUniqueIdList(state)`: the map worker's answer
    // path for a GetBlockIDs request. Whether it runs at all separates "DAPI
    // never got to the block-id lookup" from "the lookup produced nothing".
    let unique_ids = base_address + (0x0084_a2f8u32 - ORIGINAL_BASE);
    
crate::os::code_stub::trap_site(unicorn, unique_ids, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:987", move |uc: &mut Unicorn<'_, Context>| {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) >= 40 {
                return 0;
            }
            let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
            let mut buf = [0u8; 4];
            let this = read(RegisterARM::R0);
            let job = {
                uc.mem_read((this + 8) as u64, &mut buf)
                    .map(|_| u32::from_le_bytes(buf))
                    .unwrap_or(0)
            };
            log::warn!(
                "DAPI map-worker u16SendUniqueIdList this={:#x} job={:#x} lr={:#x}",
                this,
                job,
                read(RegisterARM::LR) & !1,
            );
        
            0
        });

    // Step-by-step inside `u16SendUniqueIdList`. These are mid-function sites in
    // already-translated code, and unicorn 2.x only emits the per-instruction
    // hook check while a translation block is generated, so a narrow range on a
    // hot function registers `Ok` but is never called (AGENTS.md). Hook the whole
    // 4 KiB page and dispatch on the fetch address instead.
    //
    // The first two targets also capture the deserialized request:
    // `fastmapfi_tclMsgGetBlockIDsMethodStart` is constructed at 0x84a310 (r0 is
    // the message) and filled by `bGetDapFiMsgFromAction`, so the words dumped
    // one instruction later are exactly what DAPI read out of procmapengine's
    // GetBlockIDs request (dataset id, data type, map scale, bbox).
    if crate::libs::procmapengine::render_trace_enabled() {
    let page_lo = base_address + (0x0084_a000u32 - ORIGINAL_BASE);
    let page_hi = page_lo + 0xfff;
    let stash = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let stashed_msg = std::sync::Arc::clone(&stash);
    
{
            // r746: page hook expanded to per-arm svc#0 trap sites.
            for __arm in [0x0084_a310u32, 0x0084_a344u32, 0x0084_a3d4u32, 0x0084_a424u32, 0x0084_a434u32, 0x0084_a4ccu32, 0x0084_a4e4u32, 0x0084_a4fcu32, 0x0084_a514u32, 0x0084_a524u32, 0x0084_a5a8u32, 0x0084_a5dcu32, 0x0084_a63cu32, 0x0084_a7c0u32, 0x0084_a8ccu32, 0x0084_a908u32] {
                let base_address = base_address.clone();
                let stash = stash.clone();
                let stashed_msg = stashed_msg.clone();
                let __arm: u32 = __arm;
                let __site_pc = base_address.wrapping_add(__arm.wrapping_sub(ORIGINAL_BASE));
                crate::os::code_stub::trap_site(unicorn, __site_pc, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:1060", move |uc: &mut Unicorn<'_, Context>| {
            use std::sync::atomic::Ordering as Ord;
            static HITS: std::sync::OnceLock<
                std::sync::Mutex<std::collections::HashMap<u32, u32>>,
            > = std::sync::OnceLock::new();
            let pc = __site_pc;
            let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
            if original == 0x0084_a310 {
                stashed_msg.store(
                    uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                    Ord::Relaxed,
                );
                return 0;
            }
            let label = match original {
                0x0084_a344 => "after bGetDapFiMsgFromAction (1=deserialized)",
                0x0084_a3d4 => "after u16CheckDatasetIdLocked (0xffff=ok)",
                0x0084_a424 => "after bIsRnwRegProfOutlinesValid_Locked (1=valid)",
                0x0084_a434 => "after bIsRsRegProfOutlinesValid_Locked (1=valid)",
                0x0084_a4cc => "after u16Lock (0xffff=ok)",
                0x0084_a4e4 => "after bIsLinkedTableInitialized",
                0x0084_a4fc => "after u16InitLinkedTable (0xffff=ok)",
                0x0084_a514 => "after bIsInitIdxCache",
                0x0084_a524 => "after bInitIdxCache (0=ok)",
                0x0084_a5a8 => "after u16CheckBBox (0xffff=ok, 0x218=invalid/too huge)",
                0x0084_a5dc => "after u16GenerateUniqueIds (0xffff=ok)",
                0x0084_a63c => "after u16Unlock (0xffff=ok)",
                0x0084_a7c0 => "after u16SendResponse (answer sent)",
                0x0084_a8cc => "after u16ProcessLoadRegProfOutlines (RNW outlines)",
                0x0084_a908 => "after u16ProcessLoadRsRegProfOutlines (RS outlines)",
                _ => return 0,
            };
            let seen = {
                let mut counts = HITS.get_or_init(Default::default).lock().unwrap();
                if counts.len() > 40 {
                    return 0;
                }
                *counts.entry(original).or_insert(0) += 1;
                *counts.get(&original).unwrap()
            };
            if seen > 8 {
                return 0;
            }
            let result = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            if original == 0x0084_a344 {
                let msg = stash.load(Ord::Relaxed);
                let mut words = Vec::with_capacity(16);
                for offset in (0..0x40).step_by(4) {
                    let mut buf = [0u8; 4];
                    words.push(
                        uc.mem_read((msg + offset) as u64, &mut buf)
                            .map(|_| u32::from_le_bytes(buf))
                            .unwrap_or(0xffff_ffff),
                    );
                }
                log::warn!(
                    "DAPI blockids-step {:#x}: {} r0={:#x} request_msg={:#x} words={:x?}",
                    original,
                    label,
                    result,
                    msg,
                    words,
                );
                return 0;
            }
            let worker = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
            let mut rnw = [0u8; 4];
            let mut rs = [0u8; 4];
            let rnwfi_ref = uc
                .mem_read((worker + 0x180) as u64, &mut rnw)
                .map(|_| u32::from_le_bytes(rnw))
                .unwrap_or(0xffff_ffff);
            let rsfi_ref = uc
                .mem_read((worker + 0x184) as u64, &mut rs)
                .map(|_| u32::from_le_bytes(rs))
                .unwrap_or(0xffff_ffff);
            log::warn!(
                "DAPI blockids-step {:#x}: {} r0={:#x} t={}ms worker={:#x} rnwfi_ref={:#x} ref+4={:#x}",
                original,
                label,
                result,
                crate::libs::dapi::emu_t0_elapsed_ms(),
                worker,
                rnwfi_ref,
                rsfi_ref,
            );
        
                    0
                });
            }
        }
    }
}

/// Step-by-step page watch inside `dap_map_tclWorker::u16ProcessOpenChannel`
/// (0x84b310) and `u16ProcessLoadBlocks` (0x849894), the two fastmapfi server
/// entry points procmapengine hits once the FastMap service is "available".
/// Narrow hooks are untrustworthy mid-TB (AGENTS.md), so each function's page
/// is hooked and dispatched on the original VA; sites picked from the
/// disassembly (bl + 4 after each decisive callee).
fn hook_fastmap_worker_steps(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    for page in [0x0084_9000u32, 0x0084_b000u32] {
        let page_lo = base_address + (page - ORIGINAL_BASE);
        let page_hi = page_lo + 0xfff;
        
{
                // r746: page hook expanded to per-arm svc#0 trap sites.
                for __arm in [0x0084_b350u32, 0x0084_b40cu32, 0x0084_b46cu32, 0x0084_b4bcu32, 0x0084_98f8u32, 0x0084_9a2cu32, 0x0084_9a9cu32, 0x0084_9af0u32] {
                    let base_address = base_address.clone();
                    let __arm: u32 = __arm;
                    let __site_pc = base_address.wrapping_add(__arm.wrapping_sub(ORIGINAL_BASE));
                    crate::os::code_stub::trap_site(unicorn, __site_pc, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:1162", move |uc: &mut Unicorn<'_, Context>| {
                use std::sync::atomic::Ordering as Ord;
                static HITS: std::sync::OnceLock<
                    std::sync::Mutex<std::collections::HashMap<u32, u32>>,
                > = std::sync::OnceLock::new();
                let pc = __site_pc;
                let original = pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                let label = match original {
                    // u16ProcessOpenChannel
                    0x0084_b350 => "OpenChannel: after bGetDapFiMsgFromAction (1=deserialized)",
                    0x0084_b40c => "OpenChannel: after u16OpenChannelToPoiServer (0xffff=fall through)",
                    0x0084_b46c => "OpenChannel: after u16CreateSharedMemChannel (0xffff=ok, else error)",
                    0x0084_b4bc => "OpenChannel: after u16SendResponse (r0=0 sent)",
                    // u16ProcessLoadBlocks
                    0x0084_98f8 => "LoadBlocks: after bGetDapFiMsgFromAction (1=deserialized)",
                    0x0084_9a2c => "LoadBlocks: after rfoGetChannelController/bHandleIsValid (0=invalid handle)",
                    0x0084_9a9c => "LoadBlocks: after rfoGetChannel-ish (0=fail branch)",
                    0x0084_9af0 => "LoadBlocks: after enSetAccessMode (2=write buffer ok)",
                    _ => return 0,
                };
                let seen = {
                    let mut counts = HITS.get_or_init(Default::default).lock().unwrap();
                    if counts.len() > 40 {
                        return 0;
                    }
                    *counts.entry(original).or_insert(0) += 1;
                    *counts.get(&original).unwrap()
                };
                if seen > 6 {
                    return 0;
                }
                let regs = [
                    RegisterARM::R0,
                    RegisterARM::R4,
                    RegisterARM::R7,
                ]
                .map(|r| (uc.reg_read(r).unwrap_or(0) as u32))
                    .iter()
                    .map(|v| format!("{:#x}", v))
                    .collect::<Vec<_>>()
                    .join(",");
                log::warn!(
                    "DAPI fastmap-step {:#x}: {} r0,r4,r7=[{}] t={}ms",
                    original,
                    label,
                    regs,
                    crate::libs::dapi::emu_t0_elapsed_ms(),
                );
            
                        0
                    });
                }
            }
    }
}

/// Trace the registry scan inside `ail_bHandleMsgServiceData` that decides
/// error id 6: the register-id taken from the request, the registry list
/// head (app+0x58), and every `ail_tclServiceRegistry::bIsDataSet` compare
/// with the entry's five u16 key fields.
fn hook_service_data_scan(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SCAN_COUNT: AtomicU32 = AtomicU32::new(0);
    static CMP_COUNT: AtomicU32 = AtomicU32::new(0);

    // Watch writes to the fwl_List header at app+0x58 (0xf614c8: head/tail/
    // count): entries are added correctly then vanish, so something rewrites
    // the header (or splices the chain) outside the add/remove API.
    {
        use unicorn_engine::unicorn_const::HookType;
        let hdr_lo = 0x00f6_14c8u64;
        let hdr_hi = 0x00f6_14d8u64;
        let res = unicorn.add_mem_hook(HookType::MEM_WRITE, hdr_lo, hdr_hi, move |uc, _t, address, size, value| {
            static HDR_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if HDR_COUNT.fetch_add(1, Ordering::Relaxed) >= 500 {
                return true;
            }
            let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
            log::warn!(
                "DAPI {} [{}] HDR-WRITE addr={:#x} size={} value={:#x} pc={:#x}",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                address,
                size,
                value,
                pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE),
            );
            true
        });
        log::warn!("DAPI registry hdr write-hook installed: {}", res.is_ok());
    }

    for (address, name, is_scan) in [
        (0x00b4_3218, "scan-begin (r6=request-register-id, r1=registry-head)", true),
        (0x00b4_61c8, "registry bIsDataSet compare", false),
        (0x00b4_3c00, "registry-list ADD notfound-entry (r0=list r1=sp+0x64)", false),
        (0x00b4_3c30, "registry-list ADD found-entry (r0=list r1=sp+0x74)", false),
        (0x00b3_69b0, "registry-list REMOVE (r0=list r1=&entry)", false),
        (0x00b3_5ec0, "registry-list NODE-DEL (r0=list r1=&iter)", false),
    ] {
        // (register-id wildcard patch is installed separately below)
        let address = base_address + (address - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:1228", move |uc: &mut Unicorn<'_, Context>| {
                let addr = address as u64;
                let counter = if is_scan { &SCAN_COUNT } else { &CMP_COUNT };
                let count = counter.fetch_add(1, Ordering::Relaxed);
                if count >= 600 {
                    return 0;
                }
                let read = |reg| uc.reg_read(reg).unwrap_or(0) as u32;
                let r0 = read(RegisterARM::R0);
                let r1 = read(RegisterARM::R1);
                let r2 = read(RegisterARM::R2);
                let r3 = read(RegisterARM::R3);
                let mut extra = [0u8; 4];
                let sp = read(RegisterARM::SP);
                let stack_arg = if uc.mem_read(sp as u64, &mut extra).is_ok() {
                    u32::from_le_bytes(extra)
                } else {
                    0
                };
                let mut fields = Vec::with_capacity(5);
                let entry_at_r1 = name.contains("ADD") || name.contains("REMOVE");
                let base = if name.contains("NODE-DEL") {
                    let mut nb = [0u8; 4];
                    if uc.mem_read(r1 as u64, &mut nb).is_ok() {
                        u32::from_le_bytes(nb).wrapping_add(8)
                    } else {
                        0
                    }
                } else if entry_at_r1 {
                    r1
                } else {
                    r0
                };
                if !is_scan {
                    let mut buf = [0u8; 2];
                    for offset in (0..8u32).step_by(2) {
                        let value = if uc.mem_read(base.wrapping_add(offset) as u64, &mut buf).is_ok()
                        {
                            u16::from_le_bytes(buf)
                        } else {
                            0xffff
                        };
                        fields.push(value);
                    }
                    let mut state = [0u8; 1];
                    fields.push(
                        if uc.mem_read(base.wrapping_add(8) as u64, &mut state).is_ok() {
                            state[0] as u16
                        } else {
                            0xff
                        },
                    );
                }
                let r6 = read(RegisterARM::R6);
                let lr = read(RegisterARM::LR);
                if entry_at_r1 {
                    // fwl_List header: [4]=head [8]=tail [0xc]=count. vAdd
                    // silently no-ops when count!=0 && tail==0.
                    let mut hdr = [0u8; 12];
                    let (head, tail, count) = if uc.mem_read(r0 as u64, &mut hdr).is_ok() {
                        (
                            u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) & 0,
                            u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]),
                            u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]),
                        )
                    } else {
                        (0, 0, 0)
                    };
                    let _ = head;
                    let mut vt = [0u8; 12];
                    let slots = if uc
                        .mem_read(
                            base_address.wrapping_add(0x00e7_2770 - ORIGINAL_BASE) as u64,
                            &mut vt,
                        )
                        .is_ok()
                    {
                        [
                            u32::from_le_bytes([vt[0], vt[1], vt[2], vt[3]]),
                            u32::from_le_bytes([vt[4], vt[5], vt[6], vt[7]]),
                            u32::from_le_bytes([vt[8], vt[9], vt[10], vt[11]]),
                        ]
                    } else {
                        [0; 3]
                    };
                    log::warn!(
                        "DAPI {} [{}] ADD live-vtable=[{:08x},{:08x},{:08x}] tail-node={:#x} prev={:#x} next={:#x}",
                        uc.get_data().elf_path,
                        uc.get_data().inner.thread_id(),
                        slots[0],
                        slots[1],
                        slots[2],
                        tail,
                        {
                            let mut p = [0u8; 8];
                            if uc.mem_read(tail as u64, &mut p).is_ok() {
                                u32::from_le_bytes([p[0], p[1], p[2], p[3]])
                            } else {
                                0
                            }
                        },
                        {
                            let mut p = [0u8; 8];
                            if uc.mem_read(tail.wrapping_add(4) as u64, &mut p).is_ok() {
                                u32::from_le_bytes([p[0], p[1], p[2], p[3]])
                            } else {
                                0
                            }
                        },
                    );
                    log::warn!(
                        "DAPI {} [{}] ADD list={:#x} head(+8)={:#x} tail(+c)={:#x} count+8={:#x}",
                        uc.get_data().elf_path,
                        uc.get_data().inner.thread_id(),
                        r0,
                        tail,
                        count,
                        {
                            let mut c = [0u8; 4];
                            if uc.mem_read(r0.wrapping_add(0xc) as u64, &mut c).is_ok() {
                                u32::from_le_bytes(c)
                            } else {
                                0xffffffff
                            }
                        },
                    );
                }
                log::warn!(
                    "DAPI {} [{}] {} addr={:#x} r0={:#x} r1={:#x} r2={:#x} r3={:#x} stack0={:#x} r6={:#x} lr={:#x} entry_u16s+state={:04x?}",
                    uc.get_data().elf_path,
                    uc.get_data().inner.thread_id(),
                    name,
                    addr,
                    r0,
                    r1,
                    r2,
                    r3,
                    stack_arg,
                    r6,
                    lr - base_address + ORIGINAL_BASE,
                    fields
                );
                if is_scan {
                    // Walk the ail_tclServiceRegistry list at r1 (app+0x58):
                    // node = [prev, next, entry{u16 key0..3, u8 state, ...}]
                    let r7 = read(RegisterARM::R7);
                    let mut app = [0u8; 4];
                    let app_id = if uc.mem_read(r7 as u64, &mut app).is_ok() {
                        u32::from_le_bytes(app)
                    } else {
                        0
                    };
                    let mut hdr = [0u8; 16];
                    let (w0, head, tail, count) = if uc.mem_read(r1 as u64, &mut hdr).is_ok() {
                        (
                            u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]),
                            u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]),
                            u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]),
                            u32::from_le_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]),
                        )
                    } else {
                        (0, 0, 0, 0)
                    };
                    log::warn!(
                        "DAPI {} [{}] scan-list app+0x10={:04x} app+0x58={:#x} hdr=[{:08x},{:08x},{:08x},{:08x}] tuple=({:04x},{:04x},{:04x},{:04x})",
                        uc.get_data().elf_path,
                        uc.get_data().inner.thread_id(),
                        app_id & 0xffff,
                        r1,
                        w0,
                        head,
                        tail,
                        count,
                        r6,
                        read(RegisterARM::R9),
                        read(RegisterARM::R8),
                        read(RegisterARM::R11),
                    );
                    let mut node = {
                        let mut b = [0u8; 4];
                        if uc.mem_read(r1.wrapping_add(4) as u64, &mut b).is_ok() {
                            u32::from_le_bytes(b)
                        } else {
                            0
                        }
                    };
                    let mut steps = 0;
                    while node != 0 && steps < 40 {
                        steps += 1;
                        let mut buf = [0u8; 2];
                        let mut nf = Vec::with_capacity(5);
                        for offset in (0..8u32).step_by(2) {
                            nf.push(if uc.mem_read(node.wrapping_add(8 + offset) as u64, &mut buf).is_ok() {
                                u16::from_le_bytes(buf)
                            } else {
                                0xffff
                            });
                        }
                        let mut st = [0u8; 1];
                        nf.push(if uc.mem_read(node.wrapping_add(16) as u64, &mut st).is_ok() {
                            st[0] as u16
                        } else {
                            0xff
                        });
                        let nx = {
                            let mut b = [0u8; 4];
                            if uc.mem_read(node.wrapping_add(4) as u64, &mut b).is_ok() {
                                u32::from_le_bytes(b)
                            } else {
                                0
                            }
                        };
                        log::warn!(
                            "DAPI {} [{}]   node={:#x} entry={:04x?} next={:#x}",
                            uc.get_data().elf_path,
                            uc.get_data().inner.thread_id(),
                            node,
                            nf,
                            nx
                        );
                        node = nx;
                    }
                }
            
                0
            });
    }

    // `ail_vAddRegistryEntryFromMessage` (0xb43b80) is reached from a handler
    // table and adds entries to the application's service registry; the register
    // handler itself starts at 0xb439c4. Log both entries with the key each one
    // carries, to find out which message produces the client entry for
    // procmapengine and whether the ServiceRegister reaches the handler at all.
    for (address, name) in [
        (0x00b4_39c4u32, "ail register handler"),
        (0x00b4_3b80, "ail_vAddRegistryEntryFromMessage"),
    ] {
        let entry = base_address + (address - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, entry, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:1465", move |uc: &mut Unicorn<'_, Context>| {
                static ADDER_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if ADDER_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 60 {
                    return 0;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                let r0 = read(RegisterARM::R0);
                let r1 = read(RegisterARM::R1);
                let r2 = read(RegisterARM::R2);
                let r3 = read(RegisterARM::R3);
                let lr = read(RegisterARM::LR)
                    .wrapping_sub(base_address)
                    .wrapping_add(ORIGINAL_BASE);
                // Whatever looks like a registry key (regid, service, client,
                // sub) at the pointer arguments.
                let key_at = |base: u32| -> Option<[u16; 4]> {
                    if base < 0x0010_0000 || base > 0xf000_0000 {
                        return None;
                    }
                    let mut buf = [0u8; 8];
                    uc.mem_read(base as u64, &mut buf).ok()?;
                    Some([
                        u16::from_le_bytes([buf[0], buf[1]]),
                        u16::from_le_bytes([buf[2], buf[3]]),
                        u16::from_le_bytes([buf[4], buf[5]]),
                        u16::from_le_bytes([buf[6], buf[7]]),
                    ])
                };
                let k1 = key_at(r1);
                let k2 = key_at(r2);
                let k3 = key_at(r3);
                log::warn!(
                    "DAPI {} [{}] {} at {:#x}: r0={:#x} r1={:#x} r2={:#x} r3={:#x} key@r1={:04x?} key@r2={:04x?} key@r3={:04x?} lr={:#x}",
                    uc.get_data().elf_path,
                    uc.get_data().inner.thread_id(),
                    name,
                    entry,
                    r0,
                    r1,
                    r2,
                    r3,
                    k1,
                    k2,
                    k3,
                    lr,
                );
            
                0
            });
    }

}

/// Track server-side CCA service registration: DAPI answers client requests
/// only for services present in the app's own service-registry list. An
/// empty list makes `ail_bHandleMsgServiceData` answer error id 6.
fn hook_service_registration(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

    for (address, name) in [
        (0x0082_8158, "dap_tclDataManager::u16RegisterServices"),
        (0x00b6_2630, "dap_tclSrvCrtl::u16AddService"),
        (0x00b3_b368, "ail::u16RegisterService"),
        (0x00b5_2d3c, "dap_tclCommunicationContext::u16RegisterService"),
    ] {
        let address = base_address + (address - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:1533", move |uc: &mut Unicorn<'_, Context>| {
                let addr = address as u64;
                let count = TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 60 {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                    log::warn!(
                        "DAPI {} [{}] {} addr={:#x} r0={:#x} r1={:#x} r2={:#x} r3={:#x} lr={:#x}",
                        uc.get_data().elf_path,
                        uc.get_data().inner.thread_id(),
                        name,
                        addr,
                        r0,
                        r1,
                        r2,
                        r3,
                        lr
                    );
                }
            
                0
            });
    }
}

/// `ail_bHandleMsgServiceRegister` decides whether a client REGISTER gets a
/// registry entry + positive RegisterConf. Trace its entry, both vtable
/// checks and the three negative-conf branches so the observed status-4
/// confirmation (impossible for this handler) can be attributed.
fn hook_service_register_handler(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

    for (address, name) in [
        (0x00b4_39c4, "ServiceRegister entry (r0=wrapper)"),
        (0x00b4_3a70, "ServiceRegister after-version-check (r0=ver ok)"),
        (0x00b4_3ad0, "ServiceRegister after-service-check (r0=1 ok)"),
        (0x00b4_3e40, "ServiceRegister conf status=2 service-rejected"),
        (0x00b4_3e64, "ServiceRegister conf status=3 version-rejected"),
        (0x00b4_3e88, "ServiceRegister conf status=0 list-access-failed"),
        (0x00b4_35b8, "ServiceRegisterConf handler entry"),
    ] {
        let address = base_address + (address - ORIGINAL_BASE);
        let inject_at = address;
        
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:1579", move |uc: &mut Unicorn<'_, Context>| {
                let addr = address as u64;
                let count = TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 40 {
                    return 0;
                }
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let r9 = uc.reg_read(RegisterARM::R9).unwrap_or(0) as u32;
                let r10 = uc.reg_read(RegisterARM::R10).unwrap_or(0) as u32;
                let r11 = uc.reg_read(RegisterARM::R11).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                log::warn!(
                    "DAPI {} [{}] {} addr={:#x} r0={:#x} r9(srcApp)={:#x} r10(svc)={:#x} r11(sub)={:#x} lr={:#x}",
                    uc.get_data().elf_path,
                    uc.get_data().inner.thread_id(),
                    name,
                    addr,
                    r0,
                    r9,
                    r10,
                    r11,
                    lr
                );
                let _ = inject_at;
            
                0
            });
    }
}

/// `ail_bHandleMsgServiceData` builds `amt_tclServiceDataError` with error
/// id 6 (unknown register-id: registry has no matching entry) or 0xb
/// (service known but not available). Dump the offending request header so
/// the missing register-id is visible.
fn hook_service_data_errors(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

    for (address, name) in [
        (0x00b4_32fc, "ServiceDataError id=6 unknown-register"),
        (0x00b4_33f4, "ServiceDataError id=0xb temp-unavailable"),
    ] {
        let address = base_address + (address - ORIGINAL_BASE);
        
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:1621", move |uc: &mut Unicorn<'_, Context>| {
                let addr = address as u64;
                let count = TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count < 20 {
                    let service_data = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let mut buf = [0u8; 4];
                    let mut read_u32 = |address: u32| {
                        if uc.mem_read(address as u64, &mut buf).is_ok() {
                            u32::from_le_bytes(buf)
                        } else {
                            0
                        }
                    };
                    let message = read_u32(service_data);
                    let mut dump = Vec::with_capacity(8);
                    for offset in (0..0x20u32).step_by(4) {
                        dump.push(read_u32(message.wrapping_add(offset)));
                    }
                    log::warn!(
                        "DAPI {} [{}] {} addr={:#x} request_message={:#x} header={:08x?}",
                        uc.get_data().elf_path,
                        uc.get_data().inner.thread_id(),
                        name,
                        addr,
                        message,
                        dump
                    );
                }
            
                0
            });
    }
}
fn read_u32_at(unicorn: &mut Unicorn<'_, Context>, address: u32) -> u32 {
    let mut buf = [0u8; 4];
    unicorn
        .mem_read(address as u64, &mut buf)
        .map(|_| u32::from_le_bytes(buf))
        .unwrap_or(0xffff_ffff)
}

/// `dap_tclSrvCrtl::poGetWorker` (0xb62680) chooses the worker that will serve a job
/// by scanning the `{worker, message type}` pairs of its control block (8 bytes per
/// entry, entry count as u16 at `+0x40`) for the job's message type at `job + 0x20`.
/// The regionselectionfi request (type 0x17, the RS region-profile outlines DAPI needs
/// before it can answer `GetBlockIDs`) comes back without a worker - DAPI then reports
/// "Unexpected message" and answers 0x305 - even though `u16AddService` really did
/// register a worker for 0x17.  Dump the table that is searched, the types it holds and
/// what the job asks for, so the mismatch (wrong control block, wrong type, or a worker
/// that refuses the message through its vtable+0x08 filter) becomes visible.
fn hook_srvctrl_worker_lookup(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    const PO_GET_WORKER: u32 = 0x00b6_2680 - ORIGINAL_BASE;
    let address = base_address + PO_GET_WORKER;
    
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:1674", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            let hits = HITS.fetch_add(1, Ordering::Relaxed);
            if hits >= 48 {
                return 0;
            }
            let srvctrl = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let job_slot = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let job = read_u32_at(uc, job_slot);
            let message_type = read_u32_at(uc, job + 0x20);
            let count = u16::try_from(
                read_u32_at(uc, srvctrl + 0x40) & 0xffff,
            )
            .unwrap_or(0xffff);
            let types = (0..u32::from(count).min(16))
                .map(|index| format!("{:#x}", read_u32_at(uc, srvctrl + index * 8 + 4)))
                .collect::<Vec<_>>()
                .join(" ");
            // The worker for the requested type (if any) is a sub-object of the data
            // manager; its first word is the vtable, and vtable+0x08 is the method
            // `vProcessJob` calls.  Print those so the handler that produced the error
            // can be opened in Ghidra.
            let mut matched = String::new();
            for index in 0..u32::from(count).min(16) {
                let slot = read_u32_at(uc, srvctrl + index * 8 + 4);
                if slot != message_type {
                    continue;
                }
                let worker = read_u32_at(uc, srvctrl + index * 8);
                let vtable = read_u32_at(uc, worker);
                let slots = (0..5u32)
                    .map(|n| format!("{:#x}", read_u32_at(uc, vtable + n * 4)))
                    .collect::<Vec<_>>()
                    .join(" ");
                matched = format!("worker={:#x} vtable={:#x} vt=[{}]", worker, vtable, slots);
            }
            let _ = &types;
            log::warn!(
                "DAPI srvctrl worker lookup t={}ms thread=[{}] srvctrl={:#x} count={} types=[{}] job={:#x} type={:#x} fct={:#x} {matched} lr={:#x}",
                emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                srvctrl,
                count,
                types,
                job,
                message_type,
                read_u32_at(uc, job + 0x0a) & 0xffff,
                uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
            );
        
            0
        });

    // `vProcessJob` calls the *assigned* worker's vtable+0x08 right after
    // `poGetWorker` returns (the bl is at 0x82af9c, so the result is in r0 one
    // instruction later).  For the regionselectionfi request that assigned worker
    // is a freshly created `rs_tclWorker`, and the method it runs is the code that
    // answers with error 0x305 - so capture the vtable it dispatches through.
    // Hooked as a page range on purpose (mid-TB narrow hooks can never fire).
    if crate::libs::procmapengine::render_trace_enabled() {
    let page_lo = base_address + 0x0082_a000u32 - ORIGINAL_BASE;
    let page_hi = page_lo + 0xfff;
    
{
            // r746: page hook expanded to per-arm svc#0 trap sites.
            const AFTER_PO_GET_WORKER: u32 = 0x0082_afa0;
            for __arm in [AFTER_PO_GET_WORKER] {
                let base_address = base_address.clone();
                let __arm: u32 = __arm;
                let __site_pc = base_address.wrapping_add(__arm.wrapping_sub(ORIGINAL_BASE));
                crate::os::code_stub::trap_site(unicorn, __site_pc, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:1780", move |uc: &mut Unicorn<'_, Context>| {
            static ASSIGNED: AtomicU32 = AtomicU32::new(0);
            let pc = __site_pc;
            if pc.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE) != AFTER_PO_GET_WORKER {
                return 0;
            }
            let hits = ASSIGNED.fetch_add(1, Ordering::Relaxed);
            if hits >= 12 {
                return 0;
            }
            let worker = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let vtable = read_u32_at(uc, worker);
            let slots = (0..4u32)
                .map(|n| format!("{:#x}", read_u32_at(uc, vtable + n * 4)))
                .collect::<Vec<_>>()
                .join(" ");
            log::warn!(
                "DAPI assigned worker t={}ms thread=[{}] worker={:#x} vtable={:#x} vt=[{}]",
                emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                worker,
                vtable,
                slots,
            );
        
                    0
                });
            }
        }
    }
}

/// `rs_tclWorker::enProcessGetRegionOutlines` (0x869ab4) answers the regionselectionfi
/// `GETREGIONOUTLINES` command the map worker sends to obtain the region-profile
/// outlines DAPI requires before it can turn a `GetBlockIDs` request into block ids.
/// It looks the requested media id up in its `rs_tclDataContext`: the stored media id at
/// `context + 0x354` (with the medium's name at `context + 0x78`) or the one at
/// `context + 0x1a0` (name at `context + 0x70`).  When neither matches it stores error
/// 0x305 ("medium not found media id %d") - which is exactly the answer the engine gets
/// today, so the RS data context has no medium registered.  Dump the ids and names the
/// context really holds next to the stack window that carries the requested id.
fn hook_rs_region_outlines_gate(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static WORKER: AtomicU32 = AtomicU32::new(0);
    static HITS: AtomicU32 = AtomicU32::new(0);
    // Both addresses live in one 4 KiB page: hook the page and dispatch on the fetch
    // address, because a narrow mid-TB hook can register `Ok` and never be called.
    let page_lo = base_address + 0x0086_9000u32 - ORIGINAL_BASE;
    let page_hi = page_lo + 0xfff;
    
{
            // r746: page hook expanded to per-arm svc#0 trap sites.
            for __arm in [0x0086_9ab4u32, 0x0086_9f90u32] {
                let base_address = base_address.clone();
                let __arm: u32 = __arm;
                let __site_pc = base_address.wrapping_add(__arm.wrapping_sub(ORIGINAL_BASE));
                crate::os::code_stub::trap_site(unicorn, __site_pc, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:1828", move |uc: &mut Unicorn<'_, Context>| {
            let offset = (__site_pc)
                .wrapping_sub(base_address)
                .wrapping_add(ORIGINAL_BASE);
            if offset == 0x0086_9ab4 {
                WORKER.store(uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32, Ordering::Relaxed);
                return 0;
            }
            if offset != 0x0086_9f90 {
                return 0;
            }
            let hits = HITS.fetch_add(1, Ordering::Relaxed);
            if hits >= 6 {
                return 0;
            }
            let worker = WORKER.load(Ordering::Relaxed);
            let context = read_u32_at(uc, worker + 0x14);
            let name_a = read_u32_at(uc, context + 0x70);
            let name_b = read_u32_at(uc, context + 0x78);
            let text = |uc: &mut Unicorn<'_, Context>, at: u32| -> String {
                if at == 0 || at == 0xffff_ffff {
                    return "<null>".to_string();
                }
                (0..24u32)
                    .map(|i| read_u32_at(uc, at + i))
                    .flat_map(|word| word.to_le_bytes())
                    .take_while(|byte| *byte != 0)
                    .map(|byte| char::from(byte))
                    .collect()
            };
            let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
            let stack = (0..24u32)
                .map(|i| format!("{:#x}", read_u32_at(uc, sp + i * 4)))
                .collect::<Vec<_>>()
                .join(" ");
            log::warn!(
                "DAPI rs-getregionoutlines t={}ms thread=[{}] worker={:#x} ctx={:#x} media_at_1a0={:#x} name[+0x70]={} media_at_354={:#x} name[+0x78]={} stack=[{}]",
                emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                worker,
                context,
                read_u32_at(uc, context + 0x1a0),
                text(uc, name_a),
                read_u32_at(uc, context + 0x354),
                text(uc, name_b),
                stack,
            );
        
                    0
                });
            }
        }
}

/// `rs_tclManager::u16InterpretBroadcast` (0x858880) is how the regionselection component
/// hears about media and devices: it only acts on messages whose `+0x15` byte is 6 and
/// then routes function id 0x83 to `u16InterpretMediaInfoMsg` and 0x84 to
/// `u16InterpretDeviceInfoMsg` - the messages that fill the media id/name slots of the
/// `rs_tclDataContext` (see `hook_rs_region_outlines_gate`).  Log every broadcast that
/// reaches the manager to see whether the media-info one is missing or arrives and is
/// refused.
fn hook_rs_broadcasts(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    const INTERPRET_BROADCAST: u32 = 0x0085_8880 - ORIGINAL_BASE;
    let address = base_address + INTERPRET_BROADCAST;
    
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:1847", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            let hits = HITS.fetch_add(1, Ordering::Relaxed);
            if hits >= 16 {
                return 0;
            }
            let job = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            log::warn!(
                "DAPI rs-broadcast t={}ms thread=[{}] manager={:#x} job={:#x} type={:#x} fct={:#x} byte15={:#x} lr={:#x}",
                emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                job,
                read_u32_at(uc, job + 0x20),
                read_u32_at(uc, job + 0x0a) & 0xffff,
                read_u32_at(uc, job + 0x15) & 0xff,
                uc.reg_read(RegisterARM::R14).unwrap_or(0) as u32,
            );
        
            0
        });
}

/// `rs_tclManager::u16InterpretMediaInfoMsg` (0x8580fc) only registers a medium when the
/// media-info control reports the medium *names* the manager holds at `+0x470` (link
/// medium) and `+0x474` (nav medium) as available - the nav one additionally has to
/// report state 6 before `u32GetFullAvailMediaId` is stored into the data context.  Dump
/// those names on entry and the data context's media slots again right after
/// `u16InterpretBroadcast` returns (called at 0x858b08 from `poAssignWorker`), so a
/// missing name and a refused availability become distinguishable.
fn hook_rs_media_info(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static ENTRY_HITS: AtomicU32 = AtomicU32::new(0);
    static AFTER_HITS: AtomicU32 = AtomicU32::new(0);
    let page_lo = base_address + 0x0085_8000u32 - ORIGINAL_BASE;
    let page_hi = page_lo + 0xfff;
    
{
            // r746: page hook expanded to per-arm svc#0 trap sites.
            for __arm in [0x0085_80fcu32, 0x0085_8b0cu32] {
                let base_address = base_address.clone();
                let __arm: u32 = __arm;
                let __site_pc = base_address.wrapping_add(__arm.wrapping_sub(ORIGINAL_BASE));
                crate::os::code_stub::trap_site(unicorn, __site_pc, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:1928", move |uc: &mut Unicorn<'_, Context>| {
            let offset = (__site_pc)
                .wrapping_sub(base_address)
                .wrapping_add(ORIGINAL_BASE);
            let text = |uc: &mut Unicorn<'_, Context>, name: u32| -> String {
                if name == 0 || name == 0xffff_ffff {
                    return "<null>".to_string();
                }
                (0..32u32)
                    .map(|i| read_u32_at(uc, name + i))
                    .flat_map(|word| word.to_le_bytes())
                    .take_while(|byte| *byte != 0)
                    .map(|byte| char::from(byte))
                    .collect()
            };
            if offset == 0x0085_80fc {
                let hits = ENTRY_HITS.fetch_add(1, Ordering::Relaxed);
                if hits >= 8 {
                    return 0;
                }
                let manager = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let context = manager + 0x54;
                if hits < 3 {
                    // What does the media-info message actually carry?  The manager
                    // compares the message's media names against its own `+0x470` and
                    // `+0x474` strings, so dump the job and every pointer it reaches,
                    // with the ASCII of each, to see whether a name ever arrives.
                    let job = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let ascii_at = |uc: &mut Unicorn<'_, Context>, at: u32| -> String {
                        (0..40u32)
                            .map(|i| read_u32_at(uc, at + i))
                            .flat_map(|word| word.to_le_bytes())
                            .map(|byte| if byte.is_ascii_graphic() || byte == b'/' || byte == b' ' {
                                char::from(byte)
                            } else {
                                '.'
                            })
                            .collect::<String>()
                    };
                    let mut pointers = Vec::new();
                    for index in (0..0x60u32).step_by(4) {
                        let word = read_u32_at(uc, job + index);
                        if (0x00f0_0000..0x0140_0000).contains(&word) {
                            pointers.push(format!("+{index:#x}->{word:#x}:{}", ascii_at(uc, word)));
                        }
                    }
                    let config = read_u32_at(uc, manager + 0x08);
                    let mut config_refs = Vec::new();
                    for index in (0..0x60u32).step_by(4) {
                        let word = read_u32_at(uc, config + index);
                        if (0x00f0_0000..0x0140_0000).contains(&word) {
                            config_refs.push(format!("+{index:#x}->{word:#x}:{}", ascii_at(uc, word)));
                        }
                    }
                    log::warn!(
                        "DAPI rs-media-info config dump t={}ms manager={:#x} config={:#x} words={:x?}\n  cfg refs: {}",
                        emu_t0_elapsed_ms(),
                        manager,
                        config,
                        (0..0x18u32)
                            .map(|i| read_u32_at(uc, config + i * 4))
                            .collect::<Vec<_>>(),
                        config_refs.join("\n  "),
                    );
                    log::warn!(
                        "DAPI rs-media-info job dump t={}ms job={:#x} words={:x?}\n  refs: {}",
                        emu_t0_elapsed_ms(),
                        job,
                        (0..0x30u32)
                            .map(|i| read_u32_at(uc, job + i * 4))
                            .collect::<Vec<_>>(),
                        pointers.join("\n  "),
                    );
                }
                let link_name = read_u32_at(uc, manager + 0x470);
                let nav_name = read_u32_at(uc, manager + 0x474);
                let link_avail = read_u32_at(uc, context + 0x4a0) & 0xff;
                let media_a = read_u32_at(uc, context + 0x1a0);
                let name_a = read_u32_at(uc, context + 0x70);
                let media_b = read_u32_at(uc, context + 0x354);
                let name_b = read_u32_at(uc, context + 0x78);
                log::warn!(
                    "DAPI rs-media-info entry t={}ms thread=[{}] manager={:#x} link_name[+0x470]={} nav_name[+0x474]={} link_avail={} ctx_media[1a0]={:#x} name[+0x70]={} ctx_media[354]={:#x} name[+0x78]={}",
                    emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    manager,
                    text(uc, link_name),
                    text(uc, nav_name),
                    link_avail,
                    media_a,
                    text(uc, name_a),
                    media_b,
                    text(uc, name_b),
                );
                return 0;
            }
            if offset != 0x0085_8b0c {
                return 0;
            }
            let hits = AFTER_HITS.fetch_add(1, Ordering::Relaxed);
            if hits >= 8 {
                return 0;
            }
            // r0 carries the error the interpreter left behind in `*param_2`'s place;
            // the manager is the object that `poAssignWorker` worked on (r7 there).
            let manager = uc.reg_read(RegisterARM::R7).unwrap_or(0) as u32;
            let context = manager + 0x54;
            let media_a = read_u32_at(uc, context + 0x1a0);
            let name_a = read_u32_at(uc, context + 0x70);
            let media_b = read_u32_at(uc, context + 0x354);
            let name_b = read_u32_at(uc, context + 0x78);
            let status = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            log::warn!(
                "DAPI rs-media-info after t={}ms thread=[{}] manager={:#x} r0={:#x} ctx_media[1a0]={:#x} name[+0x70]={} ctx_media[354]={:#x} name[+0x78]={}",
                emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                manager,
                status,
                media_a,
                text(uc, name_a),
                media_b,
                text(uc, name_b),
            );
        
                    0
                });
            }
        }
}

/// `dap_tclMediaInfoCtrl` is nothing but a deserialized `dap_devmanfi_tclMsgMediaInfoStatus`
/// (device-manager FI media-info status) message: the media it lists live in a
/// `bpstl::vector<fi_tcl_DevMediaInfo>` at `ctrl + 0x08`, and the regionselection manager
/// only registers a medium whose *name* appears in that vector (with entry type 6).  Dump
/// the requested name together with the raw buffer of the vector, so it is visible which
/// media the device manager actually announces - that is the name the (currently empty)
/// `APP_CONFIG/DEVICES` registry values `DEVICE_NAV_1..4` / `DEFAULT_DEV` have to provide.
fn hook_media_info_vector(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    const NAV_ACCESSABLE: u32 = 0x008c_4bb8 - ORIGINAL_BASE;
    let address = base_address + NAV_ACCESSABLE;
    
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:2022", move |uc: &mut Unicorn<'_, Context>| {
            static HITS: AtomicU32 = AtomicU32::new(0);
            let hits = HITS.fetch_add(1, Ordering::Relaxed);
            if hits >= 4 {
                return 0;
            }
            let ctrl = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let asked = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            let name = |uc: &mut Unicorn<'_, Context>, at: u32| -> String {
                if at == 0 || at == 0xffff_ffff {
                    return "<null>".to_string();
                }
                (0..40u32)
                    .map(|i| read_u32_at(uc, at + i))
                    .flat_map(|word| word.to_le_bytes())
                    .take_while(|byte| *byte != 0)
                    .map(|byte| char::from(byte))
                    .collect()
            };
            // `bpstl::vector` is {allocator/vtable, begin, end, capacity} and it sits at
            // `ctrl + 0x08`, so the data pointers are `ctrl + 0x0c` / `ctrl + 0x10`.
            let begin = read_u32_at(uc, ctrl + 0x0c);
            let finish = read_u32_at(uc, ctrl + 0x10);
            let capacity = read_u32_at(uc, ctrl + 0x14);
            let span = finish.wrapping_sub(begin) as usize;
            let words = (0..span.min(0x60) / 4)
                .map(|i| format!("{:#x}", read_u32_at(uc, begin + (i * 4) as u32)))
                .collect::<Vec<_>>()
                .join(" ");
            let ascii = (0..span.min(0x80))
                .map(|i| {
                    let byte = (read_u32_at(uc, begin + i as u32) & 0xff) as u8;
                    if byte == 0 {
                        '.'
                    } else if byte.is_ascii_graphic() || byte == b'/' || byte == b' ' {
                        char::from(byte)
                    } else {
                        '?'
                    }
                })
                .collect::<String>();
            let mut derefs = Vec::new();
            for index in (0..60u32).step_by(4) {
                let word = read_u32_at(uc, begin + index);
                if (0x00e0_0000..0x0140_0000).contains(&word) {
                    derefs.push(format!("+{index:#04x}->{word:#x}:{:?}", name(uc, word)));
                }
            }
            log::warn!(
                "DAPI media-info deref t={}ms entries={} {}\n  raw ascii: {}",
                emu_t0_elapsed_ms(),
                span / 60,
                derefs.join("\n  "),
                ascii,
            );
            log::warn!(
                "DAPI media-info vector t={}ms thread=[{}] ctrl={:#x} asked={:?} entries={} vector=[{:#x},{:#x},{:#x}] span={:#x} words=[{}]\n  ascii: {}",
                emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                ctrl,
                name(uc, asked),
                span / 60,
                begin,
                finish,
                capacity,
                span,
                words,
                ascii,
            );
        
            0
        });
}

/// `dap_tclDapiApp::vEvalDeviceConfig` (0x822630) is what transfers the
/// `APP_CONFIG/DEVICES` registry values into the shared `dap_tclConfiguration` (at
/// `dapiapp + 0x7c`): `DEFAULT_DEV` into `pcGetDefaultDevice()`, `DEVICE_ONLINE_LINKING`
/// straight into the configuration's inline name buffer at `+0x54e` (which
/// `rs_tclManager`'s constructor hands out as its link medium at `+0x470`), and each
/// `DEVICE_NAV_n` triple into `bAddNavDevice` (0x82255c).  Log every evaluation and every
/// registration so it becomes visible which of the values the accessor actually serves.
fn hook_dapi_device_config(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static EVAL_HITS: AtomicU32 = AtomicU32::new(0);
    static POST_AFTER: AtomicU32 = AtomicU32::new(0);
    static ADD_HITS: AtomicU32 = AtomicU32::new(0);
    const EVAL: u32 = 0x0082_2630 - ORIGINAL_BASE;
    const ADD_DEVICE: u32 = 0x0082_255c - ORIGINAL_BASE;
    for (offset, _) in [(EVAL, "eval"), (ADD_DEVICE, "add")] {
        let address = base_address + offset;
        let which = if offset == EVAL { 0u8 } else { 1u8 };
        
crate::os::code_stub::trap_site(unicorn, address, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:2113", move |uc: &mut Unicorn<'_, Context>| {
                let text = |uc: &mut Unicorn<'_, Context>, at: u32| -> String {
                    if at == 0 || at == 0xffff_ffff {
                        return "<null>".to_string();
                    }
                    (0..32u32)
                        .map(|i| read_u32_at(uc, at + i))
                        .flat_map(|word| word.to_le_bytes())
                        .take_while(|byte| *byte != 0)
                        .map(|byte| char::from(byte))
                        .collect()
                };
                if which == 0 {
                    let hits = EVAL_HITS.fetch_add(1, Ordering::Relaxed);
                    if hits >= 12 {
                        return 0;
                    }
                    let app = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let config = app + 0x7c;
                    log::warn!(
                        "DAPI device-config eval t={}ms thread=[{}] app={:#x} config={:#x} accessor={:#x} config[+0x54e]={:?} config[+0x56c]={:?}",
                        emu_t0_elapsed_ms(),
                        uc.get_data().inner.thread_id(),
                        app,
                        config,
                        uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32,
                        text(uc, config + 0x54e),
                        text(uc, config + 0x56c),
                    );
                    // One-shot probe at the return site: if a `DEVICE_ONLINE_LINKING` or
                    // `DEVICE_OL_SOURCE_DEVICE` read succeeded it landed in these inline
                    // configuration buffers, which is what `rs_tclManager` reads as its
                    // medium names, so their content after the call is decisive.
                    let after = (uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32).wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE);
                    if (0x0082_0000..0x0083_0000).contains(&after) {
                        let posted = POST_AFTER.fetch_add(1, Ordering::Relaxed);
                        if posted < 4 {
                            let lo = base_address + after - ORIGINAL_BASE;
                            // r751: our trap-site flavour of the old one-shot
                            // return tap.  Queued from inside a handler it is
                            // patched at the next svc-flush (every syscall);
                            // for a 4-shot log probe that is early enough.
                            crate::os::code_stub::trap_site(uc, lo, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:2246", move |inner: &mut Unicorn<'_, Context>| {
                                let app = {
                                    let sp = inner.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                                    read_u32_at(inner, sp + 0x20)
                                };
                                let name = |uc: &mut Unicorn<'_, Context>, at: u32| -> String {
                                    (0..32u32)
                                        .map(|i| read_u32_at(uc, at + i))
                                        .flat_map(|word| word.to_le_bytes())
                                        .take_while(|byte| *byte != 0)
                                        .map(|byte| char::from(byte))
                                        .collect()
                                };
                                let mut dump = Vec::new();
                                if (0x00f0_0000..0x0140_0000).contains(&app) {
                                    for off in (0x540u32..0x5f0).step_by(4) {
                                        let word = name(inner, app + off);
                                        if word.chars().any(|c| c != '.') {
                                            dump.push(format!("+{off:#x}={word:?}"));
                                        }
                                    }
                                }
                                log::warn!(
                                    "DAPI device-config post t={}ms r0={:#x} candidate_app={:#x} raw[sp+0x10..0x38]={:x?}",
                                    emu_t0_elapsed_ms(),
                                    inner.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                                    app,
                                    (0x10u32..0x38).step_by(4).map(|o| read_u32_at(inner, inner.reg_read(RegisterARM::SP).unwrap_or(0) as u32 + o)).collect::<Vec<_>>(),
                                );
                                if !dump.is_empty() {
                                    log::warn!("DAPI device-config post strings: {}", dump.join(" "));
                                }
                            0
                            });
                        }
                    }
                    return 0;
                }
                let hits = ADD_HITS.fetch_add(1, Ordering::Relaxed);
                if hits >= 12 {
                    return 0;
                }
                let name = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                let prm = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                let max_fd = (uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32) & 0xff;
                let stack = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                let app = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                // The `DEVICE_ONLINE_LINKING` branch passes the configuration's own inline
                // name buffer (`dapiapp + 0x7c + 0x54e`) - the field `rs_tclManager` uses as
                // its medium name - while the `DEVICE_NAV_n` branches pass a stack buffer.
                // The call site in `lr` separates them unambiguously.
                let from_config_field = name == app.wrapping_add(0x7c + 0x54e);
                let online_field = text(uc, app.wrapping_add(0x7c + 0x54e));
                log::warn!(
                    "DAPI device-config add-nav-device t={}ms thread=[{}] name={:?} prm={:?} max_fd={} optical={} lr={:#x} name_is_config_54e={} config_field={:?}",
                    emu_t0_elapsed_ms(),
                    uc.get_data().inner.thread_id(),
                    text(uc, name),
                    text(uc, prm),
                    max_fd,
                    read_u32_at(uc, stack) & 0xff,
                    lr.wrapping_sub(base_address).wrapping_add(ORIGINAL_BASE),
                    from_config_field,
                    online_field,
                );
            
                0
            });
    }
}

/// `dap_dev_tclFileManagement::s32OpenCreate` (0x8ac488) is DAPI's own file layer: before
/// any syscall happens it looks the path up in its registered-file table
/// (`s32SearchForCompleteFileName`, 0x8aba88: `count` at `this + 4`, descriptor array at
/// `*this`, 0x7c bytes each, case-insensitive compare against `pcGetCompleteFileName`) and
/// returns 0xffff without touching the OS when the file is unknown.  Measured at
/// 0x8ac4a4 - right after that lookup, where r0 holds the index (0xffffffff = unknown),
/// r10 the path and r6 the file-management object - so the table content itself is dumped
/// whenever a lookup misses.
fn hook_dapi_file_lookup(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static HITS: AtomicU32 = AtomicU32::new(0);
    static MISSES: AtomicU32 = AtomicU32::new(0);
    let page_lo = base_address + 0x008a_c000u32 - ORIGINAL_BASE;
    let page_hi = page_lo + 0xfff;
    
{
            // r746: page hook expanded to per-arm svc#0 trap sites.
            for __arm in [0x008a_c4a4u32] {
                let base_address = base_address.clone();
                let __arm: u32 = __arm;
                let __site_pc = base_address.wrapping_add(__arm.wrapping_sub(ORIGINAL_BASE));
                crate::os::code_stub::trap_site(unicorn, __site_pc, crate::os::code_stub::TrapMode::Arm, "libs/dapi.rs:2283", move |uc: &mut Unicorn<'_, Context>| {
            let offset = (__site_pc)
                .wrapping_sub(base_address)
                .wrapping_add(ORIGINAL_BASE);
            if offset != 0x008a_c4a4 {
                return 0;
            }
            let hits = HITS.fetch_add(1, Ordering::Relaxed);
            if hits >= 400 {
                return 0;
            }
            let path_ptr = uc.reg_read(RegisterARM::R10).unwrap_or(0) as u32;
            let files = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
            let index = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            let path = {
                (0..64u32)
                    .map(|i| read_u32_at(uc, path_ptr + i))
                    .flat_map(|word| word.to_le_bytes())
                    .take_while(|byte| *byte != 0)
                    .map(|byte| char::from(byte))
                    .collect::<String>()
            };
            let count = read_u32_at(uc, files + 0x04);
            let base = read_u32_at(uc, files + 0x00);
            log::warn!(
                "DAPI filemgmt open t={}ms thread=[{}] path={:?} index={:#x} table={} files={}",
                emu_t0_elapsed_ms(),
                uc.get_data().inner.thread_id(),
                path,
                index,
                base,
                count,
            );
            if index != 0xffff_ffff {
                return 0;
            }
            let misses = MISSES.fetch_add(1, Ordering::Relaxed);
            if misses >= 8 || base == 0 || base == 0xffff_ffff {
                return 0;
            }
            // Descriptor layout is only partly known; the registered path is the longest
            // printable run inside the 0x7c-byte record, so extract it that way.
            let names = (0..count.min(30))
                .map(|entry| {
                    let record = base + entry * 0x7c;
                    let mut best = String::new();
                    let mut current = String::new();
                    for i in 0..0x7cu32 {
                        let byte = (read_u32_at(uc, record + i) & 0xff) as u8;
                        if byte.is_ascii_graphic() || byte == b'/' || byte == b' ' {
                            current.push(char::from(byte));
                        } else {
                            if current.len() > best.len() {
                                best = std::mem::take(&mut current);
                            }
                            current.clear();
                        }
                    }
                    if current.len() > best.len() {
                        best = current;
                    }
                    best
                })
                .collect::<Vec<_>>()
                .join(", ");
            log::warn!("DAPI filemgmt table ({} entries): {}", count, names);
        
                    0
                });
            }
        }
}
