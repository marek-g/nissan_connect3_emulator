use crate::emulator::context::Context;
use std::io::Write;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn uname(unicorn: &mut Unicorn<Context>, buf: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] uname(buf = {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        buf,
    );

    let res = {
        const UTS_LEN: usize = 65;

        let mut data = [0u8; UTS_LEN * 6];
        write!(&mut data[UTS_LEN * 0..], "Linux").unwrap(); // sysname
        write!(&mut data[UTS_LEN * 1..], "Linux-Marek").unwrap(); // nodename
        write!(&mut data[UTS_LEN * 2..], "2.6.32").unwrap(); // release
        write!(&mut data[UTS_LEN * 3..], "#1-Linux").unwrap(); // version
        write!(&mut data[UTS_LEN * 4..], "armv6l").unwrap(); // machine
        write!(&mut data[UTS_LEN * 5..], "(none)").unwrap(); // domainname

        unicorn.mem_write(buf as u64, &data).unwrap();
        0
    };

    log::trace!(
        "{:#x}: [{}] uname => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        res,
    );
    res
}
