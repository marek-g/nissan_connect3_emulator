use crate::emulator::context::Context;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use unicorn_engine::Unicorn;

const SVG_IOCTL_ALLOC_STATUS: u32 = 0;
const SVG_IOCTL_GET_STATUS: u32 = 2;

const STATUS_SIZE: usize = 0x7330;
const MAX_VRAM_ENTRIES: u32 = 64;

const VRAM_COUNT_INDEX: u32 = 0;
const VRAM_ENTRY_INDEX: u32 = 1;
const VRAM_ENTRY_BASE: u32 = VRAM_ENTRY_INDEX * 4;
const ENTRY_SIZE_OFFSET: u32 = 0;
const ENTRY_OFFSET_FIELD_OFFSET: u32 = 8;
const ENTRY_TYPE_OFFSET: u32 = 16;
const PCS_COUNT_INDEX: u32 = 0x1b81;
const PCS_ENTRY_INDEX: u32 = 0x1b82;
const PCS_ENTRY_BASE: u32 = PCS_ENTRY_INDEX * 4;
const ENTRY_STRIDE: u32 = 0x6e * 4;

const VRAM_SIZE: u32 = 0x0100_0000;
const PCS_SIZE: u32 = 0x0010_0000;
const MMAP_OFFSET: u32 = 0x1000;
const ERR_NOT_ALLOCATED: u32 = 0xffffffa3;

static KERNEL_STORAGE_ALLOCATED: AtomicBool = AtomicBool::new(false);
static STATUS: Mutex<Option<Vec<u8>>> = Mutex::new(None);

pub fn ioctl(unicorn: &mut Unicorn<'_, Context>, request: u32, addr: u32) -> i32 {
    if request != 0 {
        return 0;
    }

    let cmd = match read_u32(unicorn, addr) {
        Some(cmd) => cmd,
        None => return -22i32,
    };

    match cmd {
        SVG_IOCTL_ALLOC_STATUS => alloc_status(unicorn, addr),
        SVG_IOCTL_GET_STATUS => get_status(unicorn, addr),
        _ => {
            log::debug!(
                "[{}] [DEV SVG_RESOURCE] ioctl cmd={} not emulated",
                unicorn.get_data().inner.thread_id(),
                cmd
            );
            write_result(unicorn, addr, 0);
            0i32
        }
    }
}

