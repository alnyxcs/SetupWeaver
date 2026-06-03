// common/src/config.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallConfig {
    pub app: AppSection,
    pub install: InstallSection,
    #[serde(default)]
    pub ui: UiSection,
    #[serde(default)]
    pub files: Vec<FileSpec>,
    #[serde(default)]
    pub shortcuts: Vec<ShortcutSpec>,
    #[serde(default)]
    pub registry: Vec<RegistryKeySpec>,
    #[serde(default)]
    pub run: Vec<RunSpec>,
}

impl InstallConfig {
    pub fn parse(input: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(input)
    }

    pub fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = Vec::new();

        if self.app.name.trim().is_empty() {
            errors.push(ValidationError::new("app.name", "must not be empty"));
        }
        if self.app.version.trim().is_empty() {
            errors.push(ValidationError::new("app.version", "must not be empty"));
        }
        if self.install.default_dir.trim().is_empty() {
            errors.push(ValidationError::new(
                "install.default_dir",
                "must not be empty",
            ));
        }
        if self.files.is_empty() {
            errors.push(ValidationError::new(
                "files",
                "must contain at least one [[files]] entry",
            ));
        }

        for (index, file) in self.files.iter().enumerate() {
            if file.src.trim().is_empty() {
                errors.push(ValidationError::new(
                    format!("files[{index}].src"),
                    "must not be empty",
                ));
            }
            if file.dest.trim().is_empty() {
                errors.push(ValidationError::new(
                    format!("files[{index}].dest"),
                    "must not be empty",
                ));
            }
        }

        for (index, shortcut) in self.shortcuts.iter().enumerate() {
            if shortcut.name.trim().is_empty() {
                errors.push(ValidationError::new(
                    format!("shortcuts[{index}].name"),
                    "must not be empty",
                ));
            }
            if shortcut.target.trim().is_empty() {
                errors.push(ValidationError::new(
                    format!("shortcuts[{index}].target"),
                    "must not be empty",
                ));
            }
        }

        for (index, key) in self.registry.iter().enumerate() {
            if key.key.trim().is_empty() {
                errors.push(ValidationError::new(
                    format!("registry[{index}].key"),
                    "must not be empty",
                ));
            }
            for (value_index, value) in key.values.iter().enumerate() {
                if value.name.trim().is_empty() {
                    errors.push(ValidationError::new(
                        format!("registry[{index}].values[{value_index}].name"),
                        "must not be empty",
                    ));
                }
            }
        }

