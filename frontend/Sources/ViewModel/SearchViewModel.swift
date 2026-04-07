import SwiftUI

@MainActor
class SearchViewModel: ObservableObject {
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
    
    // Prefix cache (F3)
    let prefixCache = PrefixCache()
    
    // F6: QueryFieldView state
    @Published var showSpinner: Bool = false
    @Published var parsedFilters: [QueryFilter] = []
    @Published var strippedQueryText: String = ""
    
    var showClearButton: Bool {
        !queryText.isEmpty
    }
    
    // F8: ScopeBarView state
    @Published var activeScope: SearchScope = .all
    @Published var activeScopes: Set<SearchScope> = []
    
    var filteredResults: [SearchResult] {
        guard !activeScopes.isEmpty && !activeScopes.contains(.all) else {
            return displayResults
        }
        
        return displayResults.filter { result in
            activeScopes.contains { scope in
                switch scope {
                case .all:
                    return true
                case .documents:
                    return result.fileKind == .document
                case .images:
                    return result.fileKind == .image
                case .code:
                    return result.fileKind == .code
                case .folders:
                    return result.fileKind == .folder
                }
            }
        }
    }
    
    init(backend: SearchBackendProtocol) {
        self.backend = backend
    }
    
    // MARK: - Query Lifecycle
    
    func onQueryChange(_ newText: String) {
        // 1. Cancel any pending debounce or search tasks
        debounceTask?.cancel()
        searchTask?.cancel()
        
        // 2. Synchronous: update generation, dim results
        queryGeneration &+= 1
        queryText = newText
        
        // 3. Parse query for filters (F6)
        let parsed = QueryParser.parse(newText)
        parsedFilters = parsed.filters
        strippedQueryText = parsed.text
        
        // 4. Update UI state (F6)
        showSpinner = false
        
        // 5. Handle empty query
        guard !newText.isEmpty else {
            queryState = .idle
            displayResults = []
            selectedIndex = nil
            return
        }
        
        // 6. Check prefix cache for instant results
        if let cachedResults = prefixCache.lookup(prefix: newText) {
            queryState = .streaming
            displayResults = cachedResults
            return
        }
        
        // 7. Dim stale results immediately (synchronous)
        queryState = .typing
        dimCurrentResults()
        
        // 8. Start debounce task (80ms trailing-edge)
        let currentGeneration = queryGeneration
        debounceTask = Task { @MainActor in
            do {
                try await Task.sleep(nanoseconds: 80_000_000) // 80ms
                
                // Show spinner after debounce (F6)
                guard currentGeneration == queryGeneration else { return }
                showSpinner = true
                
                // After debounce, start search
                await startSearch(query: newText, generation: currentGeneration)
            } catch {
                // Task was cancelled, do nothing
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
            setActiveScope(.all)
        case "2":
            setActiveScope(.documents)
        case "3":
            setActiveScope(.images)
        case "4":
            setActiveScope(.code)
        case "5":
            setActiveScope(.folders)
        default:
            break
        }
    }
    
    private func startSearch(query: String, generation: UInt64) async {
        queryState = .searching
        
        searchTask = Task { @MainActor in
            let stream = backend.search(
                query: query,
                filters: parsedFilters,
                scope: .all,
                cancellationToken: CancellationToken()
            )
            
            for await result in stream {
                guard generation == queryGeneration else { break }
                await applyResult(result, fromGeneration: generation)
            }
            
            guard generation == queryGeneration else { return }
            queryState = .complete
            showSpinner = false // Hide spinner when complete (F6)
        }
    }
    
    func applyResult(_ result: SearchResult, fromGeneration generation: UInt64) async {
        // Generation check — discard stale results
        guard generation == queryGeneration else { return }
        
        // Track selected document ID before insertion
        let selectedDocId = selectedIndex.flatMap { displayResults[safe: $0]?.id }
        
        // Insert result maintaining sort order (insertion sort)
        insertResultSorted(result)
        
        // Restore selection to same document (not same position)
        if let docId = selectedDocId,
           let newIndex = displayResults.firstIndex(where: { $0.id == docId }) {
            selectedIndex = newIndex
        }
    }
    
    // MARK: - Private Helpers
    
    private func dimCurrentResults() {
        for i in displayResults.indices {
            displayResults[i].opacity = 0.4
        }
    }
    
    private func insertResultSorted(_ result: SearchResult) {
        // Binary search for insertion point
        var left = 0
        var right = displayResults.count
        
        while left < right {
            let mid = (left + right) / 2
            if displayResults[mid].rank > result.rank {
                left = mid + 1
            } else {
                right = mid
            }
        }
        
        displayResults.insert(result, at: left)
        
        // Cap at 20 results
        if displayResults.count > 20 {
            displayResults.removeLast()
        }
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

struct SearchResult: Identifiable, Equatable {
    let id: String
    let filename: String
    let path: String
    let rank: Float
    let fileKind: FileKind
    var opacity: Float = 1.0
    
    static func == (lhs: SearchResult, rhs: SearchResult) -> Bool {
        lhs.id == rhs.id && lhs.rank == rhs.rank && lhs.opacity == rhs.opacity && lhs.fileKind == rhs.fileKind
    }
}

// MARK: - Backend Protocol

protocol SearchBackendProtocol {
    func search(
        query: String,
        filters: [QueryFilter],
        scope: SearchScope,
        cancellationToken: CancellationToken
    ) -> AsyncStream<SearchResult>
    
    func systemState() -> AsyncStream<SystemState>
    func indexProgress() -> AsyncStream<IndexProgress?>
    func prefetchPrefix(_ prefix: String) async
}

// MARK: - Supporting Types

enum SearchScope: Equatable, CaseIterable {
    case all
    case documents
    case images
    case code
    case folders
}

struct CancellationToken {
    // Placeholder
}

struct SystemState: Equatable {
    static let nominal = SystemState()
}

struct IndexProgress {
    let phase: String
    let percent: Double
    let etaMinutes: Int
}

// MARK: - Array Safe Subscript

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
