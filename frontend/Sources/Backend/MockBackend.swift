import Foundation

// MARK: - Mock Backend for Development and Testing

class MockBackend: SearchBackendProtocol {
    func search(query: String, filters: [QueryFilter], scope: SearchScope, cancellationToken: CancellationToken) -> AsyncStream<SearchResult> {
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
}
