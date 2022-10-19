use std::collections::HashMap;

use crate::emulator::context::Context;
use unicorn_engine::unicorn_const::Permission;
use unicorn_engine::Unicorn;

pub struct Mmu {
    map_infos: HashMap<u32, MapInfo>,
}

impl Mmu {
    pub fn new() -> Self {
        Self {
            map_infos: HashMap::new(),
        }
    }
}

pub trait MmuExtension {
    fn mmu_map(&mut self, address: u32, size: u32, perms: Permission, description: &str) -> bool;
    fn add_mapinfo(&mut self, map_info: MapInfo);
    fn mmu_unmap(&mut self, address: u32, size: usize);
    fn is_mapped(&mut self, address: u32, size: u32) -> bool;
}

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct MapInfo {
    pub memory_start: u32,
    pub memory_end: u32,
    pub memory_perms: Permission,
    pub description: String,
}

impl Clone for MapInfo {
    fn clone(&self) -> Self {
        MapInfo {
            memory_start: self.memory_start,
            memory_end: self.memory_end,
            memory_perms: self.memory_perms,
            description: self.description.clone(),
        }
    }
}

impl std::fmt::Display for MapInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "memory_start {:x} memory_end {:x} memory_perms : {}  description: {}",
            self.memory_start,
            self.memory_end,
            self.memory_perms.bits(),
            self.description
        )
        .unwrap();
        Ok(())
    }
}

impl<'a> MmuExtension for Unicorn<'a, Context> {
    fn mmu_map(&mut self, address: u32, size: u32, perms: Permission, description: &str) -> bool {
        if self.is_mapped(address, size as u32) {
            return true;
        }

        let _ = self
            .mem_map(address as u64, size as libc::size_t, perms)
            .unwrap();

        let desc = match description.len() {
            0 => String::from("[mapped]"),
            _ => String::from(description),
        };

        let map_info = MapInfo {
            memory_start: address,
            memory_end: address.checked_add(size).unwrap(),
            memory_perms: perms,
            description: desc.clone(),
        };

        self.add_mapinfo(map_info);

        log::debug!(
            "mmu_map: {:#x} - {:#x} (size: {:#x}), {:?} {}",
            address,
            address + size,
            size,
            perms,
            desc
        );

        true
    }

    fn add_mapinfo(&mut self, map_info: MapInfo) {
        self.get_data_mut()
            .mmu
            .map_infos
            .insert(map_info.memory_start, map_info);
    }

    fn mmu_unmap(&mut self, address: u32, size: usize) {
        _ = self.get_data_mut().mmu.map_infos.remove_entry(&address);
        self.mem_unmap(address as u64, size).unwrap();
    }

    fn is_mapped(&mut self, address: u32, size: u32) -> bool {
        let regions = self.mem_regions().unwrap();
        if regions.len() <= 1 {
            return false;
        }

        if let Ok(region) = self.mem_regions() {
            let val = (region[0].begin >= address as u64)
                & ((address + size - 1) as u64 <= region[1].begin);
            match val {
                true => {
                    return true;
                }
                _ => {}
            }
        }

        false
    }
}
