use crate::emulator::context::Context;
use crate::emulator::memory_map::*;
use crate::emulator::mmu::MmuExtension;
use crate::emulator::users::{EGID, EUID, GID, UID};
use crate::emulator::utils::{
    load_binary, mem_align_down, mem_align_up, pack_u32, push_text_on_stack, to_unicorn_permissions,
};
use elfloader::*;
use unicorn_engine::unicorn_const::Permission;
use unicorn_engine::Unicorn;
use xmas_elf::header;
use xmas_elf::header::{Data, Machine};
use xmas_elf::program::Flags;

/// Auxiliary vector placed on the stack by ELF loader.
/// See: https://man7.org/linux/man-pages/man3/getauxval.3.html
#[repr(u32)]
enum AUX {
    AtNull = 0,
    //AtIgnore = 1,
    //AtExecFd = 2,
    AtPhdr = 3,
    AtPhent = 4,
    AtPhnum = 5,
    AtPageSz = 6,
    AtBase = 7,
    AtFlags = 8,
    AtEntry = 9,
    //AtNotElf = 10,
    AtUid = 11,
    AtEuid = 12,
    AtGid = 13,
    AtEgid = 14,
    AtPlatform = 15,
    AtHwcap = 16,
    AtClkTck = 17,
    AtSecure = 23,
    //AtBasePlatform = 24,
    AtRandom = 25,
    AtHwcap2 = 26,
    AtExecFn = 31,
}

struct ArmElfLoader<'a, 'b> {
    // input
    unicorn: &'a mut Unicorn<'b, Context>,
    filepath: &'a str,
    load_address: u32,

    // output
    mem_start: u32,
    mem_end: u32,
}

impl<'a, 'b> ElfLoader for ArmElfLoader<'a, 'b> {
    fn allocate(&mut self, load_headers: LoadableHeaders) -> Result<(), ElfLoaderErr> {
        for header in load_headers {
            let mem_start = mem_align_down(self.load_address + header.virtual_addr() as u32, None);
            let mem_end = mem_align_up(
                self.load_address + header.virtual_addr() as u32 + header.mem_size() as u32,
                None,
            );
            let perms = to_unicorn_permissions(header.flags());

            self.mem_start = self.mem_start.min(mem_start);
            self.mem_end = self.mem_end.max(mem_end);

            self.unicorn
                .mmu_map(mem_start, mem_end - mem_start, perms, "", self.filepath);
        }
        Ok(())
    }

    fn load(&mut self, _flags: Flags, base: VAddr, region: &[u8]) -> Result<(), ElfLoaderErr> {
        let start = self.load_address + base as u32;
        let end = self.load_address + base as u32 + region.len() as u32;

        log::debug!(
            "load region: {:#x} - {:#x} (size: {:#x})",
            start,
            end,
            end - start
        );

        self.unicorn.mem_write(start as u64, region).unwrap();

        Ok(())
    }

    fn relocate(&mut self, _entry: RelocationEntry) -> Result<(), ElfLoaderErr> {
        //use elfloader::arch::arm::RelocationTypes::*;
        //use RelocationType::Arm;

        //let addr: *mut u32 = (self.load_address + entry.offset as u32) as *mut u32;

        //println!("relocation: {:?}", entry.rtype);

        // TODO:

        /*match entry.rtype {
            x86_64(R_AMD64_RELATIVE) => {
                // This type requires addend to be present
                let addend = entry
                    .addend
                    .ok_or(ElfLoaderErr::UnsupportedRelocationEntry)?;

                // This is a relative relocation, add the offset (where we put our
                // binary in the vspace) to the addend and we're done.
                //info!("R_RELATIVE *{:p} = {:#x}", addr, self.vbase + addend);
                Ok(())
            }
            _ => Ok(()), // not implemented
        }*/

        Ok(())
    }

    fn tls(
        &mut self,
        tdata_start: VAddr,
        _tdata_length: u64,
        total_size: u64,
        _align: u64,
    ) -> Result<(), ElfLoaderErr> {
        let tls_end = tdata_start + total_size;
        println!(
            "Initial TLS region is at = {:#x} -- {:#x}",
            tdata_start, tls_end
        );
        Ok(())
    }
}

pub fn load_elf(
    unicorn: &mut Unicorn<Context>,
    elf_filepath: &str,
    buf: &[u8],
    program_args: &Vec<String>,
    program_envs: &Vec<(String, String)>,
) -> Result<(u32, u32, u32), &'static str> {
    // parse elf file
    let binary = ElfBinary::new(buf).expect("Got proper ELF file");

    // verify architecture
    if binary.get_arch() != Machine::Arm {
        return Err("Wrong architecture!");
    }

    let data = binary.file.header.pt1.data.as_data();
    if data != Data::LittleEndian {
        return Err("Wrong endianness!");
    }

    // choose load address
    let load_address = if binary.file.header.pt2.type_().as_type() == header::Type::Executable {
        EXE_LOAD_ADDRESS
    } else {
        SO_LOAD_ADDRESS
    };

    // load to memory
    let mut loader = ArmElfLoader {
        unicorn,
        filepath: elf_filepath,
        load_address,
        mem_start: 0xFFFFFFFFu32,
        mem_end: 0u32,
    };

    binary.load(&mut loader).expect("Can't load the binary?");

    let mem_start = loader.mem_start;
    let mem_end = loader.mem_end;
    log::debug!("mem_start: {:#x}", mem_start);
    log::debug!("mem_end: {:#x}", mem_end);

    unicorn.get_data_mut().mmu.brk_mem_end = mem_end;
    unicorn.get_data_mut().mmu.heap_mem_end = HEAP_START_ADDRESS;

    // load interpreter
    let interp_address = 0u32;
    let mut interp_entry_point = 0u32;
    if let Some(interp_path) = binary.interpreter() {
        log::debug!("Load interpreter: {:?}", &interp_path);

        let interp_bin = load_binary(unicorn, &interp_path);
        let binary = ElfBinary::new(&interp_bin).expect("Got proper ELF file");

        let mut interp_loader = ArmElfLoader {
            unicorn,
            filepath: interp_path,
            load_address: interp_address,
            mem_start: 0xFFFFFFFFu32,
            mem_end: 0u32,
        };
        binary
            .load(&mut interp_loader)
            .expect("Can't load the binary?");

        interp_entry_point = binary.file.header.pt2.entry_point() as u32;

        log::debug!("Interpreter entry point: {:#x}", interp_entry_point);
    }

    // setup stack
    let stack_ptr = setup_stack(
        unicorn,
        elf_filepath,
        program_args,
        program_envs,
        &binary,
        load_address,
        mem_start,
        interp_address,
    );

    let elf_entry = load_address + binary.file.header.pt2.entry_point() as u32;

    Ok((interp_entry_point, elf_entry, stack_ptr))
}

