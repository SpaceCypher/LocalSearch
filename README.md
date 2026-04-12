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

The Rust side includes:

- WAL components (`wal/`)
- In-memory and segment index pieces (`index/`)
- Query parsing/ranking/execution (`query/`)
- Filesystem and startup flows (`fs/`, `startup.rs`)
- C-ABI entry points for frontend integration (`ffi.rs`)

The crate is built as both `rlib` and `cdylib` (see `Cargo.toml`), and the Swift app consumes the `cdylib`.

### Frontend (SwiftUI + AppKit)

The frontend is a Swift Package executable that:

- Presents a Spotlight-like floating panel UI
- Handles keyboard shortcuts and app/window behavior
- Calls into Rust through `FFIBackend`
- Falls back to `MockBackend` if the Rust dylib cannot be loaded

Backend selection is done in `frontend/Sources/App/LocalSearchApp.swift`.

## Prerequisites

- macOS 14+
- Xcode 15+ (Swift 5.10 toolchain)
- Rust stable toolchain (`rustup`, `cargo`)

## Quick Start (End-to-End)

Run these from the repository root unless noted.

1. Build the Rust dynamic library:

```bash
cargo build
```

2. Build and launch the frontend app bundle:

```bash
cd frontend
./run.sh
```

The app bundle is created at:

```text
frontend/.build/release/LocalSearch.app
```

## Why `cargo build` Matters

`frontend/run.sh` builds the Swift app and bundles resources, but it does not build the Rust crate.

If `target/debug/liblocalsearch.dylib` does not exist, the frontend cannot load the real backend and will fall back to mock behavior.

## Backend Loading Rules

`FFIBackend` attempts to load the Rust dylib in this order:

1. `LOCALSEARCH_DYLIB_PATH` (if set)
2. `./target/debug/liblocalsearch.dylib`
3. `../target/debug/liblocalsearch.dylib`
4. `../../target/debug/liblocalsearch.dylib`
5. repo-root `target/debug/liblocalsearch.dylib` (resolved via source path)

If all fail, app logs indicate fallback to `MockBackend`.

## Search Scope Defaults

When `LOCALSEARCH_ROOT` is not set, the Rust FFI search path defaults to existing directories among:

- `~/Downloads`
- `~/Documents`
- `~/Desktop`

Override search root explicitly when needed:

```bash
LOCALSEARCH_ROOT="$HOME/Downloads" ./run.sh
```

## Development Workflows

### Backend

```bash
cargo check
cargo test
```

### Frontend

```bash
cd frontend
swift build
swift test
```

### Optional backend CLI run

```bash
cargo run
```

Debug panel mode:

```bash
cargo run -- --debug-panel
```

## Troubleshooting

### App launches but every query says "No results found"

Most common causes:

1. Rust dylib not built or not discoverable
2. App running with `MockBackend` fallback
3. Querying outside current scan roots

Checks:

```bash
ls -l target/debug/liblocalsearch.dylib
```

If missing:

```bash
cargo build
```

Then relaunch frontend.

### Warning: unhandled `Info.plist` in SwiftPM

This occurs when `Sources/Resources/Info.plist` is inside target sources but not declared/excluded in `frontend/Package.swift`.

Current package config excludes that file, and `run.sh` copies `Info.plist` into the app bundle manually.

### Global hotkey does not work

Grant Accessibility permission in macOS:

- System Settings -> Privacy & Security -> Accessibility
- Allow the app

## Current Documentation

For detailed implementation history and audits:

- `docs/progress.md`
- `docs/audit-2026-04-07.md`
- `context/implementation_spec.md`

## License

Copyright (c) 2026 SpaceCypher. All rights reserved.
