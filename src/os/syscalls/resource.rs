use crate::emulator::context::Context;
use crate::emulator::memory_map::STACK_SIZE;
use crate::emulator::utils::pack_i64;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn ugetrlimit(unicorn: &mut Unicorn<Context>, resource: u32, r_limit: u32) -> u32 {
    let res = match resource {
        3 => {
            // RLIMIT_STACK
            unicorn
                .mem_write(r_limit as u64, &pack_i64(STACK_SIZE as i64))
                .unwrap();
            unicorn
                .mem_write((r_limit + 8) as u64, &pack_i64(-1i64))
                .unwrap();
            0
        }
        _ => panic!("not implemented"),
    };

    log::trace!(
        "{:#x}: [SYSCALL] ugetrlimit(resource = {:#x}, r_limit: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        resource,
        r_limit,
        res
    );

    res
}
