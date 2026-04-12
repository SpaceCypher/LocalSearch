import Cocoa
import SwiftUI

class SettingsWindowController: NSWindowController {
    static let shared = SettingsWindowController()
    
    convenience init() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 440, height: 600),
            styleMask: [.titled, .closable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        
        window.center()
        window.title = "Settings"
        window.titlebarAppearsTransparent = true
        window.isMovableByWindowBackground = true
        
        let hostingView = NSHostingView(rootView: SettingsView())
        window.contentView = hostingView
        
        self.init(window: window)
    }
    
    func show() {
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
}
