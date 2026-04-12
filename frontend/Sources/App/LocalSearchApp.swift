import SwiftUI
import Combine

@main
struct LocalSearchApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate
    @StateObject private var appState = AppState()

    var body: some Scene {

        Settings {
            SettingsView()
        }
    }
}

@MainActor
final class AppState: ObservableObject {
    let viewModel: SearchViewModel

    init() {
        self.viewModel = SearchViewModel(backend: makeBackend())
    }
}

func makeBackend() -> SearchBackendProtocol {
    if let ffi = FFIBackend.makeIfAvailable() {
        fputs("[LocalSearch] Using FFI backend.\n", stderr)
        return ffi
    }

    fputs("[LocalSearch] Using MockBackend fallback.\n", stderr)
    return MockBackend()
}

class AppDelegate: NSObject, NSApplicationDelegate {
    static private(set) var shared: AppDelegate!
    
    var windowController: SearchWindowController?
    var statusItem: NSStatusItem?
    private var cancellables = Set<AnyCancellable>()

    func applicationDidFinishLaunching(_ notification: Notification) {
        AppDelegate.shared = self
        setupMainMenu()
        updateDisplayMode()
        
        // Observe settings changes
        SettingsManager.shared.objectWillChange
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in
                // Delay slightly to allow AppStorage to update
                DispatchQueue.main.async {
                    self?.updateDisplayMode()
                }
            }
            .store(in: &cancellables)

        // Attempt to load and set the application icon
        let iconName = "AppIcon"
        if let iconURL = Bundle.module.url(forResource: iconName, withExtension: "png") {
            if let iconImage = NSImage(contentsOf: iconURL) {
                NSLog("AppDelegate: Successfully loaded %@.png from bundle", iconName)
                NSApp.applicationIconImage = applyPadding(to: iconImage, marginRatio: 0.12)
            } else {
                NSLog("AppDelegate: Failed to create NSImage from %@", iconURL.path)
                setFallbackIcon()
            }
        } else {
            NSLog("AppDelegate: Could not find %@.png in Bundle.module", iconName)
            if let iconImage = NSImage(named: NSImage.Name(iconName)) {
                NSLog("AppDelegate: Successfully loaded %@ via NSImage(named:)", iconName)
                NSApp.applicationIconImage = applyPadding(to: iconImage, marginRatio: 0.12)
            } else {
                setFallbackIcon()
            }
        }
        
        // Force Dock to refresh
        NSApp.dockTile.display()

        windowController = SearchWindowController()
        
        // Show immediately for testing
        windowController?.window?.makeKeyAndOrderFront(nil)
        
        // Wire up HotkeyManager to window controller (F5)
        if let controller = windowController {
            HotkeyManager.shared.setWindowController(controller)
        }
        
