use crate::emulator::mmu::Mmu;
use std::path::PathBuf;

pub struct Context {
    pub mmu: Mmu,
    pub root_path: PathBuf,
    pub sd_card_path: PathBuf,
}
