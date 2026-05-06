// common/src/packaged.rs
use serde::{Deserialize, Serialize};

use crate::InstallConfig;

pub const PAYLOAD_MAGIC: [u8; 8] = *b"SWPAYLD2";
pub const PAYLOAD_HEADER_SIZE: usize = 16;
pub const PAYLOAD_CHUNK_SIZE: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackagedInstaller {
    pub config: InstallConfig,
    #[serde(default)]
    pub license_text: Option<String>,
    pub payload: Vec<PackagedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackagedFile {
    pub destination: String,
    pub size: u64,
    #[serde(default)]
    pub chunks: Vec<PackagedChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackagedChunk {
    pub payload_offset: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
}
