// runtime/src/engine.rs
use std::fs::{self, File};
use std::io::{BufWriter, Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
#[cfg(windows)]
use std::time::{SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use setupweaver_common::{
    InstallState, InstalledRegistryValue, PackagedFile, PackagedInstaller, PathEntryState, RunWhen,
    INSTALL_STATE_DIR_NAME, INSTALL_STATE_FILE_NAME, UNINSTALLER_FILE_NAME,
};
#[cfg(windows)]
use setupweaver_common::{RawRegistryValue, ShortcutSpec};
use thiserror::Error;

use crate::payload::{EmbeddedPayload, PayloadError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPhase {
    Preparing,
    Extracting,
    Configuring,
    Finishing,
}

#[derive(Debug, Clone)]
pub struct InstallProgress {
    pub phase: InstallPhase,
    pub progress: f32,
    pub status: String,
    pub detail: String,
    pub completed_files: usize,
    pub total_files: usize,
    pub completed_bytes: u64,
    pub total_bytes: u64,
}

impl InstallProgress {
    fn new(
        phase: InstallPhase,
        progress: f32,
        status: impl Into<String>,
        detail: impl Into<String>,
        completed_files: usize,
        total_files: usize,
        completed_bytes: u64,
        total_bytes: u64,
    ) -> Self {
        Self {
            phase,
            progress: progress.clamp(0.0, 1.0),
            status: status.into(),
            detail: detail.into(),
            completed_files,
            total_files,
            completed_bytes,
            total_bytes,
        }
    }
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Payload(#[from] PayloadError),
    #[error("failed to create directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to create output file {path}: {source}")]
    CreateFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write output file {path}: {source}")]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("decoded chunk size mismatch for {path} chunk {chunk_index}: expected {expected} bytes, got {actual}")]
    ChunkSizeMismatch {
        path: PathBuf,
        chunk_index: usize,
        expected: u64,
        actual: u64,
    },
    #[error("failed to resolve path template {template}: {reason}")]
    ResolvePath { template: String, reason: String },
    #[error("failed to launch post-install command {program}: {source}")]
    LaunchCommand {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid argument string {value}: {reason}")]
    InvalidArguments { value: String, reason: String },
    #[error("failed to read embedded archive: {0}")]
    Archive(#[from] std::io::Error),
    #[error("failed to read install state {path}: {source}")]
    ReadState {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse install state {path}: {source}")]
    ParseState {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to write install state {path}: {source}")]
    WriteState {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to copy uninstall helper to {path}: {source}")]
    CopyUninstaller {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("no install state found in {0}")]
    MissingInstallState(PathBuf),
    #[error("install directory already contains a different app: {found_app}")]
    InstallConflict { found_app: String },
    #[cfg(windows)]
    #[error("registry operation failed for {key} ({value_name}): {source}")]
    RegistryIo {
        key: String,
        value_name: String,
        #[source]
        source: std::io::Error,
    },
    #[cfg(windows)]
    #[error("unsupported registry hive in key {0}")]
    UnsupportedRegistryHive(String),
    #[cfg(windows)]
    #[error("failed to create shortcut {path}: {source}")]
    CreateShortcut {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[cfg(windows)]
    #[error("shortcut helper failed for {path} with exit code {code:?}")]
    ShortcutToolFailed { path: PathBuf, code: Option<i32> },
    #[cfg(windows)]
    #[error("failed to broadcast environment change: {0}")]
    BroadcastEnvironmentChange(std::io::Error),
}

pub struct InstallerEngine {
    payload: EmbeddedPayload,
    manifest: PackagedInstaller,
}

impl InstallerEngine {
    pub fn from_current_exe() -> Result<Self, EngineError> {
        let payload = EmbeddedPayload::from_current_exe()?;
        let manifest = payload.read_manifest()?;
        Ok(Self { payload, manifest })
    }

    pub fn manifest(&self) -> &PackagedInstaller {
        &self.manifest
    }

    pub fn default_install_dir(&self) -> Result<PathBuf, EngineError> {
        let context = InstallContext::for_manifest(&self.manifest, None)?;
        Ok(context.install_dir)
    }

    pub fn install(&self, install_dir: Option<&Path>) -> Result<(), EngineError> {
        let context = InstallContext::for_manifest(&self.manifest, install_dir)?;
        self.ensure_install_target_available(&context)?;
        let plans = self.build_extract_plans(&context)?;
        let mut rollback = self.extract_files_parallel(&plans)?;
        let metadata = match self.configure_system(&context, &mut rollback) {
            Ok(metadata) => metadata,
            Err(error) => {
                rollback.rollback();
                return Err(error);
            }
        };
        if let Err(error) = self.persist_install_state(&context, &plans, &metadata, &mut rollback) {
            rollback.rollback();
            return Err(error);
        }
        if let Err(error) = self.run_hooks(&context, RunWhen::After) {
            rollback.rollback();
            return Err(error);
        }
        Ok(())
    }

    pub fn install_with_progress<F>(&self, install_dir: Option<&Path>, mut progress: F) -> Result<(), EngineError>
    where
        F: FnMut(InstallProgress),
    {
        let context = InstallContext::for_manifest(&self.manifest, install_dir)?;
        self.ensure_install_target_available(&context)?;
        let plans = self.build_extract_plans(&context)?;
        let total_files = self.manifest.payload.len();
        let total_bytes = self.manifest.payload.iter().map(|file| file.size).sum();

        progress(InstallProgress::new(
            InstallPhase::Preparing,
            0.02,
            "Preparing installer",
            context.install_dir.display().to_string(),
            0,
            total_files,
            0,
            total_bytes,
        ));

        let (mut rollback, completed_files, completed_bytes) =
            self.extract_files_with_progress(&plans, &mut progress, total_files, total_bytes)?;
        let metadata = match self.configure_system_with_progress(
            &context,
            &mut progress,
            &mut rollback,
            completed_files,
            completed_bytes,
            total_files,
            total_bytes,
        ) {
            Ok(metadata) => metadata,
            Err(error) => {
                rollback.rollback();
                return Err(error);
            }
        };
        progress(InstallProgress::new(
            InstallPhase::Configuring,
            0.985,
            "Writing uninstall data",
            context.install_dir.display().to_string(),
            completed_files,
            total_files,
            completed_bytes,
            total_bytes,
        ));
        if let Err(error) = self.persist_install_state(&context, &plans, &metadata, &mut rollback) {
            rollback.rollback();
            return Err(error);
        }
        progress(InstallProgress::new(
            InstallPhase::Finishing,
            0.99,
            "Starting post-install tasks",
            self.manifest.config.app.name.clone(),
            completed_files,
            total_files,
            completed_bytes,
            total_bytes,
        ));
        if let Err(error) = self.run_hooks(&context, RunWhen::After) {
            rollback.rollback();
            return Err(error);
        }
        Ok(())
    }

    pub fn finish(&self, install_dir: Option<&Path>) -> Result<(), EngineError> {
        let context = InstallContext::for_manifest(&self.manifest, install_dir)?;
        self.run_hooks(&context, RunWhen::Finish)
    }

    pub fn uninstall(&self, install_dir: Option<&Path>) -> Result<(), EngineError> {
        let install_dir = resolve_uninstall_dir(install_dir, self.payload.exe_path(), self.default_install_dir()?.as_path())?;
        let state = load_install_state(&install_dir)?;
        self.uninstall_from_state(&state, self.payload.exe_path())
    }

    fn build_extract_plans(&self, context: &InstallContext) -> Result<Vec<ExtractPlan>, EngineError> {
        self.manifest
            .payload
            .iter()
            .map(|packaged| {
                let output_path = resolve_template(&packaged.destination, context)?;
                Ok(ExtractPlan {
                    packaged: packaged.clone(),
                    output_path: output_path.clone(),
                    file_existed: output_path.exists(),
                    created_dirs: missing_parent_dirs(output_path.parent()),
                })
            })
            .collect()
    }

    fn extract_files_parallel(&self, plans: &[ExtractPlan]) -> Result<InstallRollback, EngineError> {
        let rollback = Mutex::new(InstallRollback::default());
        let result = plans.par_iter().try_for_each(|plan| {
            self.write_extract_plan(plan, true)?;
            rollback.lock().expect("rollback mutex poisoned").record_extraction(plan);
            Ok(())
        });

        let rollback = rollback.into_inner().expect("rollback mutex poisoned");
        if let Err(error) = result {
            rollback.rollback();
            return Err(error);
        }

        Ok(rollback)
    }

    fn extract_files_with_progress(
        &self,
        plans: &[ExtractPlan],
        progress: &mut dyn FnMut(InstallProgress),
        total_files: usize,
        total_bytes: u64,
    ) -> Result<(InstallRollback, usize, u64), EngineError> {
        let mut rollback = InstallRollback::default();
        let mut completed_files = 0usize;
        let mut completed_bytes = 0u64;

        for plan in plans {
            progress(InstallProgress::new(
                InstallPhase::Extracting,
                extraction_progress(completed_bytes, total_bytes),
                format!("Extracting {}", file_label(&plan.output_path)),
                plan.output_path.display().to_string(),
                completed_files,
                total_files,
                completed_bytes,
                total_bytes,
            ));

            if let Err(error) = self.write_extract_plan(plan, false) {
                rollback.rollback();
                return Err(error);
            }
            rollback.record_extraction(plan);

            completed_files += 1;
            completed_bytes = completed_bytes.saturating_add(plan.packaged.size);
            progress(InstallProgress::new(
                InstallPhase::Extracting,
                extraction_progress(completed_bytes, total_bytes),
                format!("Installed {}", file_label(&plan.output_path)),
                plan.output_path.display().to_string(),
                completed_files,
                total_files,
                completed_bytes,
                total_bytes,
            ));
        }

        Ok((rollback, completed_files, completed_bytes))
    }

    fn write_extract_plan(&self, plan: &ExtractPlan, allow_parallel_chunks: bool) -> Result<(), EngineError> {
        let result = if allow_parallel_chunks && plan.packaged.chunks.len() > 1 {
            self.write_payload_file_parallel(&plan.packaged, &plan.output_path)
        } else {
            self.write_payload_file_sequential(&plan.packaged, &plan.output_path)
        };

        if let Err(error) = result {
            if !plan.file_existed {
                let _ = fs::remove_file(&plan.output_path);
            }
            return Err(error);
        }

        Ok(())
    }

    fn write_payload_file_sequential(&self, packaged: &PackagedFile, output_path: &Path) -> Result<(), EngineError> {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|source| EngineError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let file = File::create(output_path).map_err(|source| EngineError::CreateFile {
            path: output_path.to_path_buf(),
            source,
        })?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, file);
        for (chunk_index, chunk) in packaged.chunks.iter().enumerate() {
            let compressed = self.payload.payload_chunk_bytes(packaged, chunk, chunk_index)?;
            let mut decoder = zstd::stream::read::Decoder::new(Cursor::new(compressed))?;
            std::io::copy(&mut decoder, &mut writer).map_err(|source| EngineError::WriteFile {
                path: output_path.to_path_buf(),
                source,
            })?;
        }
        Ok(())
    }

    fn write_payload_file_parallel(&self, packaged: &PackagedFile, output_path: &Path) -> Result<(), EngineError> {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|source| EngineError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let file = Arc::new(File::create(output_path).map_err(|source| EngineError::CreateFile {
            path: output_path.to_path_buf(),
            source,
        })?);
        file.set_len(packaged.size).map_err(|source| EngineError::WriteFile {
            path: output_path.to_path_buf(),
            source,
        })?;

        let chunk_offsets = chunk_output_offsets(packaged);
        packaged
            .chunks
            .par_iter()
            .enumerate()
            .try_for_each(|(chunk_index, chunk)| {
                let compressed = self.payload.payload_chunk_bytes(packaged, chunk, chunk_index)?;
                let mut decoder = zstd::stream::read::Decoder::new(Cursor::new(compressed))?;
                let mut decoded = Vec::with_capacity(chunk.uncompressed_size as usize);
                decoder.read_to_end(&mut decoded).map_err(EngineError::Archive)?;
                if decoded.len() as u64 != chunk.uncompressed_size {
                    return Err(EngineError::ChunkSizeMismatch {
                        path: output_path.to_path_buf(),
                        chunk_index,
                        expected: chunk.uncompressed_size,
                        actual: decoded.len() as u64,
                    });
                }

                write_all_at(&file, chunk_offsets[chunk_index], &decoded).map_err(|source| EngineError::WriteFile {
                    path: output_path.to_path_buf(),
                    source,
                })
            })
    }

    fn configure_system(&self, context: &InstallContext, rollback: &mut InstallRollback) -> Result<InstallMetadata, EngineError> {
        let registry_values = self.apply_registry(context, rollback)?;
        let path_entry = self.update_path(context, rollback)?;
        let shortcuts = self.create_shortcuts(context, rollback)?;
        Ok(InstallMetadata {
            registry_values,
            path_entry,
            shortcuts,
        })
    }

    fn configure_system_with_progress(
        &self,
        context: &InstallContext,
        progress: &mut dyn FnMut(InstallProgress),
        rollback: &mut InstallRollback,
        completed_files: usize,
        completed_bytes: u64,
        total_files: usize,
        total_bytes: u64,
    ) -> Result<InstallMetadata, EngineError> {
        progress(InstallProgress::new(
            InstallPhase::Configuring,
            0.90,
            "Writing registry entries",
            self.manifest.config.app.name.clone(),
            completed_files,
            total_files,
            completed_bytes,
            total_bytes,
        ));
        let registry_values = self.apply_registry(context, rollback)?;

        progress(InstallProgress::new(
            InstallPhase::Configuring,
            0.94,
            "Updating PATH",
            context.install_dir.display().to_string(),
            completed_files,
            total_files,
            completed_bytes,
            total_bytes,
        ));
        let path_entry = self.update_path(context, rollback)?;

        progress(InstallProgress::new(
            InstallPhase::Configuring,
            0.97,
            "Creating shortcuts",
            context.desktop.display().to_string(),
            completed_files,
            total_files,
            completed_bytes,
            total_bytes,
        ));
        let shortcuts = self.create_shortcuts(context, rollback)?;

        Ok(InstallMetadata {
            registry_values,
            path_entry,
            shortcuts,
        })
    }

    fn ensure_install_target_available(&self, context: &InstallContext) -> Result<(), EngineError> {
        if !install_state_path(&context.install_dir).exists() {
            return Ok(());
        }

        let state = load_install_state(&context.install_dir)?;
        if state.app_name == self.manifest.config.app.name {
            return Err(EngineError::InstallConflict {
                found_app: format!("{} {}", state.app_name, state.app_version),
            });
        }

        Err(EngineError::InstallConflict {
            found_app: state.app_name,
        })
    }

    fn persist_install_state(
        &self,
        context: &InstallContext,
        plans: &[ExtractPlan],
        metadata: &InstallMetadata,
        rollback: &mut InstallRollback,
    ) -> Result<(), EngineError> {
        let state_dir = install_state_dir(&context.install_dir);
        fs::create_dir_all(&state_dir).map_err(|source| EngineError::CreateDir {
            path: state_dir.clone(),
            source,
        })?;

        let uninstaller_path = uninstaller_path(&context.install_dir);
        let uninstaller_existed = uninstaller_path.exists();
        fs::copy(self.payload.exe_path(), &uninstaller_path).map_err(|source| EngineError::CopyUninstaller {
            path: uninstaller_path.clone(),
            source,
        })?;
        if !uninstaller_existed {
            rollback.record_created_file(uninstaller_path.clone());
        }

        let state = InstallState {
            app_name: self.manifest.config.app.name.clone(),
            app_version: self.manifest.config.app.version.clone(),
            install_dir: context.install_dir.display().to_string(),
            installed_files: plans
                .iter()
                .map(|plan| plan.output_path.display().to_string())
                .collect(),
            shortcuts: metadata
                .shortcuts
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            registry_values: metadata.registry_values.clone(),
            path_entry: metadata.path_entry.clone(),
        };

        let state_path = install_state_path(&context.install_dir);
        let state_text = toml::to_string_pretty(&state).expect("state serialization must succeed");
        fs::write(&state_path, state_text).map_err(|source| EngineError::WriteState {
            path: state_path.clone(),
            source,
        })?;
        rollback.record_created_file(state_path);
        Ok(())
    }

    fn uninstall_from_state(&self, state: &InstallState, current_exe: &Path) -> Result<(), EngineError> {
        let install_dir = PathBuf::from(&state.install_dir);
        for shortcut in &state.shortcuts {
            let _ = fs::remove_file(shortcut);
        }

        for path in &state.installed_files {
            let _ = fs::remove_file(path);
        }

        restore_registry_values(&state.registry_values)?;
        restore_path_entry(state.path_entry.as_ref())?;

        let state_path = install_state_path(&install_dir);
        let helper_path = uninstaller_path(&install_dir);
        let state_dir = install_state_dir(&install_dir);

        if current_exe == helper_path {
            schedule_self_delete(current_exe, &state_path, &state_dir)?;
        } else {
            let _ = fs::remove_file(&helper_path);
            let _ = fs::remove_file(&state_path);
            let _ = fs::remove_dir(&state_dir);
        }

        let _ = fs::remove_dir(&install_dir);
        Ok(())
    }

    fn run_hooks(&self, context: &InstallContext, when: RunWhen) -> Result<(), EngineError> {
        for hook in &self.manifest.config.run {
            if hook.when != when {
                continue;
            }

            let program = resolve_template(&hook.cmd, context)?;
            let mut command = Command::new(&program);
            let resolved_args = resolve_arg_tokens(&hook.args, context);
            for arg in parse_windows_args(&resolved_args)? {
                command.arg(arg);
            }
            command.spawn().map_err(|source| EngineError::LaunchCommand {
                program: program.display().to_string(),
                source,
            })?;
        }

        Ok(())
    }

    #[cfg(not(windows))]
    fn apply_registry(&self, _context: &InstallContext, _rollback: &mut InstallRollback) -> Result<Vec<InstalledRegistryValue>, EngineError> {
        Ok(Vec::new())
    }

    #[cfg(windows)]
    fn apply_registry(&self, context: &InstallContext, rollback: &mut InstallRollback) -> Result<Vec<InstalledRegistryValue>, EngineError> {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        use winreg::RegKey;

        let mut installed = Vec::new();
        for entry in &self.manifest.config.registry {
            let (hive_name, subkey) = split_registry_key(&entry.key)
                .ok_or_else(|| EngineError::UnsupportedRegistryHive(entry.key.clone()))?;

            let root = match hive_name {
                "HKCU" => RegKey::predef(HKEY_CURRENT_USER),
                "HKLM" => RegKey::predef(HKEY_LOCAL_MACHINE),
                _ => return Err(EngineError::UnsupportedRegistryHive(entry.key.clone())),
            };

            let (target_key, _) = root.create_subkey(subkey).map_err(|source| EngineError::RegistryIo {
                key: entry.key.clone(),
                value_name: String::from("<create_subkey>"),
                source,
            })?;

            for value in &entry.values {
                let previous = match target_key.get_raw_value(&value.name) {
                    Ok(raw) => Some(RawRegistryValue {
                        reg_type: raw.vtype as u32,
                        bytes: raw.bytes,
                    }),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(source) => {
                        return Err(EngineError::RegistryIo {
                            key: entry.key.clone(),
                            value_name: value.name.clone(),
                            source,
                        });
                    }
                };

                let change = InstalledRegistryValue {
                    key: entry.key.clone(),
                    value_name: value.name.clone(),
                    previous,
                };
                match value.value_type {
                    setupweaver_common::RegistryValueType::String => target_key
                        .set_value(&value.name, &resolve_arg_tokens(&value.data, context))
                        .map_err(|source| EngineError::RegistryIo {
                            key: entry.key.clone(),
                            value_name: value.name.clone(),
                            source,
                        })?,
                    setupweaver_common::RegistryValueType::Dword => {
                        let parsed: u32 = resolve_arg_tokens(&value.data, context).parse().map_err(|_| EngineError::ResolvePath {
                            template: value.data.clone(),
                            reason: String::from("DWORD data must parse as u32"),
                        })?;
                        target_key.set_value(&value.name, &parsed).map_err(|source| EngineError::RegistryIo {
                            key: entry.key.clone(),
                            value_name: value.name.clone(),
                            source,
                        })?;
                    }
                    setupweaver_common::RegistryValueType::Qword => {
                        let parsed: u64 = resolve_arg_tokens(&value.data, context).parse().map_err(|_| EngineError::ResolvePath {
                            template: value.data.clone(),
                            reason: String::from("QWORD data must parse as u64"),
                        })?;
                        target_key.set_value(&value.name, &parsed).map_err(|source| EngineError::RegistryIo {
                            key: entry.key.clone(),
                            value_name: value.name.clone(),
                            source,
                        })?;
                    }
                }
                rollback.record_registry_change(change.clone());
                installed.push(change);
            }
        }

        Ok(installed)
    }

    #[cfg(not(windows))]
    fn update_path(&self, _context: &InstallContext, _rollback: &mut InstallRollback) -> Result<Option<PathEntryState>, EngineError> {
        Ok(None)
    }

    #[cfg(windows)]
    fn update_path(&self, context: &InstallContext, rollback: &mut InstallRollback) -> Result<Option<PathEntryState>, EngineError> {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_EXPAND_SZ};
        use winreg::{RegKey, RegValue};

        if !self.manifest.config.install.add_to_path {
            return Ok(None);
        }

        let (key_name, root, subkey, system) = if self.manifest.config.install.require_admin {
            (
                String::from("HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment"),
                RegKey::predef(HKEY_LOCAL_MACHINE),
                "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment",
                true,
            )
        } else {
            (String::from("HKCU\\Environment"), RegKey::predef(HKEY_CURRENT_USER), "Environment", false)
        };

        let (env_key, _) = root.create_subkey(subkey).map_err(|source| EngineError::RegistryIo {
            key: key_name.clone(),
            value_name: String::from("Path"),
            source,
        })?;

        let current = match env_key.get_value::<String, _>("Path") {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(EngineError::RegistryIo {
                    key: key_name.clone(),
                    value_name: String::from("Path"),
                    source,
                });
            }
        };

        let install_dir = context.install_dir.display().to_string();
        let updated = append_path_entry(&current, &install_dir);
        if updated == current {
            return Ok(None);
        }

        let value = RegValue {
            bytes: encode_utf16_nul(&updated),
            vtype: REG_EXPAND_SZ,
        };
        env_key.set_raw_value("Path", &value).map_err(|source| EngineError::RegistryIo {
            key: key_name.clone(),
            value_name: String::from("Path"),
            source,
        })?;
        rollback.record_path_restore(PathRestore {
            key: key_name,
            previous: current,
        });

        broadcast_environment_change()?;
        Ok(Some(PathEntryState {
            entry: install_dir,
            system,
        }))
    }

    #[cfg(not(windows))]
    fn create_shortcuts(&self, _context: &InstallContext, _rollback: &mut InstallRollback) -> Result<Vec<PathBuf>, EngineError> {
        Ok(Vec::new())
    }

    #[cfg(windows)]
    fn create_shortcuts(&self, context: &InstallContext, rollback: &mut InstallRollback) -> Result<Vec<PathBuf>, EngineError> {
        let mut shortcuts = Vec::new();
        let mut created_paths = Vec::new();

        if self.manifest.config.shortcuts.is_empty() && self.manifest.config.install.create_desktop_shortcut {
            shortcuts.push(ShortcutSpec {
                name: self.manifest.config.app.name.clone(),
                target: format!("{{install_dir}}\\{}.exe", self.manifest.config.app.name),
                args: String::new(),
                icon: String::new(),
            });
        } else {
            shortcuts.extend(self.manifest.config.shortcuts.iter().cloned());
        }

        for shortcut in shortcuts {
            let target = resolve_template(&shortcut.target, context)?;
            let icon = if shortcut.icon.trim().is_empty() {
                target.clone()
            } else {
                resolve_template(&shortcut.icon, context)?
            };
            let link_path = context.desktop.join(shortcut_file_name(&shortcut.name));

            if let Some(parent) = link_path.parent() {
                fs::create_dir_all(parent).map_err(|source| EngineError::CreateDir {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }

            create_shortcut_via_wsh(
                &link_path,
                &target,
                &resolve_arg_tokens(&shortcut.args, context),
                &icon,
                target.parent(),
            )?;
            rollback.record_shortcut(link_path.clone());
            created_paths.push(link_path);
        }

        Ok(created_paths)
    }
}

#[derive(Clone)]
struct ExtractPlan {
    packaged: PackagedFile,
    output_path: PathBuf,
    file_existed: bool,
    created_dirs: Vec<PathBuf>,
}

struct InstallMetadata {
    registry_values: Vec<InstalledRegistryValue>,
    path_entry: Option<PathEntryState>,
    shortcuts: Vec<PathBuf>,
}

#[cfg(windows)]
struct PathRestore {
    key: String,
    previous: String,
}

#[derive(Default)]
struct InstallRollback {
    created_files: Vec<PathBuf>,
    created_dirs: Vec<PathBuf>,
    created_shortcuts: Vec<PathBuf>,
    registry_changes: Vec<InstalledRegistryValue>,
    #[cfg(windows)]
    path_restore: Option<PathRestore>,
}

impl InstallRollback {
    fn record_extraction(&mut self, plan: &ExtractPlan) {
        if !plan.file_existed {
            self.created_files.push(plan.output_path.clone());
        }
        self.created_dirs.extend(plan.created_dirs.iter().cloned());
    }

    fn record_created_file(&mut self, path: PathBuf) {
        self.created_files.push(path);
    }

    #[cfg(windows)]
    fn record_shortcut(&mut self, path: PathBuf) {
        self.created_shortcuts.push(path);
    }

    #[cfg(windows)]
    fn record_registry_change(&mut self, change: InstalledRegistryValue) {
        self.registry_changes.push(change);
    }

    #[cfg(windows)]
    fn record_path_restore(&mut self, path_restore: PathRestore) {
        self.path_restore = Some(path_restore);
    }

    fn rollback(self) {
        let _ = restore_registry_values(&self.registry_changes);
        #[cfg(windows)]
        let _ = restore_path_value(self.path_restore.as_ref());

        for path in self.created_shortcuts.into_iter().rev() {
            let _ = fs::remove_file(path);
        }

        for path in self.created_files.into_iter().rev() {
            let _ = fs::remove_file(path);
        }

        let mut created_dirs = self.created_dirs;
        created_dirs.sort_by(|left, right| {
            right
                .components()
                .count()
                .cmp(&left.components().count())
                .then_with(|| left.cmp(right))
        });
        created_dirs.dedup();
        for path in created_dirs {
            let _ = fs::remove_dir(&path);
        }
    }
}

struct InstallContext {
    install_dir: PathBuf,
    program_files: PathBuf,
    app_data: PathBuf,
    desktop: PathBuf,
    temp: PathBuf,
    app_name: String,
}

impl InstallContext {
    fn for_manifest(manifest: &PackagedInstaller, install_dir_override: Option<&Path>) -> Result<Self, EngineError> {
        let program_files = std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        let app_data = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Users\Default\AppData\Roaming"));
        let desktop = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"))
            .join("Desktop");
        let temp = std::env::temp_dir();

        let provisional = Self {
            install_dir: PathBuf::new(),
            program_files,
            app_data,
            desktop,
            temp,
            app_name: manifest.config.app.name.clone(),
        };

        let install_dir = match install_dir_override {
            Some(path) => path.to_path_buf(),
            None => resolve_template(&manifest.config.install.default_dir, &provisional)?,
        };

        Ok(Self {
            install_dir,
            ..provisional
        })
    }
}

fn resolve_template(template: &str, context: &InstallContext) -> Result<PathBuf, EngineError> {
    let expanded = resolve_arg_tokens(template, context);
    if expanded.contains('{') {
        return Err(EngineError::ResolvePath {
            template: template.to_string(),
            reason: String::from("contains unknown token"),
        });
    }
    Ok(PathBuf::from(expanded.replace('/', "\\")))
}

fn resolve_arg_tokens(input: &str, context: &InstallContext) -> String {
    input
        .replace("{ProgramFiles}", &context.program_files.display().to_string())
        .replace("{AppData}", &context.app_data.display().to_string())
        .replace("{Desktop}", &context.desktop.display().to_string())
        .replace("{Temp}", &context.temp.display().to_string())
        .replace("{AppName}", &context.app_name)
        .replace("{install_dir}", &context.install_dir.display().to_string())
}

fn install_state_dir(install_dir: &Path) -> PathBuf {
    install_dir.join(INSTALL_STATE_DIR_NAME)
}

fn install_state_path(install_dir: &Path) -> PathBuf {
    install_state_dir(install_dir).join(INSTALL_STATE_FILE_NAME)
}

fn uninstaller_path(install_dir: &Path) -> PathBuf {
    install_state_dir(install_dir).join(UNINSTALLER_FILE_NAME)
}

fn load_install_state(install_dir: &Path) -> Result<InstallState, EngineError> {
    let path = install_state_path(install_dir);
    let content = fs::read_to_string(&path).map_err(|source| EngineError::ReadState {
        path: path.clone(),
        source,
    })?;
    toml::from_str(&content).map_err(|source| EngineError::ParseState { path, source })
}

fn resolve_uninstall_dir(install_dir_override: Option<&Path>, current_exe: &Path, fallback: &Path) -> Result<PathBuf, EngineError> {
    if let Some(path) = install_dir_override {
        return Ok(path.to_path_buf());
    }

    if current_exe.file_name().is_some_and(|name| name == UNINSTALLER_FILE_NAME)
        && current_exe.parent().is_some_and(|parent| parent.file_name().is_some_and(|name| name == INSTALL_STATE_DIR_NAME))
    {
        return Ok(current_exe.parent().and_then(Path::parent).unwrap_or(fallback).to_path_buf());
    }

    let fallback = fallback.to_path_buf();
    if install_state_path(&fallback).exists() {
        Ok(fallback)
    } else {
        Err(EngineError::MissingInstallState(fallback))
    }
}

fn missing_parent_dirs(parent: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut current = parent;

    while let Some(path) = current {
        if path.exists() {
            break;
        }
        dirs.push(path.to_path_buf());
        current = path.parent();
    }

    dirs
}

fn chunk_output_offsets(file: &PackagedFile) -> Vec<u64> {
    let mut next_offset = 0u64;
    file.chunks
        .iter()
        .map(|chunk| {
            let offset = next_offset;
            next_offset += chunk.uncompressed_size;
            offset
        })
        .collect()
}

#[cfg(windows)]
fn write_all_at(file: &File, offset: u64, mut bytes: &[u8]) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;

    let mut position = offset;
    while !bytes.is_empty() {
        let written = file.seek_write(bytes, position)?;
        if written == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::WriteZero, "failed to write file chunk"));
        }
        bytes = &bytes[written..];
        position += written as u64;
    }
    Ok(())
}

#[cfg(unix)]
fn write_all_at(file: &File, offset: u64, mut bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;

    let mut position = offset;
    while !bytes.is_empty() {
        let written = file.write_at(bytes, position)?;
        if written == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::WriteZero, "failed to write file chunk"));
        }
        bytes = &bytes[written..];
        position += written as u64;
    }
    Ok(())
}

fn parse_windows_args(value: &str) -> Result<Vec<String>, EngineError> {
    let mut args = Vec::new();
    let characters = value.chars().collect::<Vec<_>>();
    let mut index = 0usize;

    while index < characters.len() {
        while index < characters.len() && characters[index].is_whitespace() {
            index += 1;
        }
        if index >= characters.len() {
            break;
        }

        let mut argument = String::new();
        let mut in_quotes = false;
        let mut backslashes = 0usize;

        while index < characters.len() {
            let ch = characters[index];
            index += 1;

            match ch {
                '\\' => backslashes += 1,
                '"' => {
                    for _ in 0..(backslashes / 2) {
                        argument.push('\\');
                    }

                    if backslashes % 2 == 1 {
                        argument.push('"');
                    } else if in_quotes && index < characters.len() && characters[index] == '"' {
                        argument.push('"');
                        index += 1;
                    } else {
                        in_quotes = !in_quotes;
                    }

                    backslashes = 0;
                }
                _ if ch.is_whitespace() && !in_quotes => {
                    for _ in 0..backslashes {
                        argument.push('\\');
                    }
                    backslashes = 0;
                    break;
                }
                _ => {
                    for _ in 0..backslashes {
                        argument.push('\\');
                    }
                    backslashes = 0;
                    argument.push(ch);
                }
            }
        }

        for _ in 0..backslashes {
            argument.push('\\');
        }

        if in_quotes {
            return Err(EngineError::InvalidArguments {
                value: value.to_string(),
                reason: String::from("missing closing quote"),
            });
        }

        args.push(argument);
    }

    Ok(args)
}

#[cfg(any(windows, test))]
fn append_path_entry(current: &str, entry: &str) -> String {
    let normalized_entry = normalize_env_path(entry);
    if current
        .split(';')
        .map(normalize_env_path)
        .any(|candidate| candidate.eq_ignore_ascii_case(&normalized_entry))
    {
        return current.to_string();
    }

    if current.trim().is_empty() {
        entry.to_string()
    } else {
        format!("{current};{entry}")
    }
}

#[cfg(windows)]
fn remove_path_entry(current: &str, entry: &str) -> String {
    let normalized_entry = normalize_env_path(entry);
    current
        .split(';')
        .filter(|candidate| !normalize_env_path(candidate).eq_ignore_ascii_case(&normalized_entry))
        .filter(|candidate| !candidate.trim().is_empty())
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(any(windows, test))]
fn normalize_env_path(value: &str) -> String {
    let mut normalized = value.trim().replace('/', "\\");
    while normalized.ends_with('\\') && normalized.len() > 3 {
        normalized.pop();
    }
    normalized
}

fn extraction_progress(completed_bytes: u64, total_bytes: u64) -> f32 {
    let ratio = if total_bytes == 0 {
        1.0
    } else {
        completed_bytes as f32 / total_bytes as f32
    };
    0.06 + ratio.clamp(0.0, 1.0) * 0.82
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(String::from)
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(not(windows))]
fn restore_registry_values(_changes: &[InstalledRegistryValue]) -> Result<(), EngineError> {
    Ok(())
}

#[cfg(windows)]
fn registry_type_from_u32(value: u32) -> winreg::enums::RegType {
    use winreg::enums::*;

    match value {
        x if x == REG_NONE as u32 => REG_NONE,
        x if x == REG_SZ as u32 => REG_SZ,
        x if x == REG_EXPAND_SZ as u32 => REG_EXPAND_SZ,
        x if x == REG_BINARY as u32 => REG_BINARY,
        x if x == REG_DWORD as u32 => REG_DWORD,
        x if x == REG_DWORD_BIG_ENDIAN as u32 => REG_DWORD_BIG_ENDIAN,
        x if x == REG_LINK as u32 => REG_LINK,
        x if x == REG_MULTI_SZ as u32 => REG_MULTI_SZ,
        x if x == REG_RESOURCE_LIST as u32 => REG_RESOURCE_LIST,
        x if x == REG_FULL_RESOURCE_DESCRIPTOR as u32 => REG_FULL_RESOURCE_DESCRIPTOR,
        x if x == REG_RESOURCE_REQUIREMENTS_LIST as u32 => REG_RESOURCE_REQUIREMENTS_LIST,
        x if x == REG_QWORD as u32 => REG_QWORD,
        _ => REG_BINARY,
    }
}

#[cfg(windows)]
fn restore_registry_values(changes: &[InstalledRegistryValue]) -> Result<(), EngineError> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::{RegKey, RegValue};

    for change in changes.iter().rev() {
        let (hive_name, subkey) = split_registry_key(&change.key)
            .ok_or_else(|| EngineError::UnsupportedRegistryHive(change.key.clone()))?;
        let root = match hive_name {
            "HKCU" => RegKey::predef(HKEY_CURRENT_USER),
            "HKLM" => RegKey::predef(HKEY_LOCAL_MACHINE),
            _ => return Err(EngineError::UnsupportedRegistryHive(change.key.clone())),
        };

        let (target_key, _) = root.create_subkey(subkey).map_err(|source| EngineError::RegistryIo {
            key: change.key.clone(),
            value_name: String::from("<create_subkey>"),
            source,
        })?;

        match &change.previous {
            Some(previous) => target_key
                .set_raw_value(
                    &change.value_name,
                    &RegValue {
                        vtype: registry_type_from_u32(previous.reg_type),
                        bytes: previous.bytes.clone(),
                    },
                )
                .map_err(|source| EngineError::RegistryIo {
                    key: change.key.clone(),
                    value_name: change.value_name.clone(),
                    source,
                })?,
            None => match target_key.delete_value(&change.value_name) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(EngineError::RegistryIo {
                        key: change.key.clone(),
                        value_name: change.value_name.clone(),
                        source,
                    });
                }
            },
        }
    }

    Ok(())
}

