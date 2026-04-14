import Foundation

// Backend used when native FFI backend cannot be loaded.
final class UnavailableBackend: SearchBackendProtocol {
    func search(query: String, filters: [QueryFilter], scope: SearchScope, cancellationToken: CancellationToken) -> AsyncStream<[SearchResult]> {
        AsyncStream { continuation in
            continuation.yield([])
            continuation.finish()
        }
    }

    func systemState() -> AsyncStream<SystemState> {
        AsyncStream { continuation in
            continuation.yield(.fuzzyPaused)
            continuation.finish()
        }
    }

    func indexProgress() -> AsyncStream<IndexProgress?> {
        AsyncStream { continuation in
            continuation.yield(nil)
            continuation.finish()
        }
    }

    func prefetchPrefix(_ prefix: String) async {}

    func spellingSuggestions(for query: String) async -> [String] {
        []
    }
}
