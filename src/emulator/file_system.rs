use std::path::PathBuf;

pub struct FileSystem {
    pub root_path: PathBuf,
    pub sd_card_path: PathBuf,
}

impl FileSystem {
    pub fn new(root_path: PathBuf, sd_card_path: PathBuf) -> Self {
        Self {
            root_path,
            sd_card_path,
        }
    }

    pub fn path_transform_to_real(&self, guest_path: &str) -> PathBuf {
        if guest_path.starts_with("/") {
            self.root_path.join(&guest_path[1..])
        } else {
            panic!("not implemented");
        }
    }
}
