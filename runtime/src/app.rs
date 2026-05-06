// runtime/src/app.rs
use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{bail, Result};

use crate::{engine::InstallerEngine, ui};

#[derive(Debug, Default)]
struct Cli {
    print_manifest: bool,
    silent: bool,
    install_dir: Option<PathBuf>,
}

pub fn run() -> Result<()> {
    let cli = parse_cli()?;
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

fn parse_cli() -> Result<Cli> {
    let mut cli = Cli::default();
    let mut args = std::env::args_os();
    let program = args.next().unwrap_or_else(|| OsString::from("setupweaver-runtime"));

    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--print-manifest" => cli.print_manifest = true,
            "--silent" => cli.silent = true,
            "--install-dir" => {
                let value = args.next().ok_or_else(|| anyhow::anyhow!("--install-dir requires a value"))?;
                cli.install_dir = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                print_help(&program);
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!(env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    Ok(cli)
}

fn print_help(program: &OsString) {
    let program = program.to_string_lossy();
    println!(
        concat!(
            "SetupWeaver runtime stub\n\n",
            "Usage:\n",
            "  {program} [--silent] [--install-dir <path>]\n",
            "  {program} --print-manifest\n\n",
            "Options:\n",
            "  --silent            Install without showing the Slint UI\n",
            "  --install-dir PATH  Override the default install directory\n",
            "  --print-manifest    Print embedded manifest data and exit\n",
            "  -h, --help          Show this help\n",
            "  -V, --version       Show version\n"
        ),
        program = program,
    );
}

#[cfg(test)]
mod tests {
    use super::print_help;

    #[test]
    fn help_printer_accepts_program_name() {
        print_help(&"setupweaver-runtime".into());
    }
}
