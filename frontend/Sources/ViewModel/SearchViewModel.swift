import SwiftUI
import AppKit

@MainActor
class SearchViewModel: ObservableObject {
    private let minimumSearchLength = 2

    // Query state
    @Published var queryText: String = ""
    @Published var queryState: QueryState = .idle
    @Published var displayResults: [SearchResult] = []
    @Published var selectedIndex: Int? = nil
    
    // Query generation counter for cancellation
    private(set) var queryGeneration: UInt64 = 0
    
    // Backend channel
    private let backend: SearchBackendProtocol
    
    // Task references for cancellation (F3)
    var debounceTask: Task<Void, Never>?
    var searchTask: Task<Void, Never>?
    private var systemStateTask: Task<Void, Never>?
    private var slowStateTask: Task<Void, Never>?
    private var longSearchTask: Task<Void, Never>?
    
    // Prefix cache (F3)
    let prefixCache = PrefixCache()
    
    // F6: QueryFieldView state
    @Published var showSpinner: Bool = false
    @Published var showSkeletons: Bool = false
    @Published var isLongSearch: Bool = false
    @Published var parsedFilters: [QueryFilter] = []
    @Published var strippedQueryText: String = ""
    
    var skeletonCount: Int {
        3
    }
    
    var showClearButton: Bool {
        !queryText.isEmpty
    }
    
    // F11: StatusBarView state
    var statusText: String {
        switch queryState {
        case .idle:
            if backendUnavailableReason != nil {
                return "Search engine not loaded"
            }
            return "Start typing to search"
        case .typing:
            return "Typing…"
        case .searching:
            return "Searching…"
        case .searchingSlow:
            return isLongSearch ? "Search is taking longer than usual" : "Searching…"
        case .streaming:
            return "Streaming results…"
        case .complete:
            if backendUnavailableReason != nil {
                return "Search engine not loaded"
            } else if displayResults.isEmpty {
                // The panel itself says what was searched for and why nothing matched
                return "No results"
            } else {
                let count = displayResults.count
                return "\(count) result\(count == 1 ? "" : "s")"
            }
        }
    }
    
    // F8: ScopeBarView state
    @Published var activeScope: SearchScope = .files
    @Published var activeScopes: Set<SearchScope> = []
    
    var filteredResults: [SearchResult] {
        guard !activeScopes.isEmpty && !activeScopes.contains(.files) else {
            return displayResults
        }
        
        return displayResults.filter { result in
            activeScopes.contains { scope in
                switch scope {
                case .applications:
                    return result.path.hasSuffix(".app") || result.path.hasSuffix(".app/")
                case .files:
                    return true // Handled by guard above generally, but acts as "All" for now
                case .actions, .clipboard:
                    return false // Mocked empty for now as backend doesn't support them
                }
            }
        }
    }
    
    // F10: Keyboard navigation
    @Published var expandedResult: SearchResult? = nil
    var queryHistory: [String] = []
    private var historyIndex: Int = -1
    private var isNavigatingHistory: Bool = false
    private var isProgrammaticQueryChange: Bool = false
    
    // F16: Alerter for permission-denied dialogs
    var alerter: AlerterProtocol?
    
    // F17: Accessibility
    var isReduceMotionEnabled: Bool = false
    
    var searchFieldAccessibilityLabel: String {
        "Search files"
    }
    
    var searchFieldAccessibilityHint: String {
        "Type to search your files"
    }
    
    var statusBarIsLiveRegion: Bool {
        true
    }
    
    // F14: Index progress
    @Published var indexProgress: IndexProgress? = nil
    private var indexProgressTask: Task<Void, Never>?
    
    /// Why each result matched, by result id: the matching line of its content.
    /// Filled in after the results appear, for the rows near the top.
    @Published var snippets: [String: AttributedString] = [:]
    private var snippetTask: Task<Void, Never>?
    static let snippetLimit = 20
    
    /// Set when the search engine could not be loaded; search cannot work.
    @Published var backendUnavailableReason: String? = nil
    
    /// Asks the window to hide after a result has been opened.
    var onDismissRequested: (() -> Void)?
    
    /// With an empty query the panel is only the search field, unless indexing
    /// is running or the engine failed to load, which the status bar reports.
    var showsStatusBar: Bool {
        !queryText.isEmpty || indexProgress != nil || backendUnavailableReason != nil
    }
    
