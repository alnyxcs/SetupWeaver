<h1 align="center">SetupWeaver</h1>

<p align="center">
  <b>Blazing-fast modern Windows installer builder written in Rust</b>
</p>

<p align="center">
  <a href="https://github.com/alnyx-dev/SetupWeaver/actions"><img src="https://github.com/alnyx-dev/SetupWeaver/actions/workflows/rust.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/alnyx-dev/SetupWeaver/releases"><img src="https://img.shields.io/github/v/release/alnyx-dev/SetupWeaver?label=release&color=blue" alt="Release"></a>
  <img src="https://img.shields.io/badge/platform-Windows-0078D6?logo=windows" alt="Windows">
  <img src="https://img.shields.io/badge/lang-Rust-dea584?logo=rust" alt="Rust">
</p>

---

SetupWeaver takes a simple `install.toml` config and your application files, and produces a **single self-contained `setup.exe`** — no .NET, no external unpacker, no network required.

## How it works

```
install.toml + app files  ──►  setupweaver-packager  ──►  setup.exe
```

The output binary has this layout:

```
[ runtime stub ][ MAGIC ][ manifest ][ zstd-compressed chunks ][ trailer ]
```

At install time the embedded runtime stub reads its own tail, decompresses the payload, and runs the full install — UI wizard, file extraction, registry, PATH, shortcuts, post-install hooks.

## Features

| Category | Details |
|---|---|
| **Single-file output** | One `setup.exe` with everything embedded |
| **Modern UI** | Dark-themed Slint wizard (Welcome → License → Install → Finish → Error) |
| **Fast** | Cold-start UI under 200 ms; zstd level 19 compression; parallel extraction in silent mode |
| **Config-driven** | Readable `install.toml` — no scripting needed |
| **Registry & PATH** | Write HKCU/HKLM keys, mutate PATH, auto-restore on failure |
| **Shortcuts** | Desktop shortcut creation |
| **Post-install hooks** | Run commands after install or on finish (`[[run]]` with `when = "after"` / `"finish"`) |
| **Silent mode** | `--silent` for unattended install, `--uninstall` for removal |
| **Rollback** | Files, shortcuts, registry, and PATH changes rolled back on failure |
| **In-place upgrade** | Safe reinstall over existing managed install (auto-cleans old files) |
| **Dual stubs** | Normal and `requireAdministrator` variants |
| **Streaming packager** | Low peak RAM — compressed chunks streamed to disk, not held in memory |
| **Visual packager GUI** | Slint-based builder with 8 screens — no command line needed |
| **CI-ready** | GitHub Actions: fmt, clippy, Linux + Windows builds, auto-release on tag |

## Getting started

### Prerequisites

