use crate::common::registry::RegistryValue;
use crate::emulator::context::Context;
use crate::os::code_stub::add_code_stub;
use unicorn_engine::{RegisterARM, Unicorn};

const ORIGINAL_BASE: u32 = 0x484d_8000;
const REGISTRY_OPEN: u32 = 0x484f_34f0 - ORIGINAL_BASE;
const REGISTRY_CREATE: u32 = 0x484f_3624 - ORIGINAL_BASE;
const REGISTRY_CONTROL: u32 = 0x484e_aca0 - ORIGINAL_BASE;
const REGISTRY_CLOSE: u32 = 0x484e_9860 - ORIGINAL_BASE;

const SUCCESS: u32 = 0x72000;
const ERROR_PARAM: u32 = 0x72002;
const ERROR_LENGTH: u32 = 0x72003;
const ERROR_FLAGS: u32 = 0x72006;
const ERROR_EXISTS: u32 = 0x72007;
const ERROR_NOT_FOUND: u32 = 0x72008;
const ERROR_HANDLE: u32 = 0x7200c;
const ERROR_COMMAND: u32 = 0x72011;

pub fn hook_registry_code(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    add_code_stub(
        unicorn,
        "LIBOSAL",
        base_address + REGISTRY_OPEN,
        "REGISTRY_u32IOOpen",
        open,
    );
    add_code_stub(
        unicorn,
        "LIBOSAL",
        base_address + REGISTRY_CREATE,
        "REGISTRY_u32IOCreate",
        create,
    );
    add_code_stub(
        unicorn,
        "LIBOSAL",
        base_address + REGISTRY_CONTROL,
        "REGISTRY_u32IOControl",
        io_control,
    );
    add_code_stub(
        unicorn,
        "LIBOSAL",
        base_address + REGISTRY_CLOSE,
        "REGISTRY_u32IOClose",
        close,
    );
}

fn open(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let path_addr = reg(unicorn, RegisterARM::R0);
    let flags = reg(unicorn, RegisterARM::R1);
    let out_addr = reg(unicorn, RegisterARM::R3);
    let caller = reg(unicorn, RegisterARM::R14);

    if path_addr == 0 || out_addr == 0 {
        return ERROR_PARAM;
    }

    let path = match read_cstr(unicorn, path_addr, 0x100) {
        Some(path) => path,
        None => return ERROR_PARAM,
    };

    let handle = {
        let mut namespace = unicorn.get_data().namespace.lock().unwrap();
        namespace.registry.open_key(&path, flags)
    };

    match handle {
        Some(handle) => {
            if !write_u32(unicorn, out_addr, handle) {
                return ERROR_PARAM;
            }
            log::trace!(
                "REGISTRY_u32IOOpen(caller={:#x}, {}) flags={:#x} -> handle={:#x}",
                caller,
                path,
                flags,
                handle,
            );
            SUCCESS
        }
        None if !matches!(flags, 1 | 2 | 4) => {
            log::trace!(
                "REGISTRY_u32IOOpen(caller={:#x}, {}) flags={:#x} -> bad flags",
                caller,
                path,
                flags,
            );
            ERROR_FLAGS
        }
        None => {
            log::trace!(
                "REGISTRY_u32IOOpen(caller={:#x}, {}) flags={:#x} -> not found",
                caller,
                path,
                flags,
            );
            ERROR_NOT_FOUND
        }
    }
}

fn create(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let path_addr = reg(unicorn, RegisterARM::R0);
    let flags = reg(unicorn, RegisterARM::R1);
    let out_addr = reg(unicorn, RegisterARM::R3);

    if path_addr == 0 || out_addr == 0 {
        return ERROR_EXISTS;
    }

    let path = match read_cstr(unicorn, path_addr, 0x100) {
        Some(path) => path,
        None => return ERROR_PARAM,
    };

    let result = {
        let mut namespace = unicorn.get_data().namespace.lock().unwrap();
        namespace.registry.create_key_and_open(&path, flags)
    };

    match result {
        crate::common::registry::OpenKeyResult::Handle(handle) => {
            if !write_u32(unicorn, out_addr, handle) {
                return ERROR_PARAM;
            }
            log::trace!(
                "REGISTRY_u32IOCreate({}) flags={:#x} -> handle={:#x}",
                path,
                flags,
                handle
            );
            SUCCESS
        }
        crate::common::registry::OpenKeyResult::Exists => ERROR_EXISTS,
        crate::common::registry::OpenKeyResult::NoParent => ERROR_NOT_FOUND,
        crate::common::registry::OpenKeyResult::BadFlags => ERROR_FLAGS,
        crate::common::registry::OpenKeyResult::Error => ERROR_PARAM,
    }
}