    var showIndexProgress: Bool {
        indexProgress != nil
    }
    
    // F15: Zero-result state
    @Published var spellingSuggestions: [String] = []
    @Published var systemState: SystemState = .nominal
    
    var zeroResultsQuery: String {
        queryText
    }
    
    var showBroadeningTip: Bool {
        !parsedFilters.isEmpty && displayResults.isEmpty && queryState == .complete
    }
    
    var zeroResultsNote: String {
        if systemState == .fuzzyPaused {
            return "Exact mode active — fuzzy matching paused"
        }
        return ""
    }
    
    func selectSuggestion(_ suggestion: String) {
        isProgrammaticQueryChange = true
        queryText = suggestion
        isProgrammaticQueryChange = false
        onQueryChange(suggestion)
    }
    
    // MARK: - Selection Sync Helper
    func syncExpandedResult() {
        if expandedResult != nil {
            if let index = selectedIndex, index < filteredResults.count {
                expandedResult = filteredResults[index]
            } else {
                expandedResult = nil
            }
        }
    }
    
    init(backend: SearchBackendProtocol) {
        self.backend = backend
        self.backendUnavailableReason = backend.unavailableReason
        observeIndexProgress()
        observeSystemState()
    }
    
    deinit {
        debounceTask?.cancel()
        searchTask?.cancel()
        snippetTask?.cancel()
        systemStateTask?.cancel()
        indexProgressTask?.cancel()
        slowStateTask?.cancel()
        longSearchTask?.cancel()
    }
    
    private func observeSystemState() {
        let stream = backend.systemState()
        systemStateTask = Task { @MainActor [weak self] in
            for await state in stream {
                self?.systemState = state
            }
        }
    }
    
    private func observeIndexProgress() {
        let stream = backend.indexProgress()
        indexProgressTask = Task { @MainActor [weak self] in
            for await progress in stream {
                guard let self else { return }
                if self.indexProgress != progress {
                    self.indexProgress = progress
                }
            }
        }
    }
    
    // MARK: - Why a result matched
    
    /// Fetch matching lines for the top results, one at a time, off the main
    /// actor. Abandoned as soon as the query moves on.
    private func loadSnippets(for query: String, generation: UInt64) {
        snippetTask?.cancel()
        let results = Array(displayResults.prefix(Self.snippetLimit))
        // Drop lines belonging to results that are no longer listed
        let ids = Set(results.map(\.id))
        snippets = snippets.filter { ids.contains($0.key) }
        
        let backend = self.backend
        snippetTask = Task { @MainActor [weak self] in
            for result in results where result.fileKind != .folder {
                let raw = await backend.snippet(path: result.path, query: query)
                guard let self, !Task.isCancelled, generation == self.queryGeneration else { return }
                self.snippets[result.id] = raw.map(SnippetFormatter.attributed)
            }
        }
    }
    
    // MARK: - Opening results
    
    /// Open the result, tell the engine (opened files rank higher next time),
    /// remember the query, and get out of the user's way.
    func open(_ result: SearchResult) {
        NSWorkspace.shared.open(URL(fileURLWithPath: result.path))
        didUse(result)
    }
    
    func revealInFinder(_ result: SearchResult) {
        NSWorkspace.shared.selectFile(result.path, inFileViewerRootedAtPath: "")
        didUse(result)
    }
    
    private func didUse(_ result: SearchResult) {
        backend.recordOpen(path: result.path)
        if !queryText.isEmpty, queryHistory.first != queryText {
            queryHistory.insert(queryText, at: 0)
            queryHistory = Array(queryHistory.prefix(50))
        }
        onDismissRequested?()
    }
    
    func toggleDetails() {
        if expandedResult != nil {
            expandedResult = nil
        } else if let index = selectedIndex, index < filteredResults.count {
            expandedResult = filteredResults[index]
        }
    }
    
    /// Drop inline filters (`kind:`, `in:`, …) from the query, keeping the words.
    func clearFilters() {
        isProgrammaticQueryChange = true
        queryText = strippedQueryText.trimmingCharacters(in: .whitespaces)
        isProgrammaticQueryChange = false
        onQueryChange(queryText)
    }
    
    // MARK: - Query Lifecycle
    
