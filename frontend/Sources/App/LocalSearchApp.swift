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
        // --- Single Instance Enforcement ---
        let runningApps = NSWorkspace.shared.runningApplications
        let isAlreadyRunning = runningApps.contains { 
            $0.bundleIdentifier == Bundle.main.bundleIdentifier && 
            $0.processIdentifier != NSRunningApplication.current.processIdentifier 
        }
        
        if isAlreadyRunning {
            fputs("[LocalSearch] Another instance is already running. Terminating second instance.\n", stderr)
            NSApp.terminate(nil)
            return
        }

        AppDelegate.shared = self
        setupMainMenu()
        updateDisplayMode()
        
        // --- Runtime Application Icon ---
        if let iconPath = Bundle.module.path(forResource: "AppIcon", ofType: "icns"),
           let iconImage = NSImage(contentsOfFile: iconPath) {
            NSApp.applicationIconImage = iconImage
        }
        
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
        
        let searchWindowIsVisible = windowController?.window?.isVisible ?? false
        let policy: NSApplication.ActivationPolicy
        
        if searchWindowIsVisible {
            policy = .regular
        } else {
            policy = (mode == .menuBar) ? .accessory : .regular
        }
        
        if NSApp.activationPolicy() != policy {
            // Async dispatch to avoid UI deadlocks or focus loss during event handling
            DispatchQueue.main.async {
                NSApp.setActivationPolicy(policy)
                if searchWindowIsVisible {
                    NSApp.activate(ignoringOtherApps: true)
                }
            }
        }
        
        // Ensure status item existence according to mode
        updateStatusItem(for: mode)
    }

    private func updateStatusItem(for mode: AppDisplayMode) {
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
}