#[cfg(windows)]
fn restore_path_value(restore: Option<&PathRestore>) -> Result<(), EngineError> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_EXPAND_SZ};
    use winreg::{RegKey, RegValue};

    let Some(restore) = restore else {
        return Ok(());
    };
    let (hive_name, subkey) = split_registry_key(&restore.key)
        .ok_or_else(|| EngineError::UnsupportedRegistryHive(restore.key.clone()))?;
    let root = match hive_name {
        "HKCU" => RegKey::predef(HKEY_CURRENT_USER),
        "HKLM" => RegKey::predef(HKEY_LOCAL_MACHINE),
        _ => return Err(EngineError::UnsupportedRegistryHive(restore.key.clone())),
    };
    let (target_key, _) = root.create_subkey(subkey).map_err(|source| EngineError::RegistryIo {
        key: restore.key.clone(),
        value_name: String::from("Path"),
        source,
    })?;
    target_key
        .set_raw_value(
            "Path",
            &RegValue {
                bytes: encode_utf16_nul(&restore.previous),
                vtype: REG_EXPAND_SZ,
            },
        )
        .map_err(|source| EngineError::RegistryIo {
            key: restore.key.clone(),
            value_name: String::from("Path"),
            source,
        })?;
    broadcast_environment_change()?;
    Ok(())
}