    func onQueryChange(_ newText: String) {
        // Reset history navigation if user is typing (not programmatic change)
        if !isProgrammaticQueryChange {
            historyIndex = -1
            isNavigatingHistory = false
        }
        isProgrammaticQueryChange = false
        
        // 1. Cancel any pending debounce or search tasks
        debounceTask?.cancel()
        searchTask?.cancel()
        slowStateTask?.cancel()
        longSearchTask?.cancel()
        
        // 2. Synchronous: update generation, dim results
        queryGeneration &+= 1
        // (queryText is already updated via the TextField binding, no need to reassign)
        
        // 3. Parse query for filters (F6)
        let parsed = QueryParser.parse(newText)
        parsedFilters = parsed.filters
        strippedQueryText = parsed.text
        
        // 4. Update UI state (F6)
        showSpinner = false
        showSkeletons = false
        isLongSearch = false
        
        // 5. Handle empty query
        guard !newText.isEmpty else {
            queryState = .idle
            snippetTask?.cancel()
            snippets = [:]
            displayResults = []
            selectedIndex = nil
            expandedResult = nil
            return
        }
        
        // Speculative prefix prefetch for first 1-2 typed characters.
        if newText.count <= 2 {
            let prefix = String(newText.prefix(2))
            let backend = self.backend
            Task {
                await backend.prefetchPrefix(prefix)
            }
        }

        // A single character matches almost everything; wait for a second one.
        if strippedQueryText.count < minimumSearchLength {
            queryState = .typing
            snippetTask?.cancel()
            snippets = [:]
            displayResults = []
            selectedIndex = nil
            expandedResult = nil
            spellingSuggestions = []
            return
        }
        
        // 6. Check prefix cache for instant results
        if let cachedResults = prefixCache.lookup(prefix: newText) {
            queryState = .streaming
            displayResults = cachedResults
            return
        }
        
        // 7. Start debounce task (80ms trailing-edge)
        let currentGeneration = queryGeneration
        debounceTask = Task { @MainActor in
            do {
                // Update to typing state after a short delay to avoid immediate UI thrashing
                try await Task.sleep(nanoseconds: 40_000_000) // 40ms
                guard currentGeneration == queryGeneration else { return }
                queryState = .typing
                
                try await Task.sleep(nanoseconds: 40_000_000) // remaining 40ms to reach 80ms total
                
                // Show spinner after debounce
                guard currentGeneration == queryGeneration else { return }
                showSpinner = true
                
                // Start search
                await startSearch(query: newText, generation: currentGeneration)
            } catch {
                // Task was cancelled
            }
        }
    }
    
    func clearQuery() {
        queryText = ""
        onQueryChange("")
    }
    
    // MARK: - Scope Management (F8)
    
    func setActiveScope(_ scope: SearchScope) {
        activeScope = scope
        activeScopes = [scope]
    }
    
    func activateScope(_ scope: SearchScope) {
        activeScope = scope
        activeScopes.insert(scope)
    }
    
    func handleKeyboardShortcut(_ modifiers: EventModifiers, key: String) {
        guard modifiers.contains(.command) else { return }
        
        switch key {
        case "1":
            setActiveScope(.applications)
        case "2":
            setActiveScope(.files)
        case "3":
            setActiveScope(.actions)
        case "4":
            setActiveScope(.clipboard)
        default:
            break
        }
    }
    
    // MARK: - Keyboard Navigation (F10)
    
    func handleArrowKey(_ direction: ArrowDirection) {
        switch direction {
        case .up:
            if queryText.isEmpty || isNavigatingHistory {
                // Navigate query history
                if !queryHistory.isEmpty {
                    historyIndex = min(historyIndex + 1, queryHistory.count - 1)
                    if historyIndex >= 0 && historyIndex < queryHistory.count {
                        isProgrammaticQueryChange = true
                        queryText = queryHistory[historyIndex]
                        isNavigatingHistory = true
                    }
                }
            } else if let current = selectedIndex, current > 0 {
                selectedIndex = current - 1
                syncExpandedResult()
            }
            
        case .down:
            if isNavigatingHistory {
                // Navigate down in history (towards more recent)
                historyIndex = max(historyIndex - 1, -1)
                if historyIndex >= 0 && historyIndex < queryHistory.count {
                    isProgrammaticQueryChange = true
                    queryText = queryHistory[historyIndex]
                } else {
                    isProgrammaticQueryChange = true
                    queryText = ""
                    isNavigatingHistory = false
                }
            } else if let current = selectedIndex, current < filteredResults.count - 1 {
                selectedIndex = current + 1
                syncExpandedResult()
            } else if selectedIndex == nil && !filteredResults.isEmpty {
                selectedIndex = 0
                syncExpandedResult()
            }
            
        case .left:
            expandedResult = nil
            
        case .right:
            if let index = selectedIndex, index < filteredResults.count {
                expandedResult = filteredResults[index]
            }
        }
    }
    
