import XCTest
@testable import LocalSearch

@MainActor
class QueryFieldViewTests: XCTestCase {
    
    func test_queryField_showsSpinner_after80ms() async throws {
        let vm = SearchViewModel(backend: SlowBackend(delay: .milliseconds(500)))
        vm.onQueryChange("hello")
        
        // Spinner not shown before debounce
        XCTAssertFalse(vm.showSpinner)
        
        try await Task.sleep(nanoseconds: 80_000_000)
        XCTAssertTrue(vm.showSpinner)
    }
    
    func test_queryField_hidesSpinner_whenResultsArrive() async throws {
        let backend = ImmediateBackend(results: [.mock(rank: 1.0)])
        let vm = SearchViewModel(backend: backend)
        vm.onQueryChange("doc")
        
        try await Task.sleep(nanoseconds: 200_000_000)
        XCTAssertFalse(vm.showSpinner)
        XCTAssertFalse(vm.displayResults.isEmpty)
    }
    
    func test_clearButton_appears_whenQueryNonEmpty() {
        let vm = SearchViewModel(backend: MockBackend())
        XCTAssertFalse(vm.showClearButton)
        vm.queryText = "hello"
        XCTAssertTrue(vm.showClearButton)
    }
    
    func test_clearButton_tap_resetsToIdle() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.queryText = "hello"
        vm.displayResults = [.mock(rank: 1.0)]
        vm.clearQuery()
        XCTAssertEqual(vm.queryText, "")
        XCTAssertEqual(vm.queryState, .idle)
        XCTAssertTrue(vm.displayResults.isEmpty)
    }
    
    func test_filterChip_renderedFromParsedQuery() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.onQueryChange("kind:pdf report")
        // Parser extracts kind:pdf as a filter chip
        XCTAssertEqual(vm.parsedFilters.count, 1)
        XCTAssertEqual(vm.parsedFilters.first, .kind("pdf"))
        // Remaining query text strips the filter token
        XCTAssertEqual(vm.strippedQueryText, "report")
    }
}
