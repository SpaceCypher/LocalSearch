import XCTest
@testable import LocalSearch

@MainActor
final class ZeroResultsViewTests: XCTestCase {
    
    func test_zeroResults_showsQuery() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.queryText = "receit"
        vm.queryState = .complete
        vm.displayResults = []
        XCTAssertEqual(vm.zeroResultsQuery, "receit")
    }
    
    func test_zeroResults_showsSpellingSuggestions() async {
        let backend = MockBackend()
        backend.mockSuggestions = ["receipt", "recite"]
        let vm = SearchViewModel(backend: backend)
        vm.onQueryChange("receit")
        try? await Task.sleep(nanoseconds: 200_000_000)
        XCTAssertEqual(vm.spellingSuggestions, ["receipt", "recite"])
    }
    
    func test_clickSuggestion_replacesQuery() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.selectSuggestion("receipt")
        XCTAssertEqual(vm.queryText, "receipt")
    }
    
    func test_activeFilters_showsBroadeningTip() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.parsedFilters = [.kind("pdf")]
        vm.queryState = .complete
        vm.displayResults = []
        XCTAssertTrue(vm.showBroadeningTip)
    }
    
    func test_degradedZeroResults_showsFuzzyPausedNote() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.systemState = .fuzzyPaused
        vm.queryState = .complete
        vm.displayResults = []
        XCTAssertTrue(vm.zeroResultsNote.contains("Exact mode active"))
    }
}