    func handleReturnKey(modifiers: EventModifiers) {
        guard let index = selectedIndex, index < filteredResults.count else { return }
        let result = filteredResults[index]
        
        // Check for permission-denied state
        if result.permissionState == .revoked {
            alerter?.showAlert(
                title: "Permission Denied",
                message: "This file cannot be accessed. Grant permission in System Settings.",
                buttons: ["Open System Settings", "Cancel"]
            )
            return
        }
        
        if modifiers.contains(.command) {
            revealInFinder(result)
        } else if modifiers.contains(.option) {
            // Copy path to clipboard
            let pasteboard = NSPasteboard.general
            pasteboard.clearContents()
            pasteboard.setString(result.path, forType: .string)
        } else {
            open(result)
        }
    }
    
    private func startSearch(query: String, generation: UInt64) async {
        queryState = .searching
        showSkeletons = false
        isLongSearch = false

        // The previous results stay on screen until the new ones arrive, so
        // the list never blanks (and the window never collapses) between keystrokes.
        // Skeleton rows stand in only when there is nothing to keep showing.
        slowStateTask?.cancel()
        longSearchTask?.cancel()
        slowStateTask = Task { @MainActor in
            try? await Task.sleep(nanoseconds: 150_000_000)
            guard generation == queryGeneration else { return }
            guard queryState == .searching else { return }
            guard displayResults.isEmpty else { return }
            queryState = .searchingSlow
            showSkeletons = true
        }

        longSearchTask = Task { @MainActor in
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            guard generation == queryGeneration else { return }
            guard queryState == .searchingSlow else { return }
            isLongSearch = true
        }
        
        searchTask = Task { @MainActor in
            let stream = backend.search(
                query: query,
                filters: parsedFilters,
                scope: .files,
                cancellationToken: CancellationToken()
            )
            
            var received = false
            for await batch in stream {
                guard generation == queryGeneration else { break }
                
                received = true
                slowStateTask?.cancel()
                longSearchTask?.cancel()
                showSkeletons = false
                isLongSearch = false
                queryState = .streaming
                displayResults = batch // Atomic assignment prevents UI thrashing
                // A new result set starts at the top hit
                selectedIndex = batch.isEmpty ? nil : 0
                syncExpandedResult()
            }
            
            guard generation == queryGeneration else { return }
            loadSnippets(for: query, generation: generation)
            if !received {
                // The backend returned nothing at all
                displayResults = []
                selectedIndex = nil
                syncExpandedResult()
            }
            slowStateTask?.cancel()
            longSearchTask?.cancel()
            queryState = .complete
            showSpinner = false // Hide spinner when complete
            showSkeletons = false
            isLongSearch = false
            
            // Fetch spelling suggestions if zero results (F15)
            if displayResults.isEmpty {
                spellingSuggestions = await backend.spellingSuggestions(for: query)
            } else {
                spellingSuggestions = []
            }
        }
    }
    
    // MARK: - Private Helpers
    
    private func dimCurrentResults() {
        // Opacity dimming is now handled cleanly on the View layer via queryState
        // to prevent extreme SwiftUI array differencing lag.
    }
}

// MARK: - Query State

enum QueryState: Equatable {
    case idle
    case typing
    case searching
    case searchingSlow
    case streaming
    case complete
}

// MARK: - Search Result

enum FileKind: Equatable {
    case document
    case image
    case code
    case folder
}

enum PermissionState: Equatable {
    case granted
    case revoked
}

struct SearchResult: Identifiable, Equatable {
    let id: String
    let filename: String
    let path: String
    let rank: Float
    let fileKind: FileKind
    var permissionState: PermissionState = .granted
    
    static func == (lhs: SearchResult, rhs: SearchResult) -> Bool {
        lhs.id == rhs.id && lhs.rank == rhs.rank && lhs.fileKind == rhs.fileKind && lhs.permissionState == rhs.permissionState
    }
}

