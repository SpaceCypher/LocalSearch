import XCTest
@testable import LocalSearch

@MainActor
final class KeyboardNavigationTests: XCTestCase {
    
    func test_downArrow_movesSelection() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.displayResults = [.mock(rank: 1.0), .mock(rank: 0.9), .mock(rank: 0.8)]
        vm.selectedIndex = 0
        
        vm.handleArrowKey(.down)
        
        XCTAssertEqual(vm.selectedIndex, 1)
    }
    
    func test_upArrow_clampsAtZero() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.displayResults = [.mock(rank: 1.0)]
        vm.selectedIndex = 0
        
        vm.handleArrowKey(.up)
        
        XCTAssertEqual(vm.selectedIndex, 0)
    }
    
    func test_rightArrow_expandsMetadata() {
        let vm = SearchViewModel(backend: MockBackend())
        let result = SearchResult.mock(rank: 1.0, id: "A")
        vm.displayResults = [result]
        vm.selectedIndex = 0
        
        XCTAssertNil(vm.expandedResult)
        
        vm.handleArrowKey(.right)
        
        XCTAssertEqual(vm.expandedResult?.id, "A")
    }
    
    func test_leftArrow_collapsesMetadata() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.displayResults = [.mock(rank: 1.0)]
        vm.selectedIndex = 0
        vm.expandedResult = vm.displayResults[0]
        
        vm.handleArrowKey(.left)
        
        XCTAssertNil(vm.expandedResult)
    }
    
    func test_upArrow_emptyQuery_navigatesHistory() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.queryHistory = ["previous query", "older query"]
        vm.queryText = ""
        
        vm.handleArrowKey(.up)
        XCTAssertEqual(vm.queryText, "previous query")
        
        vm.handleArrowKey(.up)
        XCTAssertEqual(vm.queryText, "older query")
    }
    
    func test_downArrow_selectsFirstResult_whenNoneSelected() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.displayResults = [.mock(rank: 1.0), .mock(rank: 0.9)]
        vm.selectedIndex = nil
        
        vm.handleArrowKey(.down)
        
        XCTAssertEqual(vm.selectedIndex, 0)
    }
}
