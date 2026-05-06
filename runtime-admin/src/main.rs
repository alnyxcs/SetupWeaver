// runtime-admin/src/main.rs
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use anyhow::Result;

fn main() -> Result<()> {
    setupweaver_runtime::app::run()
}
