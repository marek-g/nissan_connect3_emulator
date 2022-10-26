use crate::emulator::context::Context;
use crate::emulator::utils::unpack_u32;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn writev(unicorn: &mut Unicorn<Context>, fd: u32, iov: u32, iovcnt: u32) -> u32 {
    let res = if unicorn.get_data_mut().file_system.is_open(fd as i32) {
        let mut written_bytes = 0;
        let mut iov_buf = vec![0u8; (iovcnt * 8) as usize];
        unicorn.mem_read(iov as u64, &mut iov_buf).unwrap();
        for index in 0..iovcnt as usize {
            let addr = unpack_u32(&iov_buf[index * 8..index * 8 + 4]);
            let len = unpack_u32(&iov_buf[index * 8 + 4..index * 8 + 8]);
            let mut buf = vec![0u8; len as usize];
            unicorn.mem_read(addr as u64, &mut buf).unwrap();

            match unicorn
                .get_data_mut()
                .file_system
                .write_all(fd as i32, &buf)
            {
                Ok(_) => {
                    written_bytes += len;
                }
                Err(_) => {}
            }
        }
        written_bytes
    } else {
        -1i32 as u32
    };

    log::trace!(
        "{:#x}: [SYSCALL] writev(fd: {:#x}, iov: {:#x}, iovcnt: {:#x}) => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        fd,
        iov,
        iovcnt,
        res
    );

    res
}