fn io_control(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let handle = reg(unicorn, RegisterARM::R0);
    let command = reg(unicorn, RegisterARM::R1);
    let buffer = reg(unicorn, RegisterARM::R2);

    let info = {
        let namespace = unicorn.get_data().namespace.lock().unwrap();
        match namespace.registry.handle_info(handle) {
            Some(info) => info.clone(),
            None => return ERROR_HANDLE,
        }
    };

    log::trace!(
        "REGISTRY_u32IOControl(handle={:#x}, command={:#x}, buffer={:#x}) path={}",
        handle,
        command,
        buffer,
        info.path()
    );

    match command {
        1 | 4 => query_value(unicorn, info.path(), info.flags(), buffer),
        10 => lookup_app_path(unicorn, info.path(), info.flags(), buffer),
        0xb => lookup_service_path(unicorn, info.path(), info.flags(), buffer),
        _ => {
            log::debug!(
                "REGISTRY_u32IOControl unhandled command={:#x} handle={:#x} buffer={:#x}",
                command,
                handle,
                buffer
            );
            ERROR_COMMAND
        }
    }
}

fn query_value(unicorn: &mut Unicorn<'_, Context>, path: &str, flags: u32, buffer: u32) -> u32 {
    if !readable(flags) || buffer == 0 {
        return if !readable(flags) {
            ERROR_FLAGS
        } else {
            ERROR_PARAM
        };
    }

    let name_addr = match read_u32(unicorn, buffer) {
        Some(addr) => addr,
        None => return ERROR_PARAM,
    };
    let length = match read_u32(unicorn, buffer + 4) {
        Some(length) => length,
        None => return ERROR_PARAM,
    };
    let out_type_addr = buffer + 8;
    let out_buffer_addr = match read_u32(unicorn, buffer + 0xc) {
        Some(addr) => addr,
        None => return ERROR_PARAM,
    };

    if name_addr == 0 || out_buffer_addr == 0 {
        return ERROR_PARAM;
    }

    let name = match read_cstr(unicorn, name_addr, 0x100) {
        Some(name) => name,
        None => return ERROR_PARAM,
    };

    let value = {
        let namespace = unicorn.get_data().namespace.lock().unwrap();
        namespace.registry.query_value(path, &name)
    };

    log::trace!(
        "REGISTRY query path={} value={} -> {}",
        path,
        name,
        value.is_some()
    );

    match value {
        Some(RegistryValue::U32(value)) => {
            if length <= 3 {
                return ERROR_LENGTH;
            }
            if !write_u32(unicorn, out_type_addr, 1) || !write_u32(unicorn, out_buffer_addr, value)
            {
                return ERROR_PARAM;
            }
            SUCCESS
        }
        Some(RegistryValue::String(value)) => {
            if length == 0 {
                return ERROR_LENGTH;
            }
            if !write_u32(unicorn, out_type_addr, 0) {
                return ERROR_PARAM;
            }
            if !write_cstr(unicorn, out_buffer_addr, length, &value) {
                return ERROR_PARAM;
            }
            SUCCESS
        }
        None => ERROR_NOT_FOUND,
    }
}

fn lookup_app_path(unicorn: &mut Unicorn<'_, Context>, path: &str, flags: u32, buffer: u32) -> u32 {
    if !readable(flags) || buffer == 0 {
        return if !readable(flags) {
            ERROR_FLAGS
        } else {
            ERROR_PARAM
        };
    }

    let app_id_addr = buffer + 0x100;
    let app_id = match read_u32(unicorn, app_id_addr) {
        Some(app_id) => app_id,
        None => return ERROR_PARAM,
    };

    let app_path = {
        let namespace = unicorn.get_data().namespace.lock().unwrap();
        namespace.registry.find_key_by_u32_value("APPID", app_id)
    };

    let app_path = match app_path {
        Some(app_path) => relative_registry_path(&app_path),
        None => {
            if app_id == 7 {
                log::warn!("REGISTRY lookup AppID 7 -> not found opened={}", path);
            }
            return ERROR_NOT_FOUND;
        }
    };

    if !write_cstr(unicorn, buffer, 0x100, &app_path) {
        return ERROR_PARAM;
    }
    if app_id == 7 {
        log::warn!(
            "REGISTRY lookup AppID 7 -> path={} opened={}",
            app_path,
            path
        );
    }
    SUCCESS
}

