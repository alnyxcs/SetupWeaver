// runtime/src/payload.rs
use std::fs::File;
use std::ops::Range;
use std::path::{Path, PathBuf};

use memmap2::Mmap;
use setupweaver_common::{PackagedChunk, PackagedFile, PackagedInstaller, PAYLOAD_HEADER_SIZE, PAYLOAD_MAGIC};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PayloadError {
    #[error("failed to open installer binary {path}: {source}")]
    OpenExe {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to memory-map installer binary {path}: {source}")]
    MapExe {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("installer binary is too small to contain a payload trailer")]
    MissingTrailer,
    #[error("payload offset {offset} is outside installer size {file_len}")]
    InvalidOffset { offset: u64, file_len: usize },
    #[error("payload is smaller than the indexed header")]
    InvalidHeader,
    #[error("payload magic is invalid")]
    InvalidMagic,
    #[error("manifest length {manifest_len} exceeds payload size {payload_len}")]
    InvalidManifestLength { manifest_len: u64, payload_len: usize },
    #[error("failed to parse packaged manifest: {0}")]
    ManifestParse(#[from] toml::de::Error),
    #[error("payload slice for {destination} chunk {chunk_index} is outside the embedded payload")]
    ChunkOutOfBounds { destination: String, chunk_index: usize },
}

pub struct EmbeddedPayload {
    exe_path: PathBuf,
    mmap: Mmap,
    payload_range: Range<usize>,
}

impl EmbeddedPayload {
    pub fn from_current_exe() -> Result<Self, PayloadError> {
        let exe_path = std::env::current_exe().map_err(|source| PayloadError::OpenExe {
            path: PathBuf::from("<current_exe>"),
            source,
        })?;
        Self::from_exe(&exe_path)
    }

    pub fn from_exe(path: &Path) -> Result<Self, PayloadError> {
        let file = File::open(path).map_err(|source| PayloadError::OpenExe {
            path: path.to_path_buf(),
            source,
        })?;

        // SAFETY: The file descriptor stays alive for the duration of mmap creation,
        // and the returned Mmap owns the mapping independently of File afterwards.
        let mmap = unsafe { Mmap::map(&file) }.map_err(|source| PayloadError::MapExe {
            path: path.to_path_buf(),
            source,
        })?;

        if mmap.len() < 8 {
            return Err(PayloadError::MissingTrailer);
        }

        let offset_index = mmap.len() - 8;
        let offset = u64::from_le_bytes(mmap[offset_index..].try_into().expect("slice length is fixed"));
        if offset as usize > offset_index {
            return Err(PayloadError::InvalidOffset {
                offset,
                file_len: mmap.len(),
            });
        }

        Ok(Self {
            exe_path: path.to_path_buf(),
            mmap,
            payload_range: offset as usize..offset_index,
        })
    }

    pub fn exe_path(&self) -> &Path {
        &self.exe_path
    }

    pub fn read_manifest(&self) -> Result<PackagedInstaller, PayloadError> {
        let manifest_range = self.manifest_range()?;
        Ok(toml::from_str(std::str::from_utf8(&self.payload_bytes()[manifest_range]).map_err(|_| PayloadError::InvalidHeader)?)?)
    }

    pub fn payload_chunk_bytes(
        &self,
        file: &PackagedFile,
        chunk: &PackagedChunk,
        chunk_index: usize,
    ) -> Result<&[u8], PayloadError> {
        let payload = self.payload_bytes();
        let data_start = self.data_start()?;
        let start = data_start
            .checked_add(chunk.payload_offset as usize)
            .ok_or_else(|| PayloadError::ChunkOutOfBounds {
                destination: file.destination.clone(),
                chunk_index,
            })?;
        let end = start
            .checked_add(chunk.compressed_size as usize)
            .ok_or_else(|| PayloadError::ChunkOutOfBounds {
                destination: file.destination.clone(),
                chunk_index,
            })?;

        if end > payload.len() {
            return Err(PayloadError::ChunkOutOfBounds {
                destination: file.destination.clone(),
                chunk_index,
            });
        }

        Ok(&payload[start..end])
    }

    fn payload_bytes(&self) -> &[u8] {
        &self.mmap[self.payload_range.clone()]
    }

    fn manifest_range(&self) -> Result<Range<usize>, PayloadError> {
        let payload = self.payload_bytes();
        if payload.len() < PAYLOAD_HEADER_SIZE {
            return Err(PayloadError::InvalidHeader);
        }
        if payload[..PAYLOAD_MAGIC.len()] != PAYLOAD_MAGIC {
            return Err(PayloadError::InvalidMagic);
        }

        let manifest_len = u64::from_le_bytes(
            payload[PAYLOAD_MAGIC.len()..PAYLOAD_HEADER_SIZE]
                .try_into()
                .expect("slice length is fixed"),
        );
        let manifest_end = PAYLOAD_HEADER_SIZE
            .checked_add(manifest_len as usize)
            .ok_or(PayloadError::InvalidManifestLength {
                manifest_len,
                payload_len: payload.len(),
            })?;

        if manifest_end > payload.len() {
            return Err(PayloadError::InvalidManifestLength {
                manifest_len,
                payload_len: payload.len(),
            });
        }

        Ok(PAYLOAD_HEADER_SIZE..manifest_end)
    }

    fn data_start(&self) -> Result<usize, PayloadError> {
        Ok(self.manifest_range()?.end)
    }
}
