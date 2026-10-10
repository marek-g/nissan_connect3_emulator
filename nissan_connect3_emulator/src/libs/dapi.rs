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
    unicorn
        .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
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
        })
        .unwrap();
}

fn hook_result(
    unicorn: &mut Unicorn<'_, Context>,
    base_address: u32,
    original_address: u32,
    name: &'static str,
) {
    let address = base_address + (original_address - ORIGINAL_BASE);
    unicorn
        .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
            let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            log::warn!(
                "DAPI {} [{}] {} result addr={:#x} r0={:#x}",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                name,
                addr,
                r0
            );
        })
        .unwrap();
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
    unicorn
        .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
            let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
            uc.reg_write(RegisterARM::R0, value as u64).unwrap();
            uc.reg_write(RegisterARM::PC, lr as u64).unwrap();
            log::warn!(
                "DAPI {} [{}] {} stubbed addr={:#x} -> {:#x}",
                uc.get_data().elf_path,
                uc.get_data().inner.thread_id(),
                name,
                addr,
                value
            );
        })
        .unwrap();
}

fn hook_power_dispatch_state(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let address = base_address + (0x00b4_2098 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
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
        })
        .unwrap();
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
    unicorn
        .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
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
        })
        .unwrap();
}

fn hook_power_ack_call(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let address = base_address + (0x00b4_23b0 - ORIGINAL_BASE);
    unicorn
        .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
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
        })
        .unwrap();
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
    hook_service_state_setter(unicorn, base_address);
    hook_registry_guard(unicorn, base_address);
    hook_service_status_handler(unicorn, base_address);
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
        unicorn
            .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
                let count = TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 800 {
                    return;
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
            })
            .unwrap();
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
        let res = unicorn.add_code_hook(open_entry as u64, open_entry as u64, {
            let base = base_address;
            move |uc, _addr, _| {
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
                let _ = base;
            }
        });
        if res.is_err() {
            log::debug!("dapi: could not hook OSAL_IOOpen wrapper");
        }
    }

    for (address, name) in [
        (0x008b_4d7c, "bRegPRM ioctl-before (fd in r5)"),
        (0x008b_4d84, "bRegPRM ioctl-after (r0=result)"),
        (0x008b_5094, "bRegPRM open-failed path (800)"),
        (0x008b_50c0, "bRegPRM empty-path path (0x218)"),
    ] {
        let address = base_address + (address - ORIGINAL_BASE);
        unicorn
            .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
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
            })
            .unwrap();
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
        unicorn
            .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
                let count = TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 4000 {
                    return;
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
            })
            .unwrap();
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
        unicorn
            .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
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
            })
            .unwrap();
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

