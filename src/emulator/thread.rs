use crate::emulator::context::Context;
use crate::emulator::elf_loader::load_elf;
use crate::emulator::memory_map::GET_TLS_ADDR;
use crate::emulator::mmu::MmuExtension;
use crate::emulator::utils::load_binary;
use std::error::Error;
use std::thread;
use std::thread::JoinHandle;
use unicorn_engine::unicorn_const::{Arch, HookType, MemType, Mode, Permission};
use unicorn_engine::{RegisterARM, Unicorn};

pub struct Thread {
    pub handle: JoinHandle<Result<(), Box<dyn Error + Send + Sync + 'static>>>,
}

impl Thread {
    pub fn start_elf_file(
        context: Context,
        elf_filepath: String,
        program_args: Vec<String>,
        program_envs: Vec<(String, String)>,
    ) -> Self {
        Self {
            handle: thread::spawn(move || {
                emu_thread_func(context, elf_filepath.clone(), program_args, program_envs)
            }),
        }
    }
}

fn emu_thread_func(
    context: Context,
    elf_filepath: String,
    program_args: Vec<String>,
    program_envs: Vec<(String, String)>,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let mut unicorn = Unicorn::new_with_data(Arch::ARM, Mode::LITTLE_ENDIAN, context)
        .map_err(|err| format!("Unicorn error: {:?}", err))?;

    let buf = load_binary(&mut unicorn, &elf_filepath);

    let (interp_entry_point, elf_entry, stack_ptr) = load_elf(
        &mut unicorn,
        &elf_filepath,
        &buf,
        &program_args,
        &program_envs,
    )?;

    set_kernel_traps(&mut unicorn);
    enable_vfp(&mut unicorn);

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

    unicorn
        .reg_write(RegisterARM::SP as i32, stack_ptr as u64)
        .unwrap();

    run_linker(&mut unicorn, interp_entry_point, elf_entry);

    log::info!("{}", unicorn.display_mapped());

    run_program(&mut unicorn, elf_entry);

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

fn run_linker(unicorn: &mut Unicorn<Context>, interp_entry_point: u32, elf_entry: u32) {
    log::info!("========== Start linker ==========");
    //self.disasm(interp_entry_point, 100);
    let result = unicorn.emu_start(interp_entry_point as u64, elf_entry as u64, 0, 0);

    log::debug!("PC: {:#x}", unicorn.reg_read(RegisterARM::PC).unwrap());

    if let Err(error) = result {
        log::error!("Execution error: {:?}", error);
    }

    log::info!("========== Linker done ==========");
}

fn run_program(unicorn: &mut Unicorn<Context>, elf_entry: u32) {
    log::info!("========== Start program ==========");
    let result = unicorn.emu_start(elf_entry as u64, 0, 0, 0);

    log::debug!("PC: {:#x}", unicorn.reg_read(RegisterARM::PC).unwrap());

    if let Err(error) = result {
        log::error!("Execution error: {:?}", error);
    }

    log::info!("========== Program end ==========");
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