#[cfg(not(windows))]
fn restore_path_entry(_path_entry: Option<&PathEntryState>) -> Result<(), EngineError> {
    Ok(())
}

#[cfg(windows)]
fn restore_path_entry(path_entry: Option<&PathEntryState>) -> Result<(), EngineError> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_EXPAND_SZ};
    use winreg::{RegKey, RegValue};

    let Some(path_entry) = path_entry else {
        return Ok(());
    };
    let (key_name, root, subkey) = if path_entry.system {
        (
            String::from("HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment"),
            RegKey::predef(HKEY_LOCAL_MACHINE),
            "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment",
        )
    } else {
        (String::from("HKCU\\Environment"), RegKey::predef(HKEY_CURRENT_USER), "Environment")
    };

    let (env_key, _) = root.create_subkey(subkey).map_err(|source| EngineError::RegistryIo {
        key: key_name.clone(),
        value_name: String::from("Path"),
        source,
    })?;
    let current = match env_key.get_value::<String, _>("Path") {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(EngineError::RegistryIo {
                key: key_name.clone(),
                value_name: String::from("Path"),
                source,
            });
        }
    };
    let updated = remove_path_entry(&current, &path_entry.entry);
    if updated == current {
        return Ok(());
    }
    env_key
        .set_raw_value(
            "Path",
            &RegValue {
                bytes: encode_utf16_nul(&updated),
                vtype: REG_EXPAND_SZ,
            },
        )
        .map_err(|source| EngineError::RegistryIo {
            key: key_name,
            value_name: String::from("Path"),
            source,
        })?;
    broadcast_environment_change()?;
    Ok(())
}

