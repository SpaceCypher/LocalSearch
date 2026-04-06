import SwiftUI

// MARK: - PrefixCache

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

// MARK: - SearchViewModel Debounce Extension

extension SearchViewModel {
    // Task references for cancellation
    var debounceTask: Task<Void, Never>? {
        get { objc_getAssociatedObject(self, &AssociatedKeys.debounceTask) as? Task<Void, Never> }
        set { objc_setAssociatedObject(self, &AssociatedKeys.debounceTask, newValue, .OBJC_ASSOCIATION_RETAIN) }
    }
    
    var searchTask: Task<Void, Never>? {
        get { objc_getAssociatedObject(self, &AssociatedKeys.searchTask) as? Task<Void, Never> }
        set { objc_setAssociatedObject(self, &AssociatedKeys.searchTask, newValue, .OBJC_ASSOCIATION_RETAIN) }
    }
    
    var prefixCache: PrefixCache {
        if let cache = objc_getAssociatedObject(self, &AssociatedKeys.prefixCache) as? PrefixCache {
            return cache
        }
        let cache = PrefixCache()
        objc_setAssociatedObject(self, &AssociatedKeys.prefixCache, cache, .OBJC_ASSOCIATION_RETAIN)
        return cache
    }
}

// MARK: - Associated Object Keys

private struct AssociatedKeys {
    static var debounceTask: UInt8 = 0
    static var searchTask: UInt8 = 0
    static var prefixCache: UInt8 = 0
}
