use crate::emulator::context::Context;
use crate::emulator::utils::unpack_u32;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn writev(unicorn: &mut Unicorn<Context>, fd: u32, iov: u32, iovcnt: u32) -> u32 {
    let res = if let Some(file) = unicorn.get_data_mut().file_system.fd_to_file(fd) {
        // TODO:
        panic!("not implemented")
    } else if fd == 1 || fd == 2 {
        let mut written_bytes = 0;
        let mut iov_buf = vec![0u8; (iovcnt * 8) as usize];
        unicorn.mem_read(iov as u64, &mut iov_buf).unwrap();
        for index in 0..iovcnt as usize {
            let addr = unpack_u32(&iov_buf[index * 8..index * 8 + 4]);
            let len = unpack_u32(&iov_buf[index * 8 + 4..index * 8 + 8]);
            let mut buf = vec![0u8; len as usize];
            unicorn.mem_read(addr as u64, &mut buf).unwrap();
            let str = String::from_utf8(buf).unwrap();
            if fd == 1 {
                print!("{}", str);
            } else {
                eprint!("{}", str);
            }
            written_bytes += len;
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
