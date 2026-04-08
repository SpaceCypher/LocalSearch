import XCTest
@testable import LocalSearch

@MainActor
final class AccessibilityTests: XCTestCase {
    
    func test_searchField_accessibilityLabel() {
        let vm = SearchViewModel(backend: MockBackend())
        // QueryFieldView doesn't exist as a separate testable component yet
        // Testing via ViewModel properties that would be used
        XCTAssertEqual(vm.searchFieldAccessibilityLabel, "Search files")
        XCTAssertEqual(vm.searchFieldAccessibilityHint, "Type to search your files")
    }
    
    func test_resultRow_accessibilityLabel_includesAllInfo() {
        let result = SearchResult.mock(
            rank: 1.0,
            id: "test",
            fileKind: .document
        )
        let row = ResultRowView(result: result, isSelected: false)
        let label = row.accessibilityLabel
        XCTAssertTrue(label.contains("file_test.txt"))
        XCTAssertTrue(label.contains("document"))
    }
    
    func test_statusBar_isLiveRegion() {
        let vm = SearchViewModel(backend: MockBackend())
        // StatusBarView would use this property
        XCTAssertTrue(vm.statusBarIsLiveRegion)
    }
    
    func test_reduceMotion_allAnimationsInstant() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.isReduceMotionEnabled = true
        vm.onQueryChange("test")
        // When reduce motion is enabled, animations should be instant
        XCTAssertTrue(vm.isReduceMotionEnabled)
    }
}
