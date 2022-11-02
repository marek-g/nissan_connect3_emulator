use std::collections::HashMap;
use std::error::Error;
use std::ffi::c_void;

use crate::emulator::context::Context;
use crate::emulator::utils::mem_align_up;
use unicorn_engine::unicorn_const::Permission;
use unicorn_engine::Unicorn;

pub struct Mmu {
    map_infos: HashMap<u32, MapInfo>,
    pub brk_mem_end: u32,
    pub heap_mem_end: u32,
}

impl Mmu {
    pub fn new() -> Self {
        Self {
            map_infos: HashMap::new(),
            brk_mem_end: 0u32,
            heap_mem_end: 0u32,
        }
    }
}

pub trait MmuExtension {
    fn mmu_map(
        &mut self,
        address: u32,
        size: u32,
        perms: Permission,
        description: &str,
        filepath: &str,
    );
    fn add_mapinfo(&mut self, map_info: MapInfo);
    fn mmu_unmap(&mut self, address: u32, size: u32);
    fn mmu_mem_protect(&mut self, address: u32, size: u32, perms: Permission);
    fn mmu_clone_map(
        &self,
        dest_unicorn: &mut Unicorn<Context>,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>>;
    fn is_mapped(&self, address: u32, size: u32) -> bool;
    fn update_map_info_filepath(&mut self, address: u32, size: u32, filename: &str);
    fn display_mapped(&self) -> String;

    fn heap_alloc(&mut self, size: u32, perms: Permission, filepath: &str) -> u32;

    fn read_string(&mut self, addr: u32) -> String;
}

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct MapInfo {
    pub memory_start: u32,
    pub memory_end: u32,
    pub memory_perms: Permission,
    pub description: String,
    pub filepath: String,

    // important! do not move the data after allocated
    data: Vec<u8>,
}

impl std::fmt::Display for MapInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{:08x} - {:08x} ({:>6} kB) {}{}{} {:<20} {}",
            self.memory_start,
            self.memory_end,
            (self.memory_end - self.memory_start) / 1024,
            if self.memory_perms & Permission::READ != Permission::NONE {
                "R"
            } else {
                "-"
            },
            if self.memory_perms & Permission::WRITE != Permission::NONE {
                "W"
            } else {
                "-"
            },
            if self.memory_perms & Permission::EXEC != Permission::NONE {
                "X"
            } else {
                "-"
            },
            self.description,
            self.filepath
        )
        .unwrap();
        Ok(())
    }
}

impl MmuExtension for Unicorn<Context> {
    fn mmu_map(
        &mut self,
        address: u32,
        size: u32,
        perms: Permission,
        description: &str,
        filepath: &str,
    ) {
        if self.is_mapped(address, size as u32) {
            self.mmu_mem_protect(address, mem_align_up(size, None), perms);
            self.update_map_info_filepath(address, size, filepath);
            return;
        }

        // pause all threads
        if let Some(threads) = self.get_data().inner.threads.upgrade() {
            for thread in threads.lock().unwrap().iter_mut() {
                thread.pause().unwrap();
            }
        }

        // allocate memory
        let mut data = vec![0u8; size as usize];

        if let Some(threads) = self.get_data().inner.threads.upgrade() {
            for thread in threads.lock().unwrap().iter_mut() {
                // unsafe is ok as long as:
                // 1. data will not be moved (Vec resized etc.)
                // 2. memory will be unmapped before deallocating data
                unsafe {
                    let _ = thread
                        .mem_map_ptr(
                            address as u64,
                            size as usize,
                            perms,
                            data.as_mut_ptr() as *mut c_void,
                        )
                        .unwrap();
                }
            }
        }

        let desc = match description.len() {
            0 => String::from("[mapped]"),
            _ => String::from(description),
        };

        let map_info = MapInfo {
            memory_start: address,
            memory_end: address.checked_add(size).unwrap(),
            memory_perms: perms,
            description: desc.clone(),
            filepath: filepath.to_owned(),
            data,
        };

        self.add_mapinfo(map_info);

        log::debug!(
            "mmu_map: {:#x} - {:#x} (size: {:#x}), {:?} {} {}",
            address,
            address + size,
            size,
            perms,
            desc,
            filepath
        );

        // resume all threads
        if let Some(threads) = self.get_data().inner.threads.upgrade() {
            for thread in threads.lock().unwrap().iter_mut() {
                thread.resume();
            }
        }
    }

