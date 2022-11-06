use crate::emulator::context::{Context, ContextInner};
use crate::emulator::elf_loader::load_elf;
use crate::emulator::memory_map::GET_TLS_ADDR;
use crate::emulator::mmu::mmu_clone_map;
use crate::emulator::print::{disasm, print_mmu, print_stack};
use crate::emulator::utils::{load_binary, pack_u32, read_string};
use capstone::arch::arm::ArchMode;
use capstone::prelude::{BuildsCapstone, BuildsCapstoneEndian};
use capstone::{Capstone, Endian};
use std::collections::HashMap;
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

        /*unicorn
        .add_block_hook(|f, addr, size| {
            println!("Block hook at {:#x}, size: {:#x}", addr, size);
        })
        .unwrap();*/

        add_code_hooks(&mut unicorn);

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

    pub fn clone(
        source_unicorn: &Unicorn<Context>,
        child_thread_id: u32,
        child_tls: u32,
        child_stack: u32,
    ) -> Result<
        (
            Self,
            JoinHandle<Result<(), Box<dyn Error + Send + Sync + 'static>>>,
        ),
        Box<dyn Error + Send + Sync + 'static>,
    > {
        let source_context = source_unicorn.get_data();
        let context = Context {
            inner: Arc::new(ContextInner {
                mmu: source_context.inner.mmu.clone(),
                file_system: source_context.inner.file_system.clone(),
                sys_calls_state: source_context.inner.sys_calls_state.clone(),
                threads: source_context.inner.threads.clone(),
                next_thread_id: source_context.inner.next_thread_id.clone(),
                thread_id: child_thread_id,
                instruction_tracing: Arc::new(AtomicBool::new(false)),
            }),
        };

        let mut unicorn = Unicorn::new_with_data(Arch::ARM, Mode::LITTLE_ENDIAN, context)
            .map_err(|err| format!("Unicorn error: {:?}", err))
            .unwrap();

        // copy registers
        let registers_context = source_unicorn
            .context_init()
            .map_err(|err| format!("Unicorn context init error: {:?}", err))?;
        unicorn
            .context_restore(&registers_context)
            .map_err(|err| format!("Unicorn context restore error: {:?}", err))?;
        /*unicorn
        .reg_write(
            RegisterARM::PC,
            source_unicorn.reg_read(RegisterARM::PC).unwrap(),
        )
        .unwrap();*/

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

        add_code_hooks(&mut unicorn);

        // copy memory map
        mmu_clone_map(&source_unicorn, &mut unicorn)?;

        set_kernel_traps(&mut unicorn);

        // set tls
        unicorn
            .reg_write(RegisterARM::C13_C0_3, child_tls as u64)
            .unwrap();
        unicorn
            .mem_write(GET_TLS_ADDR as u64 + 16, &pack_u32(child_tls))
            .unwrap();

        enable_vfp(&mut unicorn);

        // the address to continue is stored on new stack
        unicorn
            .reg_write(RegisterARM::SP, child_stack as u64)
            .unwrap();

        // set 0 in R0 (result from syscall)
        unicorn.reg_write(RegisterARM::R0 as i32, 0).unwrap();

        let is_paused = Arc::new(AtomicBool::new(false));
        let is_exit = Arc::new(AtomicBool::new(false));

        let (resume_tx, resume_rx) = channel();

        let handle = thread::spawn({
            let unicorn = unicorn.clone();
            let is_paused = is_paused.clone();
            let is_exit = is_exit.clone();
            move || {
                let pc = unicorn.reg_read(RegisterARM::PC).unwrap() as u32;

                log::info!("========== Clone thread at address: {:#x} ==========", pc);

                emu_thread_loop(unicorn, pc, is_paused, is_exit, resume_rx)
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

    pub fn pause(&mut self) -> Result<(), uc_error> {
        /*self.is_paused.store(true, Ordering::Relaxed);

        self.unicorn.emu_stop()*/
        Ok(())
    }

    pub fn resume(&mut self) {
        /*if self.is_paused.load(Ordering::Relaxed) {
            self.is_paused.store(false, Ordering::Relaxed);
            self.resume_tx.send(()).unwrap();
        }*/
    }

    pub fn exit(&mut self) -> Result<(), uc_error> {
        self.is_exit.store(true, Ordering::Relaxed);

        self.unicorn.emu_stop()
    }
}

fn emu_thread_loop(
    mut unicorn: Unicorn<Context>,
    mut start_address: u32,
    is_paused: Arc<AtomicBool>,
    is_exit: Arc<AtomicBool>,
    resume_rx: Receiver<()>,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    loop {
        if !is_paused.load(Ordering::Relaxed) {
            log::trace!(
                "{:#x}: [{}] thread start or resume",
                start_address,
                unicorn.get_data().inner.thread_id
            );

            match unicorn.emu_start(start_address as u64, 0, 0, 0) {
                Ok(()) => {
                    if is_exit.load(Ordering::Relaxed) {
                        // thread has ended
                        break;
                    } else {
                        // we have stopped because the pause was requested

                        start_address = unicorn.reg_read(RegisterARM::PC).unwrap() as u32;
                        log::trace!(
                            "{:#x}: [{}] thread paused",
                            start_address,
                            unicorn.get_data().inner.thread_id,
                        );

                        // wait for the signal to resume
                        resume_rx.recv().unwrap();
                    }
                }
                Err(error) => {
                    log::error!(
                        "{:#x}: [{}] Execution error: {:?}",
                        unicorn.reg_read(RegisterARM::PC).unwrap(),
                        unicorn.get_data().inner.thread_id,
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

// If the compiler for the target does not provides some primitives for some
// reasons (e.g. target limitations), the kernel is responsible to assist
// with these operations.
//
// The following is some `kuser` helpers, which can be found here:
// https://elixir.bootlin.com/linux/latest/source/arch/arm/kernel/entry-armv.S#L899
fn set_kernel_traps(unicorn: &mut Unicorn<Context>) {
    let unicorn_context = unicorn.get_data();

    // allocate memory directly by unicorn (not mmu object),
    // so it is different for every thread
    unicorn
        .mem_map(
            0xFFFF0000u64,
            0x1000usize,
            Permission::READ | Permission::EXEC,
        )
        .unwrap();

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
        "{:#x}: [{}] callback_mem_error {:?} - address {:#x}, size: {:#x}, value: {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        memtype,
        address,
        size,
        value
    );

    dump_context(unicorn);

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
        "{:#x}: [{}] callback_mem_rw {:?} - address {:#x}, size: {:#x}, value: {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id,
        memtype,
        address,
        size,
        value
    );

    dump_context(unicorn);

    false
}

fn dump_context(unicorn: &mut Unicorn<Context>) {
    println!(
        "PC: {:#10x}, LR (return code): {:#10x}, SP: {:#10x}, FP: {:#10x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.reg_read(RegisterARM::LR).unwrap(),
        unicorn.reg_read(RegisterARM::SP).unwrap(),
        unicorn.reg_read(RegisterARM::FP).unwrap()
    );
    print_mmu(unicorn);
    disasm(
        unicorn,
        unicorn.reg_read(RegisterARM::PC).unwrap() as u32 - 100,
        200,
    );
    disasm(
        unicorn,
        unicorn.reg_read(RegisterARM::LR).unwrap() as u32 - 100,
        200,
    );
    disasm(unicorn, 0x484e93ec as u32, 200);

    print_stack(unicorn);
}

fn add_code_hooks(unicorn: &mut Unicorn<Context>) {
    let mut method_entries = HashMap::new();
    insert_libosal_method_entries(&mut method_entries);

    unicorn
        .add_code_hook(0, 0xFFFFFFFF, move |uc, addr, size| {
            let mut tracing = uc.get_data().inner.instruction_tracing.load(Ordering::Relaxed);

            let addr = addr as u32;

            if let Some(method_name) = method_entries.get(&addr) {
                log::trace!("-- {:#x} [{}] OSAL: {}() [IN]", addr, uc.get_data().inner.thread_id,  *method_name);
                tracing = true;

                let method_name = method_name.to_string();

                if method_name == "vInitTrace" {
                    // skip method that normally crashes
                    uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap()).unwrap();
                }

                if method_name == "DEV_FFD_s32IODeviceInit" {
                    uc.reg_write(RegisterARM::R0, 1).unwrap();
                    uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap()).unwrap();
                }

                if method_name == "LockOsal" ||
                    method_name == "UnLockOsal" {
                    uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap()).unwrap();
                }

                if method_name == "LLD_bIsTraceActive" {
                    uc.reg_write(RegisterARM::R0, 1u64).unwrap();
                    uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap()).unwrap();
                }

                if method_name == "OSAL_vAssertFunction" {
                    let str1 = read_string(uc, uc.reg_read(RegisterARM::R0).unwrap() as u32);
                    let str2 = read_string(uc, uc.reg_read(RegisterARM::R0).unwrap() as u32);
                    if str1 == str2 {
                        log::trace!("OSAL assert OK: {} = {}", str1, str2);
                    } else {
                        log::error!("OSAL assert ERROR: {} != {}", str1, str2);
                    }
                    uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap()).unwrap();
                }

                if method_name == "OSAL_s32MessageQueueOpen" {
                    let str = read_string(uc, uc.reg_read(RegisterARM::R0).unwrap() as u32);
                    log::trace!("OSAL_s32MessageQueueOpen: {}", str);
                }

                if method_name == "vTraceMqInfo" {
                    uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap()).unwrap();
                }

                if method_name == "TraceIOString" {
                    let str = read_string(uc, uc.reg_read(RegisterARM::R0).unwrap() as u32);
                    log::warn!("TraceIOString: {}", str);
                    uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap()).unwrap();
                }

                /*if method_name == "OSAL_vAssertFunction" ||
                    method_name == "vTraceMqInfo" ||
                    method_name == "TraceIOString" ||
                    method_name == "DEV_FFD_s32IODeviceInit" ||
                    method_name == "DEV_FFD_IOOpen" ||
                    method_name == "KDS_vTrace" ||
                    method_name == "LLD_bIsTraceActive" ||
                    method_name == "LLD_vTrace" ||
                    method_name == "LLD_vRegTraceCallback" ||
                    method_name == "OSAL_s32MessagePoolCreate" ||
                    method_name == "TRACE_s32IOControl" {
                    // skip
                    uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap()).unwrap();
                    tracing = true;
                }

                if method_name.to_string() == "LLD_vRegTraceCallback" {
                    uc.get_data().inner.instruction_tracing.store(true, Ordering::Relaxed);
                }*/
            }

            /*if addr == 0x4851578c {
                log::trace!("-- OSAL_ThreadCreate({:#x}) [IN]", uc.reg_read(RegisterARM::R0).unwrap());
                tracing = true;
            }

            if addr == 0x485157d8 {
                log::trace!("-- OSAL_ThreadCreate() [OUT]");
                tracing = true;
            }

            if addr == 0x48515bd8 {
                log::trace!("-- OSAL_ThreadSpawn() [IN]");
                tracing = true;
            }

            if addr == 0x48515bf0 || addr == 0x48515c00 {
                log::trace!("-- OSAL_ThreadSpawn() [OUT]");
                //uc.get_data().inner.instruction_tracing.store(true, Ordering::Relaxed);
                tracing = true;
            }

            if addr == 0x48516334 {
                log::trace!("-- vAddProcessEntry({:#x}) [IN]", uc.reg_read(RegisterARM::R0).unwrap());
                tracing = true;
            }

            if addr == 0x48516564 {
                log::trace!("-- vAddProcessEntry() [OUT]");
                //uc.get_data().inner.instruction_tracing.store(true, Ordering::Relaxed);
                tracing = true;
            }

            if addr == 0x485084c0 {
                log::trace!("-- [libosal] entry_init1({:#x}, ...) [IN]", uc.reg_read(RegisterARM::R0).unwrap());
                tracing = true;
            }

            if addr == 0x4850863c {
                log::trace!("-- [libosal] entry_init1() [OUT]");
                //uc.get_data().inner.instruction_tracing.store(true, Ordering::Relaxed);
                tracing = true;
            }*/

            if tracing {
                let cs = Capstone::new()
                    .arm()
                    .mode(ArchMode::Arm)
                    .endian(Endian::Little)
                    .detail(true)
                    .build()
                    .unwrap();

                let mut vec = [0u8; 4];
                uc.mem_read(addr as u64, &mut vec).unwrap();
                let disasm = cs.disasm_all(&vec, addr as u64).unwrap();
                let disasm = format!("{}", disasm);
                log::trace!(
                        "R0: {:#x}, R1: {:#x}, R2: {:#x}, R3: {:#x}, R4: {:#x}, R5: {:#x}, R6: {:#x}, R7: {:#x}, R8: {:#x}, LR: {:#x}, IP: {:#x}",
                        uc.reg_read(RegisterARM::R0).unwrap(),
                        uc.reg_read(RegisterARM::R1).unwrap(),
                        uc.reg_read(RegisterARM::R2).unwrap(),
                        uc.reg_read(RegisterARM::R3).unwrap(),
                        uc.reg_read(RegisterARM::R4).unwrap(),
                        uc.reg_read(RegisterARM::R5).unwrap(),
                        uc.reg_read(RegisterARM::R6).unwrap(),
                        uc.reg_read(RegisterARM::R7).unwrap(),
                        uc.reg_read(RegisterARM::R8).unwrap(),
                        uc.reg_read(RegisterARM::LR).unwrap(),
                        uc.reg_read(RegisterARM::IP).unwrap()
                    );
                log::trace!("{}", &disasm[0..disasm.len() - 1]);
            }
        })
        .unwrap();
}

