/// Managed by MountFileSystem
pub struct FileInfo {
    pub file_details: FileDetails,

    pub filepath: String,
    pub inode: u64,
    pub file_status_flags: u32,
}

/// Get from file system
pub struct FileDetails {
    pub is_file: bool,
    pub is_symlink: bool,
    pub is_dir: bool,
    pub is_readonly: bool,
    pub length: u64,
}
