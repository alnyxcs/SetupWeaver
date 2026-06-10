// runtime/src/ui/mod.rs
slint::include_modules!();

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use slint::{CloseRequestResponse, ComponentHandle};

use crate::engine::{InstallPhase, InstallProgress, InstallerEngine};

const SCREEN_WELCOME: i32 = 0;
const SCREEN_LICENSE: i32 = 1;
const SCREEN_INSTALL: i32 = 2;
const SCREEN_FINISH: i32 = 3;
const SCREEN_ERROR: i32 = 4;

pub fn run_installer(engine: &InstallerEngine, install_dir_override: Option<&Path>) -> Result<()> {
    let manifest = engine.manifest().clone();
    let has_license = manifest
        .license_text
        .as_deref()
        .is_some_and(|text| !text.trim().is_empty());
    let install_dir = match install_dir_override {
        Some(path) => path.to_path_buf(),
        None => engine.default_install_dir()?,
    };

    let window = InstallerWindow::new()?;
    window.set_app_name(manifest.config.app.name.clone().into());
    window.set_version_text(format!("Version {}", manifest.config.app.version).into());
    window.set_publisher_text(
        manifest
            .config
            .app
            .publisher
            .clone()
            .unwrap_or_else(|| String::from("Packaged with SetupWeaver"))
            .into(),
    );
    window.set_welcome_text(
        manifest
            .config
            .ui
            .welcome_text
            .clone()
            .unwrap_or_else(|| format!("Install {} in seconds.", manifest.config.app.name))
            .into(),
    );
    window.set_license_text(manifest.license_text.unwrap_or_default().into());
    window.set_install_dir(install_dir.display().to_string().into());
    window.set_status_text("Ready to install".into());
    window.set_status_glyph("🎯".into());
    window.set_detail_text("Files will be extracted directly from the embedded payload.".into());
    window.set_error_text("".into());
    window.set_current_screen(SCREEN_WELCOME);
    window.set_progress_value(0.0);
    window.set_install_running(false);
    window.set_has_license(has_license);
    window.set_accent_color(parse_accent_color(
        manifest.config.ui.accent_color.as_deref(),
    ));

    let state = Arc::new(Mutex::new(UiState {
        has_license,
        install_running: false,
    }));

    {
        let state = state.clone();
        window.window().on_close_requested(move || {
            if state.lock().expect("ui state poisoned").install_running {
                CloseRequestResponse::KeepWindowShown
            } else {
                CloseRequestResponse::HideWindow
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_cancel(move || {
            if state.lock().expect("ui state poisoned").install_running {
                return;
            }
            if let Some(window) = window_weak.upgrade() {
                let _ = window.hide();
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_back(move || {
            if state.lock().expect("ui state poisoned").install_running {
                return;
            }
            if let Some(window) = window_weak.upgrade() {
                window.set_current_screen(SCREEN_WELCOME);
                window.set_status_text("Ready to install".into());
                window.set_status_glyph("🎯".into());
                window.set_detail_text("Choose where the app will be installed.".into());
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_finish(move || {
            if state.lock().expect("ui state poisoned").install_running {
                return;
            }
            if let Some(window) = window_weak.upgrade() {
                let _ = window.hide();
            }
        });
    }

    // New in the redesigned wizard: copy the current error text to the OS
    // clipboard. Returns the error text (so the callback signature matches
    // `copy_error() -> string`); on Windows we also pipe it to `clip.exe`.
    // No new dependencies are needed; setup.exe is Windows-only.
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_copy_error(move || -> slint::SharedString {
            if state.lock().expect("ui state poisoned").install_running {
                return slint::SharedString::default();
            }
            let text = match window_weak.upgrade() {
                Some(window) => window.get_error_text(),
                None => return slint::SharedString::default(),
            };
            #[cfg(windows)]
            {
                use std::io::Write;
                use std::process::{Command, Stdio};
                if let Ok(mut child) = Command::new("clip")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(text.as_bytes());
                    }
                    let _ = child.wait();
                }
            }
            text
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_next(move || {
            let current_screen = if let Some(window) = window_weak.upgrade() {
                window.get_current_screen()
            } else {
                return;
            };

            if state.lock().expect("ui state poisoned").install_running {
                return;
            }

            match current_screen {
                SCREEN_WELCOME if state.lock().expect("ui state poisoned").has_license => {
                    if let Some(window) = window_weak.upgrade() {
                        window.set_current_screen(SCREEN_LICENSE);
                        window.set_status_text("Review license".into());
                        window.set_status_glyph("📄".into());
                        window.set_detail_text("Read the license and continue when ready.".into());
                    }
                }
                SCREEN_WELCOME | SCREEN_LICENSE => {
                    start_install(window_weak.clone(), state.clone())
                }
                SCREEN_FINISH | SCREEN_ERROR => {
                    if let Some(window) = window_weak.upgrade() {
                        let _ = window.hide();
                    }
                }
                _ => {}
            }
        });
    }

    window.run()?;
    Ok(())
}

struct UiState {
    has_license: bool,
    install_running: bool,
}

fn start_install(window_weak: slint::Weak<InstallerWindow>, state: Arc<Mutex<UiState>>) {
    let install_dir = match window_weak.upgrade() {
        Some(window) => {
            let install_dir = window.get_install_dir().to_string();
            window.set_current_screen(SCREEN_INSTALL);
            window.set_install_running(true);
            window.set_progress_value(0.02);
            window.set_status_text("Preparing installer".into());
            window.set_status_glyph("⏳".into());
            window.set_detail_text(install_dir.clone().into());
            state.lock().expect("ui state poisoned").install_running = true;
            install_dir
        }
        None => return,
    };

    std::thread::spawn(move || {
        let install_path = normalize_install_dir(&install_dir);
        let result = (|| -> anyhow::Result<()> {
            let engine = InstallerEngine::from_current_exe()?;
            engine.install_with_progress(install_path.as_deref(), |progress| {
                report_progress(&window_weak, progress);
            })?;
            window_weak.upgrade_in_event_loop(|window| {
                window.set_status_text("Launching finish tasks".into());
                window.set_status_glyph("✨".into());
                window.set_detail_text("Running final actions.".into());
                window.set_progress_value(0.995);
            })?;
            engine.finish(install_path.as_deref())?;
            Ok(())
        })();

        match result {
            Ok(()) => {
                state.lock().expect("ui state poisoned").install_running = false;
                let _ = window_weak.upgrade_in_event_loop(|window| {
                    window.set_install_running(false);
                    window.set_current_screen(SCREEN_FINISH);
                    window.set_progress_value(1.0);
                    window.set_status_text("Installation complete".into());
                    window.set_status_glyph("✓".into());
                    window.set_detail_text("Everything is ready to go.".into());
                });
            }
            Err(error) => {
                state.lock().expect("ui state poisoned").install_running = false;
                let message = error.to_string();
                let _ = window_weak.upgrade_in_event_loop(move |window| {
                    window.set_install_running(false);
                    window.set_current_screen(SCREEN_ERROR);
                    window.set_error_text(message.clone().into());
                    window.set_status_text("Installation failed".into());
                    window.set_status_glyph("⚠".into());
                    window.set_detail_text("See the error details below.".into());
                });
            }
        }
    });
}

fn report_progress(window_weak: &slint::Weak<InstallerWindow>, progress: InstallProgress) {
    let progress_value = progress.progress;
    let detail_summary = progress_summary(&progress);
    let status = progress.status.clone();
    let detail = if progress.detail.is_empty() {
        detail_summary
    } else {
        format!("{} · {}", detail_summary, progress.detail)
    };

    let _ = window_weak.upgrade_in_event_loop(move |window| {
        window.set_current_screen(SCREEN_INSTALL);
        window.set_progress_value(progress_value);
        window.set_status_text(status.into());
        window.set_status_glyph(phase_glyph(progress.phase).into());
        window.set_detail_text(detail.into());
    });
}

fn progress_summary(progress: &InstallProgress) -> String {
    match progress.phase {
        InstallPhase::Preparing => String::from("Preparing embedded payload"),
        InstallPhase::Extracting => format!(
            "{} / {} files · {} / {}",
            progress.completed_files,
            progress.total_files,
            human_size(progress.completed_bytes),
            human_size(progress.total_bytes),
        ),
        InstallPhase::Configuring => String::from("Applying machine settings"),
        InstallPhase::Finishing => String::from("Finalizing installation"),
    }
}

/// Map an [`InstallPhase`] to a single-glyph icon that gives the user a
/// instant visual cue of which stage the installer is in. Returned as a
/// `SharedString` so the caller can pass it straight to
/// `window.set_status_glyph`.
fn phase_glyph(phase: InstallPhase) -> &'static str {
    match phase {
        InstallPhase::Preparing => "⏳",
        InstallPhase::Extracting => "📦",
        InstallPhase::Configuring => "⚙",
        InstallPhase::Finishing => "✨",
    }
}

fn normalize_install_dir(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

fn parse_accent_color(value: Option<&str>) -> slint::Color {
    let fallback = slint::Color::from_rgb_u8(124, 58, 237);
    let Some(value) = value.map(str::trim) else {
        return fallback;
    };
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 {
        return fallback;
    }

    let Ok(rgb) = u32::from_str_radix(value, 16) else {
        return fallback;
    };
    slint::Color::from_rgb_u8(
        ((rgb >> 16) & 0xff) as u8,
        ((rgb >> 8) & 0xff) as u8,
        (rgb & 0xff) as u8,
    )
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
