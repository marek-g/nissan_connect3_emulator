use crate::emulator::context::Context;
use crate::os::code_stub::add_code_stub;
use unicorn_engine::Unicorn;

const ORIGINAL_BASE: u32 = 0x484d_8000;
const CHANGE_SVG_CONFIGURATION: u32 = 0x484e_d90c - ORIGINAL_BASE;

const SVG_CONFIG_LINK: &str = "/tmp/SVGconfigSymbolicLink";
const SVG_CONFIG_TARGET: &str = "/etc/svg_config_Square_Pixel.ini";

pub fn hook_svg_code(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    add_code_stub(
        unicorn,
        "LIBOSAL",
        base_address + CHANGE_SVG_CONFIGURATION,
        "vChangeSVG_Configuration",
        change_svg_configuration,
    );
}

fn change_svg_configuration(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let thread = unicorn.get_data().inner.thread_id();
    let file_system = unicorn.get_data().inner.file_system.clone();

    {
        let mut file_system = file_system.lock().unwrap();
        let _ = file_system.unlink(SVG_CONFIG_LINK);
        match file_system.symlink(SVG_CONFIG_TARGET, SVG_CONFIG_LINK) {
            Ok(()) => log::info!(
                "[{}] [LIBOSAL] vChangeSVG_Configuration -> {} -> {}",
                thread,
                SVG_CONFIG_LINK,
                SVG_CONFIG_TARGET
            ),
            Err(err) => log::warn!(
                "[{}] [LIBOSAL] vChangeSVG_Configuration symlink failed: {:?}",
                thread,
                err
            ),
        }
    }

    0u32
}