// runtime/src/app.rs
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use crate::{engine::InstallerEngine, ui};

#[derive(Debug, Parser)]
#[command(author, version, about = "SetupWeaver runtime stub")]
struct Cli {
    #[arg(long)]
    print_manifest: bool,
    #[arg(long)]
    silent: bool,
    #[arg(long)]
    install_dir: Option<PathBuf>,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let engine = InstallerEngine::from_current_exe()?;

    if cli.print_manifest {
        println!("{:#?}", engine.manifest());
        return Ok(());
    }

    if cli.silent {
        engine.install(cli.install_dir.as_deref())?;
        engine.finish(cli.install_dir.as_deref())?;
        return Ok(());
    }

    slint::BackendSelector::new().backend_name("winit".into()).select()?;
    ui::run_installer(&engine, cli.install_dir.as_deref())
}
