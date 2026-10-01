import XCTest
@testable import LocalSearch

@MainActor
final class SearchViewModelTests: XCTestCase {
    
    func test_initialState_isIdle() {
        let vm = SearchViewModel(backend: MockBackend())
        XCTAssertEqual(vm.queryState, .idle)
        XCTAssertTrue(vm.displayResults.isEmpty)
        XCTAssertNil(vm.selectedIndex)
    }
    
    func test_queryChange_keepsPreviousResultsVisible() async {
        let vm = SearchViewModel(backend: MockBackend())
        vm.displayResults = [.mock(rank: 1.0, id: "old")]
        vm.queryState = .complete
        
        vm.onQueryChange("new")
        
        // The list must not blank between keystrokes
        XCTAssertEqual(vm.displayResults.map(\.id), ["old"])
    }
    
    func test_newResults_replaceOld_andSelectTopHit() async throws {
        let vm = SearchViewModel(backend: ImmediateBackend(results: [.mock(rank: 1.0, id: "A"), .mock(rank: 0.5, id: "B")]))
        vm.displayResults = [.mock(rank: 1.0, id: "old")]
        vm.selectedIndex = 0
        
        vm.queryText = "new"
        vm.onQueryChange("new")
        try await Task.sleep(nanoseconds: 200_000_000)
        
        XCTAssertEqual(vm.displayResults.map(\.id), ["A", "B"])
        XCTAssertEqual(vm.selectedIndex, 0)
        XCTAssertEqual(vm.queryState, .complete)
    }
    
    func test_emptyAnswer_clearsPreviousResults() async throws {
        let vm = SearchViewModel(backend: MockBackend())
        vm.displayResults = [.mock(rank: 1.0, id: "old")]
        
        vm.queryText = "nothing"
        vm.onQueryChange("nothing")
        try await Task.sleep(nanoseconds: 200_000_000)
        
        XCTAssertTrue(vm.displayResults.isEmpty)
        XCTAssertNil(vm.selectedIndex)
    }
    
    func test_generationIncrements_onEachQueryChange() async {
        let vm = SearchViewModel(backend: MockBackend())
        let gen0 = vm.queryGeneration
        vm.onQueryChange("a")
        XCTAssertEqual(vm.queryGeneration, gen0 + 1)
        vm.onQueryChange("ab")
        XCTAssertEqual(vm.queryGeneration, gen0 + 2)
    }
    
    func test_emptyQuery_transitionsToIdle_clearsResults() async {
        let vm = SearchViewModel(backend: MockBackend())
        vm.displayResults = [.mock(rank: 1.0)]
        vm.onQueryChange("")
        XCTAssertEqual(vm.queryState, .idle)
        XCTAssertTrue(vm.displayResults.isEmpty)
    }
    
    func test_staleResults_discarded_whenQueryMovesOn() async throws {
        let vm = SearchViewModel(backend: SlowBackend(delay: .milliseconds(200)))
        vm.queryText = "first"
        vm.onQueryChange("first")
        try await Task.sleep(nanoseconds: 100_000_000) // search for "first" in flight
        
        vm.queryText = ""
        vm.onQueryChange("") // user cleared the field
        try await Task.sleep(nanoseconds: 300_000_000) // "first" would have answered by now
        
        XCTAssertTrue(vm.displayResults.isEmpty)
        XCTAssertEqual(vm.queryState, .idle)
    }
    
    // MARK: - Task F3: Debounce + Cancellation Tests
    
    func test_debounce_firesAfter80ms() async throws {
        let backend = TrackingBackend()
        let vm = SearchViewModel(backend: backend)

        vm.onQueryChange("hello")

        // Before debounce: no search issued
        XCTAssertEqual(backend.searchCallCount, 0)

        // Comfortably past the 80ms debounce (a tight margin fails on a busy machine)
        try await Task.sleep(nanoseconds: 250_000_000)
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

        try await Task.sleep(nanoseconds: 250_000_000) // Wait past debounce

        // Only 1 search despite 5 keystrokes
        XCTAssertEqual(backend.searchCallCount, 1)
        XCTAssertEqual(backend.lastQuery, "hello")
    }

    func test_newQuery_cancels_previousSearchTask() async throws {
        let backend = SlowBackend(delay: .milliseconds(200))
        let vm = SearchViewModel(backend: backend)

        vm.onQueryChange("first")
        try await Task.sleep(nanoseconds: 90_000_000) // debounce fires

        vm.onQueryChange("second") // cancel first, start second

        try await Task.sleep(nanoseconds: 300_000_000)

        // Only second query's results should be in displayResults
        XCTAssertFalse(vm.displayResults.contains { $0.id == "first" })
    }

    // Task F3 - Prefix cache test
    func test_prefixCache_hit_bypasses_debounce() async {
        let vm = SearchViewModel(backend: MockBackend())
        vm.prefixCache.store(prefix: "doc", results: [.mock(rank: 1.0, id: "cached")])

        vm.onQueryChange("doc")

        // Immediate — no 80ms wait
        XCTAssertEqual(vm.displayResults.first?.id, "cached")
        XCTAssertEqual(vm.queryState, .streaming) // cache hit, not idle
    }
    
