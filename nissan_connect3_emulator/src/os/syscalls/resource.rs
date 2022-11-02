use crate::emulator::context::Context;
use crate::emulator::memory_map::STACK_SIZE;
use crate::emulator::utils::pack_i64;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn set_priority(unicorn: &mut Unicorn<Context>, which: u32, who: u32, prio: u32) -> u32 {
    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] set_priority(which = {:#x}, who: {:#x}, prio: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        which,
        who,
        prio,
        res
    );

    res
}

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
        "{:#x}: [{}] [SYSCALL] ugetrlimit(resource = {:#x}, r_limit: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        resource,
        r_limit,
        res
    );

    res
}