#[cfg(not(windows))]
fn schedule_self_delete(_current_exe: &Path, _state_path: &Path, _state_dir: &Path) -> Result<(), EngineError> {
    Ok(())
}

#[cfg(windows)]
fn schedule_self_delete(current_exe: &Path, state_path: &Path, state_dir: &Path) -> Result<(), EngineError> {
    let command = format!(
        "ping 127.0.0.1 -n 2 > nul & del /f /q \"{}\" & del /f /q \"{}\" & rmdir /s /q \"{}\"",
        state_path.display(),
        current_exe.display(),
        state_dir.display(),
    );
    Command::new("cmd")
        .args(["/C", &command])
        .spawn()
        .map_err(|source| EngineError::LaunchCommand {
            program: String::from("cmd"),
            source,
        })?;
    Ok(())
}

#[cfg(windows)]
fn split_registry_key(full_key: &str) -> Option<(&str, &str)> {
    let (hive, subkey) = full_key.split_once('\\')?;
    Some((hive, subkey))
}

#[cfg(windows)]
fn encode_utf16_nul(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(|word| word.to_le_bytes())
        .collect()
}

#[cfg(windows)]
fn shortcut_file_name(name: &str) -> String {
    let mut sanitized = name
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => ch,
        })
        .collect::<String>();

    if !sanitized.to_ascii_lowercase().ends_with(".lnk") {
        sanitized.push_str(".lnk");
    }
    sanitized
}