    // MARK: - Scopes
    
    func test_cmdNumber_shortcuts_switchScope() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.handleKeyboardShortcut(.command, key: "1")
        XCTAssertEqual(vm.activeScope, .applications)
        vm.handleKeyboardShortcut(.command, key: "2")
        XCTAssertEqual(vm.activeScope, .files)
    }
    
    func test_applicationsScope_filtersClientSide() {
        let backend = TrackingBackend()
        let vm = SearchViewModel(backend: backend)
        vm.displayResults = [
            SearchResult(id: "app", filename: "Notes.app", path: "/Applications/Notes.app", rank: 1, fileKind: .folder),
            .mock(rank: 0.8, id: "doc"),
        ]
        
        vm.setActiveScope(.applications)
        
        XCTAssertEqual(vm.filteredResults.map(\.id), ["app"])
        XCTAssertEqual(backend.searchCallCount, 0) // no new backend call for a subset filter
    }
    
    // MARK: - Engine hooks
    
    func test_unavailableBackend_isSurfacedToTheUser() {
        let vm = SearchViewModel(backend: MockBackend(unavailableReason: "engine missing"))
        vm.queryText = "anything"
        vm.queryState = .complete
        
        XCTAssertEqual(vm.backendUnavailableReason, "engine missing")
        XCTAssertEqual(vm.statusText, "Search engine not loaded")
    }
    
    func test_clearFilters_keepsWords_dropsFilters() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.queryText = "report kind:pdf"
        vm.onQueryChange("report kind:pdf")
        XCTAssertFalse(vm.parsedFilters.isEmpty)
        
        vm.clearFilters()
        
        XCTAssertEqual(vm.queryText, "report")
        XCTAssertTrue(vm.parsedFilters.isEmpty)
    }
    
    // MARK: - Inline filters applied to engine results
    
    func test_searchRequest_stripsFiltersFromEngineQuery() {
        XCTAssertEqual(SearchRequest(query: "tax return kind:pdf in:documents -draft").text, "tax return")
        // A hyphen inside a word is part of the word, not an exclusion
        XCTAssertEqual(SearchRequest(query: "LocalSearch-master").text, "LocalSearch-master")
    }
    
    func test_searchRequest_filtersByKindFolderAndExclusion() {
        let request = SearchRequest(query: "tax kind:pdf in:documents -draft")
        XCTAssertTrue(request.matches(path: "/Users/me/Documents/tax 2025.pdf"))
        XCTAssertFalse(request.matches(path: "/Users/me/Documents/tax 2025.docx"))
        XCTAssertFalse(request.matches(path: "/Users/me/Downloads/tax 2025.pdf"))
        XCTAssertFalse(request.matches(path: "/Users/me/Documents/tax draft.pdf"))
        
        XCTAssertTrue(SearchRequest(query: "logo kind:image").matches(path: "/tmp/logo.PNG"))
    }
    
    // MARK: - Indexing settings
    
    func test_indexingSettings_encodeToEngineConfigJSON() throws {
        var settings = IndexingSettings()
        settings.roots = ["~/Documents"]
        settings.maxDepth = 5
        settings.indexContent = false
        
        let object = try JSONSerialization.jsonObject(with: Data(settings.json.utf8)) as? [String: Any]
        XCTAssertEqual(object?["roots"] as? [String], ["~/Documents"])
        XCTAssertEqual(object?["max_depth"] as? Int, 5)
        XCTAssertEqual(object?["index_content"] as? Bool, false)
        XCTAssertEqual(object?["index_hidden"] as? Bool, false)
    }
    
    func test_indexingSettings_addRoot_rejectsCoveredFolders_andAbsorbsChildren() {
        let home = FileManager.default.homeDirectoryForCurrentUser
        var settings = IndexingSettings()
        settings.roots = ["~/Documents/Work"]
        
        XCTAssertFalse(settings.addRoot(home.appendingPathComponent("Documents/Work/2025")))
        XCTAssertTrue(settings.addRoot(home.appendingPathComponent("Documents")))
        XCTAssertEqual(settings.roots, ["~/Documents"])
        
        XCTAssertTrue(settings.addRoot(URL(fileURLWithPath: "/Volumes/Archive")))
        XCTAssertEqual(settings.roots, ["~/Documents", "/Volumes/Archive"])
    }
    
    func test_indexingSettings_addExclude_validates() {
        var settings = IndexingSettings()
        settings.excludes = ["build"]
        
        XCTAssertTrue(settings.addExclude("  Vendor "))
        XCTAssertFalse(settings.addExclude("vendor"))
        XCTAssertFalse(settings.addExclude(""))
        XCTAssertFalse(settings.addExclude("a/b"))
        XCTAssertEqual(settings.excludes, ["build", "vendor"])
    }
}
