use crate::emulator::context::Context;
use crate::emulator::utils::mem_align_up;
use crate::os::add_library_hook;
use std::ffi::c_void;
use unicorn_engine::unicorn_const::Prot;
use unicorn_engine::Unicorn;

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Clone)]
pub struct MmuRegion {
    pub memory_start: u32,
    pub memory_end: u32,
    pub memory_perms: Prot,
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
            if self.memory_perms & Prot::READ != Prot::NONE {
                "R"
            } else {
                "-"
            },
            if self.memory_perms & Prot::WRITE != Prot::NONE {
                "W"
            } else {
                "-"
            },
            if self.memory_perms & Prot::EXEC != Prot::NONE {
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
        unicorn: &mut Unicorn<'_, Context>,
        address: u32,
        size: u32,
        perms: Prot,
        description: &str,
        filepath: &str,
    ) {
        self.remove_internal(unicorn, address, size);

        // fresh anonymous memory is zero-filled by the emulator (like a real kernel)
        unicorn.mem_map(address as u64, size as u64, perms).unwrap();

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

    /// Map a shared-memory region backed by a caller-provided host buffer via
    /// `mem_map_ptr`. Unlike [`Mmu::map`] (which allocates fresh, private guest
    /// memory), the guest region here is a view onto `host_ptr`, so every process
    /// that maps the same buffer aliases the same physical (host) bytes.
    pub fn map_shared(
        &mut self,
        unicorn: &mut Unicorn<'_, Context>,
        address: u32,
        size: u32,
        perms: Prot,
        description: &str,
        filepath: &str,
        host_ptr: *mut c_void,
    ) {
        self.remove_internal(unicorn, address, size);

        unsafe {
            unicorn
                .mem_map_ptr(address as u64, size as u64, perms, host_ptr)
                .unwrap();
        }

        let desc = match description.len() {
            0 => String::from("[shared]"),
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
            "mmu_map_shared: {:#x} - {:#x} (size: {:#x}), {:?} {} {}",
            address,
            address + size - 1,
            size,
            perms,
            desc,
            filepath
        );
    }

    /// Canonical identity of a word that lives in named shared memory
    /// (`/dev/shm/*`): `(shm_path, offset)`. Two processes that mapped the same
    /// shm object alias the same host bytes, so this key is the same for the
    /// same word in every process - exactly what a shared futex needs to be
    /// matched across processes. Returns `None` for a word in process-private
    /// memory (which has no cross-process identity).
    pub fn shared_futex_key(&self, addr: u32) -> Option<(String, u32)> {
        self.regions.iter().find_map(|r| {
            if addr >= r.memory_start
                && addr <= r.memory_end
                && r.filepath.starts_with("/dev/shm/")
            {
                Some((r.filepath.clone(), addr - r.memory_start))
            } else {
                None
            }
        })
    }

    pub fn unmap(&mut self, unicorn: &mut Unicorn<'_, Context>, address: u32, size: u32) {
        self.remove_internal(unicorn, address, size);
        log::debug!(
            "mmu_unmap: {:#x} - {:#x}",
            address,
            address + size,
        );
    }

    pub fn mem_protect(
        &mut self,
        unicorn: &mut Unicorn<'_, Context>,
        address: u32,
        size: u32,
        perms: Prot,
    ) {
        // split regions at the beginning and end point of the range
        self.split_internal(unicorn, address);
        self.split_internal(unicorn, address + size);

        unicorn.mem_protect(address as u64, size as u64, perms).unwrap();

        for item in &mut self.regions {
            if item.memory_start >= address && item.memory_end <= address + size - 1 {
                item.memory_perms = perms;
            }
        }
    }

    pub fn get_libraries_and_base_addresses(&self) -> Vec<(String, u32)> {
        self.regions
            .iter()
            .filter(|region| {
                (region.memory_perms & Prot::EXEC) != Prot::NONE && region.filepath.len() > 0
            })
            .map(|region| (region.filepath.clone(), region.memory_start))
            .collect()
    }

    /// Adds code hooks for newly mapped libraries to the (single) VM.
    pub fn update_library_hooks(&self, unicorn: &mut Unicorn<'_, Context>) {
        let libraries = self.get_libraries_and_base_addresses();
        let hooked_libraries = unicorn.get_data().inner.hooked_libraries.clone();
        for (library, base_address) in libraries {
            if !hooked_libraries.lock().unwrap().contains(&library) {
                add_library_hook(unicorn, &library, base_address);
                hooked_libraries.lock().unwrap().insert(library);
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

    pub fn heap_alloc(
        &mut self,
        unicorn: &mut Unicorn<'_, Context>,
        size: u32,
        perms: Prot,
        filepath: &str,
    ) -> u32 {
        let heap_addr = self.heap_mem_end;

        let size = mem_align_up(size, None);
        self.map(unicorn, heap_addr, size, perms, "[heap]", filepath);

        self.heap_mem_end = heap_addr + size;

        heap_addr
    }

    /// Like [`Mmu::heap_alloc`] but backs the region with a shared host buffer
    /// (`mem_map_ptr`) instead of fresh private memory.
    pub fn heap_alloc_shared(
        &mut self,
        unicorn: &mut Unicorn<'_, Context>,
        size: u32,
        perms: Prot,
        filepath: &str,
        host_ptr: *mut c_void,
    ) -> u32 {
        let heap_addr = self.heap_mem_end;

        let size = mem_align_up(size, None);
        self.map_shared(unicorn, heap_addr, size, perms, "[heap (shared)]", filepath, host_ptr);

        self.heap_mem_end = heap_addr + size;

        heap_addr
    }

    /// unmap all regions fully covered by [address, address + size)
    fn unmap_internal(&mut self, unicorn: &mut Unicorn<'_, Context>, address: u32, size: u32) {
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
                .mem_unmap(region.0 as u64, (region.1 - region.0 + 1) as u64)
                .unwrap();
        }

        self.regions
            .retain(|item| item.memory_end < address || item.memory_start >= address + size);
    }

    fn remove_internal(&mut self, unicorn: &mut Unicorn<'_, Context>, address: u32, size: u32) {
        // split regions at the beginning and end point of the range
        self.split_internal(unicorn, address);
        self.split_internal(unicorn, address + size);

        // remove all existing regions that are fully covered by the range
        self.unmap_internal(unicorn, address, size);
    }

    /// split the region containing `address` into two at that point,
    /// preserving the existing contents
    fn split_internal(&mut self, unicorn: &mut Unicorn<'_, Context>, address: u32) {
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
                .mem_map(item.memory_start as u64, left_size as u64, item.memory_perms)
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
                .mem_map(address as u64, right_size as u64, item.memory_perms)
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
