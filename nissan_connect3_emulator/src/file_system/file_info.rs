/// Managed by MountFileSystem
pub struct FileInfo {
    pub file_details: FileDetails,

    pub file_path: String,
    pub inode: u64,
    pub file_status_flags: u32,
}

/// File type
#[derive(Debug, Clone, PartialEq)]
pub enum FileType {
    File,
    Link,
    Directory,
    Socket,
    BlockDevice,
    CharacterDevice,
    NamedPipe,
}

/// Get from file system
pub struct FileDetails {
    pub file_type: FileType,
    pub is_readonly: bool,
    pub length: u64,
}
