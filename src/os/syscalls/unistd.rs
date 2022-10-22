use crate::emulator::context::Context;
use unicorn_engine::Unicorn;

pub fn brk(unicorn: &mut Unicorn<Context>, addr: u32) -> u32 {
    let res = if addr == 0 {
        unicorn.get_data().mmu.heap_mem_end
    } else {
        panic!("not implemented");
    };

    log::trace!("brk(addr = {:#x}) => {:#x}", addr, res);
    res
}
