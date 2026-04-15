import XCTest
@testable import LocalSearch

@MainActor
final class SearchViewModelTests: XCTestCase {
    
    func test_initialState_isIdle() {
        let vm = SearchViewModel(backend: FixtureBackend())
        XCTAssertEqual(vm.queryState, .idle)
        XCTAssertTrue(vm.displayResults.isEmpty)
        XCTAssertNil(vm.selectedIndex)
    }
    
    func test_generationIncrements_onEachQueryChange() async {
        let vm = SearchViewModel(backend: FixtureBackend())
        let gen0 = vm.queryGeneration
        vm.onQueryChange("a")
        XCTAssertEqual(vm.queryGeneration, gen0 + 1)
        vm.onQueryChange("ab")
        XCTAssertEqual(vm.queryGeneration, gen0 + 2)
    }
    
    func test_emptyQuery_transitionsToIdle_clearsResults() async {
        let vm = SearchViewModel(backend: FixtureBackend())
        vm.displayResults = [.mock(rank: 1.0)]
        vm.onQueryChange("")
        XCTAssertEqual(vm.queryState, .idle)
        XCTAssertTrue(vm.displayResults.isEmpty)
    }

    func test_streams_areWired_onInit() async throws {
        let backend = FixtureBackend()
        backend.fixtureSystemStates = [.fuzzyPaused]
        backend.fixtureIndexProgress = [IndexProgress(phase: "Scanning", percent: 0.25, etaMinutes: 2)]

        let vm = SearchViewModel(backend: backend)
        try await Task.sleep(nanoseconds: 30_000_000)

        XCTAssertEqual(vm.systemState, .fuzzyPaused)
        XCTAssertEqual(vm.indexProgress?.phase, "Scanning")
    }
    
    // MARK: - Task F3: Debounce + Cancellation Tests
    
    func test_debounce_firesAfter80ms() async throws {
        let backend = TrackingBackend()
        let vm = SearchViewModel(backend: backend)

        vm.onQueryChange("hello")

        // Before debounce: no search issued
        XCTAssertEqual(backend.searchCallCount, 0)

        // After debounce + scheduling buffer: search fires
        try await Task.sleep(nanoseconds: 130_000_000)
        XCTAssertEqual(backend.searchCallCount, 1)
    }

    func test_rapidTyping_firesOnlyOneSearch() async throws {
        let backend = TrackingBackend()
        let vm = SearchViewModel(backend: backend)

        vm.onQueryChange("h")
        vm.onQueryChange("he")
        vm.onQueryChange("hel")
        vm.onQueryChange("hell")
        vm.onQueryChange("hello")

        try await Task.sleep(nanoseconds: 130_000_000) // Wait past debounce + scheduling buffer

        // Only 1 search despite 5 keystrokes
        XCTAssertEqual(backend.searchCallCount, 1)
        XCTAssertEqual(backend.lastQuery, "hello")
    }

    func test_shortPrefix_triggersPrefetch() async throws {
        let backend = TrackingBackend()
        let vm = SearchViewModel(backend: backend)

        vm.onQueryChange("d")
        try await Task.sleep(nanoseconds: 20_000_000)

        XCTAssertEqual(backend.prefetchCallCount, 1)
        XCTAssertEqual(backend.lastPrefetchedPrefix, "d")
    }

    func test_newQuery_cancels_previousSearchTask() async throws {
        let backend = TaggedDelayBackend()
        let vm = SearchViewModel(backend: backend)

        vm.onQueryChange("first")
        try await Task.sleep(nanoseconds: 90_000_000) // debounce fires

        vm.onQueryChange("second") // cancel first, start second

        try await Task.sleep(nanoseconds: 300_000_000)

        // Only second query's results should be in displayResults
        XCTAssertTrue(vm.displayResults.allSatisfy { $0.id != "first" })
        XCTAssertTrue(vm.displayResults.contains { $0.id == "second" })
    }

