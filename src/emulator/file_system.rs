use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;

pub struct FileSystem {
    pub root_path: PathBuf,
    pub sd_card_path: PathBuf,

    opened_files: HashMap<u32, File>,
}

impl FileSystem {
    pub fn new(root_path: PathBuf, sd_card_path: PathBuf) -> Self {
        Self {
            root_path,
            sd_card_path,

            opened_files: HashMap::new(),
        }
    }

    pub fn path_transform_to_real(&self, guest_path: &str) -> PathBuf {
        if guest_path.starts_with("/") {
            self.root_path.join(&guest_path[1..])
        } else {
            panic!("not implemented");
        }
    }

    pub fn open(&mut self, filepath: &str) -> u32 {
        let fullpathname = self.path_transform_to_real(&filepath);
        if let Ok(file) = File::open(fullpathname) {
            let fd = self.get_next_fd();
            self.opened_files.insert(fd, file);
            fd
        } else {
            -2i32 as u32 // no such file or directory
        }
    }

    pub fn fd_to_file(&mut self, fd: u32) -> Option<&mut File> {
        self.opened_files.get_mut(&fd)
    }

    fn get_next_fd(&self) -> u32 {
        let mut res = 3u32;
        while self.opened_files.contains_key(&res) {
            res += 1;
        }
        res
    }
}
