import XCTest
@testable import LocalSearch

@MainActor
final class StatusBarViewTests: XCTestCase {
    
    func test_idleState_showsPrompt() {
        let vm = SearchViewModel(backend: FixtureBackend())
        XCTAssertEqual(vm.statusText, "Start typing to search")
    }
    
    func test_searching_showsSearchingDots() {
        let vm = SearchViewModel(backend: FixtureBackend())
        vm.queryState = .searching
        XCTAssertEqual(vm.statusText, "Searching…")
    }
    
    func test_complete_showsResultCount() {
        let vm = SearchViewModel(backend: FixtureBackend())
        vm.queryState = .complete
        vm.displayResults = Array(repeating: .mock(rank: 1.0), count: 47)
        XCTAssertEqual(vm.statusText, "47 results")
    }
    
    func test_zeroResults_showsQuery() {
        let vm = SearchViewModel(backend: FixtureBackend())
        vm.queryState = .complete
        vm.queryText = "receit"
        vm.displayResults = []
        XCTAssertEqual(vm.statusText, "No results for \"receit\"")
    }
}