- [Rust](https://rustup.rs/) (stable, latest)
- **Linux only** (for building/developing):
  ```bash
  sudo apt-get install -y pkg-config libfontconfig1-dev libxcb-render0-dev \
    libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libwayland-dev
  ```
- **Windows**: no extra dependencies

### Build

```bash
git clone https://github.com/alnyx-dev/SetupWeaver.git
cd SetupWeaver

# Build all tools
cargo build --release -p setupweaver-packager \
                      -p setupweaver-packager-gui \
                      -p setupweaver-runtime \
                      -p setupweaver-runtime-admin
```

This produces 4 binaries in `target/release/`:

| Binary | Description |
|---|---|
| `setupweaver-packager.exe` | CLI packager — builds installers from `install.toml` |
| `setupweaver-packager-gui.exe` | GUI packager — visual installer builder |
| `setupweaver-runtime.exe` | Installer stub (standard privileges) |
| `setupweaver-runtime-admin.exe` | Installer stub (admin privileges) |

### Create an installer

**Option A — CLI:**

```bash
setupweaver-packager build \
  --config install.toml \
  --stub setupweaver-runtime.exe \
  --stub-admin setupweaver-runtime-admin.exe \
  --output my-app-setup.exe
```

**Option B — GUI:**

Launch `setupweaver-packager-gui.exe` and fill in 8 screens: App Info, Install Settings, Files, UI & Branding, Shortcuts, Registry, Run Hooks, Build.

### Run the installer

```bash
# GUI install
my-app-setup.exe

# Silent install
my-app-setup.exe --silent

# Silent uninstall
my-app-setup.exe --uninstall --install-dir "C:\Program Files\My App"

# Inspect embedded manifest
my-app-setup.exe --print-manifest
```

> If `require_admin = true`, the packager automatically uses the admin stub with the embedded `requireAdministrator` manifest.

## Configuration

Create an `install.toml` in your project root:

```toml
[app]
name = "My App"
version = "1.0.0"
publisher = "Acme Inc."
description = "A great application"
icon = "app.ico"

[install]
default_dir = "{ProgramFiles}\\My App"
add_to_path = true
create_desktop_shortcut = true
require_admin = false

[ui]
theme = "system"          # "dark", "light", or "system"
accent_color = "#7c3aed"
welcome_text = "Welcome to My App installer"
license_file = "LICENSE.txt"

[[files]]
src = "bin/**/*"
dest = "{install_dir}"
exclude = ["*.pdb", "*.log"]

[[files]]
src = "docs/readme.txt"
dest = "{install_dir}\\docs"

[[shortcuts]]
name = "My App"
target = "{install_dir}\\myapp.exe"
args = ""
icon = "{install_dir}\\myapp.ico"

[[registry]]
key = "HKCU\\Software\\MyApp"

[[registry.values]]
name = "Version"
type = "string"
data = "1.0.0"

[[registry.values]]
name = "Flags"
type = "dword"
data = "1"

[[run]]
cmd = "{install_dir}\\myapp.exe"
args = "--setup"
when = "after"

[[run]]
cmd = "{install_dir}\\myapp.exe"
when = "finish"
```

### Variables

| Variable | Expands to |
|---|---|
| `{install_dir}` | User-chosen install directory |
| `{ProgramFiles}` | `C:\Program Files` or equivalent |

### Registry value types

`string`, `dword`, `qword`

## Project structure

```
SetupWeaver/
├── common/          # Shared types: InstallConfig, PackagedInstaller, validation
├── packager/        # CLI packager: install.toml + files → setup.exe
├── packager-gui/    # Visual installer builder (Slint UI)
├── runtime/         # Embedded stub: install engine + Slint wizard UI
├── runtime-admin/   # Admin stub (requireAdministrator manifest)
├── examples/        # Sample install.toml configs
│   └── hello/       # Hello App example
├── docs/            # Architecture documentation
├── .github/         # CI workflows (build, lint, release)
├── CONTRIBUTING.md  # Build, test, and contribution guide
└── CHANGELOG.md     # Release history
```

## Architecture

The installer binary format:

```
┌──────────────┬───────┬─────────────┬──────────────────┬────────────┬─────────┐
│ Runtime stub │ MAGIC │ manifest_len│  manifest (TOML) │ zstd chunks│ trailer │
│  (.exe)      │ 8 B   │    8 B      │  variable        │  variable  │   8 B   │
└──────────────┴───────┴─────────────┴──────────────────┴────────────┴─────────┘
```

- **Runtime stub**: self-contained Rust executable with Slint UI
- **Manifest**: TOML-serialized `PackagedInstaller` (config + file index + chunk offsets)
- **Payload**: zstd-compressed file chunks (8 MB per chunk, level 19)
- **Trailer**: 8-byte offset pointing to the start of the archive section

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full deep-dive.

## Roadmap

- [ ] Reduce runtime stub size (explore Slint feature pruning, custom Win32 UI)
- [ ] Delta updates for smaller upgrade packages
- [ ] Add/Remove Programs (ARP) registration
- [ ] Start Menu shortcuts
- [ ] Light/dark/system theme switching
- [ ] Browse button for install directory
- [ ] ETA display during installation
- [ ] Localization (EN, RU)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for build instructions, coding standards, and how to submit PRs.

## License

This project is open source. See the repository for license details.
