import Foundation

// MARK: - Mock Backend for Development and Testing

/// A backend with no engine behind it. Used in tests, and as the stand-in
/// when the engine library cannot be loaded (see `makeBackend`), in which
/// case `unavailableReason` says why so the UI can tell the user.
class MockBackend: SearchBackendProtocol {
    var mockSuggestions: [String] = []
    var unavailableReason: String?
    
    init(unavailableReason: String? = nil) {
        self.unavailableReason = unavailableReason
    }
    
    func search(query: String, filters: [QueryFilter], scope: SearchScope, cancellationToken: CancellationToken) -> AsyncStream<[SearchResult]> {
        AsyncStream { continuation in
            continuation.finish()
        }
    }
    
    func systemState() -> AsyncStream<SystemState> {
        AsyncStream { continuation in
            continuation.yield(.nominal)
            continuation.finish()
        }
    }
    
    func indexProgress() -> AsyncStream<IndexProgress?> {
        AsyncStream { continuation in
            continuation.yield(nil)
            continuation.finish()
        }
    }
    
    func prefetchPrefix(_ prefix: String) async {
        // No-op for mock
    }
    
    func spellingSuggestions(for query: String) async -> [String] {
        return mockSuggestions
    }
}
