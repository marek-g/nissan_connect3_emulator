use crate::emulator::context::Context;
use crate::emulator::elf_loader::load_elf;
use crate::emulator::memory_map::GET_TLS_ADDR;
use crate::emulator::mmu::MmuExtension;
use crate::emulator::utils::load_binary;
use core::ffi::c_void;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use unicorn_engine::unicorn_const::{uc_error, Arch, HookType, MemType, Mode, Permission};
use unicorn_engine::{RegisterARM, Unicorn};

pub struct Thread {
    pub unicorn: Unicorn<Context>,

    // used to not start emulation at all when pause is requested very early
    is_paused: Arc<AtomicBool>,
    is_exit: Arc<AtomicBool>,

    // used to resume emulation
    resume_tx: Sender<()>,
}

impl Thread {
    /// Starts new thread with a new unicorn instance.
    pub fn start_elf_file(
        context: Context,
        elf_filepath: String,
        program_args: Vec<String>,
        program_envs: Vec<(String, String)>,
    ) -> Result<
        (
            Self,
            JoinHandle<Result<(), Box<dyn Error + Send + Sync + 'static>>>,
        ),
        Box<dyn Error + Send + Sync + 'static>,
    > {
        let mut unicorn = Unicorn::new_with_data(Arch::ARM, Mode::LITTLE_ENDIAN, context)
            .map_err(|err| format!("Unicorn error: {:?}", err))
            .unwrap();

        unicorn.add_intr_hook(crate::os::hook_syscall).unwrap();
        unicorn
            .add_mem_hook(HookType::MEM_FETCH_UNMAPPED, 1, 0, callback_mem_error)
            .unwrap();
        unicorn
            .add_mem_hook(HookType::MEM_READ_UNMAPPED, 1, 0, callback_mem_rw)
            .unwrap();
        unicorn
            .add_mem_hook(HookType::MEM_WRITE_UNMAPPED, 1, 0, callback_mem_rw)
            .unwrap();
        unicorn
            .add_mem_hook(HookType::MEM_WRITE_PROT, 1, 0, callback_mem_rw)
            .unwrap();

        let is_paused = Arc::new(AtomicBool::new(false));
        let is_exit = Arc::new(AtomicBool::new(false));

        let (resume_tx, resume_rx) = channel();

        let handle = thread::spawn({
            let mut unicorn = unicorn.clone();
            let is_paused = is_paused.clone();
            let is_exit = is_exit.clone();
            move || {
                let buf = load_binary(&mut unicorn, &elf_filepath);

                let (interp_entry_point, elf_entry, stack_ptr) = load_elf(
                    &mut unicorn,
                    &elf_filepath,
                    &buf,
                    &program_args,
                    &program_envs,
                )?;

                unicorn
                    .reg_write(RegisterARM::SP as i32, stack_ptr as u64)
                    .unwrap();

                set_kernel_traps(&mut unicorn);
                enable_vfp(&mut unicorn);

                log::info!(
                    "========== Start program (interp_entry_point: {:#x}, elf_entry_point: {:#x}) ==========",
                    interp_entry_point,
                    elf_entry
                );

                emu_thread_loop(unicorn, interp_entry_point, is_paused, is_exit, resume_rx)
            }
        });

        Ok((
            Self {
                unicorn,
                is_paused,
                is_exit,
                resume_tx,
            },
            handle,
        ))
    }

    /*pub fn clone(
        unicorn: &Unicorn<Context>,
        child_thread_id: u32,
        child_tls: u32,
        child_stack: u32,
    ) -> (
        Self,
        JoinHandle<Result<(), Box<dyn Error + Send + Sync + 'static>>>,
    ) {
        linux::set_tls(unicorn, child_tls);
        unicorn
            .reg_write(RegisterARM::SP, child_stack as u64)
            .unwrap();
        let res = 0i32 as u32;
    }*/

    pub fn pause(&mut self) -> Result<(), uc_error> {
        self.is_paused.store(true, Ordering::Relaxed);

        self.unicorn.emu_stop()
    }

    pub fn resume(&mut self) {
        if self.is_paused.load(Ordering::Relaxed) {
            self.is_paused.store(false, Ordering::Relaxed);
            self.resume_tx.send(()).unwrap();
        }
    }

    pub fn exit(&mut self) -> Result<(), uc_error> {
        self.is_exit.store(true, Ordering::Relaxed);

        self.unicorn.emu_stop()
    }

    pub unsafe fn mem_map_ptr(
        &mut self,
        address: u64,
        size: usize,
        perms: Permission,
        ptr: *mut c_void,
    ) -> Result<(), uc_error> {
        self.unicorn.mem_map_ptr(address, size, perms, ptr)
    }

    pub fn mem_unmap(&mut self, address: u64, size: usize) -> Result<(), uc_error> {
        self.unicorn.mem_unmap(address, size)
    }

    pub fn mem_protect(
        &mut self,
        address: u64,
        size: libc::size_t,
        perms: Permission,
    ) -> Result<(), uc_error> {
        self.unicorn.mem_protect(address, size, perms)
    }
}

