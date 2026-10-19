//! Guest addresses that outlive the VM thread which mapped them.
//!
//! Every guest process runs in its own Unicorn VM with a private address
//! space, but all VMs map heap, stacks and libraries at the *same numerical
//! addresses* (same loader, same image layout, same mmu heap cursor start).
//! A raw `u32` guest address cached in a process-global therefore names
//! different bytes in every process: dereferencing it under the wrong VM
//! does not fault - it silently reads or overwrites an unrelated object in
//! that peer's RAM (r791 audit).  Anything that caches a guest address
//! outside per-VM state must use this cell so the mismatch becomes a loud
//! warning instead of cross-process memory corruption.
//!
//! Per-binary hook modules (prochmi.rs, procmapengine.rs, ...) may keep
//! plain `AtomicU32` address statics: their hooks are only ever installed
//! in the one VM of that binary, so a process tag cannot add safety there.

use std::sync::atomic::{AtomicU32, Ordering};

use unicorn_engine::Unicorn;

use crate::emulator::context::Context;

/// A cached guest virtual address, tagged with the id of the process whose
/// VM mapped it.  [`GuestAddrCell::load`] only hands the address back to
/// that process; every other caller gets `0` and a warning.
pub struct GuestAddrCell {
    owner: AtomicU32,
    addr: AtomicU32,
}

impl GuestAddrCell {
    pub const fn new() -> Self {
        Self {
            owner: AtomicU32::new(0),
            addr: AtomicU32::new(0),
        }
    }

    /// Cache `addr`, recording the currently executing process as its owner.
    pub fn store(&self, unicorn: &Unicorn<'_, Context>, addr: u32) {
        self.owner
            .store(unicorn.get_data().inner.process_id, Ordering::Relaxed);
        self.addr.store(addr, Ordering::Relaxed);
    }

    /// The cached address if it was mapped by the currently executing
    /// process, else `0` (with a warning when a peer process owns it).
    pub fn load(&self, unicorn: &Unicorn<'_, Context>) -> u32 {
        let addr = self.addr.load(Ordering::Relaxed);
        if addr == 0 {
            return 0;
        }
        let pid = unicorn.get_data().inner.process_id;
        let owner = self.owner.load(Ordering::Relaxed);
        if owner != pid {
            log::warn!(
                "GVA-OWNER: cached guest address {addr:#x} belongs to process {owner}, \
                 not to the current process {pid}; ignoring it",
            );
            return 0;
        }
        addr
    }
}
