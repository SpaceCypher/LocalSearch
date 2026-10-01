# LocalSearch

LocalSearch is a native macOS desktop search application with:

- A Rust search/index core
- A SwiftUI/AppKit frontend
- A C-ABI FFI bridge between the two

This repository is a workspace containing both backend and frontend code.

## What Is In This Repo

Top-level folders you will use most often:

- `src/` - Rust search/index engine and FFI exports
- `frontend/` - Swift package for the macOS app
- `extractor-xpc/` - companion Rust crate
- `docs/` - plans, audits, and progress logs
- `tests/` and `benches/` - backend validation and benchmarking

## Architecture (Current)

### Backend (Rust)

`src/engine.rs` is the live engine. It owns the index and ties the other modules together:

- **Indexing** (`fs/`, `extract/`): walks the configured folders, indexes names, then extracts and indexes file contents (plain text, source code, and PDFs via PDFKit). PDFs are parsed in the `localsearch-extractor` helper process (`extractor-xpc/`), so a parser crash or hang costs one file, not the app.
- **Persistence** (`index/segment.rs`, `wal/`, `fs/identity.rs`): the index is snapshotted to a checksummed segment file; filesystem changes since the snapshot are logged to a write-ahead log and replayed on the next start; SQLite maps files to stable document IDs.
- **Live updates** (`fs/events.rs`): FSEvents drive incremental updates while the app runs. A reconcile walk on start picks up anything that changed while it was not running.
- **Queries** (`query/`): answered from memory — BM25 over names and content (calibrated from the real corpus), typo and phonetic tolerance, filename matching, and a boost for files opened before. Spotlight (`mdfind`) is consulted only while the first index is still being built.
- **C ABI** (`ffi.rs`): `localsearch_query`, `localsearch_free_results`, `localsearch_record_click`, `localsearch_configure`, `localsearch_index_status`, `localsearch_shutdown`.

The crate is built as both `rlib` and `cdylib` (see `Cargo.toml`), and the Swift app loads the `cdylib`.

### Frontend (SwiftUI + AppKit)

The frontend is a Swift Package executable that:

- Presents a Spotlight-like floating panel UI
- Handles keyboard shortcuts and app/window behavior
- Calls into Rust through `FFIBackend`
- Has a Settings window, including which folders are indexed

If the engine library cannot be loaded, the app says so in the panel and in Settings instead of returning empty results.

Shared spacing, type, colour and motion values live in `frontend/Sources/System/DesignTokens.swift`.

## Prerequisites

- macOS 14+
- Xcode 15+ (Swift 5.10 toolchain)
- Rust stable toolchain (`rustup`, `cargo`)

## Quick Start (End-to-End)

From the repository root:

```bash
frontend/run.sh
```

This builds the Rust engine and helper in release mode, builds the Swift app, assembles and signs `dist/LocalSearch.app`, and launches it. The first launch indexes your folders in the background; you can search while it runs.

## Packaging

```bash
scripts/package.sh
```

produces a self-contained `dist/LocalSearch.app` with the engine in `Contents/Frameworks` and the extractor helper in `Contents/MacOS`. By default it is ad-hoc signed, which is enough to run on the Mac that built it. To sign for distribution:

```bash
CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" scripts/package.sh
```

Notarization is not automated.

## Backend Loading Rules

`FFIBackend` looks for `liblocalsearch.dylib` in this order:

1. `LOCALSEARCH_DYLIB_PATH` (if set)
2. `Contents/Frameworks` of the running app bundle
3. `target/release`, then `target/debug`, of a development checkout

## What Gets Indexed

Defaults: `~/Downloads`, `~/Documents` and `~/Desktop`, 12 levels deep, skipping hidden files and common build/cache folder names, with file contents indexed.

Change this in **Settings → Indexing** (folders, skipped names, depth, content on/off). Changes apply to the running app.

File contents stop being indexed once the in-memory index reaches its budget (256 MB by default, `memory_budget_mb` in `config.json`); names are always indexed. Settings shows when the limit was reached.

Environment overrides, mainly for development:

- `LOCALSEARCH_ROOT` — index only this folder
- `LOCALSEARCH_DATA_DIR` — where the index lives (default `~/.localsearch`)
- `LOCALSEARCH_EXTRACTOR_PATH` — path to the extractor helper

## Search Syntax

- `kind:pdf` or `kind:image` — only that file type or kind
- `in:projects` — only inside folders whose path contains the word
- `-draft` — leave out names containing the word

## Development Workflows

### Backend

```bash
cargo check
cargo test --workspace
```

### Frontend

```bash
cd frontend
swift build
swift test
```

### Command line

```bash
cargo run --release -- index            # build or update the index
cargo run --release -- query budget     # search it
cargo run --release -- --debug-panel    # index health, from the real index
```

## Benchmarks

`docs/benchmarks/spotlight.md` compares query latency, freshness and resource cost against Spotlight on one machine. Reproduce it with:

```bash
cargo run --release --example bench_vs_spotlight > docs/benchmarks/spotlight.md
```

It builds its own index in a temporary directory, takes several minutes, and reports timings and counts only (no file names).

## Troubleshooting

### The panel says "Search isn't available"

The engine library was not found. Run `frontend/run.sh` (or `cargo build --release`) and relaunch.

### A file is not found

- Check its folder is listed in Settings → Indexing and its name is not under "Skipped names".
- While Settings shows "Indexing", newer files may not be in yet.
- If Settings shows "content limit reached", the file's name is indexed but its contents may not be.

### Global hotkey does not work

Grant Accessibility permission in macOS:

- System Settings -> Privacy & Security -> Accessibility
- Allow the app

## Current Documentation

For implementation history:

- `docs/progress.md`
- `docs/audit-2026-04-07.md` (per-component audit; predates the engine integration)
- `context/implementation_spec.md`

## License

Copyright (c) 2026 SpaceCypher. All rights reserved.
