// packager/src/builder.rs
use std::collections::HashSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use indicatif::{ProgressBar, ProgressStyle};
use setupweaver_common::{InstallConfig, PackagedFile, PackagedInstaller, PACKAGED_MANIFEST_PATH};
use tar::{Builder as TarBuilder, Header};
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
    #[error("failed to build tar payload: {0}")]
    Tar(#[from] std::io::Error),
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

    let manifest = PackagedInstaller {
        config,
        license_text,
        payload: payload.iter().map(|file| file.manifest.clone()).collect(),
    };

    let bar = ProgressBar::new_spinner();
    bar.set_style(ProgressStyle::with_template("{spinner} {msg}").unwrap());
    bar.enable_steady_tick(std::time::Duration::from_millis(80));
    bar.set_message("packing payload");

    let compressed = build_compressed_archive(&manifest, &payload)?;

    bar.set_message("writing setup.exe");

    let selected_stub_path = if manifest.config.install.require_admin {
        stub_admin_path.ok_or(PackagerError::MissingAdminStub)?
    } else {
        stub_path
    };

    let stub = fs::read(selected_stub_path).map_err(|source| PackagerError::ReadStub {
        path: selected_stub_path.to_path_buf(),
        source,
    })?;

    let archive_offset = stub.len() as u64;
    let mut output = Vec::with_capacity(stub.len() + compressed.len() + 8);
    output.extend_from_slice(&stub);
    output.extend_from_slice(&compressed);
    output.extend_from_slice(&archive_offset.to_le_bytes());

    fs::write(output_path, output).map_err(|source| PackagerError::WriteOutput {
        path: output_path.to_path_buf(),
        source,
    })?;

    bar.finish_with_message(format!(
        "built {} (payload: {:.2} MiB compressed)",
        output_path.display(),
        compressed.len() as f64 / (1024.0 * 1024.0)
    ));

    Ok(())
}

#[derive(Debug, Clone)]
struct PayloadSourceFile {
    source_path: PathBuf,
    manifest: PackagedFile,
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
            let archive_path = format!("files/{}", files.len());
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
                manifest: PackagedFile {
                    archive_path,
                    destination,
                    size,
                },
            });
        }
    }

    Ok(files)
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

fn build_compressed_archive(manifest: &PackagedInstaller, files: &[PayloadSourceFile]) -> Result<Vec<u8>, PackagerError> {
    let mut encoder = zstd::Encoder::new(Vec::new(), 19)?;

    {
        let mut tar = TarBuilder::new(&mut encoder);
        let manifest_bytes = toml::to_string_pretty(manifest).expect("manifest serialization must succeed");
        append_bytes(&mut tar, PACKAGED_MANIFEST_PATH, manifest_bytes.as_bytes())?;

        for file in files {
            let bytes = fs::read(&file.source_path).map_err(|source| PackagerError::ReadInputFile {
                path: file.source_path.clone(),
                source,
            })?;
            append_bytes(&mut tar, &file.manifest.archive_path, &bytes)?;
        }

        tar.finish()?;
    }

    Ok(encoder.finish()?)
}

fn append_bytes<W: std::io::Write>(tar: &mut TarBuilder<W>, path: &str, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut header = Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, path, Cursor::new(bytes))?;
    Ok(())
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
