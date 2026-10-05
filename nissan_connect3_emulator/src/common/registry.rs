use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const DEFAULT_REGISTRY_DIR: &str =
    "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/D605_registry";

const DEFAULT_REGISTRY_FILES: &[&str] = &[
    "base.reg",
    "StartConfigDefault.reg",
    "BuildVersion.reg",
    "procearly.reg",
    "procmw.reg",
    "procmwlx.reg",
    "prochmi.reg",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryValue {
    U32(u32),
    String(String),
}

impl RegistryValue {
    pub fn as_u32(&self) -> Option<u32> {
        match self {
            RegistryValue::U32(value) => Some(*value),
            RegistryValue::String(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            RegistryValue::String(value) => Some(value.as_str()),
            RegistryValue::U32(_) => None,
        }
    }

    pub fn display(&self) -> String {
        match self {
            RegistryValue::U32(value) => format!("dword:{:08x}", value),
            RegistryValue::String(value) => value.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RegistryNode {
    values: BTreeMap<String, RegistryValue>,
    children: BTreeMap<String, RegistryNode>,
}

impl RegistryNode {
    fn ensure_path(&mut self, path: &[String]) -> &mut Self {
        let mut node = self;
        for part in path {
            node = node.children.entry(part.clone()).or_default();
        }
        node
    }

    fn find_path(&self, path: &[String]) -> Option<&Self> {
        let mut node = self;
        for part in path {
            node = node.children.get(part)?;
        }
        Some(node)
    }

    fn collect_values_named(&self, name: &str, out: &mut Vec<RegistryValue>) {
        if let Some(value) = self.values.get(name) {
            out.push(value.clone());
        }
        for child in self.children.values() {
            child.collect_values_named(name, out);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Registry {
    root: RegistryNode,
    loaded_files: Vec<PathBuf>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_default_paths() -> Self {
        let dir = default_registry_dir();
        let files = default_registry_files(&dir);
        let mut registry = Self::new();

        for file in files {
            if let Err(error) = registry.load_file(&file) {
                log::warn!(
                    "registry: cannot load {}: {}",
                    file.display(),
                    error
                );
            }
        }

        registry.load_referenced_process_registries(&dir);
        registry
    }

    pub fn load_files<P: AsRef<Path>>(&mut self, files: impl IntoIterator<Item = P>) -> usize {
        let mut loaded = 0;
        for file in files {
            match self.load_file(file.as_ref()) {
                Ok(entries) => loaded += entries,
                Err(error) => log::warn!(
                    "registry: cannot load {}: {}",
                    file.as_ref().display(),
                    error
                ),
            }
        }
        loaded
    }

    pub fn load_file(&mut self, path: &Path) -> io::Result<usize> {
        let text = fs::read_to_string(path)?;
        let entries = parse_registry_text(&text, &mut self.root);
        if !self.loaded_files.contains(&path.to_path_buf()) {
            self.loaded_files.push(path.to_path_buf());
        }
        log::debug!(
            "registry: loaded {} entries from {}",
            entries,
            path.display()
        );
        Ok(entries)
    }

    pub fn load_referenced_process_registries(&mut self, dir: &Path) -> usize {
        let mut referenced_values = Vec::new();
        self.root
            .collect_values_named("PROC_REGISTRY", &mut referenced_values);

        let mut referenced: Vec<String> = referenced_values
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
        referenced.sort();
        referenced.dedup();

        let mut loaded = 0;
        for file in referenced {
            let file = file.trim();
            if file.is_empty() {
                continue;
            }
            let path = if Path::new(file).is_absolute() {
                PathBuf::from(file)
            } else {
                dir.join(file)
            };

            match self.load_file(&path) {
                Ok(entries) => loaded += entries,
                Err(error) => log::debug!(
                    "registry: referenced file {} unavailable: {}",
                    path.display(),
                    error
                ),
            }
        }

        loaded
    }

    pub fn loaded_files(&self) -> &[PathBuf] {
        &self.loaded_files
    }

    pub fn has_path(&self, path: &str) -> bool {
        self.find_node(path).is_some()
    }

    pub fn find_node(&self, path: &str) -> Option<&RegistryNode> {
        let path = normalize_registry_path(path);
        self.root.find_path(&path)
    }

    pub fn query_u32(&self, path: &str, value: &str) -> Option<u32> {
        self.find_node(path)?.values.get(value)?.as_u32()
    }

    pub fn query_string(&self, path: &str, value: &str) -> Option<String> {
        self.find_node(path)?
            .values
            .get(value)
            .and_then(|value| value.as_str())
            .map(str::to_string)
    }

    pub fn query_value(&self, path: &str, value: &str) -> Option<RegistryValue> {
        self.find_node(path)?.values.get(value).cloned()
    }

    pub fn subkeys(&self, path: &str) -> Vec<String> {
        match self.find_node(path) {
            Some(node) => node.children.keys().cloned().collect(),
            None => Vec::new(),
        }
    }

    pub fn values(&self, path: &str) -> Vec<(String, RegistryValue)> {
        match self.find_node(path) {
            Some(node) => node
                .values
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn set_value(&mut self, path: &str, value: &str, registry_value: RegistryValue) {
        let path = normalize_registry_path(path);
        let node = self.root.ensure_path(&path);
        node.values.insert(value.to_string(), registry_value);
    }

    pub fn set_u32(&mut self, path: &str, value: &str, number: u32) {
        self.set_value(path, value, RegistryValue::U32(number));
    }

    pub fn set_string(&mut self, path: &str, value: &str, text: impl Into<String>) {
        self.set_value(path, value, RegistryValue::String(text.into()));
    }

    pub fn create_key(&mut self, path: &str) -> bool {
        let path = normalize_registry_path(path);
        self.root.ensure_path(&path);
        true
    }

    pub fn remove_value(&mut self, path: &str, value: &str) -> bool {
        let path = normalize_registry_path(path);
        match self.root.find_path_mut(&path) {
            Some(node) => node.values.remove(value).is_some(),
            None => false,
        }
    }
}

impl RegistryNode {
    fn find_path_mut(&mut self, path: &[String]) -> Option<&mut Self> {
        let mut node = self;
        for part in path {
            node = node.children.get_mut(part)?;
        }
        Some(node)
    }
}

pub fn default_registry_dir() -> PathBuf {
    env::var_os("EMU_REGISTRY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_REGISTRY_DIR))
}

pub fn default_registry_files(dir: &Path) -> Vec<PathBuf> {
    if let Ok(files) = env::var("EMU_REGISTRY_FILES") {
        let files: Vec<PathBuf> = files
            .split(';')
            .map(str::trim)
            .filter(|file| !file.is_empty())
            .map(PathBuf::from)
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    dir.join(path)
                }
            })
            .collect();
        if !files.is_empty() {
            return files;
        }
    }

    DEFAULT_REGISTRY_FILES
        .iter()
        .map(|file| dir.join(file))
        .collect()
}

pub fn normalize_registry_path(path: &str) -> Vec<String> {
    let path = path.replace('\\', "/");
    let path = path.trim().trim_end_matches('/');
    let path = if path.starts_with("/dev/registry/") {
        path.trim_start_matches("/dev/registry/")
    } else if path == "/dev/registry" {
        ""
    } else if path.starts_with("dev/registry/") {
        path.trim_start_matches("dev/registry/")
    } else if path == "dev/registry" {
        ""
    } else {
        path
    };

    normalize_hive_path(path)
}

fn normalize_section_path(section: &str) -> Vec<String> {
    let section = section.trim().trim_matches('"').replace('\\', "/");
    normalize_hive_path(section.trim_start_matches('/'))
}

fn normalize_hive_path(path: &str) -> Vec<String> {
    let path = if path.starts_with("HKEY_LOCAL_MACHINE\\") {
        path.replacen("HKEY_LOCAL_MACHINE", "LOCAL_MACHINE", 1)
    } else if path.starts_with("HKEY_LOCAL_MACHINE/") {
        path.replacen("HKEY_LOCAL_MACHINE", "LOCAL_MACHINE", 1)
    } else if path == "HKEY_LOCAL_MACHINE" {
        "LOCAL_MACHINE".to_string()
    } else if path.is_empty() {
        String::new()
    } else {
        path.to_string()
    };

    path.split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_registry_text(text: &str, root: &mut RegistryNode) -> usize {
    let mut section = Vec::new();
    let mut entries = 0;

    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim_start().is_empty() || line.trim_start().starts_with(';') {
            continue;
        }

        if let Some((next_section, is_section)) = parse_section_line(line) {
            if is_section {
                section = next_section;
                root.ensure_path(&section);
            }
            continue;
        }

        if section.is_empty() {
            continue;
        }

        if let Some((key, value)) = parse_key_value_line(line) {
            root.ensure_path(&section).values.insert(key, value);
            entries += 1;
        }
    }

    entries
}

fn parse_section_line(line: &str) -> Option<(Vec<String>, bool)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('[') {
        return None;
    }

    let end = trimmed.rfind(']')?;
    let name = &trimmed[1..end];
    Some((normalize_section_path(name), true))
}

fn parse_key_value_line(line: &str) -> Option<(String, RegistryValue)> {
    let line = line.trim();
    let (key, rest) = if line.starts_with('"') {
        let key_end = line[1..].find('"')?;
        let key = &line[1..1 + key_end];
        (key.to_string(), &line[key_end + 2..])
    } else {
        let key_end = line.find('=')?;
        (line[..key_end].to_string(), &line[key_end..])
    };

    let key = key.trim().trim_matches('"').to_string();
    if key.is_empty() {
        return None;
    }

    let mut value = rest.trim_start().strip_prefix('=')?;
    if value.starts_with('=') {
        value = &value[1..];
    }

    let raw_value = value.trim();
    parse_registry_value(raw_value).map(|value| (key, value))
}

fn parse_registry_value(raw_value: &str) -> Option<RegistryValue> {
    let raw_value = raw_value.trim();
    if raw_value.is_empty() {
        return None;
    }

    if raw_value.starts_with('"') {
        if let Some(text) = parse_quoted_string(raw_value) {
            return Some(RegistryValue::String(text));
        }
    }

    if raw_value.starts_with("dword:") {
        let number = raw_value
            .trim_start_matches("dword:")
            .trim()
            .split_whitespace()
            .next()
            .unwrap_or("");
        if let Ok(number) = u32::from_str_radix(number, 16) {
            return Some(RegistryValue::U32(number));
        }
    }

    Some(RegistryValue::String(strip_inline_comment(raw_value).to_string()))
}

fn parse_quoted_string(value: &str) -> Option<String> {
    let mut text = String::new();
    let mut escaped = false;

    for ch in value.chars().skip(1) {
        if escaped {
            text.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '"' => return Some(text),
            _ => text.push(ch),
        }
    }

    None
}

fn strip_inline_comment(value: &str) -> &str {
    let mut previous_was_whitespace = false;
    for (index, ch) in value.char_indices() {
        if ch == ';' && (index == 0 || previous_was_whitespace) {
            return value[..index].trim_end();
        }
        previous_was_whitespace = ch.is_whitespace();
    }

    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_registry_text_into_values() {
        let text = r#"REGEDIT4

[HKEY_LOCAL_MACHINE\SOFTWARE\BLAUPUNKT]
"MAXPOOLSIZE"=dword:000FA000

[HKEY_LOCAL_MACHINE\SOFTWARE\BLAUPUNKT\PROCESS\LBASE\SPMSLV]
"APPID"=dword:0000001c           ;decimal value --> 28
"SERVICEID"="0x5f"
"#;
        let mut registry = Registry::new();
        assert_eq!(parse_registry_text(text, &mut registry.root), 3);
        assert_eq!(
            registry.query_u32("/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT", "MAXPOOLSIZE"),
            Some(0x000f_a000)
        );
        assert_eq!(
            registry.query_u32(
                "/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/PROCESS/LBASE/SPMSLV",
                "APPID"
            ),
            Some(0x1c)
        );
        assert_eq!(
            registry.query_string(
                "/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/PROCESS/LBASE/SPMSLV",
                "SERVICEID"
            )
            .as_deref(),
            Some("0x5f")
        );
    }

    #[test]
    fn runtime_writes_do_not_affect_parser_source() {
        let mut registry = Registry::new();
        registry.set_u32("/dev/registry/LOCAL_MACHINE/RUNTIME", "PROCSTARTED", 1);
        assert_eq!(
            registry.query_u32("/dev/registry/LOCAL_MACHINE/RUNTIME", "PROCSTARTED"),
            Some(1)
        );
        assert!(registry.loaded_files().is_empty());
    }
}
