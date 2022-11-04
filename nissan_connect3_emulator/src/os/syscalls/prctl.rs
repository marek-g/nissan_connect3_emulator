use crate::emulator::context::Context;
use crate::emulator::utils::read_string;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn prctl(
    unicorn: &mut Unicorn<Context>,
    option: u32,
    arg2: u32,
    arg3: u32,
    arg4: u32,
    arg5: u32,
) -> u32 {
    // TODO: implement
    let res = 0;

    if option == 15 {
        // PR_SET_NAME = set process name
        let process_name = read_string(unicorn, arg2);
        log::trace!("Process name: {}", process_name);
    }

    log::trace!(
        "{:#x}: [{}] [SYSCALL] prctl(option = {:#x}, arg2: {:#x}, arg3: {:#x}, arg4: {:#x}, arg5: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        option,
        arg2,
        arg3,
        arg4,
        arg5,
        res
    );

    res
}