fn setup_stack(
    unicorn: &mut Unicorn<Context>,
    elf_filepath: &str,
    program_args: &Vec<String>,
    program_envs: &Vec<(String, String)>,
    binary: &ElfBinary,
    load_address: u32,
    mem_start: u32,
    interp_address: u32,
) -> u32 {
    unicorn.mmu_map(
        STACK_BASE,
        STACK_SIZE,
        Permission::READ | Permission::WRITE,
        "[stack]",
        "",
    );

    // About ELF Auxiliary Vectors: http://articles.manugarg.com/aboutelfauxiliaryvectors.html

    let mut stack_ptr = STACK_BASE + STACK_SIZE;

    // data to be placed on stack
    // (strings are placed at the end of the stack, after elf_table)
    let mut elf_table = Vec::new();

    // argc
    elf_table.extend_from_slice(&pack_u32(program_args.len() as u32 + 1));

    // argv[0]
    stack_ptr = push_text_on_stack(unicorn, stack_ptr, elf_filepath);
    elf_table.extend_from_slice(&pack_u32(stack_ptr));

    // argv[1..n]
    for argv in program_args {
        stack_ptr = push_text_on_stack(unicorn, stack_ptr, argv);
        elf_table.extend_from_slice(&pack_u32(stack_ptr));
    }

    // null sentinel
    elf_table.extend_from_slice(&pack_u32(0));

    // env[0..n - 1]
    for env in program_envs {
        stack_ptr = push_text_on_stack(unicorn, stack_ptr, &format!("{}={}", env.0, env.1));
        elf_table.extend_from_slice(&pack_u32(stack_ptr));
    }

    // null sentinel
    elf_table.extend_from_slice(&pack_u32(0));

    // auxv strings
    stack_ptr = push_text_on_stack(unicorn, stack_ptr, &"a".repeat(16));
    let randaddr = stack_ptr;
    stack_ptr = push_text_on_stack(unicorn, stack_ptr, elf_filepath);
    let execfnaddr = stack_ptr;
    stack_ptr = push_text_on_stack(unicorn, stack_ptr, "armv6l");
    let platformaddr = stack_ptr;

    // auxv data
    let auxvs = get_auxv_data(
        binary,
        load_address,
        mem_start,
        interp_address,
        randaddr,
        execfnaddr,
        platformaddr,
    );
    for auxv in auxvs {
        elf_table.extend_from_slice(&pack_u32(auxv.0));
        elf_table.extend_from_slice(&pack_u32(auxv.1));
    }

    // place elf_table on the stack aligned to 16 bytes
    stack_ptr = mem_align_down(stack_ptr - elf_table.len() as u32, Some(16));
    unicorn.mem_write(stack_ptr as u64, &elf_table).unwrap();

    stack_ptr
}

fn get_auxv_data(
    binary: &ElfBinary,
    load_address: u32,
    mem_start: u32,
    interp_address: u32,
    randstraddr: u32,
    execfnaddr: u32,
    platformaddr: u32,
) -> Vec<(u32, u32)> {
    vec![
        (AUX::AtHwcap as u32, 0x1FB8D7), // for 32-bit
        (AUX::AtPageSz as u32, 0x1000),
        (AUX::AtClkTck as u32, 100),
        (
            AUX::AtPhdr as u32,
            load_address + binary.file.header.pt2.ph_offset() as u32 + mem_start,
        ),
        (
            AUX::AtPhent as u32,
            binary.file.header.pt2.ph_entry_size() as u32,
        ),
        (
            AUX::AtPhnum as u32,
            binary.file.header.pt2.ph_count() as u32,
        ),
        (AUX::AtBase as u32, interp_address),
        (AUX::AtFlags as u32, 0),
        (
            AUX::AtEntry as u32,
            load_address + binary.file.header.pt2.entry_point() as u32,
        ),
        (AUX::AtUid as u32, UID),
        (AUX::AtEuid as u32, EUID),
        (AUX::AtGid as u32, GID),
        (AUX::AtEgid as u32, EGID),
        (AUX::AtSecure as u32, 0),
        (AUX::AtRandom as u32, randstraddr),
        (AUX::AtHwcap2 as u32, 0),
        (AUX::AtExecFn as u32, execfnaddr),
        (AUX::AtPlatform as u32, platformaddr),
        (AUX::AtNull as u32, 0),
    ]
}
