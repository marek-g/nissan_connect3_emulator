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
    hook_force_return(
        unicorn,
        base_address,
        0x00b4_8a04,
        "ail::ail_bIpcMessagePost",
        0x0007_2000,
    );
}