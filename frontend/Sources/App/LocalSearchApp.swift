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
        self.viewModel = SearchViewModel(backend: Backend.shared)
    }
}

/// The one backend the whole app talks to (search panel and settings alike).
enum Backend {
    static let shared: SearchBackendProtocol = makeBackend()
}

func makeBackend() -> SearchBackendProtocol {
    if let ffi = FFIBackend.makeIfAvailable() {
        fputs("[LocalSearch] Using FFI backend.\n", stderr)
        return ffi
    }

    // Without the engine nothing can be found. Say so in the UI instead of
    // answering every query with "No results".
    fputs("[LocalSearch] Search engine unavailable.\n", stderr)
    return UnavailableBackend(
        unavailableReason: "The search engine library (liblocalsearch.dylib) could not be loaded. Reinstall LocalSearch, or in a development checkout run cargo build."
    )
}

class AppDelegate: NSObject, NSApplicationDelegate {
    static private(set) var shared: AppDelegate!
    private let hasSeenWelcomeKey = "hasSeenWelcome"
    
    var windowController: SearchWindowController?
    var welcomeWindowController: WelcomeWindowController?
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
        // Indexing settings the user has changed win over the engine's saved ones
        SettingsManager.shared.pushIndexingConfigIfCustomised()
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

        // Show onboarding only on first run, otherwise keep existing startup behavior.
        if !UserDefaults.standard.bool(forKey: hasSeenWelcomeKey) {
            showWelcomeFlow()
        } else {
            windowController?.window?.makeKeyAndOrderFront(nil)
        }
        
        // Wire up HotkeyManager to window controller (F5)
        if let controller = windowController {
            HotkeyManager.shared.setWindowController(controller)
        }
        
        HotkeyManager.shared.register()
    }

    private func showWelcomeFlow() {
        welcomeWindowController = WelcomeWindowController { [weak self] in
            guard let self else { return }
            UserDefaults.standard.set(true, forKey: self.hasSeenWelcomeKey)
            self.welcomeWindowController?.close()
            self.welcomeWindowController = nil
            self.windowController?.showWindow(nil)
        }
        welcomeWindowController?.show()
    }

    func applicationWillTerminate(_ notification: Notification) {
        // Snapshot the index so the next launch starts warm
        Backend.shared.shutdown()
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
        
        // The setting alone decides: "Menu Bar" means no Dock icon, even while
        // a LocalSearch window is open.
        let policy: NSApplication.ActivationPolicy = (mode == .menuBar) ? .accessory : .regular
        
        if NSApp.activationPolicy() != policy {
            // Changing the policy deactivates the app, which would send the
            // window the user is working in to the back. Bring it forward again.
            let frontWindow = NSApp.keyWindow
            // Async dispatch to avoid UI deadlocks or focus loss during event handling
            DispatchQueue.main.async {
                NSApp.setActivationPolicy(policy)
                if let frontWindow, frontWindow.isVisible {
                    NSApp.activate(ignoringOtherApps: true)
                    frontWindow.makeKeyAndOrderFront(nil)
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
        } else if let item = statusItem {
            NSStatusBar.system.removeStatusItem(item)
            statusItem = nil
        }
    }

    @objc private func statusItemClicked() {
        HotkeyManager.shared.show()
    }

    @objc func openSettings() {
        NSApp.activate(ignoringOtherApps: true)
        SettingsWindowController.shared.show()
    }
}