fn alloc_status(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> i32 {
    let status_ptr = match read_u32(unicorn, addr.wrapping_sub(8)) {
        Some(status_ptr) if status_ptr != 0 => status_ptr,
        _ => return -22i32,
    };

    let mut status = vec![0u8; STATUS_SIZE];
    if unicorn.mem_read(status_ptr as u64, &mut status).is_err() {
        return -14i32;
    }

    prepare_status(&mut status);
    let vram_count = read_u32_le(&status, VRAM_COUNT_INDEX * 4);
    let pcs_count = read_u32_le(&status, PCS_COUNT_INDEX * 4);
    log_status_entries(unicorn, "alloc", vram_count, pcs_count, &status);
    KERNEL_STORAGE_ALLOCATED.store(true, Ordering::Relaxed);
    *STATUS.lock().unwrap() = Some(status);
    write_result(unicorn, addr, 0);

    log::debug!(
        "[{}] [DEV SVG_RESOURCE] ioctl ALLOC_STATUS ptr=0x{:x} vram_count={} pcs_count={}",
        unicorn.get_data().inner.thread_id(),
        status_ptr,
        vram_count,
        pcs_count
    );

    0i32
}

fn get_status(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> i32 {
    let status_ptr = match read_u32(unicorn, addr.wrapping_sub(8)) {
        Some(status_ptr) if status_ptr != 0 => status_ptr,
        _ => return -22i32,
    };

    if !KERNEL_STORAGE_ALLOCATED.load(Ordering::Relaxed) {
        write_result(unicorn, addr, ERR_NOT_ALLOCATED);
        log::debug!(
            "[{}] [DEV SVG_RESOURCE] ioctl GET_STATUS ptr=0x{:x} not allocated yet",
            unicorn.get_data().inner.thread_id(),
            status_ptr
        );
        return 0i32;
    }

    let status = {
        let locked = STATUS.lock().unwrap();
        match locked.as_ref() {
            Some(status) => status.clone(),
            None => fallback_status(),
        }
    };

    write_result(unicorn, addr, 0);
    if unicorn.mem_write(status_ptr as u64, &status).is_err() {
        return -14i32;
    }

    log::debug!(
        "[{}] [DEV SVG_RESOURCE] ioctl GET_STATUS ptr=0x{:x} vram_count={} pcs_count={}",
        unicorn.get_data().inner.thread_id(),
        status_ptr,
        read_u32_le(&status, VRAM_COUNT_INDEX * 4),
        read_u32_le(&status, PCS_COUNT_INDEX * 4)
    );

    0i32
}

fn fallback_status() -> Vec<u8> {
    let mut status = vec![0u8; STATUS_SIZE];
    prepare_status(&mut status);
    status
}

fn log_status_entries(
    unicorn: &mut Unicorn<'_, Context>,
    stage: &str,
    vram_count: u32,
    pcs_count: u32,
    status: &[u8],
) {
    for i in 0..vram_count.min(8) {
        let entry = VRAM_ENTRY_BASE + i * ENTRY_STRIDE;
        log::debug!(
            "[{}] [DEV SVG_RESOURCE] {} vram[{}] size=0x{:x} offset=0x{:x} type={} f1=0x{:x} f2=0x{:x} f3=0x{:x} f4=0x{:x}",
            unicorn.get_data().inner.thread_id(),
            stage,
            i,
            read_u32_le(status, entry + ENTRY_SIZE_OFFSET),
            read_u32_le(status, entry + ENTRY_OFFSET_FIELD_OFFSET),
            read_u32_le(status, entry + ENTRY_TYPE_OFFSET),
            read_u32_le(status, entry + 4),
            read_u32_le(status, entry + 12),
            read_u32_le(status, entry + 20),
            read_u32_le(status, entry + 24)
        );
    }

    for i in 0..pcs_count.min(8) {
        let entry = PCS_ENTRY_BASE + i * ENTRY_STRIDE;
        log::debug!(
            "[{}] [DEV SVG_RESOURCE] {} pcs[{}] size=0x{:x} offset=0x{:x} type={} f1=0x{:x} f2=0x{:x}",
            unicorn.get_data().inner.thread_id(),
            stage,
            i,
            read_u32_le(status, entry + ENTRY_SIZE_OFFSET),
            read_u32_le(status, entry + ENTRY_OFFSET_FIELD_OFFSET),
            read_u32_le(status, entry + ENTRY_TYPE_OFFSET),
            read_u32_le(status, entry + 4),
            read_u32_le(status, entry + 12)
        );
    }
}

fn prepare_status(status: &mut Vec<u8>) {
    let mut vram_count = read_u32_le(status, VRAM_COUNT_INDEX * 4);
    if vram_count == 0 {
        vram_count = 1;
        write_u32_in_slice(status, VRAM_COUNT_INDEX * 4, vram_count);
    }
    vram_count = vram_count.min(MAX_VRAM_ENTRIES);

    for i in 0..vram_count {
        let entry = VRAM_ENTRY_BASE + i * ENTRY_STRIDE;
        prepare_entry(status, entry, VRAM_SIZE, MMAP_OFFSET + i * 0x1000);
    }

    let mut pcs_count = read_u32_le(status, PCS_COUNT_INDEX * 4);
    if pcs_count == 0 {
        pcs_count = 1;
        write_u32_in_slice(status, PCS_COUNT_INDEX * 4, pcs_count);
    }

    for i in 0..pcs_count {
        let entry = PCS_ENTRY_BASE + i * ENTRY_STRIDE;
        prepare_entry(status, entry, PCS_SIZE, MMAP_OFFSET + i * 0x1000);
    }
}

fn prepare_entry(status: &mut Vec<u8>, entry: u32, default_size: u32, default_offset: u32) {
    if entry + ENTRY_STRIDE > STATUS_SIZE as u32 {
        return;
    }

    if read_u32_le(status, entry + ENTRY_SIZE_OFFSET) == 0 {
        write_u32_in_slice(status, entry + ENTRY_SIZE_OFFSET, default_size);
    }
    if read_u32_le(status, entry + ENTRY_OFFSET_FIELD_OFFSET) == 0 {
        write_u32_in_slice(status, entry + ENTRY_OFFSET_FIELD_OFFSET, default_offset);
    }
}

fn write_result(unicorn: &mut Unicorn<'_, Context>, addr: u32, result: u32) {
    write_u32(unicorn, addr.wrapping_sub(4), result);
}

fn read_u32(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> Option<u32> {
    let mut raw = [0u8; 4];
    unicorn
        .mem_read(addr as u64, &mut raw)
        .ok()
        .map(|()| u32::from_le_bytes(raw))
}

fn write_u32(unicorn: &mut Unicorn<'_, Context>, addr: u32, value: u32) {
    if unicorn
        .mem_write(addr as u64, &value.to_le_bytes())
        .is_err()
    {
        log::debug!(
            "[{}] [DEV SVG_RESOURCE] failed to write u32 0x{:x}=0x{:x}",
            unicorn.get_data().inner.thread_id(),
            addr,
            value
        );
    }
}

fn read_u32_le(slice: &[u8], offset: u32) -> u32 {
    let offset = offset as usize;
    if offset + 4 <= slice.len() {
        u32::from_le_bytes(slice[offset..offset + 4].try_into().unwrap())
    } else {
        0
    }
}

fn write_u32_in_slice(slice: &mut [u8], offset: u32, value: u32) {
    let offset = offset as usize;
    if offset + 4 <= slice.len() {
        slice[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}

