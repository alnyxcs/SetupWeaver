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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppSection, FileSpec, InstallConfig, InstallSection, UiSection};

    fn sample_manifest() -> PackagedInstaller {
        PackagedInstaller {
            config: InstallConfig {
                app: AppSection {
                    name: "TestApp".into(),
                    version: "1.0.0".into(),
                    publisher: None,
                    icon: None,
                    description: None,
                },
                install: InstallSection {
                    default_dir: r"{ProgramFiles}\TestApp".into(),
                    add_to_path: false,
                    create_desktop_shortcut: false,
                    require_admin: false,
                },
                ui: UiSection::default(),
                files: vec![FileSpec {
                    src: "bin/*".into(),
                    dest: "{install_dir}".into(),
                    exclude: vec![],
                }],
                shortcuts: vec![],
                registry: vec![],
                run: vec![],
            },
            license_text: None,
            payload: vec![
                PackagedFile {
                    destination: "app.exe".into(),
                    size: 1024,
                    chunks: vec![
                        PackagedChunk {
                            payload_offset: 0,
                            compressed_size: 500,
                            uncompressed_size: 1024,
                        },
                    ],
                },
                PackagedFile {
                    destination: "data.bin".into(),
                    size: 16_000_000,
                    chunks: vec![
                        PackagedChunk {
                            payload_offset: 500,
                            compressed_size: 4_000_000,
                            uncompressed_size: 8_000_000,
                        },
                        PackagedChunk {
                            payload_offset: 4_000_500,
                            compressed_size: 4_000_000,
                            uncompressed_size: 8_000_000,
                        },
                    ],
                },
            ],
        }
    }

    #[test]
    fn manifest_roundtrip_through_toml() {
        let manifest = sample_manifest();
        let serialized = toml::to_string_pretty(&manifest).unwrap();
        let reparsed: PackagedInstaller = toml::from_str(&serialized).unwrap();
        assert_eq!(reparsed.config.app.name, "TestApp");
        assert_eq!(reparsed.payload.len(), 2);
        assert_eq!(reparsed.payload[0].destination, "app.exe");
        assert_eq!(reparsed.payload[0].chunks.len(), 1);
        assert_eq!(reparsed.payload[1].chunks.len(), 2);
    }

    #[test]
    fn payload_offsets_are_contiguous() {
        let manifest = sample_manifest();
        let mut expected_offset = 0u64;
        for file in &manifest.payload {
            for chunk in &file.chunks {
                assert_eq!(chunk.payload_offset, expected_offset);
                expected_offset += chunk.compressed_size;
            }
        }
    }

    #[test]
    fn chunk_size_constant_is_8mb() {
        assert_eq!(PAYLOAD_CHUNK_SIZE, 8 * 1024 * 1024);
    }

    #[test]
    fn payload_magic_is_8_bytes() {
        assert_eq!(PAYLOAD_MAGIC.len(), 8);
        assert_eq!(&PAYLOAD_MAGIC, b"SWPAYLD2");
    }

    #[test]
    fn payload_header_size_holds_magic_plus_manifest_len() {
        assert_eq!(PAYLOAD_HEADER_SIZE, 16);
    }

    #[test]
    fn manifest_with_license_text() {
        let mut manifest = sample_manifest();
        manifest.license_text = Some("MIT License...".into());
        let serialized = toml::to_string_pretty(&manifest).unwrap();
        let reparsed: PackagedInstaller = toml::from_str(&serialized).unwrap();
        assert_eq!(reparsed.license_text.as_deref(), Some("MIT License..."));
    }

    #[test]
    fn empty_payload_serializes() {
        let mut manifest = sample_manifest();
        manifest.payload.clear();
        let serialized = toml::to_string_pretty(&manifest).unwrap();
        let reparsed: PackagedInstaller = toml::from_str(&serialized).unwrap();
        assert!(reparsed.payload.is_empty());
    }

    #[test]
    fn total_compressed_size_matches_offsets() {
        let manifest = sample_manifest();
        let total_compressed: u64 = manifest
            .payload
            .iter()
            .flat_map(|f| &f.chunks)
            .map(|c| c.compressed_size)
            .sum();
        let last_chunk = manifest
            .payload
            .iter()
            .flat_map(|f| &f.chunks)
            .last()
            .unwrap();
        assert_eq!(
            total_compressed,
            last_chunk.payload_offset + last_chunk.compressed_size
        );
    }
}
