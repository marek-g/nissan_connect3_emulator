use crate::emulator::context::Context;
use crate::file_system::OpenFileFlags;
use byteorder::{ByteOrder, LittleEndian};
use unicorn_engine::unicorn_const::Permission;
use unicorn_engine::Unicorn;
use xmas_elf::program;

pub fn load_binary(unicorn: &mut Unicorn<Context>, filepath: &str) -> Vec<u8> {
    let data = unicorn.get_data().inner;
    let file_system = &mut data.file_system.lock().unwrap();
    if let Ok(fd) = file_system.open(filepath, OpenFileFlags::READ) {
        let size = file_system.get_length(fd);
        let mut content = vec![0u8; size as usize];
        file_system.read_all(fd, &mut content).unwrap();
        file_system.close(fd).unwrap();
        content
    } else {
        panic!("Cannot load file: {}", filepath);
    }
}

/// Converts NULL terminated string to rust string
/*pub fn null_str(input: &str) -> String {
    let res = input.trim_matches(char::from(0));
    String::from(res)
}*/

/// Align an `address` down to a specified alignment boundary.
/// If `alignment` is not specified the `address` will be aligned
/// to page size.
pub fn mem_align_down(address: u32, alignment: Option<u32>) -> u32 {
    let align = alignment.unwrap_or(0x1000);
    (address / align) * align
}

/// Align an `address` up to a specified alignment boundary.
/// If `alignment` is not specified the `address` will be aligned
/// to page size.
pub fn mem_align_up(address: u32, alignment: Option<u32>) -> u32 {
    let align = alignment.unwrap_or(0x1000);
    ((address + align - 1) / align) * align
}

pub fn to_unicorn_permissions(perms: program::Flags) -> Permission {
    let mut uc_perms: Permission = Permission::NONE;

    if perms.is_execute() {
        uc_perms = uc_perms | Permission::EXEC;
        // assumes read if execute
        uc_perms = uc_perms | Permission::READ;
    }

    if perms.is_write() {
        uc_perms = uc_perms | Permission::WRITE;
    }

    if perms.is_read() {
        uc_perms = uc_perms | Permission::READ;
    }

    uc_perms
}

/// Write a string to stack memory (aligned to pointer size).
/// Return new top of stack.
pub fn push_text_on_stack(unicorn: &mut Unicorn<Context>, address: u32, text: &str) -> u32 {
    let data = text.as_bytes();
    let address = mem_align_down(address - data.len() as u32 - 1, Some(4));
    unicorn.mem_write(address as u64, data).unwrap();
    unicorn
        .mem_write(address as u64 + data.len() as u64, &vec![0u8])
        .unwrap();
    address
}

pub fn read_string(unicorn: &Unicorn<Context>, mut addr: u32) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        unicorn.mem_read(addr as u64, &mut byte).unwrap();
        if byte[0] == 0 {
            break;
        }
        buf.push(byte[0]);
        addr += 1;
    }
    String::from_utf8(buf).unwrap()
}

pub fn pack_u16(value: u16) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn pack_i32(value: i32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn pack_u32(value: u32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn unpack_u32(value: &[u8]) -> u32 {
    LittleEndian::read_u32(value)
}

pub fn pack_i64(value: i64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn pack_u64(value: u64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}
