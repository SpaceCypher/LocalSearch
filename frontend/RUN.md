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
   - Results list (driven by FFI backend)
   - Status bar at the bottom

### Current State

- ✅ Complete UI assembled and functional
- ⚠️ Frontend test suite currently has API drift and needs a dedicated test-fix pass
- ✅ Real backend integration via FFI

### Testing the UI

The app now uses the FFI backend at runtime. Ensure `liblocalsearch.dylib` is available in one of the standard candidate paths listed in `FFIBackend.swift`, or set `LOCALSEARCH_DYLIB_PATH` explicitly.

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

1. Expand FFI API surface for advanced filters and progress channels
2. Add stronger end-to-end integration tests for FFI loading and query results

### Troubleshooting

**Hotkey not working?**
- The app needs Accessibility permission for CGEventTap
- Go to System Settings > Privacy & Security > Accessibility
- Add the LocalSearch app to the allowed list

**Window not appearing?**
- Check Console.app for any error messages
- Verify the app is running (no Dock icon, but should appear in Activity Monitor)