fn lookup_service_path(
    unicorn: &mut Unicorn<'_, Context>,
    path: &str,
    flags: u32,
    buffer: u32,
) -> u32 {
    if !readable(flags) || buffer == 0 {
        return if !readable(flags) {
            ERROR_FLAGS
        } else {
            ERROR_PARAM
        };
    }

    let service_id_addr = buffer + 0x100;
    let service_id = match read_u32(unicorn, service_id_addr) {
        Some(service_id) => service_id,
        None => return ERROR_PARAM,
    };

    let service_path = {
        let namespace = unicorn.get_data().namespace.lock().unwrap();
        namespace
            .registry
            .find_key_by_u32_value("SERVICEID", service_id)
    };

    let service_path = match service_path {
        Some(service_path) => relative_registry_path(&service_path),
        None => return ERROR_NOT_FOUND,
    };

    let app_path = format!("/dev/registry/{}", service_path);
    let app_id = {
        let namespace = unicorn.get_data().namespace.lock().unwrap();
        namespace.registry.query_u32(&app_path, "APPID")
    };

    let app_id = match app_id {
        Some(app_id) => app_id,
        None => return ERROR_NOT_FOUND,
    };

    if !write_cstr(unicorn, buffer, 0x100, &service_path)
        || !write_u32(unicorn, service_id_addr, app_id)
    {
        return ERROR_PARAM;
    }
    log::debug!(
        "REGISTRY lookup ServiceID {:#x} AppID {:#x} path={} opened={}",
        service_id,
        app_id,
        service_path,
        path
    );
    SUCCESS
}

fn relative_registry_path(path: &str) -> String {
    path.strip_prefix("/dev/registry/")
        .or_else(|| path.strip_prefix("/dev/registry"))
        .unwrap_or(path)
        .to_string()
}

fn close(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let handle = reg(unicorn, RegisterARM::R0);
    if handle == 0 {
        return ERROR_HANDLE;
    }

    let closed = {
        let mut namespace = unicorn.get_data().namespace.lock().unwrap();
        namespace.registry.close_handle(handle)
    };

    if closed {
        SUCCESS
    } else {
        ERROR_HANDLE
    }
}

fn readable(flags: u32) -> bool {
    matches!(flags, 1 | 4)
}

fn reg(unicorn: &Unicorn<'_, Context>, register: RegisterARM) -> u32 {
    unicorn.reg_read(register).unwrap_or(0) as u32
}

fn read_u32(unicorn: &Unicorn<'_, Context>, addr: u32) -> Option<u32> {
    let mut bytes = [0u8; 4];
    unicorn.mem_read(addr as u64, &mut bytes).ok()?;
    Some(u32::from_le_bytes(bytes))
}

fn write_u32(unicorn: &mut Unicorn<'_, Context>, addr: u32, value: u32) -> bool {
    unicorn.mem_write(addr as u64, &value.to_le_bytes()).is_ok()
}

fn write_cstr(unicorn: &mut Unicorn<'_, Context>, addr: u32, length: u32, text: &str) -> bool {
    if length == 0 {
        return true;
    }

    let mut data = vec![0u8; length as usize];
    let max = (length as usize).saturating_sub(1);
    let len = text.len().min(max);
    data[..len].copy_from_slice(&text.as_bytes()[..len]);
    unicorn.mem_write(addr as u64, &data).is_ok()
}

fn read_cstr(unicorn: &Unicorn<'_, Context>, addr: u32, limit: usize) -> Option<String> {
    let mut bytes = Vec::new();
    for offset in 0..limit {
        let mut byte = [0u8; 1];
        unicorn
            .mem_read((addr + offset as u32) as u64, &mut byte)
            .ok()?;
        if byte[0] == 0 {
            break;
        }
        bytes.push(byte[0]);
    }
    String::from_utf8(bytes).ok()
}
