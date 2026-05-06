// packager/tests/e2e.rs
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn installer_can_install_and_uninstall_silently() {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root must exist")
        .to_path_buf();
    let target_dir = workspace_root.join("target").join("debug");
    let runtime = target_dir.join("setupweaver-runtime.exe");
    let runtime_admin = target_dir.join("setupweaver-runtime-admin.exe");

    let status = Command::new("cargo")
        .args(["build", "-p", "setupweaver-runtime", "-p", "setupweaver-runtime-admin"])
        .current_dir(&workspace_root)
        .status()
        .expect("cargo build should start");
    assert!(status.success(), "cargo build failed: {status:?}");

    let temp_root = unique_temp_dir();
    std::fs::create_dir_all(&temp_root).expect("temp dir should be created");
    let setup_exe = temp_root.join("hello-setup.exe");
    let install_dir = temp_root.join("install-root");
    let app_dir = temp_root.join("app");
    std::fs::create_dir_all(&app_dir).expect("app dir should be created");
    std::fs::write(app_dir.join("hello.txt"), b"hello setupweaver\n").expect("sample payload should be written");
    let config_path = temp_root.join("install.toml");
    std::fs::write(
        &config_path,
        format!(
            concat!(
                "[app]\n",
                "name = \"Hello App\"\n",
                "version = \"1.0.0\"\n\n",
                "[install]\n",
                "default_dir = \"{{ProgramFiles}}\\\\Hello App\"\n",
                "add_to_path = false\n",
                "create_desktop_shortcut = false\n",
                "require_admin = false\n\n",
                "[[files]]\n",
                "src = \"{}\"\n",
                "dest = \"{{install_dir}}\"\n"
            ),
            app_dir.join("*").to_string_lossy().replace('\\', "/")
        ),
    )
    .expect("config should be written");

    let status = Command::new(env!("CARGO_BIN_EXE_setupweaver-packager"))
        .args([
            "build",
            "--config",
            config_path.to_string_lossy().as_ref(),
            "--stub",
            runtime.to_string_lossy().as_ref(),
            "--stub-admin",
            runtime_admin.to_string_lossy().as_ref(),
            "--output",
            setup_exe.to_string_lossy().as_ref(),
        ])
        .current_dir(&workspace_root)
        .status()
        .expect("packager should start");
    assert!(status.success(), "packager failed: {status:?}");

    let status = Command::new(&setup_exe)
        .args(["--silent", "--install-dir", install_dir.to_string_lossy().as_ref()])
        .status()
        .expect("installer should start");
    assert!(status.success(), "installer failed: {status:?}");

    assert!(install_dir.join("hello.txt").exists());
    let uninstall_exe = install_dir.join(".setupweaver").join("uninstall.exe");
    let state_file = install_dir.join(".setupweaver").join("install-state.toml");
    assert!(uninstall_exe.exists());
    assert!(state_file.exists());

    let status = Command::new(&setup_exe)
        .args(["--uninstall", "--install-dir", install_dir.to_string_lossy().as_ref()])
        .status()
        .expect("uninstaller should start");
    assert!(status.success(), "uninstaller failed: {status:?}");

    wait_until_missing(&install_dir.join("hello.txt"));
    wait_until_missing(&state_file);
    wait_until_missing(&uninstall_exe);

    let _ = std::fs::remove_dir_all(&temp_root);
}

fn unique_temp_dir() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should move forward")
        .as_nanos();
    std::env::temp_dir().join(format!("setupweaver-e2e-{}-{timestamp}", std::process::id()))
}

fn wait_until_missing(path: &Path) {
    for _ in 0..20 {
        if !path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(200));
    }
    panic!("path still exists: {}", path.display());
}
