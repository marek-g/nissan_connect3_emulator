use crate::emulator::mmu::Mmu;
use crate::file_system::MountFileSystem;
use std::cell::RefCell;
use std::rc::Rc;

pub struct Context {
    pub mmu: Mmu,

    pub file_system: Rc<RefCell<MountFileSystem>>,
}
