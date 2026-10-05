use crate::emulator::context::Context;
use unicorn_engine::Unicorn;

const SVG_LAYER_IOCTL_SWAP_BUFFERS: u32 = 0;
const SVG_LAYER_IOCTL_WAIT_VSYNC: u32 = 1;
const SVG_LAYER_IOCTL_GET_RESOURCE_INFO: u32 = 2;
const SVG_LAYER_IOCTL_GET_TIMING_INFO: u32 = 3;
const SVG_LAYER_IOCTL_SET_CAPTURE_SCALE: u32 = 4;
const SVG_LAYER_IOCTL_WAIT_VSYNC_FB: u32 = 5;
const SVG_LAYER_IOCTL_PRE_REG_ACCESS: u32 = 6;
const SVG_LAYER_IOCTL_POST_REG_ACCESS: u32 = 7;

pub fn ioctl(unicorn: &mut Unicorn<'_, Context>, request: u32, addr: u32) -> i32 {
    if request != 0 {
        return 0;
    }

    let cmd = match read_u32(unicorn, addr.wrapping_add(4)) {
        Some(cmd) => cmd,
        None => return -22i32,
    };

    match cmd {
        SVG_LAYER_IOCTL_GET_RESOURCE_INFO => get_resource_info(unicorn, addr),
        SVG_LAYER_IOCTL_SWAP_BUFFERS
        | SVG_LAYER_IOCTL_WAIT_VSYNC
        | SVG_LAYER_IOCTL_GET_TIMING_INFO
        | SVG_LAYER_IOCTL_SET_CAPTURE_SCALE
        | SVG_LAYER_IOCTL_WAIT_VSYNC_FB
        | SVG_LAYER_IOCTL_PRE_REG_ACCESS
        | SVG_LAYER_IOCTL_POST_REG_ACCESS => {
            log::debug!(
                "[{}] [DEV SVG_LAYER] ioctl cmd={} stubbed success words={:08x?}",
                unicorn.get_data().inner.thread_id(),
                cmd,
                read_words(unicorn, addr, 8)
            );
            write_u32(unicorn, addr, 0);
            0i32
        }
        _ => {
            log::debug!(
                "[{}] [DEV SVG_LAYER] ioctl cmd={} unknown words={:08x?}",
                unicorn.get_data().inner.thread_id(),
                cmd,
                read_words(unicorn, addr, 8)
            );
            write_u32(unicorn, addr, 0);
            0i32
        }
    }
}

fn get_resource_info(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> i32 {
    let words = read_words(unicorn, addr, 8);
    log::debug!(
        "[{}] [DEV SVG_LAYER] ioctl cmd=2 words={:08x?}",
        unicorn.get_data().inner.thread_id(),
        words
    );

    let out_pp = match read_u32(unicorn, addr.wrapping_add(8)) {
        Some(out_pp) if out_pp != 0 => out_pp,
        _ => {
            log::debug!(
                "[{}] [DEV SVG_LAYER] GET_RESOURCE_INFO bad out_pp=0x{:x}",
                unicorn.get_data().inner.thread_id(),
                words[2]
            );
            return -22i32;
        }
    };
    let out_ptr = match read_u32(unicorn, out_pp) {
        Some(out_ptr) if out_ptr != 0 => out_ptr,
        _ => {
            log::debug!(
                "[{}] [DEV SVG_LAYER] GET_RESOURCE_INFO bad out_ptr ptr=0x{:x} value=0x{:x}",
                unicorn.get_data().inner.thread_id(),
                out_pp,
                read_u32(unicorn, out_pp).unwrap_or(0)
            );
            return -22i32;
        }
    };

    const BASE_OFFSET: u32 = 0x0100_0000;
    const DISP_CTRL_LEN: u32 = 0x0004_1000;
    const LAYER_COUNT: u32 = 4;

    for (offset, value) in [
        (0x00u32, BASE_OFFSET),
        (0x08u32, DISP_CTRL_LEN),
        (0x18u32, BASE_OFFSET),
        (0x1cu32, 0x0004_0000),
        (0x24u32, LAYER_COUNT),
        (0x28u32, BASE_OFFSET),
        (0x30u32, DISP_CTRL_LEN),
    ] {
        write_u32(unicorn, out_ptr.wrapping_add(offset), value);
    }

    write_u32(unicorn, addr, 0);

    log::debug!(
        "[{}] [DEV SVG_LAYER] ioctl GET_RESOURCE_INFO out=0x{:x} base=0x{:x} disp_len=0x{:x} layers={}",
        unicorn.get_data().inner.thread_id(),
        out_ptr,
        BASE_OFFSET,
        DISP_CTRL_LEN,
        LAYER_COUNT
    );

    0i32
}

fn read_words(unicorn: &mut Unicorn<'_, Context>, addr: u32, count: u32) -> Vec<u32> {
    (0..count)
        .map(|i| read_u32(unicorn, addr.wrapping_add(i * 4)).unwrap_or(0))
        .collect()
}

fn read_u32(unicorn: &mut Unicorn<'_, Context>, addr: u32) -> Option<u32> {
    let mut raw = [0u8; 4];
    unicorn
        .mem_read(addr as u64, &mut raw)
        .ok()
        .map(|()| u32::from_le_bytes(raw))
}

fn write_u32(unicorn: &mut Unicorn<'_, Context>, addr: u32, value: u32) {
    if unicorn.mem_write(addr as u64, &value.to_le_bytes()).is_err() {
        log::debug!(
            "[{}] [DEV SVG_LAYER] failed to write u32 0x{:x}=0x{:x}",
            unicorn.get_data().inner.thread_id(),
            addr,
            value
        );
    }
}