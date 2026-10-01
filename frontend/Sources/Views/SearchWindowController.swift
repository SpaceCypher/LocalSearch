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
    private let zeroResultsHeight: CGFloat = 290
    private let statusBarHeight: CGFloat = 40
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
        let viewModel = SearchViewModel(backend: Backend.shared)
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
        // The controller sizes the window (see resize); the SwiftUI content must
        // not push its own size onto it, or the panel re-lays out around its centre.
        hostingView.sizingOptions = []
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
        
        viewModel.onDismissRequested = { [weak self] in self?.hideWindow() }
        bindWindowSizing(panel: panel, viewModel: viewModel)
        bindDetailsWidth(panel: panel, viewModel: viewModel)
        setupEventMonitor(viewModel: viewModel)
    }

    private func setupEventMonitor(viewModel: SearchViewModel) {
        localEventMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            // Only the search panel's keys; the settings window handles its own
            guard let self, event.window === self.window else { return event }

            // Left/Right are deliberately not intercepted: they move the caret
            // in the query field.
            if event.keyCode == 125 { // Down
                viewModel.handleArrowKey(.down)
                return nil
            } else if event.keyCode == 126 { // Up
                viewModel.handleArrowKey(.up)
                return nil
            } else if event.keyCode == 48 { // Tab: show or hide details
                viewModel.toggleDetails()
                return nil
            } else if event.keyCode == 36 || event.keyCode == 76 { // Return / Enter
                var modifiers = EventModifiers()
                if event.modifierFlags.contains(.command) { modifiers.insert(.command) }
                if event.modifierFlags.contains(.shift) { modifiers.insert(.shift) }
                if event.modifierFlags.contains(.option) { modifiers.insert(.option) }
                viewModel.handleReturnKey(modifiers: modifiers)
                return nil
            } else if event.keyCode == 53 { // Escape
                // First Escape closes details, the next one hides the panel
                if viewModel.expandedResult != nil {
                    viewModel.expandedResult = nil
                } else {
                    self.hideWindow()
                }
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
        .sink { [weak self, weak panel, weak viewModel] _ in
            guard let self, let panel, let viewModel else { return }
            self.updateSize(panel: panel, viewModel: viewModel)
        }
        .store(in: &cancellables)

        // The idle panel grows a status line while there is index activity
        // (or a problem) to report, and shrinks back when there is not.
        viewModel.$indexProgress
            .map { $0 != nil }
            .removeDuplicates()
            .dropFirst()
            .receive(on: RunLoop.main)
            .sink { [weak self, weak panel, weak viewModel] _ in
                guard let self, let panel, let viewModel else { return }
                self.updateSize(panel: panel, viewModel: viewModel)
            }
            .store(in: &cancellables)
    }

    /// The window is sized here and nowhere else, from the view-model's state.
    private func updateSize(panel: NSPanel, viewModel: SearchViewModel) {
        var height = targetHeight(
            query: viewModel.queryText,
            resultCount: viewModel.displayResults.count,
            state: viewModel.queryState,
            showsIdleStatus: viewModel.showsStatusBar
        )
        // The details column has its own height needs, however short the list is
        if viewModel.expandedResult != nil {
            height = max(height, MetadataPanelView.minHeight)
        }
        // Details open to the right: the search column keeps its place
        let width = SearchContentView.mainWidth
            + (viewModel.expandedResult != nil ? MetadataPanelView.width + SearchContentView.dividerWidth : 0)
        resize(panel: panel, to: height, width: width)
    }

    /// Opening or closing details resizes at once (no debounce), so the panel
    /// never draws the details column into a window that is still narrow.
    private func bindDetailsWidth(panel: NSPanel, viewModel: SearchViewModel) {
        viewModel.$expandedResult
            .map { $0 != nil }
            .removeDuplicates()
            .dropFirst()
            .receive(on: RunLoop.main)
            .sink { [weak self, weak panel, weak viewModel] _ in
                guard let self, let panel, let viewModel else { return }
                self.updateSize(panel: panel, viewModel: viewModel)
            }
            .store(in: &cancellables)
    }

    private func targetHeight(query: String, resultCount: Int, state: QueryState, showsIdleStatus: Bool) -> CGFloat {
        let settings = SettingsManager.shared
        let isComfortable = settings.resultDensity == .comfortable
        
        // Accurate measurements based on SwiftUI layouts
        let topSectionHeight: CGFloat = query.isEmpty ? 92 : (isComfortable ? 116 : 96)
        let topHitHeight: CGFloat = isComfortable ? 72 : 56
        let standardRowHeight: CGFloat = isComfortable ? 52 : 44

        if query.isEmpty {
            // Just the search field, unless there is something worth saying
            return compactHeight + (showsIdleStatus ? statusBarHeight : 0)
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

        panel.setFrame(next, display: true, animate: !AnimationTokens.isReduceMotionEnabled)
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
    
    static let mainWidth: CGFloat = 640
    static let dividerWidth: CGFloat = 0.5
    
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
                
                // Index progress lives in the status bar, so the layout does
                // not shift when indexing starts or finishes.
                
                // Horizontal line separating search bar from results
                if !viewModel.displayResults.isEmpty || viewModel.queryState == .searching {
                    Rectangle()
                        .fill(DS.Palette.separator)
                        .frame(height: 0.5)
                        .padding(.horizontal, DS.Space.s5)
                        .padding(.bottom, DS.Space.s1)
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
                        },
                        unavailableReason: viewModel.backendUnavailableReason,
                        indexingStatus: viewModel.indexProgress?.statusText,
                        onClearFilters: { viewModel.clearFilters() }
                    )
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    // Results list
                    ResultListView(viewModel: viewModel)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
                
                // Status bar at bottom. An idle, healthy panel is just the field.
                if viewModel.showsStatusBar {
                    StatusBarView(viewModel: viewModel)
                        .padding(.horizontal, DS.Space.s4)
                        .padding(.vertical, DS.Space.s2)
                }
            }
            .frame(width: SearchContentView.mainWidth)
            
            if let expanded = viewModel.expandedResult {
                Rectangle()
                    .fill(DS.Palette.border)
                    .frame(width: SearchContentView.dividerWidth)
                MetadataPanelView(
                    result: expanded,
                    onOpen: { viewModel.open(expanded) },
                    onReveal: { viewModel.revealInFinder(expanded) }
                )
                .transition(.opacity)
            }
        }
        // Pinned to the top-left and allowed to be smaller than its content: if
        // the window is ever briefly too small (mid-resize), content is cut at
        // the far edges rather than re-centred with the search field pushed off.
        .frame(minWidth: 0, maxWidth: .infinity, minHeight: 0, maxHeight: .infinity, alignment: .topLeading)
        // One flat scrim over the window material: its opacity is the
        // "Background opacity" setting. No decorative gradients on a surface
        // people read from.
        .background(Color.black.opacity(0.25 + settings.glassIntensity * 0.6))
        .animation(DS.Motion.ease(DS.Motion.base), value: viewModel.expandedResult)
        .animation(DS.Motion.ease(), value: settings.resultDensity)
    }
}
