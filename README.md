# SetupWeaver

Blazing-fast modern Windows installer builder in Rust.

SetupWeaver takes a simple `install.toml` and emits a single self-contained `setup.exe`:

```text
[runtime stub][zstd-compressed payload][8-byte payload offset]
```

## Goals

- cold start UI under 200 ms on HDD
- one-file output
- readable config
- modern installer UI
- no .NET, no external unpacker, no network by default

## Workspace

```text
SetupWeaver/
├── common/      # shared schema + packaged manifest types
├── packager/    # install.toml -> setup.exe
├── runtime/     # embedded runtime stub + UI + install engine
├── examples/    # sample packages
└── docs/        # architecture notes
```

## Current status

Implemented:

- TOML parsing + validation
- trailer-based single-file payload format
- indexed payload manifest + per-file zstd frames
- runtime payload mmap + zero-copy manifest loading
- file extraction
- registry writes
- PATH mutation
- desktop shortcut creation
- post-install hooks
- Slint-based installer wizard
- `--silent` runtime mode
- dual runtime stubs for normal/admin installers

Known issue:

- release `packager.exe` fits target well
- release `runtime.exe` is still above the long-term `< 3 MB` target with current Slint+winit software-renderer stack
- GUI installs keep sequential extraction for smooth progress reporting; silent installs use the fast path

## Build

```bash
cargo build --release \
  -p setupweaver-packager \
  -p setupweaver-runtime \
  -p setupweaver-runtime-admin
```

## Example

Sample configs:

- `examples/hello/install.toml`
- `examples/hello/install-admin.toml`

Build installer:

```bash
./target/release/setupweaver-packager.exe build \
  --config examples/hello/install.toml \
  --stub ./target/release/setupweaver-runtime.exe \
  --stub-admin ./target/release/setupweaver-runtime-admin.exe \
  --output ./target/release/hello-setup.exe
```

Inspect embedded manifest:

```bash
./target/release/hello-setup.exe --print-manifest
```

Silent install:

```bash
./target/release/hello-setup.exe --silent
```

If `install.require_admin = true`, the packager automatically switches to the admin stub and preserves the embedded `requireAdministrator` manifest.

Quoted post-install args with spaces are supported. Example:

```toml
[[run]]
cmd = "{install_dir}\\HelloApp.exe"
args = "--profile \"safe install\" --root \"{install_dir}\\data\""
when = "finish"
```

## install.toml shape

```toml
[app]
name = "My App"
version = "1.0.0"
publisher = "Acme"

[install]
default_dir = "{ProgramFiles}\\My App"
add_to_path = false
create_desktop_shortcut = true
require_admin = false

[ui]
theme = "system"
accent_color = "#7c3aed"
welcome_text = "Welcome"

[[files]]
src = "app/**/*"
dest = "{install_dir}"
exclude = ["*.pdb"]
```

## Architecture docs

- `docs/ARCHITECTURE.md`

## Short roadmap

- reduce runtime stub size
- v2 indexed payload format for true parallel extraction
