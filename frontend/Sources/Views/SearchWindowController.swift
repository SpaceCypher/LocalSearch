import Cocoa
import SwiftUI

class SearchWindowController: NSWindowController {
    
    convenience init() {
        // Create the NSPanel
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 640, height: 564),
            styleMask: [.nonactivatingPanel, .titled, .closable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        
        // Configure panel behavior
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = false
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.isMovableByWindowBackground = true
        
        // Center on primary display
        if let screen = NSScreen.main {
            let screenFrame = screen.visibleFrame
            let windowFrame = panel.frame
            let x = screenFrame.midX - windowFrame.width / 2
            let y = screenFrame.midY - windowFrame.height / 2
            panel.setFrameOrigin(NSPoint(x: x, y: y))
        }
        
        // Create SwiftUI content view
        let viewModel = SearchViewModel()
        let contentView = SearchContentView(viewModel: viewModel)
        panel.contentView = NSHostingView(rootView: contentView)
        
        self.init(window: panel)
    }
    
    func handleEscapeKey() {
        window?.orderOut(nil)
    }
}

// Placeholder content view
struct SearchContentView: View {
    @ObservedObject var viewModel: SearchViewModel
    
    var body: some View {
        VStack {
            Text("LocalSearch")
                .font(.title)
            Text("Search window placeholder")
                .foregroundColor(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(NSColor.windowBackgroundColor))
    }
}
