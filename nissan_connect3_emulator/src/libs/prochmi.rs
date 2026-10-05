use crate::emulator::context::Context;
use unicorn_engine::Unicorn;

const ORIGINAL_BASE: u32 = 0x0000_8000;
const B_IS_HMI_THREAD_RUNNING_GOT: u32 = 0x02a1_faa8;
const HMI_THREAD_RUNNING_TRUE_BYTE: u32 = 0x02a1_faa0;
const HMICCA_TCL_APP_B_ON_INIT: u32 = 0x00fc_6ec0 - ORIGINAL_BASE;
const CL_HMI_MNGR_S_INITIALIZE: u32 = 0x0134_f540 - ORIGINAL_BASE;
const CL_LUA_DEBUGGER_S_INITIALIZE: u32 = 0x0133_ae10 - ORIGINAL_BASE;
const TRAMPOLINE_SIZE: usize = 20;

pub fn prochmi_add_code_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    install_hmi_mngr_initialize_before_b_on_init(unicorn, base_address);
    force_hmi_thread_running_true(unicorn);
}

fn install_hmi_mngr_initialize_before_b_on_init(
    unicorn: &mut Unicorn<'_, Context>,
    base_address: u32,
) {
    let b_on_init = base_address + HMICCA_TCL_APP_B_ON_INIT;
    let hmi_initialize = base_address + CL_HMI_MNGR_S_INITIALIZE;
    let trampoline = base_address + CL_LUA_DEBUGGER_S_INITIALIZE;

    let mut original_prologue = [0_u8; 4];
    unicorn
        .mem_read(b_on_init as u64, &mut original_prologue)
        .unwrap();

    let mut trampoline_bytes = Vec::with_capacity(TRAMPOLINE_SIZE);
    trampoline_bytes.extend_from_slice(&[0x11, 0x40, 0x2d, 0xe9]); // push {r0, r4, lr}
    trampoline_bytes.extend_from_slice(&arm_bl(trampoline + 4, hmi_initialize));
    trampoline_bytes.extend_from_slice(&[0x11, 0x40, 0xbd, 0xe8]); // pop {r0, r4, lr}
    trampoline_bytes.extend_from_slice(&original_prologue);
    trampoline_bytes.extend_from_slice(&arm_b(trampoline + 16, b_on_init + 4));

    unicorn
        .mem_write(trampoline as u64, &trampoline_bytes)
        .unwrap();
    unicorn
        .mem_write(b_on_init as u64, &arm_b(b_on_init, trampoline))
        .unwrap();

    log::debug!(
        "PROCHMI: hmicca_tclApp::bOnInit({:#x}) -> clHMIMngr::s_initialize({:#x}) via trampoline({:#x})",
        b_on_init,
        hmi_initialize,
        trampoline
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

fn arm_b(from: u32, to: u32) -> [u8; 4] {
    arm_branch(from, to, 0xea00_0000)
}

fn arm_branch(from: u32, to: u32, opcode: u32) -> [u8; 4] {
    let offset = to
        .wrapping_sub(from + 8)
        .checked_shr(2)
        .expect("ARM branch offset must be word aligned");
    (opcode | (offset & 0x00ff_ffff)).to_le_bytes()
}