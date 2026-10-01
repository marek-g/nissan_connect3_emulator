use crate::emulator::context::Context;
use crate::emulator::utils::mem_align_up;
use crate::os::add_library_hook;
use unicorn_engine::unicorn_const::Permission;
use unicorn_engine::Unicorn;

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Clone)]
pub struct MmuRegion {
    pub memory_start: u32,
    pub memory_end: u32,
    pub memory_perms: Permission,
    pub description: String,
    pub filepath: String,
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

/// Bookkeeping of the guest address space. All operations apply directly to the
/// single Unicorn VM - there is no per-thread fan-out and no mirror copy of the
/// mapped data (the VM's own memory is the single source of truth).
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
        self.remove_internal(unicorn, address, size);

        // fresh anonymous memory is zero-filled by the emulator (like a real kernel)
        unicorn.mem_map(address as u64, size as usize, perms).unwrap();

        let desc = match description.len() {
            0 => String::from("[mapped]"),
            _ => String::from(description),
        };

        self.regions.push(MmuRegion {
            memory_start: address,
            memory_end: address.checked_add(size).unwrap().checked_sub(1).unwrap(),
            memory_perms: perms,
            description: desc.clone(),
            filepath: filepath.to_owned(),
        });

        log::debug!(
            "mmu_map: {:#x} - {:#x} (size: {:#x}), {:?} {} {}",
            address,
            address + size - 1,
            size,
            perms,
            desc,
            filepath
        );
    }

    pub fn unmap(&mut self, unicorn: &mut Unicorn<Context>, address: u32, size: u32) {
        self.remove_internal(unicorn, address, size);

        log::debug!(
            "mmu_unmap: {:#x} - {:#x}",
            address,
            address + size,
        );
    }

    pub fn mem_protect(
        &mut self,
        unicorn: &mut Unicorn<Context>,
        address: u32,
        size: u32,
        perms: Permission,
    ) {
        // split regions at the beginning and end point of the range
        self.split_internal(unicorn, address);
        self.split_internal(unicorn, address + size);

        unicorn.mem_protect(address as u64, size as usize, perms).unwrap();

        for item in &mut self.regions {
            if item.memory_start >= address && item.memory_end <= address + size - 1 {
                item.memory_perms = perms;
            }
        }
    }

    pub fn get_regions(&self) -> &Vec<MmuRegion> {
        &self.regions
    }

    pub fn get_libraries_and_base_addresses(&self) -> Vec<(String, u32)> {
        self.regions
            .iter()
            .filter(|region| {
                region.memory_perms.contains(Permission::EXEC) && region.filepath.len() > 0
            })
            .map(|region| (region.filepath.clone(), region.memory_start))
            .collect()
    }

    /// Adds code hooks for newly mapped libraries to the (single) VM.
    pub fn update_library_hooks(&self, unicorn: &mut Unicorn<Context>) {
        let libraries = self.get_libraries_and_base_addresses();
        let data = unicorn.get_data();
        for (library, base_address) in libraries {
            if !data
                .inner
                .hooked_libraries
                .lock()
                .unwrap()
                .contains(&library)
            {
                add_library_hook(unicorn, &library, base_address);
                data.inner.hooked_libraries.lock().unwrap().insert(library);
            }
        }
    }

    pub fn display_mapped(&self) -> String {
        let mut v: Vec<_> = self.regions.clone();
        v.sort_by(|x, y| x.memory_start.cmp(&y.memory_start));

        let mut str = format!("{} regions:", v.len());
        for map_info in v {
            str.push_str(&format!("\n{}", map_info));
        }
        str
    }

    pub fn display_mapped_unicorn(unicorn: &Unicorn<Context>) -> String {
        let mut v: Vec<_> = Vec::new();
        for mem_region in unicorn.mem_regions().unwrap() {
            v.push(MmuRegion {
                memory_start: mem_region.begin as u32,
                memory_end: mem_region.end as u32,
                memory_perms: mem_region.perms,
                description: "".to_string(),
                filepath: "".to_string(),
            });
        }
        v.sort_by(|x, y| x.memory_start.cmp(&y.memory_start));

        let mut str = format!("{} regions:", v.len());
        for map_info in v {
            str.push_str(&format!("\n{}", map_info));
        }
        str
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

    /// unmap all regions fully covered by [address, address + size)
    fn unmap_internal(&mut self, unicorn: &mut Unicorn<Context>, address: u32, size: u32) {
        let regions_to_unmap: Vec<_> = self
            .regions
            .iter()
            .filter(|item| item.memory_start >= address && item.memory_end <= address + size - 1)
            .map(|item| (item.memory_start, item.memory_end))
            .collect();

        if regions_to_unmap.len() == 0 {
            return;
        }

        for region in &regions_to_unmap {
            unicorn
                .mem_unmap(region.0 as u64, (region.1 - region.0 + 1) as usize)
                .unwrap();
        }

        self.regions
            .retain(|item| item.memory_end < address || item.memory_start >= address + size);
    }

    fn remove_internal(&mut self, unicorn: &mut Unicorn<Context>, address: u32, size: u32) {
        // split regions at the beginning and end point of the range
        self.split_internal(unicorn, address);
        self.split_internal(unicorn, address + size);

        // remove all existing regions that are fully covered by the range
        self.unmap_internal(unicorn, address, size);
    }

    /// split the region containing `address` into two at that point,
    /// preserving the existing contents
    fn split_internal(&mut self, unicorn: &mut Unicorn<Context>, address: u32) {
        let to_be_split: Vec<_> = self
            .regions
            .iter()
            .filter(|item| item.memory_start < address && item.memory_end >= address)
            .cloned()
            .collect();

        for item in to_be_split {
            let size = item.memory_end - item.memory_start + 1;
            let left_size = address - item.memory_start;
            let right_size = item.memory_end - address + 1;

            // read the whole region's contents
            let mut data = vec![0u8; size as usize];
            unicorn.mem_read(item.memory_start as u64, &mut data).unwrap();

            // remove the old region from the VM and the bookkeeping
            self.unmap_internal(unicorn, item.memory_start, size);

            let split_offset = left_size as usize;

            // left part
            unicorn
                .mem_map(item.memory_start as u64, left_size as usize, item.memory_perms)
                .unwrap();
            unicorn
                .mem_write(item.memory_start as u64, &data[..split_offset])
                .unwrap();
            self.regions.push(MmuRegion {
                memory_start: item.memory_start,
                memory_end: address - 1,
                memory_perms: item.memory_perms,
                description: item.description.clone(),
                filepath: item.filepath.clone(),
            });

            // right part
            unicorn
                .mem_map(address as u64, right_size as usize, item.memory_perms)
                .unwrap();
            unicorn.mem_write(address as u64, &data[split_offset..]).unwrap();
            self.regions.push(MmuRegion {
                memory_start: address,
                memory_end: item.memory_end,
                memory_perms: item.memory_perms,
                description: item.description.clone(),
                filepath: item.filepath.clone(),
            });
        }
    }
}
