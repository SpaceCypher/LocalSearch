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

- **Indexing** (`fs/`, `extract/`): walks the configured folders, indexes names, then extracts and indexes file contents: plain text, source code, PDFs (PDFKit), and Word, RTF and OpenDocument files (AppKit importers). PDFs and documents are parsed in the `localsearch-extractor` helper process (`extractor-xpc/`), which sandboxes itself (no network, no file writes) before reading anything, so a parser crash, hang or exploit costs one file, not the app.
- **Storage** (`index/base.rs`, `index/delta.rs`, `wal/`, `fs/identity.rs`): the index is a compressed, memory-mapped set of files (a term dictionary plus delta-coded posting lists, with files numbered in folder order), so only the parts a query touches are in memory. Files changed since it was written are marked dead there and re-indexed into a small in-memory delta; a merge writes the next generation and empties the delta. Changes since the last merge are also logged to a write-ahead log and replayed on the next start; SQLite maps files to stable document IDs.
- **Live updates** (`fs/events.rs`): FSEvents drive incremental updates while the app runs. A reconcile walk on start picks up anything that changed while it was not running.
- **Queries** (`query/`): answered from memory — BM25 over names and content (calibrated from the real corpus), typo and phonetic tolerance, filename matching, and a boost for files opened before. Spotlight (`mdfind`) is consulted only while the first index is still being built.
- **C ABI** (`ffi.rs`): `localsearch_query`, `localsearch_free_results`, `localsearch_record_click`, `localsearch_configure`, `localsearch_index_status`, `localsearch_shutdown`.

**Failure policy.** Nothing in the engine may take the app down: panics are caught at the FFI boundary, the indexer restarts itself after a panic, locks are taken poison-tolerantly, and an unreadable or old-format snapshot is rebuilt from the filesystem rather than trusted. Only one process writes an index at a time; a second one searches the saved index read-only.

**Privacy.** The index holds the names and words of your files. It lives in `~/.localsearch`, readable only by you (mode 0700), and nothing is sent anywhere.

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

produces a self-contained `dist/LocalSearch.app` (engine in `Contents/Frameworks`, extractor helper in `Contents/MacOS`) and `dist/LocalSearch-<version>.zip`. The version comes from `Cargo.toml`; the build number is the commit count. By default the app is ad-hoc signed, which is enough to run on the Mac that built it.

To distribute it, sign with a Developer ID and notarize:

```bash
xcrun notarytool store-credentials localsearch --apple-id you@example.com --team-id TEAMID   # once
CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" NOTARY_PROFILE=localsearch scripts/package.sh
```

The notarization path has not been exercised yet (it needs an Apple Developer account).

CI (`.github/workflows/ci.yml`) runs the Rust and Swift tests on macOS and uploads an ad-hoc signed build.

## Backend Loading Rules

`FFIBackend` looks for `liblocalsearch.dylib` in this order:

1. `LOCALSEARCH_DYLIB_PATH` (if set)
2. `Contents/Frameworks` of the running app bundle
3. `target/release`, then `target/debug`, of a development checkout

## What Gets Indexed

Defaults: `~/Downloads`, `~/Documents` and `~/Desktop`, 12 levels deep, skipping hidden files and common build/cache folder names, with file contents indexed.

Change this in **Settings → Indexing** (folders, skipped names, depth, content on/off). Changes apply to the running app.

Contents are read from text and source files, PDFs, and Word/RTF/OpenDocument files (the first 64 KB of text in each). Spreadsheets, presentations, images and mail are indexed by name only.

There is no ceiling on how much content is indexed: the index lives on disk and only recent changes are held in memory.

Environment overrides, mainly for development:

- `LOCALSEARCH_ROOT` — index only this folder
- `LOCALSEARCH_DATA_DIR` — where the index lives (default `~/.localsearch`)
- `LOCALSEARCH_EXTRACTOR_PATH` — path to the extractor helper

## Search Syntax

