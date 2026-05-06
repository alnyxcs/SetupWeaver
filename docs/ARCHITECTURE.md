# SetupWeaver architecture

## Workspace

```text
SetupWeaver/
├── common/      # schema + packaged manifest types
├── packager/    # install.toml -> setup.exe
├── runtime/     # embedded stub + extraction engine
└── examples/    # reference configs
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
  - build tar payload
  - zstd compress
  - append 8-byte offset trailer
    |
    v
setup.exe = [selected runtime stub][payload.zst][u64 offset]
    |
    v
runtime
  - mmap own exe
  - read trailer offset
  - stream zstd -> tar
  - parse manifest first
  - extract files
  - write registry
  - mutate PATH
  - create desktop shortcuts
  - run hooks
```

## Module boundaries

```text
common::config
  - InstallConfig
  - validation

common::packaged
  - PackagedInstaller
  - PackagedFile

packager::builder
  - collect_payload()
  - build_archive()
  - build_installer()

runtime::payload
  - EmbeddedPayload::from_current_exe()
  - EmbeddedPayload::read_manifest()

runtime::engine
  - InstallerEngine::install()
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
Single zstd-compressed tar stream.

### Upside
- simplest format
- tiny implementation
- great compression ratio
- fast sequential HDD reads

### Downside
- true parallel extraction is not possible without an index or per-file frames
- random access is limited to manifest-at-front scanning

### Recommendation
Keep v1 as tar+zstd for simplicity.
Plan v2 payload format as:

```text
[manifest][file table][zstd frames per file][offset]
```

That unlocks rayon-based parallel extraction while preserving one-file output.

## Binary size note

### Option
Slint + winit + software renderer.

### Upside
- premium native UI
- no webview
- single-binary friendly
- deterministic rendering

### Downside
- current optimized runtime stub is ~7.5 MB release on this toolchain
- misses the < 3 MB target

### Recommendation
Keep this as the UX baseline.
For the size target, evaluate one of:
- aggressive Slint feature pruning / custom backend
- split tiny bootstrapper + compressed UI runtime block
- custom Win32 shell around a smaller rendering surface

## Perf targets

- cold start UI visible: < 200 ms on HDD
- manifest read from trailer: < 30 ms for 1 GB installer
- packager throughput: > 250 MB/s input scan on NVMe
- extract 500 MB payload:
  - NVMe: < 4 s
  - SATA SSD: < 7 s
  - HDD: < 18 s