        for (index, item) in self.run.iter().enumerate() {
            if item.cmd.trim().is_empty() {
                errors.push(ValidationError::new(
                    format!("run[{index}].cmd"),
                    "must not be empty",
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationErrors(errors))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSection {
    pub name: String,
    pub version: String,
    pub publisher: Option<String>,
    pub icon: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallSection {
    pub default_dir: String,
    #[serde(default)]
    pub add_to_path: bool,
    #[serde(default)]
    pub create_desktop_shortcut: bool,
    #[serde(default)]
    pub require_admin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiSection {
    #[serde(default)]
    pub theme: UiTheme,
    pub accent_color: Option<String>,
    pub welcome_text: Option<String>,
    pub license_file: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UiTheme {
    Dark,
    Light,
    #[default]
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSpec {
    pub src: String,
    pub dest: String,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutSpec {
    pub name: String,
    pub target: String,
    #[serde(default)]
    pub args: String,
    #[serde(default)]
    pub icon: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryKeySpec {
    pub key: String,
    #[serde(default)]
    pub values: Vec<RegistryValueSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryValueSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub value_type: RegistryValueType,
    pub data: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegistryValueType {
    String,
    Dword,
    Qword,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSpec {
    pub cmd: String,
    #[serde(default)]
    pub args: String,
    #[serde(default)]
    pub when: RunWhen,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RunWhen {
    #[default]
    After,
    Finish,
}

#[derive(Debug, Clone)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

impl ValidationError {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug)]
pub struct ValidationErrors(pub Vec<ValidationError>);

impl std::fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "configuration validation failed:")?;
        for error in &self.0 {
            writeln!(f, "- {}: {}", error.path, error.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationErrors {}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_toml() -> &'static str {
        r##"
[app]
name = "TestApp"
version = "1.0.0"

[install]
default_dir = "{ProgramFiles}\\TestApp"

[[files]]
src = "app/**/*"
dest = "{install_dir}"
"##
    }

    #[test]
    fn parse_minimal_config() {
        let config = InstallConfig::parse(minimal_toml()).unwrap();
        assert_eq!(config.app.name, "TestApp");
        assert_eq!(config.app.version, "1.0.0");
        assert_eq!(config.install.default_dir, r"{ProgramFiles}\TestApp");
        assert!(!config.install.add_to_path);
        assert!(!config.install.require_admin);
        assert!(!config.install.create_desktop_shortcut);
    }

    #[test]
    fn parse_full_config() {
        let toml = r##"
[app]
name = "Hello"
version = "2.0.0"
publisher = "Acme"
icon = "app.ico"
description = "A great app"

[install]
default_dir = "{ProgramFiles}\\Hello"
add_to_path = true
create_desktop_shortcut = false
require_admin = true

[ui]
theme = "dark"
accent_color = "#ff0000"
welcome_text = "Welcome!"
license_file = "LICENSE.txt"

[[files]]
src = "bin/**/*"
dest = "{install_dir}"
exclude = ["*.pdb", "*.log"]

[[files]]
src = "docs/readme.txt"
dest = "{install_dir}\\docs"

[[shortcuts]]
name = "Hello App"
target = "{install_dir}\\hello.exe"
args = "--start"
icon = "{install_dir}\\hello.ico"

[[registry]]
key = "HKCU\\Software\\Hello"

[[registry.values]]
name = "Version"
type = "string"
data = "2.0.0"

[[registry.values]]
name = "Flags"
type = "dword"
data = "42"

[[run]]
cmd = "{install_dir}\\hello.exe"
args = "--setup"
when = "after"

[[run]]
cmd = "{install_dir}\\hello.exe"
when = "finish"
"##;

        let config = InstallConfig::parse(toml).unwrap();
        assert_eq!(config.app.publisher.as_deref(), Some("Acme"));
        assert_eq!(config.app.icon.as_deref(), Some("app.ico"));
        assert!(config.install.require_admin);
        assert!(config.install.add_to_path);
        assert!(!config.install.create_desktop_shortcut);
        assert!(matches!(config.ui.theme, UiTheme::Dark));
        assert_eq!(config.ui.accent_color.as_deref(), Some("#ff0000"));
        assert_eq!(config.files.len(), 2);
        assert_eq!(config.files[0].exclude, vec!["*.pdb", "*.log"]);
        assert_eq!(config.shortcuts.len(), 1);
        assert_eq!(config.shortcuts[0].args, "--start");
        assert_eq!(config.registry.len(), 1);
        assert_eq!(config.registry[0].values.len(), 2);
        assert!(matches!(
            config.registry[0].values[1].value_type,
            RegistryValueType::Dword
        ));
        assert_eq!(config.run.len(), 2);
        assert_eq!(config.run[0].when, RunWhen::After);
        assert_eq!(config.run[1].when, RunWhen::Finish);
    }

    #[test]
    fn validate_minimal_passes() {
        let config = InstallConfig::parse(minimal_toml()).unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn validate_empty_app_name() {
        let toml = r##"
[app]
name = ""
version = "1.0.0"
[install]
default_dir = "C:\\App"
[[files]]
src = "a"
dest = "b"
"##;
        let config = InstallConfig::parse(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.0.iter().any(|e| e.path == "app.name"));
    }

    #[test]
    fn validate_empty_version() {
        let toml = r##"
[app]
name = "App"
version = "  "
[install]
default_dir = "C:\\App"
[[files]]
src = "a"
dest = "b"
"##;
        let config = InstallConfig::parse(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.0.iter().any(|e| e.path == "app.version"));
    }

    #[test]
    fn validate_empty_default_dir() {
        let toml = r##"
[app]
name = "App"
version = "1.0"
[install]
default_dir = ""
[[files]]
src = "a"
dest = "b"
"##;
        let config = InstallConfig::parse(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.0.iter().any(|e| e.path == "install.default_dir"));
    }

    #[test]
    fn validate_no_files() {
        let toml = r##"
[app]
name = "App"
version = "1.0"
[install]
default_dir = "C:\\App"
"##;
        let config = InstallConfig::parse(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.0.iter().any(|e| e.path == "files"));
    }

    #[test]
    fn validate_empty_file_src() {
        let toml = r##"
[app]
name = "App"
version = "1.0"
[install]
default_dir = "C:\\App"
[[files]]
src = ""
dest = "b"
"##;
        let config = InstallConfig::parse(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.0.iter().any(|e| e.path == "files[0].src"));
    }

    #[test]
    fn validate_empty_file_dest() {
        let toml = r##"
[app]
name = "App"
version = "1.0"
[install]
default_dir = "C:\\App"
[[files]]
src = "a"
dest = "  "
"##;
        let config = InstallConfig::parse(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.0.iter().any(|e| e.path == "files[0].dest"));
    }

    #[test]
    fn validate_empty_shortcut_name() {
        let toml = r##"
[app]
name = "App"
version = "1.0"
[install]
default_dir = "C:\\App"
[[files]]
src = "a"
dest = "b"
[[shortcuts]]
name = ""
target = "x.exe"
"##;
        let config = InstallConfig::parse(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.0.iter().any(|e| e.path == "shortcuts[0].name"));
    }

    #[test]
    fn validate_empty_shortcut_target() {
        let toml = r##"
[app]
name = "App"
version = "1.0"
[install]
default_dir = "C:\\App"
[[files]]
src = "a"
dest = "b"
[[shortcuts]]
name = "MyApp"
target = ""
"##;
        let config = InstallConfig::parse(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.0.iter().any(|e| e.path == "shortcuts[0].target"));
    }

    #[test]
    fn validate_empty_registry_key() {
        let toml = r##"
[app]
name = "App"
version = "1.0"
[install]
default_dir = "C:\\App"
[[files]]
src = "a"
dest = "b"
[[registry]]
key = ""
"##;
        let config = InstallConfig::parse(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.0.iter().any(|e| e.path == "registry[0].key"));
    }

    #[test]
    fn validate_empty_registry_value_name() {
        let toml = r##"
[app]
name = "App"
version = "1.0"
[install]
default_dir = "C:\\App"
[[files]]
src = "a"
dest = "b"
[[registry]]
key = "HKCU\\Software\\Test"
[[registry.values]]
name = ""
type = "string"
data = "x"
"##;
        let config = InstallConfig::parse(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err
            .0
            .iter()
            .any(|e| e.path == "registry[0].values[0].name"));
    }

    #[test]
    fn validate_empty_run_cmd() {
        let toml = r##"
[app]
name = "App"
version = "1.0"
[install]
default_dir = "C:\\App"
[[files]]
src = "a"
dest = "b"
[[run]]
cmd = ""
"##;
        let config = InstallConfig::parse(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.0.iter().any(|e| e.path == "run[0].cmd"));
    }

    #[test]
    fn validate_collects_multiple_errors() {
        let toml = r##"
[app]
name = ""
version = ""
[install]
default_dir = ""
"##;
        let config = InstallConfig::parse(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.0.len() >= 4); // name, version, default_dir, files
    }

    #[test]
    fn parse_invalid_toml_is_error() {
        let result = InstallConfig::parse("not valid toml {{{}");
        assert!(result.is_err());
    }

    #[test]
    fn parse_missing_required_section_is_error() {
        let result = InstallConfig::parse("[app]\nname = \"A\"\nversion = \"1\"");
        assert!(result.is_err());
    }

    #[test]
    fn ui_theme_defaults_to_system() {
        let config = InstallConfig::parse(minimal_toml()).unwrap();
        assert!(matches!(config.ui.theme, UiTheme::System));
    }

    #[test]
    fn run_when_defaults_to_after() {
        let toml = r##"
[app]
name = "A"
version = "1"
[install]
default_dir = "C:\\A"
[[files]]
src = "a"
dest = "b"
[[run]]
cmd = "x.exe"
"##;
        let config = InstallConfig::parse(toml).unwrap();
        assert_eq!(config.run[0].when, RunWhen::After);
    }

    #[test]
    fn config_roundtrip_through_toml() {
        let config = InstallConfig::parse(minimal_toml()).unwrap();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let reparsed = InstallConfig::parse(&serialized).unwrap();
        assert_eq!(config.app.name, reparsed.app.name);
        assert_eq!(config.app.version, reparsed.app.version);
        assert_eq!(config.files.len(), reparsed.files.len());
    }

    #[test]
    fn validation_errors_display() {
        let toml = r##"
[app]
name = ""
version = "1"
[install]
default_dir = "C:\\A"
[[files]]
src = "a"
dest = "b"
"##;
        let config = InstallConfig::parse(toml).unwrap();
        let err = config.validate().unwrap_err();
        let display = format!("{err}");
        assert!(display.contains("app.name"));
        assert!(display.contains("must not be empty"));
    }
}
