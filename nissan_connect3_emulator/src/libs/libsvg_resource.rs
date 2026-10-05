use crate::emulator::context::Context;
use unicorn_engine::Unicorn;

pub fn libsvg_resource_add_code_hooks(_unicorn: &mut Unicorn<'_, Context>, _base_address: u32) {}