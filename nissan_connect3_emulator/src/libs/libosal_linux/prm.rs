use crate::emulator::context::Context;
use crate::os::code_stub::add_code_stub;
use unicorn_engine::Unicorn;

const ORIGINAL_BASE: u32 = 0x484d_8000;
const PRM_INITIALIZE_USB: u32 = 0x4850_3554 - ORIGINAL_BASE;
const PRM_SET_OC_HANDLER: u32 = 0x4850_371c - ORIGINAL_BASE;
const PRM_USB_INITIALIZED_ADDRESS: u32 = 0x4856_cd7c;

pub fn hook_prm_code(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    // TODO: emulate the USB hub/power monitor once the host-side USB backend
    // exists. Until then report the hub initialization and oc_handler setup as
    // successful so PRM_LIBUSB does not exit while waiting for sysfs attributes.
    add_code_stub(
        unicorn,
        "LIBOSAL",
        base_address + PRM_INITIALIZE_USB,
        "PRM_s32InitializePRMUSB_data",
        initialize_prm_usb,
    );
    add_code_stub(
        unicorn,
        "LIBOSAL",
        base_address + PRM_SET_OC_HANDLER,
        "PRM_vSetOcHandler",
        |_| 0,
    );
}

fn initialize_prm_usb(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    unicorn
        .mem_write(PRM_USB_INITIALIZED_ADDRESS as u64, &1u32.to_le_bytes())
        .unwrap();
    0
}
