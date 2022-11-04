use std::collections::HashMap;
use std::error::Error;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use crate::emulator::context::Context;
use crate::emulator::thread::Thread;
use crate::emulator::utils::mem_align_up;
use unicorn_engine::unicorn_const::Permission;
use unicorn_engine::Unicorn;

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Clone)]
pub struct MmuRegion {
    pub memory_start: u32,
    pub memory_end: u32,
    pub memory_perms: Permission,
    pub description: String,
    pub filepath: String,

    // important! do not move the data after allocated
    data: Vec<u8>,
}

impl std::fmt::Display for MmuRegion {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{:08x} - {:08x} ({:>6} kB) {}{}{} {:<20} {}",
            self.memory_start,
            self.memory_end,
            (self.memory_end - self.memory_start + 1) / 1024,
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

pub struct Mmu {
    regions: Vec<MmuRegion>,
    pub brk_mem_end: u32,
    pub heap_mem_end: u32,
}

impl Mmu {
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            brk_mem_end: 0u32,
            heap_mem_end: 0u32,
        }
    }

    pub fn map(
        &mut self,
        unicorn: &mut Unicorn<Context>,
        address: u32,
        size: u32,
        perms: Permission,
        description: &str,
        filepath: &str,
    ) {
        let threads = unicorn.get_data().inner.threads.upgrade().unwrap();

        Self::pause_all_threads(&threads);

        self.remove_internal(address, size, &threads);

        // allocate memory
        let data = vec![0u8; size as usize];

        let desc = match description.len() {
            0 => String::from("[mapped]"),
            _ => String::from(description),
        };

        self.map_internal(&threads, address, size, perms, &desc, filepath, data);

        log::debug!(
            "mmu_map: {:#x} - {:#x} (size: {:#x}), {:?} {} {}",
            address,
            address + size - 1,
            size,
            perms,
            desc,
            filepath
        );

        Self::resume_all_threads(&threads);
    }

    pub fn unmap(&mut self, unicorn: &mut Unicorn<Context>, address: u32, size: u32) {
        let threads = unicorn.get_data().inner.threads.upgrade().unwrap();

        Self::pause_all_threads(&threads);

        self.remove_internal(address, size, &threads);

        log::debug!(
            "mmu_unmap: {:#x} - {:#x} (size: {:#x})",
            address,
            address + size,
            size,
        );

        Self::resume_all_threads(&threads);
    }

    pub fn mem_protect(
        &mut self,
        unicorn: &mut Unicorn<Context>,
        address: u32,
        size: u32,
        perms: Permission,
    ) {
        let threads = unicorn.get_data().inner.threads.upgrade().unwrap();

        Self::pause_all_threads(&threads);

        // split regions at the beginning and end point of the range
        self.split_internal(address, &threads);
        self.split_internal(address + size, &threads);

        // change permissions
        for thread in threads.lock().unwrap().iter_mut() {
            thread
                .unicorn
                .mem_protect(address as u64, size as usize, perms)
                .unwrap();
        }
        for item in &mut self.regions {
            if item.memory_start >= address && item.memory_end <= address + size - 1 {
                item.memory_perms = perms;
            }
        }

        Self::resume_all_threads(&threads);
    }

    pub fn display_mapped(&self) -> String {
        /*let mut v: Vec<_> = Vec::new();
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
        str*/
        "".to_string()
    }

    pub fn heap_alloc(
        &mut self,
        unicorn: &mut Unicorn<Context>,
        size: u32,
        perms: Permission,
        filepath: &str,
    ) -> u32 {
        let heap_addr = self.heap_mem_end;

        let size = mem_align_up(size, None);
        self.map(unicorn, heap_addr, size, perms, "[heap]", filepath);

        self.heap_mem_end = heap_addr + size;

        heap_addr
    }

    /// map region to all threads without verifying overlaps
    fn map_internal(
        &mut self,
        threads: &Arc<Mutex<Vec<Thread>>>,
        address: u32,
        size: u32,
        perms: Permission,
        description: &str,
        filepath: &str,
        mut data: Vec<u8>,
    ) {
        // map allocated memory to all threads
        for thread in threads.lock().unwrap().iter_mut() {
            // unsafe is ok as long as:
            // 1. data will not be moved (Vec resized etc.)
            // 2. memory will be unmapped before deallocating data
            unsafe {
                let _ = thread
                    .unicorn
                    .mem_map_ptr(
                        address as u64,
                        size as usize,
                        perms,
                        data.as_mut_ptr() as *mut c_void,
                    )
                    .unwrap();
            }
        }

        let region = MmuRegion {
            memory_start: address,
            memory_end: address.checked_add(size).unwrap().checked_sub(1).unwrap(),
            memory_perms: perms,
            description: description.to_string(),
            filepath: filepath.to_owned(),
            data,
        };

        self.regions.push(region);
    }

    /// unmap region from all threads without verifying overlaps
    fn unmap_internal(&mut self, address: u32, size: u32, threads: &Arc<Mutex<Vec<Thread>>>) {
        if !self
            .regions
            .iter()
            .any(|item| item.memory_start >= address && item.memory_end <= address + size - 1)
        {
            return;
        }

        for thread in threads.lock().unwrap().iter_mut() {
            thread
                .unicorn
                .mem_unmap(address as u64, size as usize)
                .unwrap();
        }

        self.regions
            .retain(|item| item.memory_end < address || item.memory_start >= address + size);
    }

    fn remove_internal(&mut self, address: u32, size: u32, threads: &Arc<Mutex<Vec<Thread>>>) {
        // split regions at the beginning and end point of the range
        self.split_internal(address, threads);
        self.split_internal(address + size, threads);

        // remove all existing regions that are fully covered by the range
        self.unmap_internal(address, size, threads);
    }

    fn split_internal(&mut self, address: u32, threads: &Arc<Mutex<Vec<Thread>>>) {
        let to_be_split: Vec<_> = self
            .regions
            .iter()
            .filter(|item| item.memory_start < address && item.memory_end >= address)
            .map(|item| item.clone())
            .collect();

        for item in to_be_split {
            self.unmap_internal(
                item.memory_start,
                item.memory_end - item.memory_start + 1,
                threads,
            );

            // left item
            self.map_internal(
                threads,
                item.memory_start,
                address - item.memory_start,
                item.memory_perms,
                &item.description,
                &item.filepath,
                Vec::from(&item.data[0..(address - item.memory_start) as usize]),
            );

            // right item
            self.map_internal(
                threads,
                address,
                item.memory_end - address + 1,
                item.memory_perms,
                &item.description,
                &item.filepath,
                Vec::from(
                    &item.data[(address - item.memory_start) as usize
                        ..(item.memory_end - item.memory_start + 1) as usize],
                ),
            );
        }
    }

    fn pause_all_threads(threads: &Arc<Mutex<Vec<Thread>>>) {
        for thread in threads.lock().unwrap().iter_mut() {
            thread.pause().unwrap();
        }
    }

    fn resume_all_threads(threads: &Arc<Mutex<Vec<Thread>>>) {
        for thread in threads.lock().unwrap().iter_mut() {
            thread.resume();
        }
    }
}

