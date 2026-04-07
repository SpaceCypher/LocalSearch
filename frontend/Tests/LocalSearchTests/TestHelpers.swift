import Foundation
@testable import LocalSearch

// MARK: - Mock Extensions

extension SearchResult {
    static func mock(rank: Float, id: String = UUID().uuidString) -> SearchResult {
        SearchResult(
            id: id,
            filename: "file_\(id).txt",
            path: "/Users/test/\(id).txt",
            rank: rank
        )
    }
}

// MARK: - Mock Backends

class ImmediateBackend: SearchBackendProtocol {
    let results: [SearchResult]
    
    init(results: [SearchResult]) {
        self.results = results
    }
    
    func search(query: String, filters: [QueryFilter], scope: SearchScope, cancellationToken: CancellationToken) -> AsyncStream<SearchResult> {
        AsyncStream { continuation in
            for result in results {
                continuation.yield(result)
            }
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
    
    func prefetchPrefix(_ prefix: String) async {}
}

class SlowBackend: SearchBackendProtocol {
    let delay: Duration
    
    init(delay: Duration = .milliseconds(500)) {
        self.delay = delay
    }
    
    func search(query: String, filters: [QueryFilter], scope: SearchScope, cancellationToken: CancellationToken) -> AsyncStream<SearchResult> {
        AsyncStream { continuation in
            Task {
                try? await Task.sleep(for: delay)
                continuation.yield(.mock(rank: 1.0))
                continuation.finish()
            }
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
    
    func prefetchPrefix(_ prefix: String) async {}
}

class TrackingBackend: SearchBackendProtocol {
    var searchCallCount = 0
    var lastQuery: String?
    
    func search(query: String, filters: [QueryFilter], scope: SearchScope, cancellationToken: CancellationToken) -> AsyncStream<SearchResult> {
        searchCallCount += 1
        lastQuery = query
        
        return AsyncStream { continuation in
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
    
    func prefetchPrefix(_ prefix: String) async {}
}
