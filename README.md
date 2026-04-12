# LocalSearch

LocalSearch is a high-performance, native macOS search engine designed for instant document and file retrieval. It combines a robust Rust-based indexing core with a premium SwiftUI frontend, delivering a seamless search experience that feels integrated with the macOS ecosystem.

## System Architecture

The application is built on a split-architecture model to maximize performance and UI responsiveness.

- **Search Engine (Rust)**: A low-latency core responsible for file system monitoring, indexing, and query execution. It utilizes a custom Suffix Array for instant substring matching and BM25 ranking for relevance.
- **Frontend (Swift/SwiftUI)**: A high-fidelity macOS interface utilizing AppKit and SwiftUI. It features frosted glass materials, a native 4-tab settings architecture, and high-performance result rendering.
- **Inter-Process Communication**: The frontend communicates with the Rust engine through a thin C-ABI bridge and FFI (Foreign Function Interface), ensuring near-zero latency between keystroke and result.

## Key Features

### High-Performance Core
- **Suffix Array Substring Search**: Instant matches for mid-word and path-based queries.
- **WAL-Based Durability**: Write-Ahead-Log ensures that index state is never lost during system crashes.
- **FSEvents Integration**: Real-time monitoring of the filesystem with automatic metadata extraction.
- **Adaptive Memory Control**: Intelligent RAM management that evicts auxiliary caches under system memory pressure.

### Premium Experience
- **Frosted Glass UI**: Native macOS textures with customizable glass intensity and tint.
- **Flexible System Presence**: Choose between Dock-only, Menu Bar-only, or both modes for minimized footprint.
- **Global Shortcuts**: Customizable global hotkeys for instant search activation.
- **Single-Instance Enforcement**: Application logic prevents multiple conflicting versions from running simultaneously.

## Technical Specifications

| Component | Technology | Performance Target |
|---|---|---|
| Indexer | Rust (Suffix Array) | < 10ms per 1M docs |
| Query Engine | BM25 Ranking | P50 < 80ms |
| UI Framework | SwiftUI / AppKit | 60 FPS Fluid |
| Memory usage | Minimal-to-Full Scalable | 50MB - 500MB (budgeted) |

## Getting Started

### Prerequisites
- macOS 13.0+
- Rust 1.75+
- Xcode 15.0+ (for Swift compilation)

### Build and Launch

The project includes a unified build script to handle the Swift/Rust compilation and app bundling.

```bash
cd frontend
./run.sh
```

This script will:
1. Compile the Rust engine binaries.
2. Build the Swift frontend bundle.
3. Automatically kill any existing background instances for a clean launch.
4. Launch the application located at `.build/release/LocalSearch.app`.

## License

Copyright (c) 2026 SpaceCypher. All rights reserved.
