use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;

pub struct FileInfo {
    pub file: File,
    pub filepath: String,
    pub inode: u64,
}

pub struct FileSystem {
    pub root_path: PathBuf,
    pub sd_card_path: PathBuf,

    pub current_working_dir: String,

    opened_files: HashMap<u32, FileInfo>,
    inodes: HashMap<String, u64>,
}

impl FileSystem {
    pub fn new(root_path: PathBuf, sd_card_path: PathBuf) -> Self {
        Self {
            root_path,
            sd_card_path,

            current_working_dir: "/opt/process".to_string(),

            opened_files: HashMap::new(),
            inodes: HashMap::new(),
        }
    }

    pub fn path_transform_to_real(&self, guest_path: &str) -> PathBuf {
        if guest_path.starts_with("/") {
            self.root_path.join(&guest_path[1..])
        } else {
            // TODO: rewrite using pwd
            self.root_path.join(&guest_path)
        }
    }

    pub fn open(&mut self, filepath: &str) -> u32 {
        let fullpathname = self.path_transform_to_real(&filepath);
        if let Ok(file) = File::open(fullpathname) {
            let fd = self.get_next_fd();
            let fileinfo = FileInfo {
                file,
                filepath: filepath.to_string(),
                inode: self.get_inode_for_filepath(filepath.to_string()),
            };
            self.opened_files.insert(fd, fileinfo);
            fd
        } else {
            -2i32 as u32 // no such file or directory
        }
    }

    pub fn close(&mut self, fd: u32) -> bool {
        self.opened_files.remove(&fd).map(|_| true).unwrap_or(false)
    }

    pub fn fd_to_file(&mut self, fd: u32) -> Option<&mut FileInfo> {
        self.opened_files.get_mut(&fd)
    }

    fn get_next_fd(&self) -> u32 {
        let mut res = 3u32;
        while self.opened_files.contains_key(&res) {
            res += 1;
        }
        res
    }

    fn get_inode_for_filepath(&mut self, filepath: String) -> u64 {
        let next_inode = self.inodes.len() as u64 + 1;
        let entry = self.inodes.entry(filepath).or_insert(next_inode);
        *entry
    }
}