#[cfg(windows)]
fn create_shortcut_via_wsh(
    link_path: &Path,
    target: &Path,
    arguments: &str,
    icon: &Path,
    working_directory: Option<&Path>,
) -> Result<(), EngineError> {
    let script_path = std::env::temp_dir().join(format!(
        "setupweaver-shortcut-{}-{}.vbs",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));

    let script = format!(
        concat!(
            "Set shell = CreateObject(\"WScript.Shell\")\r\n",
            "Set shortcut = shell.CreateShortcut(\"{link}\")\r\n",
            "shortcut.TargetPath = \"{target}\"\r\n",
            "shortcut.Arguments = \"{arguments}\"\r\n",
            "shortcut.IconLocation = \"{icon}\"\r\n",
            "shortcut.WorkingDirectory = \"{working_directory}\"\r\n",
            "shortcut.Save\r\n"
        ),
        link = vbs_escape(&link_path.display().to_string()),
        target = vbs_escape(&target.display().to_string()),
        arguments = vbs_escape(arguments),
        icon = vbs_escape(&icon.display().to_string()),
        working_directory = vbs_escape(
            &working_directory
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        ),
    );

    fs::write(&script_path, script).map_err(|source| EngineError::CreateShortcut {
        path: link_path.to_path_buf(),
        source,
    })?;

    let cscript = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| root.join("System32").join("cscript.exe"))
        .unwrap_or_else(|| PathBuf::from("cscript.exe"));

    let status = Command::new(cscript)
        .arg("//nologo")
        .arg(&script_path)
        .status()
        .map_err(|source| EngineError::CreateShortcut {
            path: link_path.to_path_buf(),
            source,
        })?;

    let _ = fs::remove_file(&script_path);

    if !status.success() {
        return Err(EngineError::ShortcutToolFailed {
            path: link_path.to_path_buf(),
            code: status.code(),
        });
    }

    Ok(())
}

