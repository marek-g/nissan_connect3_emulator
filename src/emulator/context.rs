use crate::emulator::file_system::FileSystem;
use crate::emulator::mmu::Mmu;

pub struct Context {
    pub mmu: Mmu,
    pub file_system: FileSystem,
}
