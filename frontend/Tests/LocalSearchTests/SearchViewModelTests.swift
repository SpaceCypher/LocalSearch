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
    
    func test_queryChange_immediatelyDimsStaleResults() async {
        let vm = SearchViewModel(backend: MockBackend())
        vm.displayResults = [.mock(rank: 1.0)]
        vm.queryState = .complete
        
        vm.onQueryChange("new")
        
        // Synchronous — must be true immediately, before any await
        XCTAssertEqual(vm.queryState, .typing)
        XCTAssertEqual(vm.displayResults.first?.opacity, 0.4)
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
    
    func test_staleResults_discarded_whenGenerationMismatch() async {
        let backend = MockBackend()
        let vm = SearchViewModel(backend: backend)
        let genBefore = vm.queryGeneration
        
        // Simulate result arriving from a cancelled generation
        await vm.applyResult(.mock(rank: 1.0), fromGeneration: genBefore)
        
        // Now generation has moved on
        vm.onQueryChange("new") // gen++
        await vm.applyResult(.mock(rank: 0.9), fromGeneration: genBefore) // old gen
        
        // Old result must be discarded
        XCTAssertFalse(vm.displayResults.contains { $0.rank == 0.9 })
    }
    
    func test_insertResult_maintainsSortOrder() async {
        let vm = SearchViewModel(backend: MockBackend())
        let gen = vm.queryGeneration
        
        await vm.applyResult(.mock(rank: 0.8, id: "A"), fromGeneration: gen)
        await vm.applyResult(.mock(rank: 0.95, id: "B"), fromGeneration: gen)
        await vm.applyResult(.mock(rank: 0.6, id: "C"), fromGeneration: gen)
        
        XCTAssertEqual(vm.displayResults.map(\.id), ["B", "A", "C"])
    }
    
    func test_selectionTracksDocument_notPosition() async {
        let vm = SearchViewModel(backend: MockBackend())
        let gen = vm.queryGeneration
        
        await vm.applyResult(.mock(rank: 0.8, id: "A"), fromGeneration: gen)
        await vm.applyResult(.mock(rank: 0.7, id: "B"), fromGeneration: gen)
        
        vm.selectedIndex = 0 // selected "A" at position 0
        
        // New result inserted at rank 1.0 (above "A")
        await vm.applyResult(.mock(rank: 1.0, id: "C"), fromGeneration: gen)
        
        // "A" moved to position 1 — selection must follow
        XCTAssertEqual(vm.selectedIndex, 1)
        XCTAssertEqual(vm.displayResults[1].id, "A")
    }
}

// MARK: - Mock Extensions

extension SearchResult {
    static func mock(rank: Float, id: String = UUID().uuidString) -> SearchResult {
        SearchResult(
            id: id,
            filename: "file_\(id).txt",
            path: "/Users/test/\(id)",
            rank: rank,
            opacity: 1.0
        )
    }
}
