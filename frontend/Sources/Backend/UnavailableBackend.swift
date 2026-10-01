import Foundation

/// Backend used when the native engine library cannot be loaded. Nothing can
/// be found without the engine, so it carries the reason for the UI to show
/// instead of letting every query end in "No results".
final class UnavailableBackend: SearchBackendProtocol {
    let unavailableReason: String?

    init(unavailableReason: String? = "The search engine library could not be loaded.") {
        self.unavailableReason = unavailableReason
    }

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
