import Cocoa
import Combine
import SwiftUI

class FloatingPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }
}

class SearchWindowController: NSWindowController, WindowControllerProtocol {
    private var cancellables = Set<AnyCancellable>()
    private var localEventMonitor: Any?

    private let compactHeight: CGFloat = 92
    private let maxExpandedHeight: CGFloat = 564
    private let zeroResultsHeight: CGFloat = 240
    private let baseExpandedHeight: CGFloat = 156
    private let perResultHeight: CGFloat = 56
    
    convenience init() {
        // Create the NSPanel
        let panel = FloatingPanel(
            contentRect: NSRect(x: 0, y: 0, width: 640, height: 92),
            styleMask: [.nonactivatingPanel, .borderless, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        
        // Configure panel behavior
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.level = .floating
        panel.hasShadow = true
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = true
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
        let backend = makeBackend()
        let viewModel = SearchViewModel(backend: backend)
        let contentView = SearchContentView(viewModel: viewModel)

        let visualEffectView = NSVisualEffectView(frame: panel.contentRect(forFrameRect: panel.frame))
        visualEffectView.material = .fullScreenUI
        visualEffectView.blendingMode = .behindWindow
        visualEffectView.state = .active
        visualEffectView.appearance = NSAppearance(named: .vibrantDark)
        visualEffectView.wantsLayer = true
        visualEffectView.layer?.cornerRadius = 28
        visualEffectView.layer?.masksToBounds = true
        visualEffectView.layer?.borderWidth = 0.5
        visualEffectView.layer?.borderColor = NSColor.white.withAlphaComponent(0.1).cgColor

        let hostingView = NSHostingView(rootView: contentView)
        hostingView.translatesAutoresizingMaskIntoConstraints = false
        visualEffectView.addSubview(hostingView)
        NSLayoutConstraint.activate([
            hostingView.leadingAnchor.constraint(equalTo: visualEffectView.leadingAnchor),
            hostingView.trailingAnchor.constraint(equalTo: visualEffectView.trailingAnchor),
            hostingView.topAnchor.constraint(equalTo: visualEffectView.topAnchor),
            hostingView.bottomAnchor.constraint(equalTo: visualEffectView.bottomAnchor)
        ])

        panel.contentView = visualEffectView
        
        self.init(window: panel)
        
        bindWindowSizing(panel: panel, viewModel: viewModel)
        setupEventMonitor(viewModel: viewModel)
    }

    private func setupEventMonitor(viewModel: SearchViewModel) {
        localEventMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            // Arrow keys
            if event.keyCode == 125 { // Down
                viewModel.handleArrowKey(.down)
                return nil
            } else if event.keyCode == 126 { // Up
                viewModel.handleArrowKey(.up)
                return nil
            } else if event.keyCode == 124 { // Right
                viewModel.handleArrowKey(.right)
                return nil
            } else if event.keyCode == 123 { // Left
                viewModel.handleArrowKey(.left)
                return nil
            } else if event.keyCode == 36 { // Return
                var modifiers = EventModifiers()
                if event.modifierFlags.contains(.command) { modifiers.insert(.command) }
                if event.modifierFlags.contains(.shift) { modifiers.insert(.shift) }
                if event.modifierFlags.contains(.option) { modifiers.insert(.option) }
            } else if event.keyCode == 53 { // Escape
                self.hideWindow()
                return nil
            } else if event.modifierFlags.contains(.command) {
                if let chars = event.charactersIgnoringModifiers, ["1", "2", "3", "4"].contains(chars) {
                    var modifiers = EventModifiers()
                    modifiers.insert(.command)
                    viewModel.handleKeyboardShortcut(modifiers, key: chars)
                    return nil
                }
            }
            
            return event
        }
    }

    private func bindWindowSizing(panel: NSPanel, viewModel: SearchViewModel) {
        Publishers.CombineLatest4(
            viewModel.$queryText, 
            viewModel.$displayResults, 
            viewModel.$queryState, 
            viewModel.$expandedResult
        )
        .debounce(for: 0.1, scheduler: RunLoop.main)
        .receive(on: RunLoop.main)
        .sink { [weak self, weak panel] (query, results, state, expanded) in
            guard let self, let panel else { return }
            let targetHeight = self.targetHeight(
                query: query,
                resultCount: results.count,
                state: state
            )
            let targetWidth: CGFloat = expanded != nil ? 900 : 640
            self.resize(panel: panel, to: targetHeight, width: targetWidth)
        }
        .store(in: &cancellables)
    }

    private func targetHeight(query: String, resultCount: Int, state: QueryState) -> CGFloat {
        let settings = SettingsManager.shared
        let isComfortable = settings.resultDensity == .comfortable
        
        // Accurate measurements based on SwiftUI layouts
        let topSectionHeight: CGFloat = query.isEmpty ? 92 : (isComfortable ? 116 : 96)
        let statusBarHeight: CGFloat = 40
        let topHitHeight: CGFloat = isComfortable ? 72 : 56
        let standardRowHeight: CGFloat = isComfortable ? 52 : 44

        if query.isEmpty {
            return compactHeight
        }

        if state == .complete && resultCount == 0 {
            return zeroResultsHeight
        }

        if resultCount <= 0 {
            return topSectionHeight + statusBarHeight
        }

        let visibleCount = min(resultCount, 8)
        var totalResultsHeight: CGFloat = topHitHeight // First row is always Top Hit
        if visibleCount > 1 {
            totalResultsHeight += CGFloat(visibleCount - 1) * standardRowHeight
        }
        
        let total = topSectionHeight + totalResultsHeight + statusBarHeight
        return min(maxExpandedHeight, total)
    }

    private func resize(panel: NSPanel, to height: CGFloat, width: CGFloat) {
        let current = panel.frame
        if abs(current.height - height) < 0.5 && abs(current.width - width) < 0.5 {
            return
        }

        var next = current
        let heightDelta = height - current.height
        
        next.size.height = height
        next.size.width = width
        // Keep top edge fixed so the panel expands downward like Spotlight.
        next.origin.y -= heightDelta
        
        // When width changes, animate left/right depending on center pivot?
        // To keep the left side pinned:
        // (Nothing needed, standard origin expands rightwards)

        panel.setFrame(next, display: true, animate: true)
    }
    
    func handleEscapeKey() {
        window?.orderOut(nil)
    }
    
    deinit {
        if let monitor = localEventMonitor {
            NSEvent.removeMonitor(monitor)
        }
    }
    
    // WindowControllerProtocol conformance
    override func showWindow(_ sender: Any?) {
        // Explicitly activate the app to bring it to front (required for .accessory mode)
        NSApp.activate(ignoringOtherApps: true)
        
        centerOnCurrentScreen()
        super.showWindow(sender)
        
        // Ensure the window is visible and key
        window?.setIsVisible(true)
        window?.orderFrontRegardless()
        window?.makeKeyAndOrderFront(sender)
        
        // Notify delegate to update activation policy (to show main menu)
        if let appDelegate = NSApp.delegate as? AppDelegate {
            appDelegate.updateDisplayMode()
        }
    }

    @objc func hideWindow() {
        window?.orderOut(nil)
        
        // Notify delegate to update activation policy (to hide dock if in menuBar mode)
        if let appDelegate = NSApp.delegate as? AppDelegate {
            appDelegate.updateDisplayMode()
        }
    }

    func centerOnCurrentScreen() {
        guard let window = window else { return }
        
        let screen: NSScreen = NSScreen.screens.first { 
            NSMouseInRect(NSEvent.mouseLocation, $0.frame, false) 
        } ?? NSScreen.main ?? NSScreen.screens.first ?? NSScreen()
        
        let screenFrame = screen.visibleFrame
        let windowFrame = window.frame
        
        let x = screenFrame.midX - windowFrame.width / 2
        let y = screenFrame.midY - windowFrame.height / 2
        
        window.setFrameOrigin(NSPoint(x: x, y: y))
    }
}

// Main content view assembling all components
struct SearchContentView: View {
    @ObservedObject var viewModel: SearchViewModel
    @ObservedObject var settings = SettingsManager.shared
    