    // Task F3 - Prefix cache test
    func test_prefixCache_hit_bypasses_debounce() async {
        let vm = SearchViewModel(backend: FixtureBackend())
        vm.prefixCache.store(prefix: "doc", results: [.mock(rank: 1.0, id: "cached")])

        vm.onQueryChange("doc")

        // Immediate — no 80ms wait
        XCTAssertEqual(vm.displayResults.first?.id, "cached")
        XCTAssertEqual(vm.queryState, .streaming) // cache hit, not idle
    }

    func test_slowSearch_showsSkeletons_afterThreshold() async throws {
        let vm = SearchViewModel(backend: SlowBackend(delay: .milliseconds(600)))
        vm.onQueryChange("slow")

        try await Task.sleep(nanoseconds: 250_000_000)
        XCTAssertEqual(vm.queryState, .searchingSlow)
        XCTAssertTrue(vm.showSkeletons)
        XCTAssertEqual(vm.skeletonCount, 3)
    }
    
    // MARK: - Task F8 - ScopeBarView Tests
    
    func test_scopeChange_appliesInstantly_noDebounce() async {
        let backend = TrackingBackend()
        let vm = SearchViewModel(backend: backend)
        vm.displayResults = [
            .mock(rank: 1.0, id: "doc1", fileKind: .document),
            .mock(rank: 0.8, id: "img1", fileKind: .image)
        ]
        vm.queryState = .complete
        
        let before = Date()
        vm.setActiveScope(.files) // must filter client-side immediately
        let elapsed = Date().timeIntervalSince(before)
        
        XCTAssertLessThan(elapsed, 0.016) // within one frame
        XCTAssertEqual(vm.filteredResults.count, 2)
        XCTAssertEqual(backend.searchCallCount, 0) // no new backend call for subset filter
    }
    
    func test_cmd1_5_shortcuts_switchScope() {
        let vm = SearchViewModel(backend: FixtureBackend())
        vm.handleKeyboardShortcut(.command, key: "1")
        XCTAssertEqual(vm.activeScope, .applications)
        vm.handleKeyboardShortcut(.command, key: "2")
        XCTAssertEqual(vm.activeScope, .files)
        vm.handleKeyboardShortcut(.command, key: "4")
        XCTAssertEqual(vm.activeScope, .clipboard)
    }
    
    func test_multipleScopes_areORd() {
        let vm = SearchViewModel(backend: FixtureBackend())
        vm.displayResults = [
            SearchResult(id: "app", filename: "Preview.app", path: "/Applications/Preview.app", rank: 1.0, fileKind: .document),
            .mock(rank: 0.9, id: "img", fileKind: .image),
            .mock(rank: 0.8, id: "code", fileKind: .code),
        ]
        vm.activateScope(.applications)
        vm.activateScope(.actions)
        XCTAssertEqual(vm.filteredResults.count, 1)
        XCTAssertEqual(vm.filteredResults.first?.id, "app")
    }

    func test_rightArrow_expandsSelectedResult() {
        let vm = SearchViewModel(backend: FixtureBackend())
        vm.displayResults = [.mock(rank: 1.0, id: "selected")]
        vm.selectedIndex = 0

        vm.handleArrowKey(.right)

        XCTAssertEqual(vm.expandedResult?.id, "selected")
    }

    func test_leftArrow_collapsesMetadataPanel() {
        let vm = SearchViewModel(backend: FixtureBackend())
        vm.expandedResult = .mock(rank: 1.0, id: "expanded")

        vm.handleArrowKey(.left)

        XCTAssertNil(vm.expandedResult)
    }
}

private final class TaggedDelayBackend: SearchBackendProtocol {
    func search(query: String, filters: [QueryFilter], scope: SearchScope, cancellationToken: CancellationToken) -> AsyncStream<[SearchResult]> {
        AsyncStream { continuation in
            Task {
                let delay: UInt64 = query == "first" ? 250_000_000 : 50_000_000
                try? await Task.sleep(nanoseconds: delay)
                continuation.yield([.mock(rank: 1.0, id: query)])
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

    func spellingSuggestions(for query: String) async -> [String] {
        []
    }
}