/// Register-id DAPIAPP currently holds a client registration for, for the map
/// data service and client application 0x0400, or 0xffff when it has none.
/// Maintained by the service registry list hooks (see
/// `ail_vPostServiceDataError6AndUnregister`, which removes the entry it finds
/// rather than the one a message names).
pub static MAP_DATA_LIVE_REGISTER_ID: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0xffff);

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
    unicorn
        .add_code_hook(address as u64, address as u64, move |uc, _, _| {
            let count = SET_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= 300 {
                return;
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
        })
        .unwrap();
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
        unicorn
            .add_code_hook(address as u64, address as u64, move |uc, _, _| {
                static HOOK_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if HOOK_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 40 {
                    return;
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
            })
            .unwrap();
    }

    // Service-data success path: after the registry scan + state gate passes,
    // `ail_bHandleMsgServiceData` tail-calls the app vtable slot +0x20 at
    // 0xb4357c. Log the resolved handler target, and also the return-0
    // fall-through at 0xb43518 (dispatch skipped).
    {
        let dispatch = base_address + (0x00b4_3578u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(dispatch as u64, dispatch as u64, move |uc, _, _| {
                static D_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if D_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 40 {
                    return;
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
            })
            .unwrap();

        // The dispatch decision itself: `cmp r9,#0xfffe` at 0xb434e0 picks
        // between the generic service handler (vtable+0x20) and the
        // app-info/no-op fall-through.
        let decide = base_address + (0x00b4_34d4u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(decide as u64, decide as u64, move |uc, _, _| {
                static P_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if P_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 40 {
                    return;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                log::warn!(
                    "DAPI svcdata-PATH r7(msg)={:#x} r9={:#x} r10={:#x}",
                    read(RegisterARM::R7),
                    read(RegisterARM::R9),
                    read(RegisterARM::R10),
                );
            })
            .unwrap();

        let skip = base_address + (0x00b4_3518u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(skip as u64, skip as u64, move |uc, _, _| {
                static S_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if S_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 40 {
                    return;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                log::warn!(
                    "DAPI svcdata-SKIP-DISPATCH r9={:#x} r10={:#x} r4={:#x}",
                    read(RegisterARM::R9),
                    read(RegisterARM::R10),
                    read(RegisterARM::R4),
                );
            })
            .unwrap();

        // `dap_tclCommunicationContext::vSendToTask`: a request whose target
        // task index is 0 or whose connection-table entry is missing is
        // dropped with status 0x213 and the client never gets an answer.
        let send = base_address + (0x00b5_1ca8u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(send as u64, send as u64, move |uc, _, _| {
                static T_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if T_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 4000 {
                    return;
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
                    return;
                }
                log::warn!(
                    "DAPI vSendToTask jobtype={} thread-idx={} task={:#x} job={:#x}",
                    job_type,
                    read(RegisterARM::R7),
                    read(RegisterARM::R0),
                    job,
                );
            })
            .unwrap();

        let drop = base_address + (0x00b5_1cc8u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(drop as u64, drop as u64, move |uc, _, _| {
                static X_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if X_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 40 {
                    return;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                log::warn!(
                    "DAPI vSendToTask DROPPED (status 0x213) thread-idx={} job={:#x} lr={:#x}",
                    read(RegisterARM::R7),
                    read(RegisterARM::R4),
                    read(RegisterARM::LR),
                );
            })
            .unwrap();

        // `dap_tclJob::u16AddAction` -> `dap_tclActionList::bSetBlock`: an
        // external CCA request is turned into a job here; when the action
        // block cannot be opened the job is discarded and the client never
        // hears back. Log the list state so the failure mode is visible
        // (+0x30 = capacity, +0x34 = block-descriptor array).
        // `dap_map_tclWorker::vReportError(code, file, line, func)`: every
        // failure of the CCA map-data worker ends here.
        let report = base_address + (0x0084_4e78u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(report as u64, report as u64, move |uc, _, _| {
                static R_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if R_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 40 {
                    return;
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
            })
            .unwrap();

        // `ActionList::bSetBlock` entry logging is installed below.
        let setblock = base_address + (0x00b6_b610u32 - ORIGINAL_BASE);
        unicorn
            .add_code_hook(setblock as u64, setblock as u64, move |uc, _, _| {
                static A_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if A_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 60 {
                    return;
                }
                let read = |r| uc.reg_read(r).unwrap_or(0) as u32;
                let this = read(RegisterARM::R0);
                let mut buf = [0u8; 4];
                let cap = uc
                    .mem_read((this + 0x30) as u64, &mut buf)
                    .map(|_| u16::from_le_bytes([buf[0], buf[1]]))
                    .unwrap_or(0xffff);
                let blocks = uc
                    .mem_read((this + 0x34) as u64, &mut buf)
                    .map(|_| u32::from_le_bytes(buf))
                    .unwrap_or(0);
                log::warn!(
                    "DAPI ActionList::bSetBlock this={:#x} name={:#x} size={:#x} idx={} capacity={} blocks={:#x} lr={:#x}",
                    this,
                    read(RegisterARM::R1),
                    read(RegisterARM::R2),
                    read(RegisterARM::R3),
                    cap,
                    blocks,
                    read(RegisterARM::LR),
                );
            })
            .unwrap();
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
        unicorn
            .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
                let counter = if is_scan { &SCAN_COUNT } else { &CMP_COUNT };
                let count = counter.fetch_add(1, Ordering::Relaxed);
                if count >= 600 {
                    return;
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
                // Which register-id DAPIAPP currently has registered for the map
                // data service (0xffff when none). The mailbox bridge uses this
                // to drop a deregistration naming a different handle: the
                // handler for that message removes the entry it *finds* by
                // service and client and answers error 6, which destroys the
                // registration procmapengine actually uses.
                if !is_scan && fields.len() == 5 && fields[1] == 0x0026 && fields[2] == 0x0400 {
                    if name.contains("ADD") {
                        MAP_DATA_LIVE_REGISTER_ID.store(fields[0] as u32, Ordering::Relaxed);
                    } else if name.contains("REMOVE") || name.contains("NODE-DEL") {
                        MAP_DATA_LIVE_REGISTER_ID.store(0xffff, Ordering::Relaxed);
                    }
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
            })
            .unwrap();
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
        unicorn
            .add_code_hook(entry as u64, entry as u64, move |uc, _, _| {
                static ADDER_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                if ADDER_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 60 {
                    return;
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
            })
            .unwrap();
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
        unicorn
            .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
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
            })
            .unwrap();
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
        unicorn
            .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
                let count = TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
                if count >= 40 {
                    return;
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
            })
            .unwrap();
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
        unicorn
            .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
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
            })
            .unwrap();
    }
}