use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn ioctl(mut unicorn: &mut Unicorn<'_, Context>, fd: u32, request: u32, addr: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] ioctl(fd = {:#x}, request: {:#x}, addr: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        fd,
        request,
        addr,
    );

    let file_system = unicorn.get_data().inner.file_system.clone();
    let mut fs = file_system.lock().unwrap();
    let path = fs.get_file_info(fd as i32).map(|info| info.file_path);

    if !crate::os::dev::iosc::is_iosc_fd(&unicorn, fd) {
        log::warn!(
            "ioctl path fd={:#x} path={:?} request={:#x} addr={:#x}",
            fd,
            path,
            request,
            addr
        );
    }

    // /dev/iosc fds are backed by the emulated IOSC driver, not a filesystem
    if crate::os::dev::iosc::is_iosc_fd(&unicorn, fd) {
        return crate::os::dev::iosc::ioctl(&mut unicorn, fd, request, addr);
    }

    let res = match path.as_deref() {
        // SD card control device. OSAL_s32IOControl(0x7ffffffc) asks for the
        // media status bit-mask consumed by DAPIAPP bRegPRMNotifications:
        // bit0 medium inserted, bit2 medium ready, and the *inverted* bits
        // ~bit1 device-ok, ~bit3 device-access, ~bit4 temperature ok.
        Some("/dev/cryptcard") if request == 0x7ffffffc => {
            log::info!("ioctl /dev/cryptcard: media status request -> inserted|ready|ok");
            5
        }
        Some("/dev/svg_resource") => {
            crate::os::dev::svg_resource::ioctl(&mut unicorn, request, addr) as u32
        }
        Some("/dev/svg_layer") => {
            crate::os::dev::svg_layer::ioctl(&mut unicorn, request, addr) as u32
        }
        _ => fs.ioctl(&mut unicorn, fd as i32, request, addr) as u32,
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] ioctl => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}