fn insert_libosal_method_entries(method_entries: &mut HashMap<u32, &str>) {
    method_entries.insert(0x484ecf50, "vRegisterOsalIO_Callback");
    method_entries.insert(0x48530a68, "bReadPublicKey");
    method_entries.insert(0x485140f4, "tThreadTableGetFreeEntry");
    method_entries.insert(0x48529b10, "BT_UGZZC_s32IOWrite");
    method_entries.insert(0x485142a0, "bGetThreadNameForTID");
    method_entries.insert(0x48559bb0, "CRC32TAB");
    method_entries.insert(0x484faac4, "LFS_u32IOCreate");
    method_entries.insert(0x4856995c, "szErrorString_UNKNOWN");
    method_entries.insert(0x4852d2b0, "s32FFDReloadDataFromFile");
    method_entries.insert(0x48523abc, "u32AcousticOutIOCtrl_Version");
    method_entries.insert(0x4852d75c, "s32FFDCheckValidReadData");
    method_entries.insert(0x484f3d80, "OSAL_u32IOErrorAsync");
    method_entries.insert(0x4857145c, "_u32RcvData");
    method_entries.insert(0x4851b118, "OSAL_s32TimerCreate");
    method_entries.insert(0x4853800c, "fd_device_ctrl_vTraceInfo");
    method_entries.insert(0x48518584, "s32SemaphoreTableCreate");
    method_entries.insert(0x00000000, "hFD_CryptBPCLSemaphore");
    method_entries.insert(0x484ee290, "vOsalInitLoadTaskInfoEntry");
    method_entries.insert(0x4853834c, "OSALUTIL_s32CreateDir");
    method_entries.insert(0x4852a2c8, "bDrvBtAsipDeInit");
    method_entries.insert(0x484efcc0, "GetOsalDeviceName");
    method_entries.insert(0x484fc490, "vSearchOpenFileDescriptorsOfMountPoint");
    method_entries.insert(0x484fc134, "FSFilePrcAccessOK");
    method_entries.insert(0x48571a98, "sNavSDCard_meta");
    method_entries.insert(0x4851e3e4, "CheckValid_Signal");
    method_entries.insert(0x48527038, "ACOUSTICSRC_s32IOControl");
    method_entries.insert(0x4852a178, "asipGetHeaderPayLen");
    method_entries.insert(0x484f08f4, "OSAL_device_getd_line");
    method_entries.insert(0x48506f30, "OSAL_s32EventDelete");
    method_entries.insert(0x4852b52c, "s32DrvBtAsipRcvData");
    method_entries.insert(0x484efdac, "pattern_compare");
    method_entries.insert(0x48536220, "libminxml_priv_find");
    method_entries.insert(0x4852fd98, "ERRMEM_s32IORead");
    method_entries.insert(0x4852d910, "FFD_vTraceMoreInfo");
    method_entries.insert(0x4850d8a0, "s32GetResources");
    method_entries.insert(0x48505d8c, "vTraceErrorCode");
    method_entries.insert(0x485381d4, "OSALUTIL_s32TracePrintf");
    method_entries.insert(0x48511730, "OSAL_vSetCheck");
    method_entries.insert(0x484efcd0, "OSAL_InitTable");
    method_entries.insert(0x48572c44, "prm_hPrmMQ");
    method_entries.insert(0x484fa90c, "s32DNFS_IO_Close");
    method_entries.insert(0x485251cc, "u32AcousticOutIOCtrl_Start");
    method_entries.insert(0x484ef178, "vWokerTask");
    method_entries.insert(0x485067c4, "OSAL_s32EventWait");
    method_entries.insert(0x485358e4, "bInitFetch");
    method_entries.insert(0x48524d54, "ACOUSTICOUT_s32IOWrite");
    method_entries.insert(0x48517788, "vTraceShMemInfo");
    method_entries.insert(0x485140f0, "vErrorHookFunc");
    method_entries.insert(0x484f563c, "vCheckForValidDvdFile");
    method_entries.insert(0x4850da14, "s32MqueueTableDeleteEntries");
    method_entries.insert(0x48518634, "vSetTraceFlagForSem");
    method_entries.insert(0x4854c4a4, "PRODUCT_C_STRING_NAV_VERSION_SHORT");
    method_entries.insert(0x4856cc90, "u32dummy");
    method_entries.insert(0x484fb21c, "LFS_u32IOOpen");
    method_entries.insert(0x48522fe8, "u32AcousticOutIOCtrl_SetChannels");
    method_entries.insert(0x48505784, "u32ConvertErrorCore");
    method_entries.insert(0x4850ba98, "u32OpenMsgQueue");
    method_entries.insert(0x48506da8, "OSAL_s32EventOpen");
    method_entries.insert(0x48570404, "_u8TxBuf");
    method_entries.insert(0x48517bd0, "OSAL_s32SharedMemoryClose");
    method_entries.insert(0x48533a60, "trGetRevocationSignature");
    method_entries.insert(0x485150a8, "OSAL_s32ThreadResume");
    method_entries.insert(0x48511208, "u32GetMsgHeaderSize");
    method_entries.insert(0x485292cc, "vTraceAcousticSrcPrintf");
    method_entries.insert(0x484f0b70, "OSAL_s32IORead");
    method_entries.insert(0x48545db8, "field_element_is_negative");
    method_entries.insert(0x4853fe80, "BPCL_RC5_Encrypt");
    method_entries.insert(0x48522ae4, "bIsChannelnumValid");
    method_entries.insert(0x48538d84, "SD_Refresh_s32ForcedRefresh");
    method_entries.insert(0x4852f924, "DRV_DIAG_EOL_s32IOClose");
    method_entries.insert(0x4850c8e0, "vGetGlobalIoscMq");
    method_entries.insert(0x484fae20, "s32DNFS_IO_Create");
    method_entries.insert(0x48509024, "vSetTraceFlag");
    method_entries.insert(0x48529db8, "BT_UGZZC_IOOpen");
    method_entries.insert(0x485699b6, "bNativeTimeHldr");
    method_entries.insert(0x48511c0c, "OSAL_s32MessagePoolDelete");
    method_entries.insert(0x48512b7c, "OSAL_vPrintMessageList");
    method_entries.insert(0x48511af4, "vPrintPreceedingMsgInfo");
    method_entries.insert(0x4856c744, "_gkmkc");
    method_entries.insert(0x4852e084, "KDS_s32IORead");
    method_entries.insert(0x4853c240, "point_jacobian_add_mixed");
    method_entries.insert(0x484fbe48, "vCheckforOpenFilesInRoot");
    method_entries.insert(0x48518bcc, "OSAL_s32SemaphoreWait");
    method_entries.insert(0x48523f5c, "u32AcousticOutIOCtrl_SetSampleformat");
    method_entries.insert(0x4850cfcc, "OSAL_u32MemPoolFixSizeCreate");
    method_entries.insert(0x485088b4, "OSALCore_vShutdown");
    method_entries.insert(0x48500490, "s32CheckPrmSignalStatus");
    method_entries.insert(0x484fca14, "u32ProveLowVoltage");
    method_entries.insert(0x484fc850, "vCloseFileDescriptorsOfMountPoint");
    method_entries.insert(0x4852b5cc, "_Z13dwThreadDrvBtPv");
    method_entries.insert(0x484fcd1c, "prm_bIsAPrmFun");
    method_entries.insert(0x4853cb34, "point_jacobian_subtract_mixed");
    method_entries.insert(0x4856994c, "szErrorString_INPROGRESS");
    method_entries.insert(0x484f16dc, "OSAL_s32IOControl_plain");
    method_entries.insert(0x48529fec, "asipSetHeaderID");
    method_entries.insert(0x484eeda4, "s32TraceErrmem");
    method_entries.insert(0x4852ef48, "s32KDSDeleteMsgQueue");
    method_entries.insert(0x485241e4, "u32UnInitOutputDevice");
    method_entries.insert(0x4853086c, "fd_crypt_destroy");
    method_entries.insert(0x48535c38, "libminxml_get_next_attr");
    method_entries.insert(0x4853707c, "fd_device_ctrl_u32DirectSDCardInfo");
    method_entries.insert(0x4852a7e4, "s32DrvBtAsipRcvDataSync");
    method_entries.insert(0x4852fbd8, "DRV_DIAG_EOL_s32IODeviceInit");
    method_entries.insert(0x4852975c, "vTraceAcousticOutInfo");
    method_entries.insert(0x48504ab8, "OSAL_vSetAssertMode");
    method_entries.insert(0x48549244, "field_element_reduce_barrett");
    method_entries.insert(0x485315d4, "fd_crypt_vTraceInfo");
    method_entries.insert(0x48518978, "s32SemaphoreTableDeleteEntries");
    method_entries.insert(0x48571a30, "prFdCryptPvtData");
    method_entries.insert(0x48522b88, "vResetErrorThresholds");
    method_entries.insert(0x484ed030, "vReadMemStatus");
    method_entries.insert(0x485084b0, "vInitTrace");
    method_entries.insert(0x4851c44c, "LLD_vRegTraceCallback");
    method_entries.insert(0x484ed344, "vDumpMemStatusForProcess");
    method_entries.insert(0x4850a84c, "u32SendToMessageQueue");
    method_entries.insert(0x484ebc58, "DeleteOsalLock");
    method_entries.insert(0x48569930, "szErrorString_MSGTOOLONG");
    method_entries.insert(0x4851d36c, "FileCopy");
    method_entries.insert(0x48539c50, "BPCL_EC_DSA_Verify");
    method_entries.insert(0x485021d0, "prmGetPRMUSBPortCount");
    method_entries.insert(0x48535e90, "libminxml_plat_ungetc");
    method_entries.insert(0x485230b4, "u32AcousticOutIOCtrl_GetChannels");
    method_entries.insert(0x48503554, "s32InitializePRMUSB_data");
    method_entries.insert(0x484f3964, "vInitRegistryIOSC");
    method_entries.insert(0x484f3dc0, "OSAL_s32IOReturnAsync");
    method_entries.insert(0x48569950, "szErrorString_TIMEOUT");
    method_entries.insert(0x48533ca0, "vCloseFetch");
    method_entries.insert(0x4850dae0, "vTraceMQCB");
    method_entries.insert(0x484f7650, "u32ReadExtDir2");
    method_entries.insert(0x485112d8, "vCheckIoscError");
    method_entries.insert(0x48522bc8, "u32AcousticOutIOCtrl_SetTestMode");
    method_entries.insert(0x484f0024, "bInitTripFileReplay");
    method_entries.insert(0x4851d6e8, "vCopyDirFiles");
    method_entries.insert(0x484ebe7c, "vWriteToErrMem");
    method_entries.insert(0x4850b060, "vTraceIoscShMem");
    method_entries.insert(0x484ed218, "vTraceHeap");
    method_entries.insert(0x48571461, "_bIgnoreResetTrigger");
    method_entries.insert(0x48533ba0, "trGetCertLifetimeStart");
    method_entries.insert(0x485019f0, "EvaluateNewMount");
    method_entries.insert(0x4852b738, "drv_bt_lld_uart_read");
    method_entries.insert(0x48531830, "s8IsXmlVIN");
    method_entries.insert(0x4852a308, "vCreateThread");
    method_entries.insert(0x4851d8d8, "vCopyFile");
    method_entries.insert(0x484ee124, "vStartMemConsumer");
    method_entries.insert(0x4853fca8, "BPCL_RC5_Setup");
    method_entries.insert(0x4852a614, "asipChkHeaderChksum");
    method_entries.insert(0x484f806c, "LFS_u32IOControl");
    method_entries.insert(0x48505e7c, "OSAL_u32ErrorCode");
    method_entries.insert(0x4853b210, "point_set_infinity");
    method_entries.insert(0x48529bf8, "BT_UGZZC_s32IOControl");
    method_entries.insert(0x4851743c, "vSetTraceFlagForShMem");
    method_entries.insert(0x48572cb4, "lib_handle");
    method_entries.insert(0x4852bd94, "_Z30dwThreadDrvBt_SPP_SocketserverPv");
    method_entries.insert(0x484fca1c, "u32ProveHighTemperature");
    method_entries.insert(0x485461e0, "field_element_shift_n_right");
    method_entries.insert(0x4856e7d0, "hOsalMtx");
    method_entries.insert(0x4851239c, "OSAL_u32GetMessageSize");
    method_entries.insert(0x48519ae4, "OSAL_ClockGetElapsedTime");
    method_entries.insert(0x4850b668, "OSAL_u32IoscSharedMemoryDelete");
    method_entries.insert(0x484efd88, "OSAL_get_drive_id");
    method_entries.insert(0x4854c280, "_fini");
    method_entries.insert(0x4852a0ec, "asipSetHeaderSeqNo");
    method_entries.insert(0x485313a0, "fd_crypt_verify_signaturefile");
    method_entries.insert(0x4856991c, "szErrorString_DOESNOTEXIST");
    method_entries.insert(0x4850e484, "vExecuteCallback");
    method_entries.insert(0x485115d4, "OSAL_s32MessagePoolGetAbsoluteSize");
    method_entries.insert(0x48507144, "bCleanUpEventofContext");
    method_entries.insert(0x4856d498, "hLiShMem");
    method_entries.insert(0x48519ed4, "OSAL_s32ClockSetTime");
    method_entries.insert(0x484f725c, "bMapFileSystems");
    method_entries.insert(0x48502278, "prm_vTimerCallbackPort2");
    method_entries.insert(0x485703d5, "gu8AsipRxSeqNo");
    method_entries.insert(0x484ebbe8, "vWritePrcFsToErrMem");
    method_entries.insert(0x485302c0, "bGetCryptDevStatus");
    method_entries.insert(0x4851312c, "vConfigErrmembuff");
    method_entries.insert(0x484f1d74, "OSAL_IOCreate");
    method_entries.insert(0x484f1590, "OSAL_s32IORemove");
    method_entries.insert(0x48529a2c, "BT_UGZZC_s32IORead");
    method_entries.insert(0x485208bc, "exc_check_access_stack");
    method_entries.insert(0x4853d314, "BPCL_Create_Symmetric_Key");
    method_entries.insert(0x484ee608, "vOsalKillLoadTask");
    method_entries.insert(0x4850bba8, "bGetIoscMqInfo2");
    method_entries.insert(0x48538ab4, "SD_Refresh_s32StartRefresh");
    method_entries.insert(0x48535bcc, "libminxml_get_next_content");
    method_entries.insert(0x4853fb20, "BPCL_CRC32_Update");
    method_entries.insert(0x48522a78, "bIsSampleformatValid");
    method_entries.insert(0x48538534, "OSALUTIL_s32SaveNPrintFormat");
    method_entries.insert(0x4851be04, "vActivateTimerTrace");
    method_entries.insert(0x485382c0, "OSALUTIL_s32CloseDir");
    method_entries.insert(0x4856d4a4, "hMqLiProcMain");
    method_entries.insert(0x48545d44, "field_element_is_one");
    method_entries.insert(0x4852102c, "ACOUSTICIN_s32IORead");
    method_entries.insert(0x484efecc, "TRACE_s32IOOpen");
    method_entries.insert(0x48517d98, "OSAL_SharedMemoryOpen");
    method_entries.insert(0x48545f70, "field_element_compare");
    method_entries.insert(0x48533724, "pcGetLifetimeEnd");
    method_entries.insert(0x484f3d7c, "vGetThreadName");
    method_entries.insert(0x485383b4, "OSALUTIL_s32FSetpos");
    method_entries.insert(0x48558e54, "charset");
    method_entries.insert(0x4853aff8, "point_convert_to_affine");
    method_entries.insert(0x4850c034, "SendMqOrder");
    method_entries.insert(0x48535d24, "libminxml_getcontent");
    method_entries.insert(0x48504be4, "OSAL_trace_callstack");
    method_entries.insert(0x4852a128, "asipSetPayloadChannel");
    method_entries.insert(0x4856d3c8, "OSAL_vAssert_buf");
    method_entries.insert(0x48514008, "vInitMsgPoolResources");
    method_entries.insert(0x48517f50, "OSAL_s32SharedMemoryDelete");
    method_entries.insert(0x484f38bc, "REGISTRY_u32IOClose");
    method_entries.insert(0x4852ffdc, "FD_Crypt_vLeaveCriticalSection");
    method_entries.insert(0x48571a48, "sNavSDCard");
    method_entries.insert(0x4854c450, "PRODUCT_C_STRING_SYSTEM_VERSION");
    method_entries.insert(0x4852f070, "KDS_vTrace");
    method_entries.insert(0x484ebb94, "CloseOsalLock");
    method_entries.insert(0x485703ec, "gu32TxMsgCount");
    method_entries.insert(0x48523920, "u32AcousticOutIOCtrl_GetSuppDecoder");
    method_entries.insert(0x4850204c, "scan_existing_mounts");
    method_entries.insert(0x4852e710, "KDS_s32IOClose");
    method_entries.insert(0x48507520, "vSysCntrl_Error");
    method_entries.insert(0x4852d118, "s32FFDSaveDataToFile");
    method_entries.insert(0x48501e50, "handle_umount");
    method_entries.insert(0x4852fdb8, "ERRMEM_S32IOClose");
    method_entries.insert(0x484f27ac, "pvGetMemory");
    method_entries.insert(0x4852b858, "drv_bt_lld_uart_port_set_rts");
    method_entries.insert(0x4852bbb4, "v_drv_bt_SendBtDataViaSPP");
    method_entries.insert(0x48503648, "prm_vDeleteLibUsbConnectionTask");
    method_entries.insert(0x48509bf4, "vStartIoscHdrTsk");
    method_entries.insert(0x4851983c, "vCountSeconds");
    method_entries.insert(0x48522ea8, "u32AcousticOutIOCtrl_GetSuppBuffersize");
    method_entries.insert(0x48523d28, "u32AcousticOutIOCtrl_SetBuffersize");
    method_entries.insert(0x4853f608, "BPCL_SHA1_Update");
    method_entries.insert(0x48506248, "s32EventTableDeleteEntries");
    method_entries.insert(0x484f4254, "OSAL_copy_device_name");
    method_entries.insert(0x48526558, "ACOUSTICOUT_s32IOOpen");
    method_entries.insert(0x48518ea4, "OSAL_s32SemaphoreGetValue");
    method_entries.insert(0x485208ec, "exc_check_access_code");
    method_entries.insert(0x485095c0, "vpGetValidMqPtr");
    method_entries.insert(0x48545014, "ecc_public_key_generation");
    method_entries.insert(0x484ef8c8, "vTableInit");
    method_entries.insert(0x4851145c, "OSAL_u32GetMaxMessageSize");
    method_entries.insert(0x484eedcc, "vWriteErrmemToMassstorage");
    method_entries.insert(0x48513abc, "s32AllocMsgMem");
    method_entries.insert(0x484f3e6c, "OSAL_s32IOCancelAsync");
    method_entries.insert(0x485385c4, "sd_card_refresh_interface_vTraceInfo");
    method_entries.insert(0x4852fd64, "ERRMEM_s32IOControl");
    method_entries.insert(0x48511dbc, "s32LinuxMapping");
    method_entries.insert(0x4850e940, "vTraceMqInfo");
    method_entries.insert(0x485117d4, "vPrintMessage");
    method_entries.insert(0x485301bc, "FD_Crypt_vEnterCriticalSection");
    method_entries.insert(0x485311fc, "fd_crypt_get_signaturefile_type");
    method_entries.insert(0x485715f8, "u16AtTmpCounter");
    method_entries.insert(0x4851479c, "OSAL_ThreadWhoAmI");
    method_entries.insert(0x48572ccc, "_tmpU8");
    method_entries.insert(0x48526c18, "ACOUSTICSRC_s32Deinit");
    method_entries.insert(0x484efee4, "vPutU32InTripData");
    method_entries.insert(0x485382f8, "OSALUTIL_prReadDir");
    method_entries.insert(0x485052e4, "vCallbackHandler");
    method_entries.insert(0x4852a278, "asipGetRtr");
    method_entries.insert(0x4856f808, "TraceBuffer");
    method_entries.insert(0x484efedc, "bInitTripFileRecord");
    method_entries.insert(0x4853f700, "BPCL_SHA1_Finish");
    method_entries.insert(0x48533a04, "trGetOperatorKey");
    method_entries.insert(0x484f0950, "OSAL_s32IOWrite");
    method_entries.insert(0x48518d68, "OSAL_s32SemaphorePost");
    method_entries.insert(0x484efd94, "to_upper");
    method_entries.insert(0x48535b70, "libminxml_get_next_level_element");
    method_entries.insert(0x484f78f8, "u16CheckForLowVoltRetry");
    method_entries.insert(0x4854642c, "field_element_add");
    method_entries.insert(0x484f4ef0, "bAccessAllowed");
    method_entries.insert(0x4850e1a8, "OSAL_s32MessageQueueNotify");
    method_entries.insert(0x484fc050, "bFileFind");
    method_entries.insert(0x4852b150, "_Z18s32ReadCompleteMsgPhm");
    method_entries.insert(0x48515c08, "OSAL_s32ProcessControlBlock");
    method_entries.insert(0x485703e8, "gu8TxSendCounter");
    method_entries.insert(0x48533894, "pcGetRevocTimestamp");
    method_entries.insert(0x48504b00, "OSAL_vGetAssertMode");
    method_entries.insert(0x48569880, "Drive");
    method_entries.insert(0x48538860, "SD_Refresh_s32AbortRefresh");
    method_entries.insert(0x48511200, "u32GetSharedHeaderSize");
    method_entries.insert(0x48571a70, "sNavSDCard_config");
    method_entries.insert(0x484fca24, "vOsalTraceMemError");
    method_entries.insert(0x4853afb0, "point_set_zero");
    method_entries.insert(0x48512950, "OSAL_s32MessagePoolClose");
    method_entries.insert(0x4852a040, "asipSetHeaderAck");
    method_entries.insert(0x48513850, "OSAL_s32MessageDelete");
    method_entries.insert(0x4851c330, "LLD_vCloseTrace");
    method_entries.insert(0x4852a1e8, "asipGetHeaderSeqNo");
    method_entries.insert(0x48545e00, "field_element_absolute_compare");
    method_entries.insert(0x4851948c, "OSAL_s32SemaphoreCreate_Opt");
    method_entries.insert(0x4850b270, "OSAL_u32IoscSharedMemoryMap");
    method_entries.insert(0x484f526c, "vTraceIoCtrl");
    method_entries.insert(0x4850cd18, "bUsageMmap");
    method_entries.insert(0x48513f60, "vDeInitMsgPoolResources");
    method_entries.insert(0x4852d7c8, "s32FFDSendMessage");
    method_entries.insert(0x4852f494, "DRV_DIAG_EOL_s32IORead");
    method_entries.insert(0x48570400, "gu32RxMsg4AppCount");
    method_entries.insert(0x48523424, "u32AcousticOutIOCtrl_SetSamplerate");
    method_entries.insert(0x4856adf8, "_hDrvBtThreadID");
    method_entries.insert(0x4856b7d4, "vrFFDConfigData_LCN2_KAI");
    method_entries.insert(0x484f0da4, "OSAL_s32IOControl");
    method_entries.insert(0x48519b84, "OSAL_s32ClockGetTime");
    method_entries.insert(0x48572cd0, "_mi");
    method_entries.insert(0x4850754c, "vInitMessagePool");
    method_entries.insert(0x48512020, "OSAL_s32MessagePoolCreate");
    method_entries.insert(0x4852cdc8, "DEV_FFD_s32IODeviceInit");
    method_entries.insert(0x484ff080, "prm_vCheck");
    method_entries.insert(0x4851cd70, "u32Read_Rem_Dir");
    method_entries.insert(0x4851bfbc, "CallBackExecute");
    method_entries.insert(0x48569928, "szErrorString_NAMETOOLONG");
    method_entries.insert(0x48572c48, "vu32FFDFilePos");
    method_entries.insert(0x4853c098, "point_jacobian_subtract");
    method_entries.insert(0x4850d1a4, "TraceSpecificPoolInfo");
    method_entries.insert(0x48571a10, "_u32DownStreamDataCount");
    method_entries.insert(0x4852c10c, "_Z15drvBtAsipChksumPht");
    method_entries.insert(0x48533c20, "trGetCertificateLifetime");
    method_entries.insert(0x48509744, "s32CheckForIOSCQueue");
    method_entries.insert(0x4852baf8, "v_drv_bt_SPPDataReceived");
    method_entries.insert(0x484f07cc, "OSAL_display_informations");
    method_entries.insert(0x484ec06c, "UnLockOsal");
    method_entries.insert(0x4850d8a8, "OSAL_s32MessageQueuePriorityWait");
    method_entries.insert(0x48535ea4, "libminxml_plat_fprintf");
    method_entries.insert(0x4856d4a0, "bOtherOsStarted");
    method_entries.insert(0x4853a3b8, "BPCL_EC_DH");
    method_entries.insert(0x4856c74e, "_gkmkd");
    method_entries.insert(0x4851efc0, "exception_handler_unlock");
    method_entries.insert(0x4852ea34, "KDS_s32IODeviceRemove");
    method_entries.insert(0x4852fe40, "ERRMEM_S32IOOpen_impl");
    method_entries.insert(0x4856d494, "pOsalData");
    method_entries.insert(0x48537f44, "fd_device_ctrl_u32ReadCid");
    method_entries.insert(0x485339a8, "trGetOperatorSignature");
    method_entries.insert(0x4850efa8, "u32GenerateHandle");
    method_entries.insert(0x48569908, "szErrorString_NOSPACE");
    method_entries.insert(0x4853789c, "fd_device_ctrl_u32UsbSdCardInfo");
    method_entries.insert(0x4851c334, "vSetFilterForCcaMsg");
    method_entries.insert(0x484f9e44, "s32DNFS_IO_Control");
    method_entries.insert(0x484ed508, "pcFindPattern");
    method_entries.insert(0x4850977c, "u32IoscMessageQueueStatus");
    method_entries.insert(0x484fbe08, "FSReturnLast");
    method_entries.insert(0x4850dee4, "vGetMsgQueueStatus");
    method_entries.insert(0x485458c4, "ecdh_compute_shared_secret");
    method_entries.insert(0x48567b54, "__data_start");
    method_entries.insert(0x484ecfc8, "vGetTopInfo");
    method_entries.insert(0x485310e4, "fd_crypt_get_signature_verify_status");
    method_entries.insert(0x48524e68, "u32AcousticOutIOCtrl_Pause");
    method_entries.insert(0x48523508, "u32AcousticOutIOCtrl_GetSamplerate");
    method_entries.insert(0x4852a3e0, "vResetUgzzc");
    method_entries.insert(0x484f6dc0, "s32DNFS_IO_Remove");
    method_entries.insert(0x48529714, "vTraceAcousticInInfo");
    method_entries.insert(0x4853d51c, "rnd_byte_array");
    method_entries.insert(0x485180cc, "bCleanUpShMemofContext");
    method_entries.insert(0x48539328, "GetSdCardRefreshStatus");
    method_entries.insert(0x4852c14c, "vFFDUnLock");
    method_entries.insert(0x48535ae8, "libminxml_get_content");
    method_entries.insert(0x48569960, "szErrorString_NO_ERROR");
    method_entries.insert(0x4851efe0, "exc_lock");
    method_entries.insert(0x485235d0, "u32AcousticOutIOCtrl_GetSuppSamplerate");
    method_entries.insert(0x48502418, "UvEvent_Socket_Open");
    method_entries.insert(0x4850c1b4, "u32CloseMsgQueue");
    method_entries.insert(0x484f5344, "u32CheckForValidDNFSFile");
    method_entries.insert(0x4851dcc0, "vSysCallbackHandler");
    method_entries.insert(
        0x4852c084,
        "_Z17drv_bt_vTraceInfo16TR_tenTraceLevel21tenDrvBTTraceFunction16tenDrvBTTraceMsgPKciii",
    );
    method_entries.insert(0x484edd20, "OsalIO_vShutdown");
    method_entries.insert(0x48526160, "ACOUSTICOUT_s32IOClose");
    method_entries.insert(0x48519160, "OSAL_s32SemaphoreOpen");
    method_entries.insert(0x4856c768, "__bss_start__");
    method_entries.insert(0x485117ec, "vPrinthexDump");
    method_entries.insert(0x4852d5f4, "psFFDGetConfigPointer");
    method_entries.insert(0x4853fdb8, "BPCL_RC5_Decrypt");
    method_entries.insert(0x484ed564, "s32GetPidFromName");
    method_entries.insert(0x48569958, "szErrorString_CANCELED");
    method_entries.insert(0x4851c7f8, "vDisplayManual");
    method_entries.insert(0x48518750, "vTraceSCB");
    method_entries.insert(0x484f6150, "LFS_u32IOWrite");
    method_entries.insert(0x48571ac0, "sNavSDCard_lib");
    method_entries.insert(0x4856e7e0, "hMqLiMain");
    method_entries.insert(0x484f6318, "s32DNFS_IO_Write");
    method_entries.insert(0x485089b0, "vOnProcessDetach");
    method_entries.insert(0x48517944, "OSAL_s32SharedMemoryUnmap");
    method_entries.insert(0x484ece50, "vUnregisterOsalIO_Callback");
    method_entries.insert(0x48544e5c, "BPCL_AES_Decrypt");
    method_entries.insert(0x484f7c28, "s32RemoveFile");
    method_entries.insert(0x48515188, "OSAL_s32ThreadSuspend");
    method_entries.insert(0x4851e54c, "exc_print_wrong_lock_func");
    method_entries.insert(0x48520930, "AUXCLOCK_s32IOControl");
    method_entries.insert(0x48515650, "OSAL_s32ThreadActivate");
    method_entries.insert(0x48572bd0, "AIO_JobTrigger");
    method_entries.insert(0x4852a268, "asipGetDataChannel");
    method_entries.insert(0x48569904, "szErrorString_INVALIDVALUE");
    method_entries.insert(0x4853a7e4, "BPCL_EC_Compute_Public_Key");
    method_entries.insert(0x48533838, "pcGetVIN");
    method_entries.insert(0x48537d38, "fd_device_ctrl_u32SDCardInfo");
    method_entries.insert(0x48535dd8, "libminxml_open");
    method_entries.insert(0x4852f6a4, "DRV_DIAG_EOL_s32IOControl");
    method_entries.insert(0x48509070, "u32MapErrorCodeIOSC");
    method_entries.insert(0x485338f0, "trGetFileData");
    method_entries.insert(0x48509368, "vPrintSharedMemoryTable");
    method_entries.insert(0x4852b7c8, "drv_bt_lld_uart_write");
    method_entries.insert(0x48515f90, "OSAL_vProcessExit");
    method_entries.insert(0x4851eb74, "exc_check_access");
    method_entries.insert(0x4851c664, "TraceIDString");
    method_entries.insert(0x48516d0c, "OSAL_ProcessSpawn");
    method_entries.insert(0x48505060, "vNewCallbackHandler");
    method_entries.insert(0x484ebfdc, "RelSyncObj");
    method_entries.insert(0x48545c88, "field_element_set_one");
    method_entries.insert(0x485383d4, "OSALUTIL_s32FTell");
    method_entries.insert(0x484ec0d0, "LockOsal");
    method_entries.insert(0x485192b8, "OSAL_s32SemaphoreDelete");
    method_entries.insert(0x4852a020, "asipSetRtr");
    method_entries.insert(0x48520878, "debug_exeption_handler_line");
    method_entries.insert(0x48522cf0, "u32AcousticOutIOCtrl_SetErrThr");
    method_entries.insert(0x484f12b4, "dispatcher_table_lookup");
    method_entries.insert(0x48519790, "s32TimerTableCreate");
    method_entries.insert(0x485092f0, "vBlockMemForMq");
    method_entries.insert(0x484f4280, "OSAL_vShutdownAsyncIO");
    method_entries.insert(0x48518154, "OSAL_SharedMemoryCreate");
    method_entries.insert(0x48523700, "u32AcousticOutIOCtrl_SetDecoder");
    method_entries.insert(0x485300b8, "FD_Crypt_vEnterBPCLCriticalSec");
    method_entries.insert(0x48569968, "hLI_TERM_MQ");
    method_entries.insert(0x485065ac, "OSAL_s32EventPost");
    method_entries.insert(0x4851d334, "vMkDir");
    method_entries.insert(0x4856fcc8, "u64SetTimeSystemElapsed");
    method_entries.insert(0x4852fb38, "DRV_DIAG_EOL_s32IODeviceRemove");
    method_entries.insert(0x48509b28, "vTraceCallbackExecution");
    method_entries.insert(0x48545760, "ForWipro_ecdh_compute_first_phase_value");
    method_entries.insert(0x48505d54, "OSAL_vErrorHook");
    method_entries.insert(0x4852fe30, "ERRMEM_s32IOWrite_impl");
    method_entries.insert(0x484efd20, "OSAL_get_exapp_id");
    method_entries.insert(0x4855e410, "__exidx_end");
    method_entries.insert(0x485703f6, "gu16TxLastOpcode");
    method_entries.insert(0x485239f4, "AcousticOutIOCtrl_RegNotification");
    method_entries.insert(0x4853835c, "OSALUTIL_s32FGetSize");
    method_entries.insert(0x4851cc04, "vRmFileSelection");
    method_entries.insert(0x4850fb38, "bFilterMsg");
    method_entries.insert(0x48533abc, "trGetRevocationList");
    method_entries.insert(0x484f3aa8, "vSetJobQueue");
    method_entries.insert(0x484fe13c, "prm_u32Prm");
    method_entries.insert(0x484eff3c, "vCloseTripFile");
    method_entries.insert(0x4852fde8, "ERRMEM_s32IOControl_impl");
    method_entries.insert(0x4852a204, "asipGetDataLen");
    method_entries.insert(0x484f3a8c, "IsNfsDev");
    method_entries.insert(0x48535ba4, "libminxml_get_prev_level_element");
    method_entries.insert(0x4851fc34, "exception_handler_lock");
    method_entries.insert(0x48535abc, "libminxml_validate_result_set");
    method_entries.insert(0x4856e7e4, "hMqTe");
    method_entries.insert(0x485111e4, "pu32GetSharedBaseAdress");
    method_entries.insert(0x48538174, "OSALUTIL_s32NPrintFormat");
    method_entries.insert(0x4852a5a0, "asipChkPktCRC");
    method_entries.insert(0x48525a64, "u32AcousticOutIOCtrl_WaitEvent");
    method_entries.insert(0x48569940, "szErrorString_EVENTINUSE");
    method_entries.insert(0x485361f8, "libminxml_get_next_attr_res");
    method_entries.insert(0x48547868, "field_element_square_w5_fast");
    method_entries.insert(0x4853850c, "OSALUTIL_s32SaveVarNPrintFormat");
    method_entries.insert(0x484f3624, "REGISTRY_u32IOCreate");
    method_entries.insert(0x4851489c, "bThreadTableDeleteEntryByName");
    method_entries.insert(0x48509a00, "vTraceIoscMqNotify");
    method_entries.insert(0x485703e4, "gu32AsipRxLen");
    method_entries.insert(0x4850cec4, "OSAL_pvMemoryMap");
    method_entries.insert(0x48502348, "prm_vTimerCallbackPort1");
    method_entries.insert(0x4850eaa8, "OSAL_s32MessageQueueClose");
    method_entries.insert(0x48517678, "bDelLiPrcMemPtr");
    method_entries.insert(0x4850b004, "u32UnLockOsalArea");
    method_entries.insert(0x4852a104, "asipSetHeaderSeqReset");
    method_entries.insert(0x484fbda8, "FSFileFind");
    method_entries.insert(0x48514c2c, "OSAL_s32ThreadList");
    method_entries.insert(0x48516c84, "pvLoadModule");
    method_entries.insert(0x484ecf70, "vStartErrmemWriteTask");
    method_entries.insert(0x48548a24, "field_element_modular_invert");
    method_entries.insert(0x48572bd4, "AIO_aJobFlg");
    method_entries.insert(0x4856d49c, "gpOsalProcDat");
    method_entries.insert(0x4852c8f4, "DEV_FFD_s32IOClose");
    method_entries.insert(0x48571af8, "SDrefreshCallback_fnc");
    method_entries.insert(0x485703d8, "gpu8AsipTxBuf");
    method_entries.insert(0x4851db0c, "vReadFile");
    method_entries.insert(0x485226a8, "ACOUSTICIN_s32IOOpen");
    method_entries.insert(0x4851578c, "OSAL_ThreadCreate");
    method_entries.insert(0x4852a644, "_Z11asipSendAckhh");
    method_entries.insert(0x48538590, "s32get_refresh_progress");
    method_entries.insert(0x48511554, "OSAL_s32MessagePoolGetCurrentSize");
    method_entries.insert(0x4853ac80, "BPCL_EC_Create_Key_Pair");
    method_entries.insert(0x4851722c, "vTraceShMem");
    method_entries.insert(0x485307d8, "fd_crypt_sdx_xml_verification");
    method_entries.insert(0x4850603c, "vTraceECB");
    method_entries.insert(0x48513cc0, "OSAL_s32MessageCreate");
    method_entries.insert(0x484fa7a0, "s32Intern_IO_Close");
    method_entries.insert(0x484ffc60, "vPostSystemInfo");
    method_entries.insert(0x48507468, "OSAL_s32SetFPEMode");
    method_entries.insert(0x48508848, "vStopDebugTask");
    method_entries.insert(0x48514958, "CreatePseudoThreadEntry");
    method_entries.insert(0x4856999c, "aDaysOfMonth");
    method_entries.insert(0x48535b20, "libminxml_get_attr_value");
    method_entries.insert(0x4852a130, "asipChkHeaderRemoteID");
    method_entries.insert(0x48571a0c, "_u32UpStreamDataCount");
    method_entries.insert(0x484f2228, "RegistryDeInit");
    method_entries.insert(0x484fa560, "LFS_u32IOClose");
    method_entries.insert(0x48544e90, "BPCL_Free_Memory");
    method_entries.insert(0x4852ee5c, "s32KDSSendMessageWriteData");
    method_entries.insert(0x48569948, "szErrorString_NOTINTERRUPTCALLABLE");
    method_entries.insert(0x4852f9cc, "DRV_DIAG_EOL_IOOpen");
    method_entries.insert(0x4851bda8, "OSAL_u32TimerGetResolution");
    method_entries.insert(0x4851cbc0, "vRmFile");
    method_entries.insert(0x4851e3d0, "LLD_bIsTraceActive");
    method_entries.insert(0x48507610, "vInitGlobalData");
    method_entries.insert(0x48538254, "OSALUTIL_s32FPrintf");
    method_entries.insert(0x484ece54, "vTraceIOMemAccessError");
    method_entries.insert(0x484fbf54, "vInvalidateFileDescriptors");
    method_entries.insert(0x48572d3c, "__end__");
    method_entries.insert(0x4851623c, "vSetNiceLevel");
    method_entries.insert(0x485703f0, "gu32RxMsgCount");
    method_entries.insert(0x484ec270, "DelSyncObj");
    method_entries.insert(0x48533b20, "trGetRevocTimestamp");
    method_entries.insert(0x4850fc9c, "vTraceCcaMsg");
    method_entries.insert(0x4852947c, "vTraceAcousticOutPrintf");
    method_entries.insert(0x48572d3c, "__bss_end__");
    method_entries.insert(0x48511654, "OSAL_vSetMessageTrace");
    method_entries.insert(0x48509478, "vpGetValidMemPtr");
    method_entries.insert(0x484f34f0, "REGISTRY_u32IOOpen");
    method_entries.insert(0x48500538, "bGetBoardTyp");
    method_entries.insert(0x48523344, "u32AcousticOutIOCtrl_GetSuppSampleformat");
    method_entries.insert(0x48520a78, "ACOUSTICIN_s32Deinit");
    method_entries.insert(0x484ee928, "vOsalTraceOutRegistry");
    method_entries.insert(0x48504b24, "vReadAssertMode");
    method_entries.insert(0x4854c458, "PRODUCT_C_STRING_NAV_BUILDDATE");
    method_entries.insert(0x4850368c, "prm_CreateLibUsbConnectionTask");
    method_entries.insert(0x4852f19c, "DRV_DIAG_EOL_s32IOWrite");
    method_entries.insert(0x484eed88, "vEraseErrmem");
    method_entries.insert(0x485122e8, "OSAL_s32CheckMessagePool");
    method_entries.insert(0x48569978, "rSysTimeBase");
    method_entries.insert(0x4851b074, "bStopAllTimer");
    method_entries.insert(0x48531cb4, "bFileExists");
    method_entries.insert(0x48546bf0, "field_element_multiply_w5_fast");
    method_entries.insert(0x485244fc, "u32AbortStream");
    method_entries.insert(0x4850fe18, "OSAL_s32MessageQueueWait");
    method_entries.insert(0x4856c730, "_gkmke");
    method_entries.insert(0x48516034, "vSetCgroup");
    method_entries.insert(0x484ebf88, "GetSyncObjVal");
    method_entries.insert(0x4851c540, "vTraceMemAccessError");
    method_entries.insert(0x48524060, "ACOUSTICOUT_s32Deinit");
    method_entries.insert(0x484f4eec, "vDeInitAccessCtrl");
    method_entries.insert(0x484f11d8, "GetNucleusLocName");
    method_entries.insert(0x48522afc, "bIsBuffersizeValid");
    method_entries.insert(0x4856e7dc, "pMsgQArea");
    method_entries.insert(0x484f194c, "OSAL_IOOpen");
    method_entries.insert(0x484f5180, "u32TraceBuf_Update");
    method_entries.insert(0x4850cd98, "vReactOnAllocError");
    method_entries.insert(0x4856c75c, "_gkmka");
    method_entries.insert(0x48548eb0, "field_element_degree");
    method_entries.insert(0x4850eea4, "u32GetValidMqHandle");
    method_entries.insert(0x48505a74, "OSAL_coszErrorText");
    method_entries.insert(0x4856adf0, "gu32TxTimestamp");
    method_entries.insert(0x4851c3e4, "vUnregisterSysCallback");
    method_entries.insert(0x4852b938, "drv_bt_lld_uart_init");
    method_entries.insert(0x484f08c0, "OSAL_device_compare");
    method_entries.insert(0x48569964, "hTE_TERM_MQ");
    method_entries.insert(0x48517640, "s32CheckForIoscShm");
    method_entries.insert(0x484f2c50, "bUnLockRegistry");
    method_entries.insert(0x484ecaec, "vTraceOpenFiles");
    method_entries.insert(0x4851687c, "s32ProcessTableCreate");
    method_entries.insert(0x4850f2e0, "OSAL_s32MessageQueueCreate");
    method_entries.insert(0x4850984c, "u32IoscMessageQueueNotify");
    method_entries.insert(0x4853d8c4, "rnd_word");
    method_entries.insert(0x4852c470, "DEV_FFD_s32IOWrite");
    method_entries.insert(0x4850cbc8, "vGetGlobalIoscData");
    method_entries.insert(0x48509c8c, "u32GetFromMessageQueue");
    method_entries.insert(0x48514ecc, "OSAL_s32ThreadPriority");
    method_entries.insert(0x48524f90, "u32AcousticOutIOCtrl_Stop");
    method_entries.insert(0x4850b534, "OSAL_IoscSharedMemoryOpen");
    method_entries.insert(0x4851c450, "vRegisterSysCallback");
    method_entries.insert(0x485147a0, "OSAL_vThreadExit");
    method_entries.insert(0x4850d4e8, "OSAL_pvMemPoolFixSizeGetBlockOfPool");
    method_entries.insert(0x4852eba4, "KDS_s32IODeviceInit");
    method_entries.insert(0x48535f34, "libminxml_plat_fopen");
    method_entries.insert(0x485154dc, "s32ThreadTableDeleteEntries");
    method_entries.insert(0x4853f7cc, "BPCL_HMAC_SHA1");
    method_entries.insert(0x48572cb8, "error");
    method_entries.insert(0x484f6c14, "LFS_u32IORemove");
    method_entries.insert(0x4856adf4, "gu8OwnID");
    method_entries.insert(0x4851c444, "LLD_vUnRegTraceCallback");
    method_entries.insert(0x4852c09c, "_Z12drvBtAsipCrcPht");
    method_entries.insert(0x484ec3cc, "CreSyncObj");
    method_entries.insert(0x4852fddc, "ERRMEM_S32IOOpen");
    method_entries.insert(0x48502acc, "Prm_GetUsbPortState");
    method_entries.insert(0x4852a218, "asipClearPayloadDDummy");
    method_entries.insert(0x48569924, "szErrorString_NOFILEDESCRIPTOR");
    method_entries.insert(0x4852a270, "asipGetDataAddr");
    method_entries.insert(0x4851ae54, "OSAL_s32TimerDelete");
    method_entries.insert(0x4852fd84, "ERRMEM_s32IOWrite");
    method_entries.insert(0x48535be4, "libminxml_get_next_element_name");
    method_entries.insert(0x48572be0, "cErrMemBuffer");
    method_entries.insert(0x48571462, "u8TxTestMode");
    method_entries.insert(0x4856adf4, "gu8CRC");
    method_entries.insert(0x4851c76c, "vGetResourceData");
    method_entries.insert(0x4850c980, "vDeInitOsalIOSC");
    method_entries.insert(0x48514ae0, "vSetErrorCode");
    method_entries.insert(0x48518a74, "vTraceSemInfo");
    method_entries.insert(0x48516978, "s32ThreadTableCreate");
    method_entries.insert(0x484eff98, "bSendTripMessages");
    method_entries.insert(0x4854b530, "arm_backtrace");
    method_entries.insert(0x485143e0, "tProcessTableGetFreeIndex");
    method_entries.insert(0x4851e828, "backtrace_to_errmem");
    method_entries.insert(0x48571a14, "_bDrvBtSocketWork");
    method_entries.insert(0x48501f8c, "watch_mounts");
    method_entries.insert(0x48535ea8, "libminxml_plat_loginit");
    method_entries.insert(0x48572cbc, "s32Refresh_state");
    method_entries.insert(0x48544e20, "BPCL_AES_Encrypt");
    method_entries.insert(0x484ec114, "vGenerateSyncObjects");
    method_entries.insert(0x48569954, "szErrorString_NOTSUPPORTED");
    method_entries.insert(0x4853204c, "fd_crypt_verify_xml_signature");
    method_entries.insert(0x4853acac, "point_free");
    method_entries.insert(0x4853b894, "point_jacobian_add");
    method_entries.insert(0x4852448c, "u32SetEvent");
    method_entries.insert(0x4854c4c8, "PRODUCT_C_STRING_MANUFACTURER");
    method_entries.insert(0x48517530, "vpGetValidLiPrcMemPtr");
    method_entries.insert(0x484f59b4, "LFS_u32IORead");
    method_entries.insert(0x48548d44, "field_element_modular_divide_2");
    method_entries.insert(0x4850b7a4, "OSAL_IoscSharedMemoryCreate");
    method_entries.insert(0x4851ab0c, "OSAL_s32TimerGetTime");
    method_entries.insert(0x4851a598, "OSAL_s32TimerSetTime");
    method_entries.insert(0x48506460, "OSAL_s32EventStatus");
    method_entries.insert(0x4856c768, "__bss_start");
    method_entries.insert(0x484f1fcc, "s32OSAL_get_device_id_and_filename_by_Path");
    method_entries.insert(0x485703f8, "gu32NumberSkippedMsg");
    method_entries.insert(0x48522a88, "bIsSamplerateValid");
    method_entries.insert(0x48504f68, "u32GetPrcLocalMsgQHandle");
    method_entries.insert(0x48509184, "bDelMemPtr");
    method_entries.insert(0x48558e74, "wsset");
    method_entries.insert(0x48520920, "AUXCLOCK_IOOpen");
    method_entries.insert(0x48536c08, "fd_device_ctrl_vCIDPatternAdapt");
    method_entries.insert(0x485079bc, "vGetOsalLock");
    method_entries.insert(0x48505004, "u32ExecuteLiCb");
    method_entries.insert(0x48508fec, "u32GetActualMsg");
    method_entries.insert(0x484ee414, "vOsalSpawnLoadTask");
    method_entries.insert(0x48571464, "pMyUart");
    method_entries.insert(0x48523874, "u32AcousticOutIOCtrl_NextNeededDecoder");
    method_entries.insert(0x4853d4d0, "rnd_seed");
    method_entries.insert(0x484f00ec, "TRACE_s32IOControl");
    method_entries.insert(0x485698f8, "u16CryptMedium");
    method_entries.insert(0x4853394c, "trGetPartSignature");
    method_entries.insert(0x485240a4, "ACOUSTICOUT_s32Init");
    method_entries.insert(0x485383c4, "OSALUTIL_s32FGetpos");
    method_entries.insert(0x48569910, "szErrorString_INTERRUPT");
    method_entries.insert(0x4852091c, "exc_rand_r");
    method_entries.insert(0x48545ce8, "field_element_is_zero");
    method_entries.insert(0x48572d3c, "_bss_end__");
    method_entries.insert(0x485703f4, "gu16RxLastOpcode");
    method_entries.insert(0x484efeb0, "OSAL_device_profd");
    method_entries.insert(0x4856adf6, "_bUgzzcInReset");
    method_entries.insert(0x485242f0, "u32InitOutputDevice");
    method_entries.insert(0x48511210, "bMsgAreaCorrection");
    method_entries.insert(0x4850e65c, "u32CheckForDelete");
    method_entries.insert(0x48530d34, "fd_crypt_create");
    method_entries.insert(0x48515bd8, "OSAL_ThreadSpawn");
    method_entries.insert(0x4852998c, "vTraceAcousticOutError");
    method_entries.insert(0x48523c10, "u32AcousticOutIOCtrl_SetTime");
    method_entries.insert(0x485074d8, "OSAL_u8GetFPE");
    method_entries.insert(0x48569944, "szErrorString_WRONGFUNC");
    method_entries.insert(0x4852a284, "asipGetDataOpcode");
    method_entries.insert(0x48520a70, "ACOUSTICIN_s32Init");
    method_entries.insert(0x484f2e24, "REGISTRY_u32IOControl");
    method_entries.insert(0x485715fa, "_u8Buf");
    method_entries.insert(0x48571af4, "sdRefresh_tid");
    method_entries.insert(0x4851c3e8, "s32GetFileSize");
    method_entries.insert(0x48538724, "SD_Refresh_s32RefreshReadyCallback");
    method_entries.insert(0x485247ec, "u32DoWriteOperation");
    method_entries.insert(0x48548688, "field_element_modular_subtract");
    method_entries.insert(0x4852a784, "_Z16bReleaseCriticalm");
    method_entries.insert(0x48519870, "s32TimerTableDeleteEntries");
    method_entries.insert(0x4850e748, "OSAL_s32MessageQueueDelete");
    method_entries.insert(0x4853b280, "point_jacobian_double");
    method_entries.insert(0x485384bc, "OSALUTIL_prOpenDir");
    method_entries.insert(0x484f72f0, "u32GetOpenHandles");
    method_entries.insert(0x484f692c, "s32FileCopy");
    method_entries.insert(0x485112b4, "vSetEmptyPoolInvestigation");
    method_entries.insert(0x484ffdfc, "prm_vInit");
    method_entries.insert(0x484f4924, "OSAL_s32IOWriteAsync");
    method_entries.insert(0x484edc78, "s32OsalDrvDeInit");
    method_entries.insert(0x485480b4, "field_element_reduce_secp160r1");
    method_entries.insert(0x4853653c, "libminxml_priv_cleanup");
    method_entries.insert(0x4852316c, "u32AcousticOutIOCtrl_GetSuppChannels");
    method_entries.insert(0x484f2c80, "bLockRegistry");
    method_entries.insert(0x485141d8, "OSAL_ProcessWhoAmI");
    method_entries.insert(0x48515d10, "OSAL_s32ProcessList");
    method_entries.insert(0x4853fb74, "BPCL_RC4_PrepareKey");
    method_entries.insert(0x4852edf0, "s32KDSCheckValidReadData");
    method_entries.insert(0x48528a14, "ACOUSTICSRC_s32IOClose");
    method_entries.insert(0x48529f84, "BT_UGZZC_s32IODeviceInit");
    method_entries.insert(0x4850b3b0, "OSAL_u32IoscSharedMemoryClose");
    method_entries.insert(0x48511984, "vPrintMsgInfo");
    method_entries.insert(0x484f5c88, "s32DNFS_IO_Read");
    method_entries.insert(0x4850dd98, "OSAL_s32MessageQueueStatus");
    method_entries.insert(0x484fcaf8, "vGetPrmInfo");
    method_entries.insert(0x4852ad50, "_Z14s32MsgCompletePh");
    method_entries.insert(0x484fc298, "FSFileRelink");
    method_entries.insert(0x4850d86c, "vMqueueLiMqMapInit");
    method_entries.insert(0x485225b0, "ACOUSTICIN_s32IOClose");
    method_entries.insert(0x48538794, "SD_Refresh_s32RegisterRefreshReadyCallback");
    method_entries.insert(0x4852feb8, "FD_Crypt_vLeaveBPCLCriticalSec");
    method_entries.insert(0x484ee7b4, "vOsalDisplayLoadTaskStatus");
    method_entries.insert(0x48512ea8, "OSAL_pu8MessageContentGet");
    method_entries.insert(0x4851c6f8, "TraceString");
    method_entries.insert(0x4850bc90, "u32CreateMsgQueue");
    method_entries.insert(0x48529f34, "BT_UGZZC_s32IODeviceRemove");
    method_entries.insert(0x4851d928, "vShowCurrentResourceSitutaion");
    method_entries.insert(0x4852a1f8, "asipGetHeaderSeqReset");
    method_entries.insert(0x4852a2c0, "bDrvBtAsipInit");
    method_entries.insert(0x48533780, "pcGetLifetimeStart");
    method_entries.insert(0x4856993c, "szErrorString_WRONGTHREAD");
    method_entries.insert(0x48515fb8, "OSAL_s32ProcessDelete");
    method_entries.insert(0x4850a338, "vReactOnIoscMqOverflow");
    method_entries.insert(0x484f47dc, "OSAL_s32IOReadAsync");
    method_entries.insert(0x484fc788, "FSFileDel");
    method_entries.insert(0x48515f9c, "OSAL_s32ProcessJoin");
    method_entries.insert(0x484eff04, "TRACE_s32IOWrite");
    method_entries.insert(0x485337dc, "pu8GetCID");
    method_entries.insert(0x48556e5c, "devname");
    method_entries.insert(0x48545c40, "field_element_set_zero");
    method_entries.insert(0x4850e61c, "vDeleteTeRes");
    method_entries.insert(0x4852a060, "asipSetHeaderPayLen");
    method_entries.insert(0x485179f0, "OSAL_pvSharedMemoryMap");
    method_entries.insert(0x48511ecc, "OSAL_s32MessagePoolOpen");
    method_entries.insert(0x485207bc, "arm_backtrace_exception");
    method_entries.insert(0x48523e4c, "u32AcousticOutIOCtrl_GetBuffersize");
    method_entries.insert(0x4852e31c, "KDS_s32IOControl");
    method_entries.insert(0x48569918, "szErrorString_ALREADYEXISTS");
    method_entries.insert(0x484ec50c, "WaiSyncObj");
    method_entries.insert(0x4857081a, "_u8RxBuf");
    method_entries.insert(0x484f7cf4, "s32GetDirContent");
    method_entries.insert(0x484efebc, "TRACE_s32IOCreate");
    method_entries.insert(0x485699a8, "aDaysOfMonthLeapYr");
    method_entries.insert(0x48505eb8, "s32EventTableCreate");
    method_entries.insert(0x4850cd20, "OSAL_pvMemoryUnMap");
    method_entries.insert(0x48501e78, "handle_mount");
    method_entries.insert(0x48505e4c, "OSAL_vSetErrorCode");
    method_entries.insert(0x4851558c, "bCleanUpThreadofContext");
    method_entries.insert(0x484f11d0, "OSAL_test_and_set_inuse_descriptor");
    method_entries.insert(0x4853fc10, "BPCL_RC4_Crypt");
    method_entries.insert(0x484f440c, "OSAL_bInitAsyncIO");
    method_entries.insert(0x48535b3c, "libminxml_get_next_element");
    method_entries.insert(0x4853ad68, "point_is_zero");
    method_entries.insert(0x48514fa8, "OSAL_s32ThreadWait");
    method_entries.insert(0x484f4684, "s32ShutdownAsyncTask");
    method_entries.insert(0x485084c0, "bOnProcessAttach");
    method_entries.insert(0x4856ae08, "_hDrvBtThreadID_SPPSS");
    method_entries.insert(0x48528d34, "ACOUSTICSRC_s32IOOpen");
    method_entries.insert(0x4852b70c, "drv_bt_lld_uart_close");
    method_entries.insert(0x4851c328, "LLD_bOpenTrace");
    method_entries.insert(0x4851caec, "vStartProc");
    method_entries.insert(0x4852c08c, "_Z14drv_bt_vPrintfPKcz");
    method_entries.insert(0x4852c1b0, "psFFDLock");
    method_entries.insert(0x4852a198, "asipGetPacketSize");
    method_entries.insert(0x48536544, "libminxml_priv_parse");
    method_entries.insert(0x4852ef80, "s32KDSCreateMsgQueue");
    method_entries.insert(0x485703fc, "gu32RxMsg2AppCount");
    method_entries.insert(0x48558e1c, "startcharset");
    method_entries.insert(0x4856ae04, "_hDrvBtWriteSemId");
    method_entries.insert(0x48515334, "OSAL_s32ThreadDelete");
    method_entries.insert(0x4850d32c, "OSAL_s32MemPoolFixSizeRelBlockOfPool");
    method_entries.insert(0x484efec4, "TRACE_s32IORemove");
    method_entries.insert(0x4850a710, "u32ExecuteCb");
    method_entries.insert(0x485299dc, "vTraceAcousticSrcError");
    method_entries.insert(0x4851efb4, "exc_unlock");
    method_entries.insert(0x484ebd14, "u32ExecuteCallback");
    method_entries.insert(0x485211dc, "ACOUSTICIN_s32IOControl");
    method_entries.insert(0x484f1488, "OSAL_set_trace_level");
    method_entries.insert(0x4852d93c, "FFD_vTraceCommand");
    method_entries.insert(0x4851ea4c, "eh_reboot");
    method_entries.insert(0x4856990c, "szErrorString_BUSY");
    method_entries.insert(0x4852d8b0, "FFD_vTrace");
    method_entries.insert(0x485703d4, "gu8AsipTxSeqNo");
    method_entries.insert(0x4850b030, "u32LockOsalArea");
    method_entries.insert(0x48572d3c, "_end");
    method_entries.insert(0x484f4644, "vPrepAsyncTask");
    method_entries.insert(0x48571a18, "s32DrvBt_newsockfd");
    method_entries.insert(0x484edd8c, "s32OsalDrvInit");
    method_entries.insert(0x4853858c, "OSALUTIL_szSaveStringNConcat");
    method_entries.insert(0x48504d48, "OSAL_vAssertFunction");
    method_entries.insert(0x48508ab8, "vDebugFacility");
    method_entries.insert(0x485383fc, "OSALUTIL_s32FSeek");
    method_entries.insert(0x48505ce8, "OSAL_s32CallErrorHook");
    method_entries.insert(0x484f1720, "OSAL_IOOpen_Ex");
    method_entries.insert(0x485449c0, "aes_decrypt");
    method_entries.insert(0x48524418, "u32ClearEvent");
    method_entries.insert(0x4850d9f8, "vGetMsgQueueMaxFillLevels");
    method_entries.insert(0x4852a7d0, "_Z14bEnterCriticalmm");
    method_entries.insert(0x485304d8, "pvGetCryptDevDBEntry");
    method_entries.insert(0x4854c4c4, "PRODUCT_C_STRING_NAV_VERSION_ADVANCED");
    method_entries.insert(0x4854c464, "PRODUCT_C_STRING_NAV_VERSION");
    method_entries.insert(0x4852a298, "asipNextSeqNo");
    method_entries.insert(0x4852a0cc, "asipSetHeaderCRC");
    method_entries.insert(0x484fcdb0, "vPrmTtfisTrace");
    method_entries.insert(0x48509058, "OSAL_s32IoscSharedMemoryUnmap");
    method_entries.insert(0x4856b7bc, "vrFFDConfigData_GM");
    method_entries.insert(0x4854b55c, "arm_backtrace_asm");
    method_entries.insert(0x4850f028, "OSAL_s32MessageQueueOpen");
    method_entries.insert(0x48572cc0, "refresh_state");
    method_entries.insert(0x485483e0, "field_element_modular_add");
    method_entries.insert(0x48538ffc, "s32obtainPath");
    method_entries.insert(0x48538e70, "s32create_RefreshData");
    method_entries.insert(0x484f4ee4, "bInitAccessCtrl");
    method_entries.insert(0x4853aea4, "point_invert");
    method_entries.insert(0x4852993c, "vTraceAcousticInError");
    method_entries.insert(0x485200a8, "OSALSignalHandlestatus");
    method_entries.insert(0x48517224, "s32SharedMemoryTableDeleteEntries");
    method_entries.insert(0x48530468, "bSetCryptDevStateProgress");
    method_entries.insert(0x48505f48, "vSetTraceFlagForEvent");
    method_entries.insert(0x4856b7c8, "vrFFDConfigData_VOLVO");
    method_entries.insert(0x4853daec, "BPCL_SHA1_Init");
    method_entries.insert(0x4852a18c, "asipGetHeaderCRC");
    method_entries.insert(0x4853c174, "point_convert_to_jacobian");
    method_entries.insert(0x48522b48, "vResetErrorCounters");
    method_entries.insert(0x485303e0, "bSetCryptDevStatus");
    method_entries.insert(0x485131f4, "OSAL_M_TRACE_MESSAGEPOOL");
    method_entries.insert(0x485125d4, "OSAL_vPrintMemoryLeaks");
    method_entries.insert(0x48519788, "OSAL_s32SemaphoreCreate");
    method_entries.insert(0x48514d50, "OSAL_s32ThreadControlBlock");
    method_entries.insert(0x484fb8a8, "s32DNFS_IO_Open");
    method_entries.insert(0x4851c494, "vTraceIOErrorCode");
    method_entries.insert(0x484f74b0, "u32ReadExtDir");
    method_entries.insert(0x4856c73a, "_gkmkb");
    method_entries.insert(0x48526c5c, "ACOUSTICSRC_s32Init");
    method_entries.insert(0x48569920, "szErrorString_MAXFILES");
    method_entries.insert(0x484fca28, "u32CardDeviceName");
    method_entries.insert(0x48524c88, "u32AcousticOutIOCtrl_Extwrite");
    method_entries.insert(0x4852ad10, "vCheckMsgError");
    method_entries.insert(0x4852c674, "DEV_FFD_s32IORead");
    method_entries.insert(0x484f3ae0, "nFindJobQueue");
    method_entries.insert(0x4853acf0, "point_allocate");
    method_entries.insert(0x48545508, "ecdsa_signature_generation");
    method_entries.insert(0x4851f044, "exit_exception_handler");
    method_entries.insert(0x4852ca40, "DEV_FFD_IOOpen");
    method_entries.insert(0x484efed4, "TRACE_s32IOClose");
    method_entries.insert(0x48536bec, "libminxml_priv_init");
    method_entries.insert(
        0x4852c088,
        "_Z16drv_bt_vTraceBuf16TR_tenTraceLevel16tenDrvBTTraceMsgPhm",
    );
    method_entries.insert(
        0x4852c098,
        "_Z19drv_bt_vTraceBufNet16TR_tenTraceLevel16tenDrvBTTraceMsghPhm",
    );
    method_entries.insert(0x48529c88, "BT_UGZZC_s32IOClose");
    method_entries.insert(0x48520140, "OSALConfigSignalHandle");
    method_entries.insert(0x48571a1c, "_bHciMode");
    method_entries.insert(0x484ed90c, "vChangeSVG_Configuration");
    method_entries.insert(0x4850bc1c, "bGetIoscMqInfo");
    method_entries.insert(0x48507518, "OSAL_vFPEReset");
    method_entries.insert(0x4852a2ac, "asicPrevSeqNo");
    method_entries.insert(0x48530740, "bGetDeviceName");
    method_entries.insert(0x4852bd0c, "vCreateThread_SPP_Socketserver");
    method_entries.insert(0x485026f0, "prmUSBPower_control");
    method_entries.insert(0x4852e16c, "KDS_s32IOWrite");
    method_entries.insert(0x485457cc, "ForWipro_ecdh_compute_shared_secret");
    method_entries.insert(0x48520928, "AUXCLOCK_s32IOClose");
    method_entries.insert(0x485459bc, "ecc_public_key_validation");
    method_entries.insert(0x484f4024, "OSAL_ASync_enGetState");
    method_entries.insert(0x4853816c, "OSALUTIL_u32GetBaseAPIVersion");
    method_entries.insert(0x48529fd4, "asipClearHeader");
    method_entries.insert(0x4850f8c4, "s32MqueueTableCreate");
    method_entries.insert(0x4852c260, "DEV_FFD_s32IOControl");
    method_entries.insert(0x48519400, "bCleanUpSemaphoreofContext");
    method_entries.insert(0x48570c30, "_au8DrvBtRxBuf");
    method_entries.insert(0x48569914, "szErrorString_NOACCESS");
    method_entries.insert(0x48531024, "fd_crypt_check_create");
    method_entries.insert(0x48512e00, "vCheckSituation");
    method_entries.insert(0x4850cfc8, "OSAL_s32MemPoolFixSizeDelete");
    method_entries.insert(0x484ed8a4, "TraceIOString");
    method_entries.insert(0x48525d00, "ACOUSTICOUT_s32IOControl");
    method_entries.insert(0x484f14c4, "OSAL_load_trace_level");
    method_entries.insert(0x48520398, "init_exception_handler");
    method_entries.insert(0x484ef8cc, "vInitOsalIO");
    method_entries.insert(0x484f2068, "vFreeMemory");
    method_entries.insert(0x48506314, "vTraceEventInfo");
    method_entries.insert(0x485237bc, "u32AcousticOutIOCtrl_GetDecoder");
    method_entries.insert(0x48516a74, "s32ProcessSetAffinity");
    method_entries.insert(0x484f06b4, "OSAL_save_trace_level");
    method_entries.insert(0x485445b0, "aes_encrypt");
    method_entries.insert(0x48508fc0, "s32GetIoscMqName");
    method_entries.insert(0x484fc1e8, "FSFileUnlink");
    method_entries.insert(0x484efd7c, "OSAL_get_device_name");
    method_entries.insert(0x484f2cb4, "REGISTRY_u32IORemove");
    method_entries.insert(0x4852a570, "asipSetPayloadData");
    method_entries.insert(0x484f08a0, "OSAL_device_getd");
    method_entries.insert(0x4856ae00, "_hDrvBtSemId");
    method_entries.insert(0x484edeb8, "vReduceMemory");
    method_entries.insert(0x4850e0a0, "vTraceMqNotify");
    method_entries.insert(0x485209c0, "AUXCLOCK_s32IORead");
    method_entries.insert(0x4852a62c, "asipSetHeaderChksum");
    method_entries.insert(0x48534f04, "bPerformFetch");
    method_entries.insert(0x4856f7f0, "s32Pid");
    method_entries.insert(0x4852e860, "KDS_IOOpen");
    method_entries.insert(0x4850d8b0, "vSetTraceFlagForChannel");
    method_entries.insert(0x48538568, "OSALUTIL_szSaveStringNCopy");
    method_entries.insert(0x4852a16c, "asipGetHeaderAck");
    method_entries.insert(0x4856c768, "_edata");
    method_entries.insert(0x484f2000, "OSAL_get_exclusive_tab_entry");
    method_entries.insert(0x4850c598, "u32DeleteMsgQueue");
    method_entries.insert(0x48535dbc, "libminxml_close");
    method_entries.insert(0x485389ac, "SD_Refresh_s32StopRefresh");
    method_entries.insert(0x485184d4, "s32SharedMemoryTableCreate");
    method_entries.insert(0x4851d87c, "vCopyDir");
    method_entries.insert(0x48538800, "writetoerrmem");
    method_entries.insert(0x4856fcbc, "OSALTmrHandleThreadExit");
    method_entries.insert(0x48546120, "field_element_assign");
    method_entries.insert(0x484ebca8, "OpenOsalLock");
    method_entries.insert(0x484f39ac, "IoscRegistryInit");
    method_entries.insert(0x4852cc5c, "DEV_FFD_s32IODeviceRemove");
    method_entries.insert(0x4852a5cc, "asipSetCRC");
    method_entries.insert(0x484ebbac, "vConvertMSectotimespec");
    method_entries.insert(0x4852327c, "u32AcousticOutIOCtrl_GetSampleformat");
    method_entries.insert(0x4853cc10, "point_jacobian_multiply_k_ary_window");
    method_entries.insert(0x4856adfc, "_bDrvClosed");
    method_entries.insert(0x48514b54, "OSAL_s32ThreadJoin");
    method_entries.insert(0x48571468, "caAtCommandTmpBuffer");
    method_entries.insert(0x484efeb4, "bOpenTrace");
    method_entries.insert(0x4852b8dc, "drv_bt_lld_uart_flushBuffer_Ugzzc");
    method_entries.insert(0x4853ae08, "point_is_infinity");
    method_entries.insert(0x4855caa8, "__exidx_start");
    method_entries.insert(0x484f0f90, "OSAL_s32IOClose");
    method_entries.insert(0x4853c774, "point_jacobian_multiply_fixed_window");
    method_entries.insert(0x4854764c, "field_element_square_classical");
    method_entries.insert(0x48522a6c, "bIsCodecValid");
    method_entries.insert(0x485071c8, "OSAL_s32EventCreate");
    method_entries.insert(0x485490ac, "field_element_wNAF");
    method_entries.insert(0x4853ae38, "point_assign");
    method_entries.insert(0x484ef180, "vOsalIoCallbackHandler");
    method_entries.insert(0x484ee3f4, "vOsalInitLoadTaskInfo");
    method_entries.insert(0x48572cc4, "fn");
    method_entries.insert(0x485469f8, "field_element_multiply_classical");
    method_entries.insert(0x48569900, "szErrorString_NOPERMISSION");
    method_entries.insert(0x485141fc, "tThreadTableSearchEntryByID");
    method_entries.insert(0x4852a918, "s32DrvBtAsipSendData");
    method_entries.insert(0x485381a8, "OSALUTIL_s32TraceWrite");
    method_entries.insert(0x48512260, "vPrintError");
    method_entries.insert(0x48539f5c, "BPCL_EC_DSA_Create");
    method_entries.insert(0x484f7e10, "u32CopyDirFiles");
    method_entries.insert(0x485293a4, "vTraceAcousticInPrintf");
    method_entries.insert(0x484fc374, "FSFileAdd");
    method_entries.insert(0x485246d8, "u32AcousticOutIOCtrl_Abort");
    method_entries.insert(0x48519900, "vTraceTimCB");
    method_entries.insert(0x48535ca8, "libminxml_free_rset");
    method_entries.insert(0x4851d2f0, "vRmDir");
    method_entries.insert(0x484ec7a4, "CreateOsalLock");
    method_entries.insert(0x48569934, "szErrorString_QUEUEFULL");
    method_entries.insert(0x48571460, "_bDrvBtTerminate");
    method_entries.insert(0x48569938, "szErrorString_WRONGPROCESS");
    method_entries.insert(0x4850d164, "vTraceOsalMpf");
    method_entries.insert(0x4852a540, "vFlushRx");
    method_entries.insert(0x48516b78, "s32ThreadSetAffinity");
    method_entries.insert(0x48535e68, "libminxml_plat_fgetc");
    method_entries.insert(0x4856992c, "szErrorString_BADFILEDESCRIPTOR");
    method_entries.insert(0x485296cc, "vTraceAcousticSrcInfo");
    method_entries.insert(0x48535acc, "libminxml_get_element_name");
    method_entries.insert(0x485703dc, "gu32AsipTxLen");
    method_entries.insert(0x48509238, "vRelMemForMq");
    method_entries.insert(0x485114d4, "OSAL_s32MessagePoolGetMinimalSize");
    method_entries.insert(0x4852bc54, "v_drv_bt_SPPdeviceConnect");
    method_entries.insert(0x48572cd4, "S");
    method_entries.insert(0x4851c470, "LLD_vTrace");
    method_entries.insert(0x4853833c, "OSALUTIL_s32RemoveDir");
    method_entries.insert(0x48548828, "field_element_modular_multiply");
    method_entries.insert(0x485703e0, "gpu8AsipRxBuf");
    method_entries.insert(0x48510650, "OSAL_s32MessageQueuePost");
    method_entries.insert(0x48548934, "field_element_modular_square");
    method_entries.insert(0x484f46dc, "s32EnterErrMem");
    method_entries.insert(0x48572cc8, "pu8buffer");
    method_entries.insert(0x48535cac, "libminxml_get_rset");
    method_entries.insert(0x48516334, "vAddProcessEntry");
    method_entries.insert(0x4850ca5c, "vInitOsalCoreIOSC");
    method_entries.insert(0x48506b38, "OSAL_s32EventClose");
    method_entries.insert(0x48522c5c, "u32AcousticOutIOCtrl_GetTime");
    method_entries.insert(0x4850c838, "vGenerateTermMqHandle");
    method_entries.insert(0x484e8e38, "_init");
    method_entries.insert(0x48545094, "ecdsa_signature_verification");
    method_entries.insert(0x48571a0a, "_bDunSkipRequest");
    method_entries.insert(0x4856fcc0, "u64SetTimeUTCElapsed");
    method_entries.insert(0x48546318, "field_element_shift_n_left");
    method_entries.insert(0x48535c0c, "libminxml_reset_attr_list");
    method_entries.insert(0x48546914, "field_element_subtract");
    method_entries.insert(0x48514548, "vTraceTCB");
    method_entries.insert(0x4851b414, "SetupSecTimer");
    method_entries.insert(0x48518fac, "OSAL_s32SemaphoreClose");
    method_entries.insert(0x484ed76c, "vOsalSystemLoadTask");
    method_entries.insert(0x4850cfc0, "u32FindPoolIdx");
    method_entries.insert(0x484ffcf0, "vExecuteSystemInfo");
    method_entries.insert(0x48535b04, "libminxml_get_attr_name");
    method_entries.insert(0x48571a09, "cCurRcvByte");
    method_entries.insert(0x48535eb8, "libminxml_plat_fclose");
}
