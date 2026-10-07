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
    ] {
        hook_entry(unicorn, base_address, address, name);
    }
}