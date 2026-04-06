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
    
    init(backend: SearchBackendProtocol) {
        self.backend = backend
    }
    
    // MARK: - Query Lifecycle
    
    func onQueryChange(_ newText: String) {
        // 1. Synchronous: update generation, dim results
        queryGeneration &+= 1
        queryText = newText
        
        // 2. Handle empty query
        guard !newText.isEmpty else {
            queryState = .idle
            displayResults = []
            selectedIndex = nil
            return
        }
        
        // 3. Dim stale results immediately (synchronous)
        queryState = .typing
        dimCurrentResults()
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

struct SearchResult: Identifiable, Equatable {
    let id: String
    let filename: String
    let path: String
    let rank: Float
    var opacity: Float = 1.0
    
    static func == (lhs: SearchResult, rhs: SearchResult) -> Bool {
        lhs.id == rhs.id && lhs.rank == rhs.rank && lhs.opacity == rhs.opacity
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

struct QueryFilter: Equatable {
    // Placeholder
}

enum SearchScope {
    case all
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
