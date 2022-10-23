use crate::emulator::context::Context;
use crate::os::syscalls::{fcntl, mmap, stat, unistd, utsname};
use unicorn_engine::{RegisterARM, Unicorn};

pub fn hook_syscall(unicorn: &mut Unicorn<Context>, int_no: u32) {
    // table:
    // - https://marcin.juszkiewicz.com.pl/download/tables/syscalls.html
    // - https://github.com/qilingframework/qiling/blob/master/qiling/os/linux/map_syscall.py
    //
    // sample implementations:
    // - https://github.com/zeropointdynamics/zelos/blob/master/src/zelos/ext/platforms/linux/syscalls/syscalls.py
    // - https://github.com/qilingframework/qiling/tree/master/qiling/os/posix/syscall
    let res = match unicorn.get_syscall_number() {
        3 => unistd::read(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        5 => fcntl::open(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        6 => unistd::close(unicorn, unicorn.get_u32_arg(0)),
        33 => unistd::access(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        45 => unistd::brk(unicorn, unicorn.get_u32_arg(0)),
        90 => mmap::mmap(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
            unicorn.get_u32_arg(5),
        ),
        122 => utsname::uname(unicorn, unicorn.get_u32_arg(0)),
        192 => mmap::mmap2(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
            unicorn.get_u32_arg(5),
        ),
        197 => stat::fstat64(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        _ => {
            panic!(
                "{:#x}: not implemented syscall #{} (int {}), args: {:#x}, {:#x}, {:#x}, ...",
                unicorn.reg_read(RegisterARM::PC).unwrap(),
                unicorn.get_syscall_number(),
                int_no,
                unicorn.get_u32_arg(0),
                unicorn.get_u32_arg(1),
                unicorn.get_u32_arg(2),
            );
        }
    };
    unicorn.set_u32_result(res);
}

trait Args {
    fn get_syscall_number(&self) -> u32;
    fn get_u32_arg(&self, num: i32) -> u32;
    fn set_u32_result(&mut self, res: u32);
}

impl<'a> Args for Unicorn<'a, Context> {
    fn get_syscall_number(&self) -> u32 {
        self.reg_read_i32(RegisterARM::R7).unwrap() as u32
    }

    fn get_u32_arg(&self, num: i32) -> u32 {
        match num {
            0 => self.reg_read_i32(RegisterARM::R0).unwrap() as u32,
            1 => self.reg_read_i32(RegisterARM::R1).unwrap() as u32,
            2 => self.reg_read_i32(RegisterARM::R2).unwrap() as u32,
            3 => self.reg_read_i32(RegisterARM::R3).unwrap() as u32,
            4 => self.reg_read_i32(RegisterARM::R4).unwrap() as u32,
            5 => self.reg_read_i32(RegisterARM::R5).unwrap() as u32,
            6 => self.reg_read_i32(RegisterARM::R6).unwrap() as u32,
            _ => panic!("wrong argument number"),
        }
    }

    fn set_u32_result(&mut self, res: u32) {
        self.reg_write(RegisterARM::R0 as i32, res as u64).unwrap();
    }
}