fn emu_thread_loop(
    mut unicorn: Unicorn<Context>,
    start_address: u32,
    is_paused: Arc<AtomicBool>,
    is_exit: Arc<AtomicBool>,
    resume_rx: Receiver<()>,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let mut start_address = start_address;
    loop {
        if !is_paused.load(Ordering::Relaxed) {
            log::trace!("{:#x}: thread start or resume", start_address);
            match unicorn.emu_start(start_address as u64, 0, 0, 0) {
                Ok(()) => {
                    if is_exit.load(Ordering::Relaxed) {
                        // thread has ended
                        break;
                    } else {
                        // we have stopped because the pause was requested

                        start_address = unicorn.reg_read(RegisterARM::PC).unwrap() as u32;
                        log::trace!("{:#x}: thread paused", start_address);

                        // wait for the signal to resume
                        resume_rx.recv().unwrap();
                    }
                }
                Err(error) => {
                    log::error!(
                        "{:#x}: Execution error: {:?}",
                        unicorn.reg_read(RegisterARM::PC).unwrap(),
                        error
                    );
                    break;
                }
            }
        }
    }

    log::info!("========== Program done ==========");

    Ok(())
}

fn set_kernel_traps(unicorn: &mut Unicorn<Context>) {
    // If the compiler for the target does not provides some primitives for some
    // reasons (e.g. target limitations), the kernel is responsible to assist
    // with these operations.
    //
    // The following is some `kuser` helpers, which can be found here:
    // https://elixir.bootlin.com/linux/latest/source/arch/arm/kernel/entry-armv.S#L899
    unicorn.mmu_map(
        0xFFFF0000,
        0x1000,
        Permission::READ | Permission::EXEC,
        "[arm_traps]",
        "",
    );

    // memory_barrier
    log::debug!("Set kernel trap: memory_barrier at 0xFFFF0FA0");
    unicorn
        .mem_write(
            0xFFFF0FA0,
            // mcr   p15, 0, r0, c7, c10, 5
            // nop
            // mov   pc, lr
            &[
                0xBA, 0x0F, 0x07, 0xEE, 0x00, 0xF0, 0x20, 0xE3, 0x0E, 0xF0, 0xA0, 0xE1,
            ],
        )
        .unwrap();

    // cmpxchg
    log::debug!("Set kernel trap: cmpxchg at 0xFFFF0FC0");
    unicorn
        .mem_write(
            0xFFFF0FC0,
            // ldr   r3, [r2]
            // subs  r3, r3, r0
            // streq r1, [r2]
            // rsbs  r0, r3, #0
            // mov   pc, lr
            &[
                0x00, 0x30, 0x92, 0xE5, 0x00, 0x30, 0x53, 0xE0, 0x00, 0x10, 0x82, 0x05, 0x00, 0x00,
                0x73, 0xE2, 0x0E, 0xF0, 0xA0, 0xE1,
            ],
        )
        .unwrap();

    // get_tls
    log::debug!("Set kernel trap: get_tls at {:#X}", GET_TLS_ADDR);
    unicorn
        .mem_write(
            GET_TLS_ADDR as u64,
            // ldr   r0, [pc, #(16 - 8)]
            // mov   pc, lr
            // mrc   p15, 0, r0, c13, c0, 3
            // padding (e7 fd de f1)
            // data:
            //   "\x00\x00\x00\x00"
            //   "\x00\x00\x00\x00"
            //   "\x00\x00\x00\x00"
            &[
                0x08, 0x00, 0x9F, 0xE5, 0x0E, 0xF0, 0xA0, 0xE1, 0x70, 0x0F, 0x1D, 0xEE, 0xE7, 0xFD,
                0xDE, 0xF1, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ],
        )
        .unwrap();
}

fn enable_vfp(unicorn: &mut Unicorn<Context>) {
    // other version? https://github.com/AeonLucid/AndroidNativeEmu/blob/40b89c8095b2aeb4a9f18ba9a853832afdb3d1b1/src/androidemu/emulator.py

    // https://github.com/qilingframework/qiling/blob/master/qiling/arch/arm.py
    let c1_c0_2 = unicorn.reg_read(RegisterARM::C1_C0_2).unwrap();
    unicorn
        .reg_write(RegisterARM::C1_C0_2, c1_c0_2 | (0b11 << 20) | (0b11 << 22))
        .unwrap();
    unicorn.reg_write(RegisterARM::FPEXC, 1 << 30).unwrap();
}

pub fn callback_mem_error(
    unicorn: &mut Unicorn<Context>,
    memtype: MemType,
    address: u64,
    size: usize,
    value: i64,
) -> bool {
    log::error!(
        "{:#x}: callback_mem_error {:?} - address {:#x}, size: {:#x}, value: {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        memtype,
        address,
        size,
        value
    );
    //dump_context(uc, address, size);
    false
}

pub fn callback_mem_rw(
    unicorn: &mut Unicorn<Context>,
    memtype: MemType,
    address: u64,
    size: usize,
    value: i64,
) -> bool {
    log::error!(
        "{:#x}: callback_mem_rw {:?} - address {:#x}, size: {:#x}, value: {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        memtype,
        address,
        size,
        value
    );
    //dump_context(uc, address, size);
    false
}
