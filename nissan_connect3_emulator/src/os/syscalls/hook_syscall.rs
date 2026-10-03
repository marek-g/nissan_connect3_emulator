use crate::emulator::context::Context;
use crate::emulator::thread::{block_current_thread, exit_current_thread, exit_process, ThreadAction};
use crate::os::syscalls::{
    fcntl, futex, ioctl, linux, mman, mqueue, prctl, resource, sched, signal, socket, stat, time,
    uio, unistd, utsname,
};
use unicorn_engine::{RegisterARM, Unicorn};

pub fn hook_syscall(unicorn: &mut Unicorn<'_, Context>, int_no: u32) {
    // A stubbed function entry was patched to `svc #0`. When it executes the intr
    // hook fires with PC already advanced past the svc, so the entry address is
    // PC - 4. If this interrupt came from one of our stubs, run its handler, return
    // the value in R0 and jump back to the caller (PC = LR), skipping the original
    // body (which never runs). Otherwise fall through to a real syscall.
    let pc = unicorn.reg_read(RegisterARM::PC).unwrap() as u32;
    let stub = {
        let map = unicorn.get_data().code_stubs.lock().unwrap();
        map.get(&pc.wrapping_sub(4)).copied()
    };
    if let Some(stub) = stub {
        let tid = unicorn.get_data().thread_id();
        log::trace!("{:#x}: [{}] [{} HOOK] {}() [IN]", pc, tid, stub.lib, stub.name);
        let res = (stub.handler)(unicorn);
        log::trace!(
            "{:#x}: [{}] [{} HOOK] {}() => {}",
            pc,
            tid,
            stub.lib,
            stub.name,
            res
        );
        unicorn.reg_write(RegisterARM::R0, res as u64).unwrap();
        unicorn
            .reg_write(RegisterARM::PC, unicorn.reg_read(RegisterARM::LR).unwrap())
            .unwrap();
        return;
    }

    // table:
    // - https://marcin.juszkiewicz.com.pl/download/tables/syscalls.html
    // - https://github.com/qilingframework/qiling/blob/master/qiling/os/linux/map_syscall.py
    //
    // sample implementations:
    // - https://github.com/zeropointdynamics/zelos/blob/master/src/zelos/ext/platforms/linux/syscalls/syscalls.py
    // - https://github.com/qilingframework/qiling/tree/master/qiling/os/posix/syscall
    let res = match unicorn.get_syscall_number() {
        1 => unistd::exit(unicorn, unicorn.get_u32_arg(0)),
        3 => unistd::read(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        4 => unistd::write(
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
        9 => unistd::link(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        10 => unistd::unlink(unicorn, unicorn.get_u32_arg(0)),
        19 => unistd::lseek(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        20 => unistd::get_pid(unicorn),
        33 => unistd::access(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        39 => stat::mkdir(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        45 => unistd::brk(unicorn, unicorn.get_u32_arg(0)),
        54 => ioctl::ioctl(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        60 => stat::umask(unicorn, unicorn.get_u32_arg(0)),
        78 => time::gettimeofday(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        83 => unistd::symlink(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        88 => unistd::reboot(unicorn, unicorn.get_u32_arg(0)),
        90 => mman::mmap(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
            unicorn.get_u32_arg(5),
        ),
        91 => mman::munmap(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        93 => unistd::ftruncate(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        97 => resource::set_priority(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        99 => stat::statfs(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        120 => sched::clone(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
        ),
        122 => utsname::uname(unicorn, unicorn.get_u32_arg(0)),
        125 => mman::mprotect(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        140 => unistd::_llseek(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
        ),
        146 => uio::writev(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        155 => sched::sched_getparam(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        156 => sched::sched_setscheduler(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        157 => sched::sched_getscheduler(unicorn, unicorn.get_u32_arg(0)),
        159 => sched::sched_get_priority_max(unicorn, unicorn.get_u32_arg(0)),
        160 => sched::sched_get_priority_min(unicorn, unicorn.get_u32_arg(0)),
        162 => time::nanosleep(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        172 => prctl::prctl(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
        ),
        173 => signal::rt_sigreturn(unicorn),
        174 => signal::rt_sigaction(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        175 => signal::rt_sigprocmask(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        177 => signal::rt_sigtimedwait(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        186 => signal::sigaltstack(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        191 => resource::ugetrlimit(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        192 => mman::mmap2(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
            unicorn.get_u32_arg(5),
        ),
        195 => stat::stat64(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        196 => stat::lstat64(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        197 => stat::fstat64(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        217 => unistd::getdents64(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        219 => mman::mincore(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        221 => fcntl::fcntl64(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        224 => unistd::get_tid(unicorn),
        240 => futex::futex(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
            unicorn.get_u32_arg(5),
        ),
        248 => unistd::exit_group(unicorn, unicorn.get_u32_arg(0)),
        256 => unistd::set_tid_address(unicorn, unicorn.get_u32_arg(0)),
        263 => time::clock_gettime(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        274 => mqueue::mq_open(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        275 => mqueue::mq_unlink(unicorn, unicorn.get_u32_arg(0)),
        276 => mqueue::mq_timedsend(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
        ),
        277 => mqueue::mq_timedreceive(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
            unicorn.get_u32_arg(4),
        ),
        278 => mqueue::mq_notify(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        279 => mqueue::mq_getsetattr(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        281 => socket::socket(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        283 => socket::connect(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
        ),
        289 => socket::send(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        322 => fcntl::openat(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        327 => stat::fstatat64(
            unicorn,
            unicorn.get_u32_arg(0),
            unicorn.get_u32_arg(1),
            unicorn.get_u32_arg(2),
            unicorn.get_u32_arg(3),
        ),
        338 => futex::set_robust_list(unicorn, unicorn.get_u32_arg(0), unicorn.get_u32_arg(1)),
        983045 => linux::set_tls(unicorn, unicorn.get_u32_arg(0)),
        x => {
            log::error!(
                "{:#x}: [{}] not implemented syscall #{} (int {}), args: {:#x}, {:#x}, {:#x}, ...",
                unicorn.reg_read(RegisterARM::PC).unwrap(),
                unicorn.get_data().thread_id(),
                x,
                int_no,
                unicorn.get_u32_arg(0),
                unicorn.get_u32_arg(1),
                unicorn.get_u32_arg(2),
            );
            -38i32 as u32 // ENOSYS
        }
    };
    unicorn.set_u32_result(res);

    // a syscall handler may have requested a scheduling action
    match unicorn.get_data().take_action() {
        ThreadAction::None => {}
        ThreadAction::Block(reason) => block_current_thread(unicorn, reason),
        ThreadAction::ExitThread(code) => exit_current_thread(unicorn, code),
        ThreadAction::ExitProcess(code) => exit_process(unicorn, code),
    }
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