    fn add_mapinfo(&mut self, map_info: MapInfo) {
        self.get_data()
            .inner
            .mmu
            .lock()
            .unwrap()
            .map_infos
            .insert(map_info.memory_start, map_info);
    }

    fn mmu_unmap(&mut self, address: u32, size: u32) {
        let (_, entry) = self
            .get_data()
            .inner
            .mmu
            .lock()
            .unwrap()
            .map_infos
            .remove_entry(&address)
            .unwrap();

        if let Some(threads) = self.get_data().inner.threads.upgrade() {
            // pause all threads
            for thread in threads.lock().unwrap().iter_mut() {
                thread.pause().unwrap();
            }

            for thread in threads.lock().unwrap().iter_mut() {
                thread.mem_unmap(address as u64, size as usize).unwrap();
            }

            // resume all threads
            for thread in threads.lock().unwrap().iter_mut() {
                thread.resume();
            }
        }

        log::debug!(
            "mmu_unmap: {:#x} - {:#x} (size: {:#x}), {:?} {} {}",
            address,
            address + size,
            size,
            entry.memory_perms,
            entry.description,
            entry.filepath
        );
    }

    fn mmu_mem_protect(&mut self, address: u32, size: u32, perms: Permission) {
        if let Some(threads) = self.get_data().inner.threads.upgrade() {
            // pause all threads
            for thread in threads.lock().unwrap().iter_mut() {
                thread.pause().unwrap();
            }

            for thread in threads.lock().unwrap().iter_mut() {
                thread
                    .mem_protect(address as u64, size as usize, perms)
                    .unwrap();
            }

            // resume all threads
            for thread in threads.lock().unwrap().iter_mut() {
                thread.resume();
            }
        }
    }

    fn mmu_clone_map(
        &self,
        dest_unicorn: &mut Unicorn<Context>,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        let data = self.get_data();
        let mmu = data.inner.mmu.lock().unwrap();
        for (_, map_info) in &mmu.map_infos {
            unsafe {
                dest_unicorn
                    .mem_map_ptr(
                        map_info.memory_start as u64,
                        (map_info.memory_end - map_info.memory_start) as usize,
                        map_info.memory_perms,
                        map_info.data.as_ptr() as *mut c_void,
                    )
                    .map_err(|err| format!("Unicorn mem_map_ptr error: {:?}", err))?;
            }
        }
        Ok(())
    }

    fn is_mapped(&self, address: u32, size: u32) -> bool {
        let regions = self.mem_regions().unwrap();
        regions
            .iter()
            .any(|r| r.begin <= address as u64 && r.end >= address as u64 + size as u64 - 1)
    }

    fn update_map_info_filepath(&mut self, address: u32, size: u32, filepath: &str) {
        let data = self.get_data();
        let map_infos = &mut data.inner.mmu.lock().unwrap().map_infos;
        for (_key, value) in map_infos {
            if value.memory_start <= address && value.memory_end >= address + size {
                value.filepath = filepath.to_string();
            }
        }
    }

    fn display_mapped(&self) -> String {
        let mut v: Vec<_> = Vec::new();
        let data = self.get_data();
        let mmu = data.inner.mmu.lock().unwrap();
        for (addr, map_info) in mmu.map_infos.iter() {
            v.push((addr, map_info));
        }
        v.sort_by(|x, y| x.0.cmp(&y.0));

        let mut str = String::from("Memory layout:");
        for (_addr, map_info) in v {
            str.push_str(&format!("\n{}", map_info));
        }
        str
    }

    fn heap_alloc(&mut self, size: u32, perms: Permission, filepath: &str) -> u32 {
        let heap_addr = self.get_data().inner.mmu.lock().unwrap().heap_mem_end;

        let size = mem_align_up(size, None);
        self.mmu_map(heap_addr, size, perms, "[heap]", filepath);

        self.get_data().inner.mmu.lock().unwrap().heap_mem_end = heap_addr + size;

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
