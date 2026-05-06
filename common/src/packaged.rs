// common/src/packaged.rs
use serde::{Deserialize, Serialize};

use crate::InstallConfig;

pub const PAYLOAD_MAGIC: [u8; 8] = *b"SWPAYLD2";
pub const PAYLOAD_HEADER_SIZE: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackagedInstaller {
    pub config: InstallConfig,
    #[serde(default)]
    pub license_text: Option<String>,
    pub payload: Vec<PackagedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackagedFile {
    pub payload_offset: u64,
    pub compressed_size: u64,
    pub destination: String,
    pub size: u64,
}