#[cfg(windows)]
fn vbs_escape(value: &str) -> String {
    value.replace('"', "\"\"")
}

#[cfg(windows)]
fn broadcast_environment_change() -> Result<(), EngineError> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    let mut result = 0usize;
    let mut parameter = "Environment".encode_utf16().chain(std::iter::once(0)).collect::<Vec<_>>();

    // SAFETY: HWND_BROADCAST and WM_SETTINGCHANGE are valid system constants.
    // `parameter` is NUL-terminated and kept alive for the duration of the call.
    let status = unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            parameter.as_mut_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5000,
            &mut result,
        )
    };

    if status == 0 {
        return Err(EngineError::BroadcastEnvironmentChange(std::io::Error::last_os_error()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use setupweaver_common::{PackagedChunk, PackagedFile};

    use super::{append_path_entry, chunk_output_offsets, missing_parent_dirs, normalize_env_path, parse_windows_args};

    #[test]
    fn appends_missing_path_entry() {
        let updated = append_path_entry(r"C:\Windows", r"C:\Apps\Hello");
        assert_eq!(updated, r"C:\Windows;C:\Apps\Hello");
    }

    #[test]
    fn avoids_duplicate_path_entries_case_insensitively() {
        let updated = append_path_entry(r"C:\Windows;C:\Apps\Hello", r"c:/apps/hello/");
        assert_eq!(updated, r"C:\Windows;C:\Apps\Hello");
    }

    #[test]
    fn normalizes_trailing_slashes() {
        assert_eq!(normalize_env_path(r" C:/Apps/Hello/ "), r"C:\Apps\Hello");
    }

    #[test]
    fn parses_windows_quoted_arguments() {
        let args = parse_windows_args(r#"--mode "safe install" --flag"#).unwrap();
        assert_eq!(args, vec!["--mode", "safe install", "--flag"]);
    }

    #[test]
    fn preserves_backslashes_before_quotes() {
        let args = parse_windows_args(r#"--path "C:\Program Files\Hello\\""#).unwrap();
        assert_eq!(args, vec!["--path", r#"C:\Program Files\Hello\"#]);
    }

    #[test]
    fn rejects_unbalanced_quotes() {
        let error = parse_windows_args(r#"--mode "broken"#).unwrap_err();
        assert!(error.to_string().contains("missing closing quote"));
    }

    #[test]
    fn no_missing_dirs_when_parent_exists() {
        let dirs = missing_parent_dirs(Some(Path::new(".")));
        assert!(dirs.is_empty());
    }

    #[test]
    fn chunk_offsets_are_prefix_sums() {
        let file = PackagedFile {
            destination: String::from("out.bin"),
            size: 30,
            chunks: vec![
                PackagedChunk {
                    payload_offset: 0,
                    compressed_size: 5,
                    uncompressed_size: 10,
                },
                PackagedChunk {
                    payload_offset: 5,
                    compressed_size: 6,
                    uncompressed_size: 8,
                },
                PackagedChunk {
                    payload_offset: 11,
                    compressed_size: 7,
                    uncompressed_size: 12,
                },
            ],
        };

        assert_eq!(chunk_output_offsets(&file), vec![0, 10, 18]);
    }
}
