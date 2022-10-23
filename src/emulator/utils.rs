use crate::emulator::context::Context;
use unicorn_engine::unicorn_const::Permission;
use unicorn_engine::Unicorn;
use xmas_elf::program;

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
    if address % align != 0 {
        (address / align + 1) * align
    } else {
        address
    }
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

pub fn pack_i32(value: i32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn pack_u32(value: u32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn pack_i64(value: i64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn pack_u64(value: u64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}
