use crate::emulator::mmu::Mmu;
use crate::file_system::MountFileSystem;

pub struct Context {
    pub mmu: Mmu,

    pub file_system: MountFileSystem,
}