pub fn mmu_clone_map(
    src_unicorn: &Unicorn<Context>,
    dest_unicorn: &mut Unicorn<Context>,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let data = src_unicorn.get_data();
    let mmu = &mut data.inner.mmu.lock().unwrap();

    for item in &mut mmu.regions {
        unsafe {
            dest_unicorn
                .mem_map_ptr(
                    item.memory_start as u64,
                    (item.memory_end - item.memory_start + 1) as usize,
                    item.memory_perms,
                    item.data.as_mut_ptr() as *mut c_void,
                )
                .unwrap();
        }
    }

    /*let mut map: Vec<_> = mmu.map_infos.values().collect();
    map.sort_by(|map1, map2| map1.memory_start.cmp(&map2.memory_start));

    let mut map2 = self.mem_regions().unwrap();

    for i in 0..map.len().min(map2.len()) {
        if map[i].memory_start != map2[i].begin as u32
            || map[i].memory_end != map2[i].end as u32
            || map[i].memory_perms != map2[i].perms
        {
            println!(
                "map[{}] {}-{}-{:?} vs {}-{}-{:?}",
                i,
                map[i].memory_start,
                map[i].memory_end,
                map[i].memory_perms,
                map2[i].begin,
                map2[i].end,
                map2[i].perms
            );
        }
    }

    for (_, map_info) in &mmu.map_infos {
        let regions = self.mem_regions().unwrap();
        let address = map_info.memory_start as u64;
        let size = (map_info.memory_end - map_info.memory_start + 1) as u64;
        let perms = regions
            .iter()
            .filter(|r| r.begin <= address && r.end >= address + size - 1)
            .next()
            .unwrap()
            .perms;

        unsafe {
            dest_unicorn
                .mem_map_ptr(
                    map_info.memory_start as u64,
                    (map_info.memory_end - map_info.memory_start + 1) as usize,
                    map_info.memory_perms,
                    map_info.data.as_ptr() as *mut c_void,
                )
                .map_err(|err| format!("Unicorn mem_map_ptr error: {:?}", err))?;
        }
    }*/
    Ok(())
}
