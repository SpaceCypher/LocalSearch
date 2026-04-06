# LocalSearch Frontend

SwiftUI macOS application for LocalSearch file search engine.

## Requirements

- macOS 14.0+ (Sonoma)
- Xcode 15.0+
- Swift 5.10+

## Architecture

```
SearchWindowController (NSWindowController)
└── SearchWindow (NSPanel, .nonactivatingPanel)
    └── SwiftUI hosting root
        ├── SearchViewModel (@MainActor ObservableObject)
        └── Backend: Rust core via C FFI over XPC
```

## Building

```bash
cd frontend
swift build
swift test
```

## Project Structure

```
frontend/
├── Sources/
│   ├── App/              # Application entry point
│   ├── ViewModel/        # State management
│   ├── Views/            # SwiftUI views
│   ├── System/           # System integration (hotkeys, etc.)
│   └── Query/            # Query parsing
└── Tests/
    └── LocalSearchTests/ # Unit tests
```

## Development Status

- [x] Task F1: Project scaffold
- [ ] Task F2: SearchViewModel state machine
- [ ] Task F3: Debounce + cancellation
- [ ] Task F4: SearchWindow NSPanel
- [ ] Task F5: Global hotkey (⌥Space)
- [ ] Task F6: QueryFieldView
- [ ] Task F7: Query parser
- [ ] Task F8: ScopeBarView
- [ ] Task F9: ResultListView
- [ ] Task F10: Keyboard navigation
- [ ] Task F11: StatusBarView
- [ ] Task F12: MetadataPanelView
- [ ] Task F13: Animation system
