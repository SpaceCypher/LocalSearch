import Cocoa
import SwiftUI

class SearchWindowController: NSWindowController, WindowControllerProtocol {
    
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
        let viewModel = SearchViewModel(backend: MockBackend())
        let contentView = SearchContentView(viewModel: viewModel)
        panel.contentView = NSHostingView(rootView: contentView)
        
        self.init(window: panel)
    }
    
    func handleEscapeKey() {
        window?.orderOut(nil)
    }
    
    // WindowControllerProtocol conformance
    @objc func hideWindow() {
        window?.orderOut(nil)
    }
}

// Main content view assembling all components
struct SearchContentView: View {
    @ObservedObject var viewModel: SearchViewModel
    
    var body: some View {
        VStack(spacing: 0) {
            // Query field at top
            QueryFieldView(viewModel: viewModel)
                .padding(.horizontal, 16)
                .padding(.top, 16)
                .padding(.bottom, 8)
            
            // Scope bar (filters)
            ScopeBarView(viewModel: viewModel)
                .padding(.horizontal, 16)
                .padding(.bottom, 8)
            
            // Index progress (shown during first-launch bootstrap)
            if viewModel.showIndexProgress, let progress = viewModel.indexProgress {
                IndexProgressView(progress: progress)
                    .padding(.horizontal, 16)
                    .padding(.bottom, 8)
            }
            
            // Results or zero-results view
            if viewModel.displayResults.isEmpty && viewModel.queryState == .complete {
                // Zero results state
                ZeroResultsView(
                    query: viewModel.zeroResultsQuery,
                    suggestions: viewModel.spellingSuggestions,
                    showBroadeningTip: viewModel.showBroadeningTip,
                    note: viewModel.zeroResultsNote,
                    onSelectSuggestion: { suggestion in
                        viewModel.selectSuggestion(suggestion)
                    }
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                // Results list
                ResultListView(viewModel: viewModel)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            
            // Status bar at bottom
            StatusBarView(viewModel: viewModel)
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(NSColor.windowBackgroundColor))
    }
}
