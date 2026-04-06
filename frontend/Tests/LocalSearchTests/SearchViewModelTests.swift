import XCTest
@testable import LocalSearch

@MainActor
final class SearchViewModelTests: XCTestCase {
    
    func test_initialState() {
        let vm = SearchViewModel()
        XCTAssertEqual(vm.queryText, "")
        XCTAssertTrue(vm.displayResults.isEmpty)
    }
}
