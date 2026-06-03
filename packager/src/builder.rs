// packager/src/builder.rs
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use setupweaver_common::{
    InstallConfig, PackagedChunk, PackagedFile, PackagedInstaller, PAYLOAD_CHUNK_SIZE, PAYLOAD_HEADER_SIZE, PAYLOAD_MAGIC,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PackagerError {
    #[error("failed to read config file {path}: {source}")]
    ReadConfig {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    ParseConfig {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error(transparent)]
    Validation(#[from] setupweaver_common::ValidationErrors),
    #[error("glob pattern is invalid: {0}")]
    InvalidGlobPattern(String),
    #[error("glob produced no files: {0}")]
    EmptyGlob(String),
    #[error("failed to walk glob result {path}: {source}")]
    WalkFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to strip {prefix} from {path}")]
    StripPrefix { prefix: PathBuf, path: PathBuf },
    #[error("failed to read input file {path}: {source}")]
    ReadInputFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read runtime stub {path}: {source}")]
    ReadStub {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("config requires admin, but no admin stub was provided")]
    MissingAdminStub,
    #[error("failed to write output file {path}: {source}")]
    WriteOutput {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to build payload: {0}")]
    PayloadBuild(#[from] std::io::Error),
}

pub fn build_installer(
    config_path: &Path,
    stub_path: &Path,
    stub_admin_path: Option<&Path>,
    output_path: &Path,
) -> Result<(), PackagerError> {
    let config_raw = fs::read_to_string(config_path).map_err(|source| PackagerError::ReadConfig {
        path: config_path.to_path_buf(),
        source,
    })?;
    let config: InstallConfig = InstallConfig::parse(&config_raw).map_err(|source| PackagerError::ParseConfig {
        path: config_path.to_path_buf(),
        source,
    })?;
    config.validate()?;

    let base_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let license_text = load_license_text(base_dir, config.ui.license_file.as_deref())?;
    let payload = collect_payload(&config, base_dir)?;

    let bar = ProgressBar::new_spinner();
    bar.set_style(ProgressStyle::with_template("{spinner} {msg}").unwrap());
    bar.enable_steady_tick(std::time::Duration::from_millis(80));
    bar.set_message("compressing payload chunks");

    let selected_stub_path = if config.install.require_admin {
        stub_admin_path.ok_or(PackagerError::MissingAdminStub)?
    } else {
        stub_path
    };

    bar.set_message("writing setup.exe");

    let output_file = File::create(output_path).map_err(|source| PackagerError::WriteOutput {
        path: output_path.to_path_buf(),
        source,
    })?;
    let mut writer = BufWriter::with_capacity(256 * 1024, output_file);

    // 1. Write runtime stub
    let stub_len = stream_file_to_writer(selected_stub_path, &mut writer)?;
    let archive_offset = stub_len as u64;

    // 2. Compress payload chunks and stream to temp file; collect metadata
    bar.set_message("compressing payload chunks");
    let (chunk_metas, temp_path) = compress_payload_streaming(&payload)?;

    // 3. Build manifest from chunk metadata
    let manifest = build_manifest(config, license_text, &payload, &chunk_metas);
    let manifest_bytes = toml::to_string_pretty(&manifest).expect("manifest serialization must succeed");

    // 4. Write payload header: [magic][manifest_len][manifest_toml]
    writer.write_all(&PAYLOAD_MAGIC)?;
    writer.write_all(&(manifest_bytes.len() as u64).to_le_bytes())?;
    writer.write_all(manifest_bytes.as_bytes())?;

    // 5. Stream compressed chunks from temp file into output
    let payload_data_size = stream_file_to_writer_path(&temp_path, &mut writer)?;

    // 6. Write 8-byte archive offset trailer
    writer.write_all(&archive_offset.to_le_bytes())?;
    writer.flush()?;

    let _ = fs::remove_file(&temp_path);

    let total_payload_size = PAYLOAD_HEADER_SIZE + manifest_bytes.len() + payload_data_size;
    bar.finish_with_message(format!(
        "built {} (payload: {:.2} MiB)",
        output_path.display(),
        total_payload_size as f64 / (1024.0 * 1024.0)
    ));

    Ok(())
}

#[derive(Debug, Clone)]
struct PayloadSourceFile {
    source_path: PathBuf,
    destination: String,
    size: u64,
}

struct ChunkMeta {
    compressed_size: u64,
    uncompressed_size: u64,
}

struct FileMeta {
    chunks: Vec<ChunkMeta>,
}

fn load_license_text(base_dir: &Path, license_file: Option<&str>) -> Result<Option<String>, PackagerError> {
    let Some(license_file) = license_file else {
        return Ok(None);
    };

    let path = base_dir.join(license_file);
    let content = fs::read_to_string(&path).map_err(|source| PackagerError::ReadInputFile {
        path: path.clone(),
        source,
    })?;
    Ok(Some(content))
}

fn collect_payload(config: &InstallConfig, base_dir: &Path) -> Result<Vec<PayloadSourceFile>, PackagerError> {
    let mut files = Vec::new();
    let mut seen_destinations = HashSet::new();

    for spec in &config.files {
        let matches = expand_matches(base_dir, &spec.src, &spec.exclude)?;
        if matches.is_empty() {
            return Err(PackagerError::EmptyGlob(spec.src.clone()));
        }

        let root = glob_root(base_dir, &spec.src);
        for matched_path in matches {
            let relative = relative_entry_path(&root, &matched_path)?;
            let destination = join_destination(&spec.dest, &relative);
            let size = fs::metadata(&matched_path)
                .map_err(|source| PackagerError::WalkFile {
                    path: matched_path.clone(),
                    source,
                })?
                .len();

            if !seen_destinations.insert(destination.clone()) {
                continue;
            }

            files.push(PayloadSourceFile {
                source_path: matched_path,
                destination,
                size,
            });
        }
    }

    Ok(files)
}

/// Compresses all payload files in parallel, streaming compressed chunks to a
/// temp file on disk instead of holding them all in RAM.
///
/// Returns per-file chunk metadata and the path to the temp data file.
fn compress_payload_streaming(
    files: &[PayloadSourceFile],
) -> Result<(Vec<FileMeta>, PathBuf), PackagerError> {
    // First pass: compress each file's chunks in parallel, collect (bytes, meta)
    // per file but write bytes to temp file sequentially to maintain order.
    let per_file_chunks: Vec<Vec<(Vec<u8>, ChunkMeta)>> = files
        .par_iter()
        .map(|file| {
            let input = fs::File::open(&file.source_path).map_err(|source| PackagerError::ReadInputFile {
                path: file.source_path.clone(),
                source,
            })?;
            let mut reader = BufReader::with_capacity(PAYLOAD_CHUNK_SIZE, input);
            let mut buffer = vec![0u8; PAYLOAD_CHUNK_SIZE];
            let mut chunks = Vec::new();

            loop {
                let read = reader.read(&mut buffer).map_err(|source| PackagerError::ReadInputFile {
                    path: file.source_path.clone(),
                    source,
                })?;
                if read == 0 {
                    break;
                }

                let compressed_bytes = zstd::stream::encode_all(std::io::Cursor::new(&buffer[..read]), 19)?;
                let meta = ChunkMeta {
                    compressed_size: compressed_bytes.len() as u64,
                    uncompressed_size: read as u64,
                };
                chunks.push((compressed_bytes, meta));
            }

            Ok(chunks)
        })
        .collect::<Result<Vec<_>, PackagerError>>()?;

    // Second pass: write all compressed data to temp file sequentially.
    let temp_path = output_temp_path();
    let temp_file = File::create(&temp_path)?;
    let mut writer = BufWriter::with_capacity(256 * 1024, temp_file);

    let mut file_metas = Vec::with_capacity(files.len());
    for file_chunks in per_file_chunks {
        let mut metas = Vec::with_capacity(file_chunks.len());
        for (bytes, meta) in file_chunks {
            writer.write_all(&bytes)?;
            metas.push(meta);
        }
        file_metas.push(FileMeta { chunks: metas });
    }
    writer.flush()?;

    Ok((file_metas, temp_path))
}

fn build_manifest(
    config: InstallConfig,
    license_text: Option<String>,
    files: &[PayloadSourceFile],
    file_metas: &[FileMeta],
) -> PackagedInstaller {
    let mut next_offset = 0u64;
    let payload = files
        .iter()
        .zip(file_metas.iter())
        .map(|(file, meta)| {
            let chunks = meta
                .chunks
                .iter()
                .map(|chunk| {
                    let packaged = PackagedChunk {
                        payload_offset: next_offset,
                        compressed_size: chunk.compressed_size,
                        uncompressed_size: chunk.uncompressed_size,
                    };
                    next_offset += packaged.compressed_size;
                    packaged
                })
                .collect();

            PackagedFile {
                destination: file.destination.clone(),
                size: file.size,
                chunks,
            }
        })
        .collect();

    PackagedInstaller {
        config,
        license_text,
        payload,
    }
}

fn stream_file_to_writer(path: &Path, writer: &mut BufWriter<File>) -> Result<usize, PackagerError> {
    let mut input = File::open(path).map_err(|source| PackagerError::ReadStub {
        path: path.to_path_buf(),
        source,
    })?;
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0usize;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        total += read;
    }
    Ok(total)
}

fn stream_file_to_writer_path(path: &Path, writer: &mut BufWriter<File>) -> Result<usize, PackagerError> {
    let mut input = File::open(path)?;
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0usize;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        total += read;
    }
    Ok(total)
}

fn output_temp_path() -> PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("setupweaver-payload-{ts}.tmp"))
}

fn expand_matches(base_dir: &Path, pattern: &str, excludes: &[String]) -> Result<Vec<PathBuf>, PackagerError> {
    let absolute_pattern = base_dir.join(pattern).to_string_lossy().replace('\\', "/");
    let entries = glob::glob(&absolute_pattern).map_err(|_| PackagerError::InvalidGlobPattern(pattern.to_string()))?;

    let exclude_matchers = excludes
        .iter()
        .map(|value| glob::Pattern::new(value).map_err(|_| PackagerError::InvalidGlobPattern(value.clone())))
        .collect::<Result<Vec<_>, _>>()?;

    let mut results = Vec::new();
    for entry in entries.flatten() {
        if entry.is_dir() {
            continue;
        }

        let file_name = entry.file_name().and_then(|value| value.to_str()).unwrap_or_default();
        if exclude_matchers.iter().any(|matcher| matcher.matches(file_name)) {
            continue;
        }

        results.push(entry);
    }

    results.sort();
    Ok(results)
}

fn relative_entry_path(root: &Path, path: &Path) -> Result<PathBuf, PackagerError> {
    if root.is_file() {
        return Ok(path.file_name().map(PathBuf::from).unwrap_or_default());
    }

    path.strip_prefix(root)
        .map(|value| value.to_path_buf())
        .map_err(|_| PackagerError::StripPrefix {
            prefix: root.to_path_buf(),
            path: path.to_path_buf(),
        })
}

fn join_destination(base: &str, relative: &Path) -> String {
    let mut output = base.trim_end_matches(['/', '\\']).to_string();
    let relative = relative.to_string_lossy().replace('\\', "/");
    if !relative.is_empty() {
        if !output.ends_with('/') && !output.ends_with('\\') {
            output.push('/');
        }
        output.push_str(&relative);
    }
    output
}

fn glob_root(base_dir: &Path, pattern: &str) -> PathBuf {
    let mut root = PathBuf::from(base_dir);
    let mut wildcard_seen = false;

    for component in Path::new(pattern).components() {
        let text = component.as_os_str().to_string_lossy();
        if text.contains('*') || text.contains('?') || text.contains('[') {
            wildcard_seen = true;
            break;
        }
        root.push(component.as_os_str());
    }

    if wildcard_seen {
        root
    } else {
        let candidate = base_dir.join(pattern);
        if candidate.is_file() {
            candidate.parent().unwrap_or(base_dir).to_path_buf()
        } else {
            candidate
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PAYLOAD_CHUNK_SIZE;

    #[test]
    fn chunk_constant_is_large_enough_for_good_throughput() {
        assert!(PAYLOAD_CHUNK_SIZE >= 1024 * 1024);
    }
}
