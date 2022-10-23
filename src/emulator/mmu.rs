use std::collections::HashMap;

use crate::emulator::context::Context;
use crate::emulator::utils::mem_align_up;
use unicorn_engine::unicorn_const::Permission;
use unicorn_engine::Unicorn;

pub struct Mmu {
    map_infos: HashMap<u32, MapInfo>,
    pub heap_mem_end: u32,
}

impl Mmu {
    pub fn new() -> Self {
        Self {
            map_infos: HashMap::new(),
            heap_mem_end: 0u32,
        }
    }
}

pub trait MmuExtension {
    fn mmu_map(&mut self, address: u32, size: u32, perms: Permission, description: &str);
    fn add_mapinfo(&mut self, map_info: MapInfo);
    fn mmu_unmap(&mut self, address: u32, size: usize);
    fn is_mapped(&mut self, address: u32, size: u32) -> bool;

    fn heap_alloc(&mut self, size: u32, perms: Permission) -> u32;

    fn read_string(&mut self, addr: u32) -> String;
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
    fn mmu_map(&mut self, address: u32, size: u32, perms: Permission, description: &str) {
        if self.is_mapped(address, size as u32) {
            return;
        }

        let _ = self
            .mem_map(address as u64, size as libc::size_t, perms)
            .unwrap();

        // clear allocated memory
        let buf = vec![0; size as usize];
        self.mem_write(address as u64, &buf).unwrap();

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

    fn heap_alloc(&mut self, size: u32, perms: Permission) -> u32 {
        let heap_addr = self.get_data().mmu.heap_mem_end;

        let size = mem_align_up(size, None);
        self.mmu_map(heap_addr, size, perms, "[heap]");

        self.get_data_mut().mmu.heap_mem_end = heap_addr + size;

        heap_addr
    }

    fn read_string(&mut self, mut addr: u32) -> String {
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            self.mem_read(addr as u64, &mut byte).unwrap();
            if byte[0] == 0 {
                break;
            }
            buf.push(byte[0]);
            addr += 1;
        }
        String::from_utf8(buf).unwrap()
    }
}
