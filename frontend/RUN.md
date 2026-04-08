# Running LocalSearch Frontend

## Quick Start

The LocalSearch frontend is now fully assembled and ready to run!

### Build and Run

```bash
cd frontend
swift run
```

### What You'll See

When you launch the app:

1. **No Dock Icon** - The app runs as an accessory (LSUIElement=true)
2. **Global Hotkey** - Press `⌥Space` (Option+Space) to show/hide the search window
3. **Search Window** - A floating panel with:
   - Search input field at the top (auto-focused)
   - Scope filter buttons (All, Documents, Images, Code, Folders)
   - Results list (currently using MockBackend with no results)
   - Status bar at the bottom

### Current State

- ✅ Complete UI assembled and functional
- ✅ All 78 frontend tests passing
- ✅ MockBackend provides test data
- ❌ Real backend integration not yet connected (Task F18)

### Testing the UI

The app currently uses `MockBackend` which returns no results by default. To test with mock data, you can modify `SearchWindowController.swift`:

```swift
// Change this line:
let viewModel = SearchViewModel(backend: MockBackend())

// To this (with mock results):
let backend = MockBackend()
backend.mockResults = [
    SearchResult(id: "1", filename: "report.pdf", path: "~/Documents/report.pdf", rank: 1.0, fileKind: .document),
    SearchResult(id: "2", filename: "notes.txt", path: "~/Documents/notes.txt", rank: 0.9, fileKind: .document),
]
let viewModel = SearchViewModel(backend: backend)
```

### Keyboard Shortcuts

- `⌥Space` - Show/hide window
- `Esc` - Hide window
- `↑/↓` - Navigate results
- `←/→` - Collapse/expand metadata panel
- `⏎` - Open selected file
- `⌘⏎` - Reveal in Finder
- `⌥⏎` - Copy path to clipboard
- `⌘1-5` - Switch scope filters

### Next Steps

To connect to the real Rust backend:
1. Complete Task 21 (Backend) - C FFI Layer
2. Complete Task F18 (Frontend) - XPC Backend Channel
3. Replace MockBackend with XPCBackendChannel

### Troubleshooting

**Hotkey not working?**
- The app needs Accessibility permission for CGEventTap
- Go to System Settings > Privacy & Security > Accessibility
- Add the LocalSearch app to the allowed list

**Window not appearing?**
- Check Console.app for any error messages
- Verify the app is running (no Dock icon, but should appear in Activity Monitor)
