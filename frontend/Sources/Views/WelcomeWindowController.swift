import Cocoa
import SwiftUI

class WelcomeWindowController: NSWindowController {
    convenience init(onContinue: @escaping () -> Void) {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 680, height: 500),
            styleMask: [.titled, .closable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )

        window.center()
        window.title = "Welcome"
        window.titlebarAppearsTransparent = true
        window.isMovableByWindowBackground = true

        let root = WelcomeView(onContinue: onContinue)
        let hostingView = NSHostingView(rootView: root)
        window.contentView = hostingView

        self.init(window: window)
    }

    func show() {
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
}
