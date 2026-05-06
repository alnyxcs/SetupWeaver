// runtime/src/engine.rs
use std::fs::{self, File};
use std::io::{BufWriter, Cursor};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use setupweaver_common::{PackagedFile, PackagedInstaller, RunWhen, ShortcutSpec};
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
        let plans = self.build_extract_plans(&context)?;
        let mut rollback = self.extract_files_parallel(&plans)?;
        if let Err(error) = self.configure_system(&context, &mut rollback) {
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
        if let Err(error) = self.configure_system_with_progress(
            &context,
            &mut progress,
            &mut rollback,
            completed_files,
            completed_bytes,
            total_files,
            total_bytes,
        ) {
            rollback.rollback();
            return Err(error);
        }
        Ok(())
    }

    pub fn finish(&self, install_dir: Option<&Path>) -> Result<(), EngineError> {
        let context = InstallContext::for_manifest(&self.manifest, install_dir)?;
        self.run_hooks(&context, RunWhen::Finish)
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
            self.write_payload_file(&plan.packaged, &plan.output_path)?;
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

            if let Err(error) = self.write_payload_file(&plan.packaged, &plan.output_path) {
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

    fn write_payload_file(&self, packaged: &PackagedFile, output_path: &Path) -> Result<(), EngineError> {
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
        let compressed = self.payload.payload_file_bytes(packaged)?;
        let mut decoder = zstd::stream::read::Decoder::new(Cursor::new(compressed))?;
        std::io::copy(&mut decoder, &mut writer).map_err(|source| EngineError::WriteFile {
            path: output_path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    fn configure_system(&self, context: &InstallContext, rollback: &mut InstallRollback) -> Result<(), EngineError> {
        self.apply_registry(context)?;
        self.update_path(context)?;
        self.create_shortcuts(context, rollback)?;
        self.run_hooks(context, RunWhen::After)
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
    ) -> Result<(), EngineError> {
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
        self.apply_registry(context)?;

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
        self.update_path(context)?;

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
        self.create_shortcuts(context, rollback)?;

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
        self.run_hooks(context, RunWhen::After)
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
    fn apply_registry(&self, _context: &InstallContext) -> Result<(), EngineError> {
        Ok(())
    }

    #[cfg(windows)]
    fn apply_registry(&self, context: &InstallContext) -> Result<(), EngineError> {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        use winreg::RegKey;

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
                let data = resolve_arg_tokens(&value.data, context);
                match value.value_type {
                    setupweaver_common::RegistryValueType::String => target_key
                        .set_value(&value.name, &data)
                        .map_err(|source| EngineError::RegistryIo {
                            key: entry.key.clone(),
                            value_name: value.name.clone(),
                            source,
                        })?,
                    setupweaver_common::RegistryValueType::Dword => {
                        let parsed: u32 = data.parse().map_err(|_| EngineError::ResolvePath {
                            template: value.data.clone(),
                            reason: String::from("DWORD data must parse as u32"),
                        })?;
                        target_key
                            .set_value(&value.name, &parsed)
                            .map_err(|source| EngineError::RegistryIo {
                                key: entry.key.clone(),
                                value_name: value.name.clone(),
                                source,
                            })?;
                    }
                    setupweaver_common::RegistryValueType::Qword => {
                        let parsed: u64 = data.parse().map_err(|_| EngineError::ResolvePath {
                            template: value.data.clone(),
                            reason: String::from("QWORD data must parse as u64"),
                        })?;
                        target_key
                            .set_value(&value.name, &parsed)
                            .map_err(|source| EngineError::RegistryIo {
                                key: entry.key.clone(),
                                value_name: value.name.clone(),
                                source,
                            })?;
                    }
                }
            }
        }

        Ok(())
    }

    #[cfg(not(windows))]
    fn update_path(&self, _context: &InstallContext) -> Result<(), EngineError> {
        Ok(())
    }

    #[cfg(windows)]
    fn update_path(&self, context: &InstallContext) -> Result<(), EngineError> {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_EXPAND_SZ};
        use winreg::{RegKey, RegValue};

        if !self.manifest.config.install.add_to_path {
            return Ok(());
        }

        let (key_name, root, subkey) = if self.manifest.config.install.require_admin {
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

        let install_dir = context.install_dir.display().to_string();
        let updated = append_path_entry(&current, &install_dir);
        if updated == current {
            return Ok(());
        }

        let value = RegValue {
            bytes: encode_utf16_nul(&updated),
            vtype: REG_EXPAND_SZ,
        };
        env_key.set_raw_value("Path", &value).map_err(|source| EngineError::RegistryIo {
            key: key_name,
            value_name: String::from("Path"),
            source,
        })?;

        broadcast_environment_change()?;
        Ok(())
    }

    #[cfg(not(windows))]
    fn create_shortcuts(&self, _context: &InstallContext, _rollback: &mut InstallRollback) -> Result<(), EngineError> {
        Ok(())
    }

    #[cfg(windows)]
    fn create_shortcuts(&self, context: &InstallContext, rollback: &mut InstallRollback) -> Result<(), EngineError> {
        let mut shortcuts = Vec::new();

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
            rollback.record_shortcut(link_path);
        }

        Ok(())
    }
}

#[derive(Clone)]
struct ExtractPlan {
    packaged: PackagedFile,
    output_path: PathBuf,
    file_existed: bool,
    created_dirs: Vec<PathBuf>,
}

#[derive(Default)]
struct InstallRollback {
    created_files: Vec<PathBuf>,
    created_dirs: Vec<PathBuf>,
    created_shortcuts: Vec<PathBuf>,
}

impl InstallRollback {
    fn record_extraction(&mut self, plan: &ExtractPlan) {
        if !plan.file_existed {
            self.created_files.push(plan.output_path.clone());
        }
        self.created_dirs.extend(plan.created_dirs.iter().cloned());
    }

    fn record_shortcut(&mut self, path: PathBuf) {
        self.created_shortcuts.push(path);
    }

    fn rollback(self) {
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

    use super::{append_path_entry, missing_parent_dirs, normalize_env_path, parse_windows_args};

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
}