        HotkeyManager.shared.register()
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        // If window is hidden, show it
        HotkeyManager.shared.show()
        return true
    }

    private func setupMainMenu() {
        let mainMenu = NSMenu()
        
        // App Menu
        let appMenu = NSMenu()
        let appName = "LocalSearch"
        
        let settingsItem = NSMenuItem(title: "Settings...", action: #selector(openSettings), keyEquivalent: ",")
        appMenu.addItem(settingsItem)
        
        appMenu.addItem(NSMenuItem.separator())
        
        let quitItem = NSMenuItem(title: "Quit \(appName)", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appMenu.addItem(quitItem)
        
        let appMenuItem = NSMenuItem()
        appMenuItem.submenu = appMenu
        mainMenu.addItem(appMenuItem)
        
        // Edit Menu (Standard search apps need this for Copy/Paste/Select All)
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Undo", action: #selector(UndoManager.undo), keyEquivalent: "z")
        editMenu.addItem(withTitle: "Redo", action: #selector(UndoManager.redo), keyEquivalent: "Z")
        editMenu.addItem(NSMenuItem.separator())
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        
        let editMenuItem = NSMenuItem()
        editMenuItem.submenu = editMenu
        mainMenu.addItem(editMenuItem)
        
        NSApp.mainMenu = mainMenu
    }

    func updateDisplayMode() {
        let mode = SettingsManager.shared.displayMode
        
        // IMPORTANT: We only switch to .regular if the SEARCH WINDOW is visible.
        // We ignore the Settings window visibility so the Dock icon can disappear
        // immediately when the user selects "Menu Bar" mode.
        let searchWindowIsVisible = windowController?.window?.isVisible ?? false
        
        let policy: NSApplication.ActivationPolicy
        if searchWindowIsVisible {
            policy = .regular
        } else {
            policy = (mode == .menuBar) ? .accessory : .regular
        }
        
        if NSApp.activationPolicy() != policy {
            NSApp.setActivationPolicy(policy)
            if searchWindowIsVisible {
                // Ensure we take focus when switching to regular
                NSApp.activate(ignoringOtherApps: true)
            }
        }
        
        // 2. Status Item (Menu Bar Icon)
        if mode == .menuBar || mode == .both {
            if statusItem == nil {
                statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
                if let button = statusItem?.button {
                    button.image = NSImage(systemSymbolName: "magnifyingglass", accessibilityDescription: "LocalSearch")
                }
                
                let menu = NSMenu()
                menu.addItem(withTitle: "Open LocalSearch", action: #selector(statusItemClicked), keyEquivalent: "")
                menu.addItem(withTitle: "Settings...", action: #selector(openSettings), keyEquivalent: ",")
                menu.addItem(NSMenuItem.separator())
                menu.addItem(withTitle: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
                statusItem?.menu = menu
            }
        } else {
            statusItem = nil
        }
    }

    @objc private func statusItemClicked() {
        HotkeyManager.shared.show()
    }

    @objc func openSettings() {
        print("AppDelegate: openSettings() triggered")
        
        // Ensure app is active
        NSApp.activate(ignoringOtherApps: true)
        
        // 1. Try standard SwiftUI settings trigger
        let success = NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
        
        // 2. Fall-back to manual window if the standard one didn't show up
        // We check success, but sendAction returns true if it FOUND the selector, 
        // not necessarily if the window showed up. For safety, we can use our manual one.
        if !success {
            print("AppDelegate: showSettingsWindow selector failed, using manual fallback")
            SettingsWindowController.shared.show()
        } else {
            // Even if success is true, sometimes it doesn't orderFront if the app was accessory.
            // Our SettingsWindowController.shared.show() is safer for now.
            SettingsWindowController.shared.show()
        }
    }

    private func setFallbackIcon() {
        NSLog("AppDelegate: Setting fallback SF Symbol icon")
        let config = NSImage.SymbolConfiguration(pointSize: 128, weight: .regular)
        if let symbolImage = NSImage(systemSymbolName: "magnifyingglass.circle.fill", accessibilityDescription: "LocalSearch")?
            .withSymbolConfiguration(config) {
            
            // Create a nice gradient background for the fallback icon
            let size = NSSize(width: 512, height: 512)
            let finalImage = NSImage(size: size)
            finalImage.lockFocus()
            
            let rect = NSRect(origin: .zero, size: size)
            let path = NSBezierPath(roundedRect: rect, xRadius: 110, yRadius: 110)
            
            // Background Gradient
            let gradient = NSGradient(starting: NSColor.systemIndigo, ending: NSColor.systemPurple)
            gradient?.draw(in: path, angle: 45)
            
            // Draw Symbol
            let symbolRect = NSRect(x: 128, y: 128, width: 256, height: 256)
            symbolImage.draw(in: symbolRect)
            
            finalImage.unlockFocus()
            NSApp.applicationIconImage = finalImage
        }
    }

    private func applyPadding(to image: NSImage, marginRatio: CGFloat) -> NSImage {
        let size = image.size
        let padding = size.width * marginRatio
        let newSize = NSSize(width: size.width + padding * 2, height: size.height + padding * 2)
        
        let newImage = NSImage(size: newSize)
        newImage.lockFocus()
        image.draw(in: NSRect(x: padding, y: padding, width: size.width, height: size.height),
                   from: NSRect(origin: .zero, size: size),
                   operation: .sourceOver,
                   fraction: 1.0)
        newImage.unlockFocus()
        return newImage
    }



    private func preparedGlyph(from source: NSImage) -> NSImage? {
        var rect = NSRect(origin: .zero, size: source.size)
        guard let cgSource = source.cgImage(forProposedRect: &rect, context: nil, hints: nil) else {
            return nil
        }

        let width = cgSource.width
        let height = cgSource.height
        let bytesPerPixel = 4
        let bytesPerRow = width * bytesPerPixel
        let bitmapInfo = CGImageAlphaInfo.premultipliedLast.rawValue
        var data = [UInt8](repeating: 0, count: height * bytesPerRow)

        guard let ctx = CGContext(
            data: &data,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: bytesPerRow,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: bitmapInfo
        ) else {
            return nil
        }

        ctx.draw(cgSource, in: CGRect(x: 0, y: 0, width: width, height: height))

        var minX = width
        var minY = height
        var maxX = 0
        var maxY = 0
        var found = false

        for y in 0..<height {
            for x in 0..<width {
                let idx = y * bytesPerRow + x * bytesPerPixel
                let r = data[idx]
                let g = data[idx + 1]
                let b = data[idx + 2]
                let a = data[idx + 3]

                // Treat near-white pixels as background and make them transparent.
                let isBackground = a < 10 || (r > 238 && g > 238 && b > 238)
                if isBackground {
                    data[idx + 3] = 0
                    continue
                }

                found = true
                minX = min(minX, x)
                minY = min(minY, y)
                maxX = max(maxX, x)
                maxY = max(maxY, y)
            }
        }

        guard found, let cleaned = ctx.makeImage() else {
            return nil
        }

        let cropRect = CGRect(
            x: minX,
            y: minY,
            width: maxX - minX + 1,
            height: maxY - minY + 1
        )

        guard let cropped = cleaned.cropping(to: cropRect) else {
            return NSImage(cgImage: cleaned, size: NSSize(width: width, height: height))
        }

        return NSImage(
            cgImage: cropped,
            size: NSSize(width: cropRect.width, height: cropRect.height)
        )
    }
}