// MARK: - Backend Protocol

protocol SearchBackendProtocol {
    func search(
        query: String,
        filters: [QueryFilter],
        scope: SearchScope,
        cancellationToken: CancellationToken
    ) -> AsyncStream<[SearchResult]>
    
    func systemState() -> AsyncStream<SystemState>
    func indexProgress() -> AsyncStream<IndexProgress?>
    func prefetchPrefix(_ prefix: String) async
    func spellingSuggestions(for query: String) async -> [String]
    
    /// Why search cannot work, if it cannot (shown to the user). Nil when healthy.
    var unavailableReason: String? { get }
    /// The user opened this result; feeds ranking.
    func recordOpen(path: String)
    /// Apply indexing settings (JSON, see `SettingsManager.indexingConfigJSON`).
    func applyIndexingConfig(_ json: String)
    /// One-line description of the index state for the settings window.
    func indexSummary() -> String?
    /// Save the index. Called once when the app quits.
    func shutdown()
    /// The line of the file's content that matches the query, with matches
    /// wrapped in U+0001 … U+0002; nil when it matched on its name alone.
    func snippet(path: String, query: String) async -> String?
}

// Backends without an engine behind them need none of the engine hooks.
extension SearchBackendProtocol {
    var unavailableReason: String? { nil }
    func recordOpen(path: String) {}
    func applyIndexingConfig(_ json: String) {}
    func indexSummary() -> String? { nil }
    func shutdown() {}
    func snippet(path: String, query: String) async -> String? { nil }
}

// MARK: - Supporting Types

enum SearchScope: Equatable, CaseIterable {
    case applications
    case files
    case actions
    case clipboard
}

struct CancellationToken {
    // Placeholder
}

enum SystemState: Equatable {
    case nominal
    case fuzzyPaused
}

struct IndexProgress: Equatable {
    let phase: String
    /// 0...1. Meaningful only when `isDeterminate`.
    let percent: Double
    /// Nil while there is not enough history for an honest estimate.
    let etaMinutes: Int?
    
    var isDeterminate: Bool { percent > 0 }
    
    var etaText: String? {
        guard let minutes = etaMinutes else { return nil }
        return minutes == 1 ? "About 1 min left" : "About \(minutes) min left"
    }
    
    /// Compact form for the status bar, e.g. "Reading file contents 42%"
    var statusText: String {
        isDeterminate ? "\(phase) \(Int(percent * 100))%" : "\(phase)…"
    }
}

// MARK: - Snippets

enum SnippetFormatter {
    static let markStart: Character = "\u{1}"
    static let markEnd: Character = "\u{2}"
    
    /// Turn the engine's marked-up line into text with the matches emphasised.
    /// Emphasis is weight and brightness, so it does not depend on colour.
    static func attributed(_ raw: String) -> AttributedString {
        var result = AttributedString()
        var buffer = ""
        var inMatch = false
        
        func flush() {
            guard !buffer.isEmpty else { return }
            var piece = AttributedString(buffer)
            if inMatch {
                piece.font = .system(size: DS.TextSize.xs, weight: .semibold)
                piece.foregroundColor = DS.Palette.text
            }
            result.append(piece)
            buffer = ""
        }
        
        for character in raw {
            if character == markStart {
                flush()
                inMatch = true
            } else if character == markEnd {
                flush()
                inMatch = false
            } else {
                buffer.append(character)
            }
        }
        flush()
        return result
    }
    
    /// The line without markup, for accessibility labels
    static func plain(_ snippet: AttributedString) -> String {
        String(snippet.characters)
    }
}

// MARK: - Array Safe Subscript

enum ArrowDirection {
    case up
    case down
    case left
    case right
}

extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}

// MARK: - PrefixCache (F3)

@MainActor
class PrefixCache {
    private var cache: [String: [SearchResult]] = [:]
    
    func store(prefix: String, results: [SearchResult]) {
        cache[prefix] = results
    }
    
    func lookup(prefix: String) -> [SearchResult]? {
        return cache[prefix]
    }
    
    func clear() {
        cache.removeAll()
    }
}


// MARK: - Alerter Protocol

protocol AlerterProtocol {
    func showAlert(title: String, message: String, buttons: [String])
}
