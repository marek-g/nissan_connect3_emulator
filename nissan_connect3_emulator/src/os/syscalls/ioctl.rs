use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

/// CID of the navigation card whose image the emulator serves. It is the value
/// the card's SDX certificate was signed for, so the emulator reports it through
/// the cryptcard device instead of faking the signature verdict.
const SD_CARD_CID_HEX: &str = "5d5342303031364712e055a86c013301";

fn hex_cid_bytes() -> Option<[u8; 16]> {
    let bytes = SD_CARD_CID_HEX.as_bytes();
    if bytes.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, chunk) in bytes.chunks(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(out)
}

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
        // OSAL_s32IOControl(0x410) hands out the SD card's 16-byte CID, which
        // the SDX certificate check hashes into its partition signature
        // (SHA1(CID-hex-uppercase + cert VIN + lifetime + the HASH_FILE_LIST
        // files) verified with the operator key). Our certificate is the
        // original one shipped with this card, so reporting the card's real CID
        // (from cid.txt next to the card image) makes that signature verify
        // instead of bypassing the check.
        Some("/dev/cryptcard") | Some("/dev/cryptcard2") if request == 0x410 => {
            log::info!(
                "ioctl {}: card CID -> {} (fd={:#x})",
                path.as_deref().unwrap_or("cryptcard"),
                SD_CARD_CID_HEX,
                fd
            );
            match hex_cid_bytes() {
                Some(bytes) => {
                    let _ = unicorn.mem_write(addr as u64, &bytes);
                    0
                }
                None => -1i32 as u32,
            }
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