    var body: some View {
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                // Top Section
                HStack(spacing: 16) {
                    // Query field
                    QueryFieldView(viewModel: viewModel)
                        .frame(maxWidth: .infinity)
                }
                .padding(.horizontal, 20)
                .padding(.top, 20)
                .padding(.bottom, viewModel.displayResults.isEmpty ? 20 : (settings.resultDensity == .comfortable ? 12 : 8))
                
                // Index progress (shown during first-launch bootstrap)
                if viewModel.showIndexProgress, let progress = viewModel.indexProgress {
                    IndexProgressView(progress: progress)
                        .padding(.horizontal, 16)
                        .padding(.bottom, 8)
                }
                
                // Horizontal line separating search bar from results
                if !viewModel.displayResults.isEmpty || viewModel.queryState == .searching {
                    Rectangle()
                        .fill(settings.accentColor.color.opacity(0.15))
                        .frame(height: 0.5)
                        .padding(.horizontal, 20)
                        .padding(.bottom, 4)
                }
                
                // Results or zero-results view
                if viewModel.showSkeletons {
                    VStack(spacing: 0) {
                        ForEach(0..<viewModel.skeletonCount, id: \.self) { _ in
                            SkeletonResultRowView()
                        }
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                } else if viewModel.displayResults.isEmpty && viewModel.queryState == .complete {
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
            .frame(width: 640)
            
            if let expanded = viewModel.expandedResult {
                Divider()
                    .background(settings.accentColor.color.opacity(0.3))
                MetadataPanelView(result: expanded)
                    .transition(.move(edge: .trailing).combined(with: .opacity))
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        .background(
            ZStack {
                // Background opacity driven by Glass Intensity setting
                Color.black.opacity(settings.glassIntensity * 0.8)
                
                // Base Gradient
                LinearGradient(
                    gradient: Gradient(colors: [
                        settings.accentColor.color.opacity(0.1),
                        Color.clear,
                        Color.black.opacity(0.1)
                    ]),
                    startPoint: .topLeading,
                    endPoint: .bottomTrailing
                )
            }
        )
        .animation(.spring(response: 0.3, dampingFraction: 0.85), value: viewModel.expandedResult)
        .animation(.easeInOut, value: settings.resultDensity)
    }
}
