// packager-gui/src/main.rs
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

slint::include_modules!();

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use setupweaver_common::{
    FileSpec, InstallConfig, InstallSection, AppSection, UiSection, UiTheme,
    ShortcutSpec, RegistryKeySpec, RegistryValueSpec, RegistryValueType, RunSpec, RunWhen,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

fn main() -> Result<()> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .select()?;

    let window = PackagerWindow::new()?;
    let state = Arc::new(Mutex::new(AppState::default()));

    setup_navigation(&window);
    setup_file_entries(&window, &state);
    setup_shortcut_entries(&window, &state);
    setup_registry_entries(&window, &state);
    setup_run_entries(&window, &state);
    setup_browse_callbacks(&window);
    setup_config_io(&window, &state);
    setup_build(&window, &state);

    window.run()?;
    Ok(())
}

#[derive(Default)]
struct AppState {
    last_config_dir: Option<PathBuf>,
}

fn setup_navigation(window: &PackagerWindow) {
    let window_weak = window.as_weak();
    window.on_nav_clicked(move |page| {
        if let Some(w) = window_weak.upgrade() {
            w.set_current_page(page);
        }
    });
}

fn setup_file_entries(window: &PackagerWindow, _state: &Arc<Mutex<AppState>>) {
    {
        let window_weak = window.as_weak();
        window.on_add_file_entry(move || {
            if let Some(w) = window_weak.upgrade() {
                let model = get_file_entries_model(&w);
                model.push(FileEntryData {
                    src: SharedString::new(),
                    dest: SharedString::from("{install_dir}"),
                    exclude: SharedString::new(),
                });
                w.set_file_entries(ModelRc::from(model));
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_remove_file_entry(move |index| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_file_entries_model(&w);
                if (index as usize) < model.row_count() {
                    model.remove(index as usize);
                    w.set_file_entries(ModelRc::from(model));
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_file_src_changed(move |index, value| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_file_entries_model(&w);
                if let Some(mut entry) = model.row_data(index as usize) {
                    entry.src = value;
                    model.set_row_data(index as usize, entry);
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_file_dest_changed(move |index, value| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_file_entries_model(&w);
                if let Some(mut entry) = model.row_data(index as usize) {
                    entry.dest = value;
                    model.set_row_data(index as usize, entry);
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_file_exclude_changed(move |index, value| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_file_entries_model(&w);
                if let Some(mut entry) = model.row_data(index as usize) {
                    entry.exclude = value;
                    model.set_row_data(index as usize, entry);
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_browse_file_src(move |index| {
            if let Some(w) = window_weak.upgrade() {
                let dialog = rfd::FileDialog::new().set_title("Select source files or folder");
                if let Some(path) = dialog.pick_folder() {
                    let pattern = format!("{}/**/*", path.display());
                    let model = get_file_entries_model(&w);
                    if let Some(mut entry) = model.row_data(index as usize) {
                        entry.src = SharedString::from(pattern);
                        model.set_row_data(index as usize, entry);
                        w.set_file_entries(ModelRc::from(model));
                    }
                }
            }
        });
    }
}

fn setup_shortcut_entries(window: &PackagerWindow, _state: &Arc<Mutex<AppState>>) {
    {
        let window_weak = window.as_weak();
        window.on_add_shortcut_entry(move || {
            if let Some(w) = window_weak.upgrade() {
                let model = get_shortcut_entries_model(&w);
                model.push(ShortcutEntryData {
                    name: SharedString::new(),
                    target: SharedString::new(),
                    args: SharedString::new(),
                    icon: SharedString::new(),
                });
                w.set_shortcut_entries(ModelRc::from(model));
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_remove_shortcut_entry(move |index| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_shortcut_entries_model(&w);
                if (index as usize) < model.row_count() {
                    model.remove(index as usize);
                    w.set_shortcut_entries(ModelRc::from(model));
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_shortcut_name_changed(move |index, value| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_shortcut_entries_model(&w);
                if let Some(mut entry) = model.row_data(index as usize) {
                    entry.name = value;
                    model.set_row_data(index as usize, entry);
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_shortcut_target_changed(move |index, value| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_shortcut_entries_model(&w);
                if let Some(mut entry) = model.row_data(index as usize) {
                    entry.target = value;
                    model.set_row_data(index as usize, entry);
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_shortcut_args_changed(move |index, value| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_shortcut_entries_model(&w);
                if let Some(mut entry) = model.row_data(index as usize) {
                    entry.args = value;
                    model.set_row_data(index as usize, entry);
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_shortcut_icon_changed(move |index, value| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_shortcut_entries_model(&w);
                if let Some(mut entry) = model.row_data(index as usize) {
                    entry.icon = value;
                    model.set_row_data(index as usize, entry);
                }
            }
        });
    }
}

fn setup_registry_entries(window: &PackagerWindow, _state: &Arc<Mutex<AppState>>) {
    {
        let window_weak = window.as_weak();
        window.on_add_registry_entry(move || {
            if let Some(w) = window_weak.upgrade() {
                let model = get_registry_entries_model(&w);
                model.push(RegistryEntryData {
                    key: SharedString::new(),
                    values: SharedString::new(),
                });
                w.set_registry_entries(ModelRc::from(model));
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_remove_registry_entry(move |index| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_registry_entries_model(&w);
                if (index as usize) < model.row_count() {
                    model.remove(index as usize);
                    w.set_registry_entries(ModelRc::from(model));
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_registry_key_changed(move |index, value| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_registry_entries_model(&w);
                if let Some(mut entry) = model.row_data(index as usize) {
                    entry.key = value;
                    model.set_row_data(index as usize, entry);
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_registry_values_changed(move |index, value| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_registry_entries_model(&w);
                if let Some(mut entry) = model.row_data(index as usize) {
                    entry.values = value;
                    model.set_row_data(index as usize, entry);
                }
            }
        });
    }
}

fn setup_run_entries(window: &PackagerWindow, _state: &Arc<Mutex<AppState>>) {
    {
        let window_weak = window.as_weak();
        window.on_add_run_entry(move || {
            if let Some(w) = window_weak.upgrade() {
                let model = get_run_entries_model(&w);
                model.push(RunEntryData {
                    cmd: SharedString::new(),
                    args: SharedString::new(),
                    when_index: 0,
                });
                w.set_run_entries(ModelRc::from(model));
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_remove_run_entry(move |index| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_run_entries_model(&w);
                if (index as usize) < model.row_count() {
                    model.remove(index as usize);
                    w.set_run_entries(ModelRc::from(model));
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_run_cmd_changed(move |index, value| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_run_entries_model(&w);
                if let Some(mut entry) = model.row_data(index as usize) {
                    entry.cmd = value;
                    model.set_row_data(index as usize, entry);
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_run_args_changed(move |index, value| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_run_entries_model(&w);
                if let Some(mut entry) = model.row_data(index as usize) {
                    entry.args = value;
                    model.set_row_data(index as usize, entry);
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_run_when_changed(move |index, value| {
            if let Some(w) = window_weak.upgrade() {
                let model = get_run_entries_model(&w);
                if let Some(mut entry) = model.row_data(index as usize) {
                    entry.when_index = value;
                    model.set_row_data(index as usize, entry);
                }
            }
        });
    }
}

fn setup_browse_callbacks(window: &PackagerWindow) {
    {
        let window_weak = window.as_weak();
        window.on_browse_icon(move || {
            if let Some(w) = window_weak.upgrade() {
                let dialog = rfd::FileDialog::new()
                    .set_title("Select application icon")
                    .add_filter("Icon files", &["ico", "png"]);
                if let Some(path) = dialog.pick_file() {
                    w.set_app_icon(path.display().to_string().into());
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_browse_license(move || {
            if let Some(w) = window_weak.upgrade() {
                let dialog = rfd::FileDialog::new()
                    .set_title("Select license file")
                    .add_filter("Text files", &["txt", "md", "rtf"]);
                if let Some(path) = dialog.pick_file() {
                    w.set_license_file(path.display().to_string().into());
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_browse_stub(move || {
            if let Some(w) = window_weak.upgrade() {
                let dialog = rfd::FileDialog::new()
                    .set_title("Select runtime stub")
                    .add_filter("Executable", &["exe"]);
                if let Some(path) = dialog.pick_file() {
                    w.set_stub_path(path.display().to_string().into());
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_browse_stub_admin(move || {
            if let Some(w) = window_weak.upgrade() {
                let dialog = rfd::FileDialog::new()
                    .set_title("Select admin runtime stub")
                    .add_filter("Executable", &["exe"]);
                if let Some(path) = dialog.pick_file() {
                    w.set_stub_admin_path(path.display().to_string().into());
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_browse_output(move || {
            if let Some(w) = window_weak.upgrade() {
                let dialog = rfd::FileDialog::new()
                    .set_title("Save installer as")
                    .add_filter("Executable", &["exe"])
                    .set_file_name("setup.exe");
                if let Some(path) = dialog.save_file() {
                    w.set_output_path(path.display().to_string().into());
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_browse_config(move || {
            if let Some(w) = window_weak.upgrade() {
                let dialog = rfd::FileDialog::new()
                    .set_title("Select install.toml")
                    .add_filter("TOML config", &["toml"]);
                if let Some(path) = dialog.pick_file() {
                    w.set_config_path(path.display().to_string().into());
                }
            }
        });
    }
}

fn setup_config_io(window: &PackagerWindow, state: &Arc<Mutex<AppState>>) {
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_save_config(move || {
            if let Some(w) = window_weak.upgrade() {
                let config = collect_config_from_ui(&w);
                let toml_str = match toml::to_string_pretty(&config) {
                    Ok(s) => s,
                    Err(e) => {
                        w.set_build_status(format!("Failed to serialize config: {e}").into());
                        return;
                    }
                };

                let initial_dir = state.lock().ok().and_then(|s| s.last_config_dir.clone());
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Save install.toml")
                    .add_filter("TOML config", &["toml"])
                    .set_file_name("install.toml");
                if let Some(dir) = &initial_dir {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(path) = dialog.save_file() {
                    if let Some(parent) = path.parent() {
                        if let Ok(mut s) = state.lock() {
                            s.last_config_dir = Some(parent.to_path_buf());
                        }
                    }
                    match std::fs::write(&path, toml_str) {
                        Ok(()) => {
                            w.set_config_path(path.display().to_string().into());
                            w.set_build_status(format!("Config saved to {}", path.display()).into());
                        }
                        Err(e) => {
                            w.set_build_status(format!("Failed to save: {e}").into());
                        }
                    }
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_load_config(move || {
            if let Some(w) = window_weak.upgrade() {
                let initial_dir = state.lock().ok().and_then(|s| s.last_config_dir.clone());
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Open install.toml")
                    .add_filter("TOML config", &["toml"]);
                if let Some(dir) = &initial_dir {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(path) = dialog.pick_file() {
                    if let Some(parent) = path.parent() {
                        if let Ok(mut s) = state.lock() {
                            s.last_config_dir = Some(parent.to_path_buf());
                        }
                    }
                    match load_config_file(&path) {
                        Ok(config) => {
                            apply_config_to_ui(&w, &config);
                            w.set_config_path(path.display().to_string().into());
                            w.set_build_status(format!("Loaded {}", path.display()).into());
                        }
                        Err(e) => {
                            w.set_build_status(format!("Failed to load: {e}").into());
                        }
                    }
                }
            }
        });
    }
}

fn setup_build(window: &PackagerWindow, _state: &Arc<Mutex<AppState>>) {
    let window_weak = window.as_weak();
    window.on_build_installer(move || {
        let Some(w) = window_weak.upgrade() else { return };

        let config_path = w.get_config_path().to_string();
        let stub_path = w.get_stub_path().to_string();
        let stub_admin_path = w.get_stub_admin_path().to_string();
        let output_path = w.get_output_path().to_string();

        if config_path.trim().is_empty() {
            w.set_build_status("Please specify or save a config file first.".into());
            return;
        }
        if stub_path.trim().is_empty() {
            w.set_build_status("Please specify the runtime stub path.".into());
            return;
        }
        if output_path.trim().is_empty() {
            w.set_build_status("Please specify the output file path.".into());
            return;
        }

        w.set_building(true);
        w.set_build_progress(0.1);
        w.set_build_status("Building installer...".into());
        w.set_build_log(String::new().into());

        let window_weak2 = w.as_weak();
        std::thread::spawn(move || {
            let config = PathBuf::from(&config_path);
            let stub = PathBuf::from(&stub_path);
            let stub_admin = if stub_admin_path.trim().is_empty() {
                None
            } else {
                Some(PathBuf::from(&stub_admin_path))
            };
            let output = PathBuf::from(&output_path);

            let update_progress = |progress: f32, msg: &str| {
                let message = msg.to_string();
                let _ = window_weak2.upgrade_in_event_loop(move |w| {
                    w.set_build_progress(progress);
                    w.set_build_status(message.into());
                });
            };

            update_progress(0.2, "Reading config...");

            let result = setupweaver_packager::builder::build_installer(
                &config,
                &stub,
                stub_admin.as_deref(),
                &output,
            );

            match result {
                Ok(()) => {
                    let output_display = output.display().to_string();
                    let size = std::fs::metadata(&output)
                        .map(|m| format!("{:.2} MB", m.len() as f64 / (1024.0 * 1024.0)))
                        .unwrap_or_else(|_| String::from("unknown size"));
                    let _ = window_weak2.upgrade_in_event_loop(move |w| {
                        w.set_building(false);
                        w.set_build_progress(1.0);
                        w.set_build_status(format!("Build successful: {output_display} ({size})").into());
                        w.set_build_log(format!("Output: {output_display}\nSize: {size}\n\nInstaller built successfully.").into());
                    });
                }
                Err(e) => {
                    let error_msg = format!("{e:#}");
                    let _ = window_weak2.upgrade_in_event_loop(move |w| {
                        w.set_building(false);
                        w.set_build_progress(0.0);
                        w.set_build_status(format!("Build failed: {error_msg}").into());
                        w.set_build_log(format!("Error:\n{error_msg}").into());
                    });
                }
            }
        });
    });
}

fn collect_config_from_ui(w: &PackagerWindow) -> InstallConfig {
    let theme = match w.get_theme_index() {
        0 => UiTheme::Dark,
        1 => UiTheme::Light,
        _ => UiTheme::System,
    };

    let files: Vec<FileSpec> = {
        let model = w.get_file_entries();
        (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .map(|entry| FileSpec {
                src: entry.src.to_string(),
                dest: entry.dest.to_string(),
                exclude: entry
                    .exclude
                    .to_string()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
            })
            .collect()
    };

    let shortcuts: Vec<ShortcutSpec> = {
        let model = w.get_shortcut_entries();
        (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .map(|entry| ShortcutSpec {
                name: entry.name.to_string(),
                target: entry.target.to_string(),
                args: entry.args.to_string(),
                icon: entry.icon.to_string(),
            })
            .collect()
    };

    let registry: Vec<RegistryKeySpec> = {
        let model = w.get_registry_entries();
        (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .map(|entry| {
                let values = parse_registry_values(&entry.values.to_string());
                RegistryKeySpec {
                    key: entry.key.to_string(),
                    values,
                }
            })
            .collect()
    };

    let run: Vec<RunSpec> = {
        let model = w.get_run_entries();
        (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .map(|entry| RunSpec {
                cmd: entry.cmd.to_string(),
                args: entry.args.to_string(),
                when: if entry.when_index == 1 {
                    RunWhen::Finish
                } else {
                    RunWhen::After
                },
            })
            .collect()
    };

    let publisher = {
        let v = w.get_app_publisher().to_string();
        if v.trim().is_empty() { None } else { Some(v) }
    };
    let description = {
        let v = w.get_app_description().to_string();
        if v.trim().is_empty() { None } else { Some(v) }
    };
    let icon = {
        let v = w.get_app_icon().to_string();
        if v.trim().is_empty() { None } else { Some(v) }
    };
    let accent_color = {
        let v = w.get_accent_color().to_string();
        if v.trim().is_empty() { None } else { Some(v) }
    };
    let welcome_text = {
        let v = w.get_welcome_text().to_string();
        if v.trim().is_empty() { None } else { Some(v) }
    };
    let license_file = {
        let v = w.get_license_file().to_string();
        if v.trim().is_empty() { None } else { Some(v) }
    };

    InstallConfig {
        app: AppSection {
            name: w.get_app_name().to_string(),
            version: w.get_app_version().to_string(),
            publisher,
            icon,
            description,
        },
        install: InstallSection {
            default_dir: w.get_default_dir().to_string(),
            add_to_path: w.get_add_to_path(),
            create_desktop_shortcut: w.get_create_desktop_shortcut(),
            require_admin: w.get_require_admin(),
        },
        ui: UiSection {
            theme,
            accent_color,
            welcome_text,
            license_file,
        },
        files,
        shortcuts,
        registry,
        run,
    }
}

fn apply_config_to_ui(w: &PackagerWindow, config: &InstallConfig) {
    w.set_app_name(config.app.name.clone().into());
    w.set_app_version(config.app.version.clone().into());
    w.set_app_publisher(config.app.publisher.clone().unwrap_or_default().into());
    w.set_app_description(config.app.description.clone().unwrap_or_default().into());
    w.set_app_icon(config.app.icon.clone().unwrap_or_default().into());

    w.set_default_dir(config.install.default_dir.clone().into());
    w.set_add_to_path(config.install.add_to_path);
    w.set_create_desktop_shortcut(config.install.create_desktop_shortcut);
    w.set_require_admin(config.install.require_admin);

    w.set_theme_index(match config.ui.theme {
        UiTheme::Dark => 0,
        UiTheme::Light => 1,
        UiTheme::System => 2,
    });
    w.set_accent_color(config.ui.accent_color.clone().unwrap_or_else(|| String::from("#7c3aed")).into());
    w.set_welcome_text(config.ui.welcome_text.clone().unwrap_or_default().into());
    w.set_license_file(config.ui.license_file.clone().unwrap_or_default().into());

    // Files
    let file_model = std::rc::Rc::new(VecModel::default());
    for spec in &config.files {
        file_model.push(FileEntryData {
            src: SharedString::from(spec.src.as_str()),
            dest: SharedString::from(spec.dest.as_str()),
            exclude: SharedString::from(spec.exclude.join(", ")),
        });
    }
    w.set_file_entries(ModelRc::from(file_model));

    // Shortcuts
    let shortcut_model = std::rc::Rc::new(VecModel::default());
    for spec in &config.shortcuts {
        shortcut_model.push(ShortcutEntryData {
            name: SharedString::from(spec.name.as_str()),
            target: SharedString::from(spec.target.as_str()),
            args: SharedString::from(spec.args.as_str()),
            icon: SharedString::from(spec.icon.as_str()),
        });
    }
    w.set_shortcut_entries(ModelRc::from(shortcut_model));

    // Registry
    let registry_model = std::rc::Rc::new(VecModel::default());
    for spec in &config.registry {
        let values_str = spec
            .values
            .iter()
            .map(|v| format!("{}:{}:{}", v.name, format_value_type(v.value_type), v.data))
            .collect::<Vec<_>>()
            .join(", ");
        registry_model.push(RegistryEntryData {
            key: SharedString::from(spec.key.as_str()),
            values: SharedString::from(values_str),
        });
    }
    w.set_registry_entries(ModelRc::from(registry_model));

    // Run hooks
    let run_model = std::rc::Rc::new(VecModel::default());
    for spec in &config.run {
        run_model.push(RunEntryData {
            cmd: SharedString::from(spec.cmd.as_str()),
            args: SharedString::from(spec.args.as_str()),
            when_index: match spec.when {
                RunWhen::After => 0,
                RunWhen::Finish => 1,
            },
        });
    }
    w.set_run_entries(ModelRc::from(run_model));
}

fn load_config_file(path: &Path) -> Result<InstallConfig> {
    let content = std::fs::read_to_string(path)?;
    let config = InstallConfig::parse(&content).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(config)
}

fn parse_registry_values(input: &str) -> Vec<RegistryValueSpec> {
    input
        .split(',')
        .filter_map(|entry| {
            let parts: Vec<&str> = entry.trim().splitn(3, ':').collect();
            if parts.len() == 3 {
                let value_type = match parts[1].trim().to_lowercase().as_str() {
                    "dword" => RegistryValueType::Dword,
                    "qword" => RegistryValueType::Qword,
                    _ => RegistryValueType::String,
                };
                Some(RegistryValueSpec {
                    name: parts[0].trim().to_string(),
                    value_type,
                    data: parts[2].trim().to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}

fn format_value_type(vt: RegistryValueType) -> &'static str {
    match vt {
        RegistryValueType::String => "string",
        RegistryValueType::Dword => "dword",
        RegistryValueType::Qword => "qword",
    }
}

use slint::Model;

fn get_file_entries_model(w: &PackagerWindow) -> std::rc::Rc<VecModel<FileEntryData>> {
    let current = w.get_file_entries();
    let model = std::rc::Rc::new(VecModel::default());
    for i in 0..current.row_count() {
        if let Some(entry) = current.row_data(i) {
            model.push(entry);
        }
    }
    model
}

fn get_shortcut_entries_model(w: &PackagerWindow) -> std::rc::Rc<VecModel<ShortcutEntryData>> {
    let current = w.get_shortcut_entries();
    let model = std::rc::Rc::new(VecModel::default());
    for i in 0..current.row_count() {
        if let Some(entry) = current.row_data(i) {
            model.push(entry);
        }
    }
    model
}

fn get_registry_entries_model(w: &PackagerWindow) -> std::rc::Rc<VecModel<RegistryEntryData>> {
    let current = w.get_registry_entries();
    let model = std::rc::Rc::new(VecModel::default());
    for i in 0..current.row_count() {
        if let Some(entry) = current.row_data(i) {
            model.push(entry);
        }
    }
    model
}

fn get_run_entries_model(w: &PackagerWindow) -> std::rc::Rc<VecModel<RunEntryData>> {
    let current = w.get_run_entries();
    let model = std::rc::Rc::new(VecModel::default());
    for i in 0..current.row_count() {
        if let Some(entry) = current.row_data(i) {
            model.push(entry);
        }
    }
    model
}
