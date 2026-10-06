use crate::emulator::context::Context;
use crate::emulator::process_launcher;
use crate::emulator::utils::read_string;
use unicorn_engine::{RegisterARM, Unicorn};

const OSAL_ERR_NONE: u32 = 0x72_0000;
const OSAL_ERR_INTERNAL: u32 = 0x4d02;
const OSAL_PROCESS_SPAWN: u32 = 0x4851_6d0c - 0x484d_8000;

fn is_out_process(filename: &str) -> bool {
    let lower = filename.to_ascii_lowercase();
    lower.ends_with(".out")
}

pub fn hook_process_code(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let address = base_address + OSAL_PROCESS_SPAWN;
    unicorn
        .add_code_hook(address as u64, address as u64, move |uc, _, _| {
            let struct_ptr = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
            if struct_ptr == 0 {
                log::warn!("OSAL_ProcessSpawn called with null process info");
                uc.reg_write(RegisterARM::R0, OSAL_ERR_INTERNAL as u64)
                    .unwrap_or_default();
                uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap_or(0))
                    .unwrap_or_default();
                return;
            }

            let Some(app_name_ptr) = read_u32(uc, struct_ptr) else {
                uc.reg_write(RegisterARM::R0, OSAL_ERR_INTERNAL as u64)
                    .unwrap_or_default();
                uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap_or(0))
                    .unwrap_or_default();
                return;
            };
            let Some(filename_ptr) = read_u32(uc, struct_ptr + 8) else {
                uc.reg_write(RegisterARM::R0, OSAL_ERR_INTERNAL as u64)
                    .unwrap_or_default();
                uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap_or(0))
                    .unwrap_or_default();
                return;
            };
            let Some(cmdline_ptr) = read_u32(uc, struct_ptr + 12) else {
                uc.reg_write(RegisterARM::R0, OSAL_ERR_INTERNAL as u64)
                    .unwrap_or_default();
                uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap_or(0))
                    .unwrap_or_default();
                return;
            };

            let app_name = read_string(uc, app_name_ptr);
            let filename = read_string(uc, filename_ptr);
            let cmdline = read_string(uc, cmdline_ptr);

            if !is_out_process(&filename) {
                log::debug!(
                    "OSAL_ProcessSpawn: leaving module load for non-.out process name={} file={} cmdline={}",
                    app_name,
                    filename,
                    cmdline
                );
                return;
            }

            match process_launcher::spawn_process(&app_name, &filename, &cmdline) {
                Ok(()) => {
                    uc.reg_write(RegisterARM::R0, OSAL_ERR_NONE as u64)
                        .unwrap_or_default();
                    uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap_or(0))
                        .unwrap_or_default();
                }
                Err(error) => {
                    log::error!(
                        "OSAL_ProcessSpawn failed app='{}' path={} cmdline='{}': {}",
                        app_name,
                        filename,
                        cmdline,
                        error
                    );
                    uc.reg_write(RegisterARM::R0, OSAL_ERR_INTERNAL as u64)
                        .unwrap_or_default();
                    uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap_or(0))
                        .unwrap_or_default();
                }
            }
        })
        .unwrap();
}

fn read_u32(unicorn: &mut Unicorn<'_, Context>, address: u32) -> Option<u32> {
    let mut bytes = [0u8; 4];
    unicorn
        .mem_read(address as u64, &mut bytes)
        .ok()
        .map(|_| u32::from_le_bytes(bytes))
}