# SetupWeaver architecture

## Workspace

```text
SetupWeaver/
├── common/         # schema + packaged manifest types
├── packager/       # install.toml -> setup.exe
├── runtime/        # embedded stub + extraction engine
├── runtime-admin/  # alternate stub with requireAdministrator manifest
└── examples/       # reference configs
```

## Data flow

```text
install.toml
    |
    v
packager
  - parse + validate
  - expand globs
  - inline license text
  - select user/admin runtime stub
  - compress each file into its own zstd frame
  - build indexed manifest
  - append 8-byte offset trailer
    |
    v
setup.exe = [selected runtime stub][indexed payload][u64 offset]
    |
    v
runtime
  - mmap own exe
  - read trailer offset
  - validate payload header
  - parse manifest directly from mmap
  - extract files
  - write registry
  - mutate PATH
  - create desktop shortcuts
  - run hooks
```

## Payload layout

```text
payload =
  [8-byte magic]
  [u64 manifest_len]
  [manifest.toml]
  [zstd frame for file 0]
  [zstd frame for file 1]
  ...
```

Each `PackagedFile` stores:

```text
payload_offset   # relative to first compressed frame
compressed_size
size
destination
```

## Module boundaries

```text
common::config
  - InstallConfig
  - validation

common::packaged
  - PackagedInstaller
  - PackagedFile
  - payload constants

packager::builder
  - collect_payload()
  - compress_payload_files()
  - build_manifest()
  - build_payload_bytes()
  - build_installer()

runtime::payload
  - EmbeddedPayload::from_current_exe()
  - EmbeddedPayload::read_manifest()
  - EmbeddedPayload::payload_file_bytes()

runtime::engine
  - InstallerEngine::install()
  - parallel silent extraction
  - progress-aware GUI extraction
  - token expansion
  - registry writes
  - PATH mutation
  - desktop shortcut creation
  - run hooks

runtime::ui
  - Slint wizard flow
  - worker-thread install orchestration
  - close protection during install
  - progress + error surfaces

runtime-admin
  - alternate stub package
  - embeds requireAdministrator manifest
```

## UAC strategy

```text
require_admin = false -> setupweaver-runtime.exe
require_admin = true  -> setupweaver-runtime-admin.exe
```

This keeps UAC elevation in an embedded manifest without patching PE resources per package.

## UI flow

```text
Welcome -> License? -> Install -> Finish
                      \-> Error
```

## Current trade-off

### Option
Indexed manifest + per-file zstd frames.

### Upside
- manifest loads directly from mmap
- silent installs can extract files in parallel with rayon
- true random access to individual files
- still one-file output

### Downside
- packager currently holds compressed frames in memory before writing
- GUI path stays sequential for stable progress updates
- slightly worse compression than one giant shared stream on some payloads

### Recommendation
Keep this as the default v2 payload.
Next perf step: chunk very large files into multiple frames to unlock intra-file parallel extraction.

## Binary size note

### Option
Slint + winit + software renderer.

### Upside
- premium native UI
- no webview
- single-binary friendly
- deterministic rendering

### Downside
- current optimized runtime stub is still above the < 3 MB target on this toolchain

### Recommendation
Keep this as the UX baseline.
For the size target, evaluate one of:
- aggressive Slint feature pruning / custom backend
- split tiny bootstrapper + compressed UI runtime block
- custom Win32 shell around a smaller rendering surface

## Perf targets

- cold start UI visible: < 200 ms on HDD
- manifest read from trailer: < 10 ms for 1 GB installer
- packager throughput: > 250 MB/s input scan on NVMe
- extract 500 MB payload:
  - NVMe silent mode: < 4 s
  - SATA SSD silent mode: < 7 s
  - HDD silent mode: < 18 s