- `kind:pdf` or `kind:image` — only that file type or kind
- `in:projects` — only inside folders whose path contains the word
- `-draft` — leave out names containing the word
- `after:2025-01-31`, `before:2025-06` — by modification date (`YYYY`, `YYYY-MM` or `YYYY-MM-DD`)
- `size:>10mb`, `size:<500kb` — by file size
- `tag:work` — files with that Finder tag

Misspellings are tolerated: if what you typed finds fewer than five results, the closest approximate matches are shown as well.

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

To review the UI without launching the app, render its main states to PNG files:

```bash
LOCALSEARCH_UI_SNAPSHOTS=/tmp/localsearch-ui swift test --filter UISnapshotTests
```

### Command line

```bash
cargo run --release -- index            # build or update the index
cargo run --release -- query budget     # search it
cargo run --release -- --debug-panel    # index health, from the real index
```

## Benchmarks

LocalSearch against Spotlight on the same three folders (59,658 items), Apple M3 with 8 GB RAM. Both are called in-process and capped at 100 results; times are medians in milliseconds.

| Query | LocalSearch | Spotlight, names only | Spotlight, names + contents |
|---|--:|--:|--:|
| `report` | 12.4 | 159 | 159 |
| `test` | 7.9 | 13.6 | 16.9 |
| `config` | 8.9 | 15.0 | 168 |
| `screenshot` | 1.2 | 152 | 156 |
| `re` (two letters) | 10.6 | **3.1** | **3.3** |
| `README.md` | 4.0 | 15.4 | 31.6 |
| `index.html` | 2.9 | 155 | 1563 |
| `meeting notes` | 14.1 | 181 | 186 |
| `function` (content word) | 4.7 | 13.5 | 170 |
| `zebrafish` (rare word) | 1.5 | 152 | 154 |
| `reprot` (typo) | 5.4 | 153 | 155 |
| **Middle of all 19 queries** | **6.6** | **152** | **158** |

LocalSearch is faster on 18 of the 19 queries; Spotlight wins on the two-letter prefix.

| | LocalSearch |
|---|--:|
| Slowest query (median) | 14.1 ms |
| Worst query while recent changes are merged into the index | 89 ms (20 ms in two earlier runs) |
| New file becomes findable | 0.46 s (Spotlight: about 1.9 s in two earlier runs) |
| First index, names searchable | 3.3 s |
| First index, names and contents | 67.9 s |
| File contents indexed (text, code, PDF, Word/RTF) | 100% |
| Memory with the index loaded | 54 MB |
| Index on disk | 22 MB |
| Loading the index at launch | 1.2 s |

What these numbers do not say:

- One machine and one set of files.
- Spotlight covers the whole disk and many more file types, needs no index build by the user, and costs the app no memory. LocalSearch here covers three folders.
- The two interpret queries differently: for several words LocalSearch matches any of them, the Spotlight queries require all; LocalSearch also returns approximate matches for typos, where Spotlight matches literally.
- Ranking quality is not measured.
- In the first of six runs, one LocalSearch query took about 10 s once. It has not recurred and the cause was not identified.

Full tables, method and per-run notes are in [`docs/benchmarks/spotlight.md`](docs/benchmarks/spotlight.md). Reproduce with:

```bash
cargo run --release --example bench_vs_spotlight > docs/benchmarks/spotlight.md
```

It builds its own index in a temporary directory, takes several minutes, and reports timings and counts only (no file names).

## Troubleshooting

### The panel says "Search isn't available"

The engine library was not found. Run `frontend/run.sh` (or `cargo build --release`) and relaunch.

### The index stopped updating

If two copies of LocalSearch run at once (for example the app and `localsearch index`), the second one logs that the index is in use and only searches what the first has saved. Quit the other copy.

To start over, quit the app and delete `~/.localsearch`; the index is rebuilt on the next launch.

Set `RUST_LOG=info` (or `debug`) in the app's environment to see engine logs, including a per-stage breakdown of any query slower than 250 ms.

### A file is not found

- Check its folder is listed in Settings → Indexing and its name is not under "Skipped names".
- While Settings shows "Indexing", newer files may not be in yet.

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
