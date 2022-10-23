use crate::emulator::context::Context;
use crate::emulator::elf_loader::load_elf;
use crate::emulator::file_system::FileSystem;
use crate::emulator::memory_map::GET_TLS_ADDR;
use crate::emulator::mmu::{Mmu, MmuExtension};
use capstone::arch::arm::ArchMode;
use capstone::prelude::*;
use capstone::Endian;
use unicorn_engine::unicorn_const::{uc_error, Arch, HookType, MemType, Mode, Permission};
use unicorn_engine::{RegisterARM, Unicorn};

pub struct Emulator<'a> {
    unicorn: Unicorn<'a, Context>,
}

impl<'a> Emulator<'a> {
    pub fn new(file_system: FileSystem) -> Result<Emulator<'a>, uc_error> {
        Ok(Self {
            unicorn: Unicorn::new_with_data(
                Arch::ARM,
                Mode::LITTLE_ENDIAN,
                Context {
                    mmu: Mmu::new(),
                    file_system,
                },
            )?,
        })
    }

    pub fn run_elf(
        &mut self,
        elf_filepath: &str,
        buf: &[u8],
        program_args: &Vec<String>,
        program_envs: &Vec<(String, String)>,
    ) -> Result<(), &'static str> {
        let (interp_entry_point, elf_entry, stack_ptr) = load_elf(
            &mut self.unicorn,
            elf_filepath,
            buf,
            program_args,
            program_envs,
        )?;
        //self.mmu.display_mapped();

        self.set_kernel_traps();
        self.enable_vfp();

        self.unicorn.add_intr_hook(crate::os::hook_syscall).unwrap();
        self.unicorn
            .add_mem_hook(HookType::MEM_FETCH_UNMAPPED, 1, 0, Self::callback_mem_error)
            .unwrap();
        self.unicorn
            .add_mem_hook(HookType::MEM_READ_UNMAPPED, 1, 0, Self::callback_mem_rw)
            .unwrap();
        self.unicorn
            .add_mem_hook(HookType::MEM_WRITE_UNMAPPED, 1, 0, Self::callback_mem_rw)
            .unwrap();
        self.unicorn
            .add_mem_hook(HookType::MEM_WRITE_PROT, 1, 0, Self::callback_mem_rw)
            .unwrap();

        self.unicorn
            .reg_write(RegisterARM::SP as i32, stack_ptr as u64)
            .unwrap();

        self.run_linker(interp_entry_point, elf_entry);
        self.run_program(elf_entry);

        Ok(())
    }

    fn set_kernel_traps(&mut self) {
        // If the compiler for the target does not provides some primitives for some
        // reasons (e.g. target limitations), the kernel is responsible to assist
        // with these operations.
        //
        // The following is some `kuser` helpers, which can be found here:
        // https://elixir.bootlin.com/linux/latest/source/arch/arm/kernel/entry-armv.S#L899
        self.unicorn.mmu_map(
            0xFFFF0000,
            0x1000,
            Permission::READ | Permission::EXEC,
            "[arm_traps]",
        );

        // memory_barrier
        log::debug!("Set kernel trap: memory_barrier at 0xFFFF0FA0");
        self.unicorn
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
        self.unicorn
            .mem_write(
                0xFFFF0FC0,
                // ldr   r3, [r2]
                // subs  r3, r3, r0
                // streq r1, [r2]
                // rsbs  r0, r3, #0
                // mov   pc, lr
                &[
                    0x00, 0x30, 0x92, 0xE5, 0x00, 0x30, 0x53, 0xE0, 0x00, 0x10, 0x82, 0x05, 0x00,
                    0x00, 0x73, 0xE2, 0x0E, 0xF0, 0xA0, 0xE1,
                ],
            )
            .unwrap();

        // get_tls
        log::debug!("Set kernel trap: get_tls at {:#X}", GET_TLS_ADDR);
        self.unicorn
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
                    0x08, 0x00, 0x9F, 0xE5, 0x0E, 0xF0, 0xA0, 0xE1, 0x70, 0x0F, 0x1D, 0xEE, 0xE7,
                    0xFD, 0xDE, 0xF1, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00,
                ],
            )
            .unwrap();
    }

    fn enable_vfp(&mut self) {
        // other version? https://github.com/AeonLucid/AndroidNativeEmu/blob/40b89c8095b2aeb4a9f18ba9a853832afdb3d1b1/src/androidemu/emulator.py

        // https://github.com/qilingframework/qiling/blob/master/qiling/arch/arm.py
        let c1_c0_2 = self.unicorn.reg_read(RegisterARM::C1_C0_2).unwrap();
        self.unicorn
            .reg_write(RegisterARM::C1_C0_2, c1_c0_2 | (0b11 << 20) | (0b11 << 22))
            .unwrap();
        self.unicorn.reg_write(RegisterARM::FPEXC, 1 << 30).unwrap();
    }

    fn run_linker(&mut self, interp_entry_point: u32, elf_entry: u32) {
        log::info!("========== Start linker ==========");
        self.disasm(interp_entry_point, 100);
        let result = self
            .unicorn
            .emu_start(interp_entry_point as u64, elf_entry as u64, 0, 0);

        log::debug!("PC: {:#x}", self.unicorn.reg_read(RegisterARM::PC).unwrap());

        if let Err(error) = result {
            log::error!("Execution error: {:?}", error);
        }

        log::info!("========== Linker done ==========");
    }

    fn run_program(&mut self, elf_entry: u32) {
        log::info!("========== Start program ==========");
        let result = self.unicorn.emu_start(elf_entry as u64, 0, 0, 0);

        log::debug!("PC: {:#x}", self.unicorn.reg_read(RegisterARM::PC).unwrap());

        if let Err(error) = result {
            log::error!("Execution error: {:?}", error);
        }

        log::info!("========== Program end ==========");
    }

    fn disasm(&mut self, address: u32, len: u32) {
        let cs = Capstone::new()
            .arm()
            .mode(ArchMode::Arm)
            .endian(Endian::Little)
            .detail(true)
            .build()
            .unwrap();

        let mut vec = vec![0u8; len as usize];
        self.unicorn.mem_read(address as u64, &mut vec).unwrap();
        let disasm = cs.disasm_all(&vec, address as u64).unwrap();
        println!("{}", disasm);
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
}
