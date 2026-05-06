// common/src/state.rs
use serde::{Deserialize, Serialize};

pub const INSTALL_STATE_DIR_NAME: &str = ".setupweaver";
pub const INSTALL_STATE_FILE_NAME: &str = "install-state.toml";
pub const UNINSTALLER_FILE_NAME: &str = "uninstall.exe";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallState {
    pub app_name: String,
    pub app_version: String,
    pub install_dir: String,
    #[serde(default)]
    pub installed_files: Vec<String>,
    #[serde(default)]
    pub shortcuts: Vec<String>,
    #[serde(default)]
    pub registry_values: Vec<InstalledRegistryValue>,
    #[serde(default)]
    pub path_entry: Option<PathEntryState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledRegistryValue {
    pub key: String,
    pub value_name: String,
    #[serde(default)]
    pub previous: Option<RawRegistryValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawRegistryValue {
    pub reg_type: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathEntryState {
    pub entry: String,
    pub system: bool,
}
