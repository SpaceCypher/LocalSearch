import XCTest
@testable import LocalSearch

@MainActor
final class MetadataPanelTests: XCTestCase {
    
    func test_panel_appearsOnRightArrow() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.displayResults = [.mock(rank: 1.0)]
        vm.selectedIndex = 0
        XCTAssertNil(vm.expandedResult)
        vm.handleArrowKey(.right)
        XCTAssertNotNil(vm.expandedResult)
    }
    
    func test_panel_windowExpandsTo900px() {
        let controller = SearchWindowController()
        let vm = controller.viewModel
        vm.expandedResult = .mock(rank: 1.0)
        // Window width should expand from 640 to 900
        XCTAssertEqual(controller.window?.frame.width, 900, accuracy: 1.0)
    }
    
    func test_panel_collapsesOnLeftArrow() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.expandedResult = .mock(rank: 1.0)
        vm.handleArrowKey(.left)
        XCTAssertNil(vm.expandedResult)
    }
    
    func test_thumbnail_showsIconWhileLoading() {
        let panel = MetadataPanelView(result: .mock(rank: 1.0))
        XCTAssertEqual(panel.thumbnailState, .loadingIcon)
    }
    
    func test_thumbnail_crossfadesWhenReady() async throws {
        let panel = MetadataPanelView(result: .mock(rank: 1.0))
        try await Task.sleep(nanoseconds: 250_000_000) // wait for QL thumbnail
        XCTAssertEqual(panel.thumbnailState, .thumbnail)
    }
    
    func test_quickActionBar_showsAllActions() {
        let panel = MetadataPanelView(result: .mock(rank: 1.0))
        XCTAssertTrue(panel.quickActions.contains(.open))
        XCTAssertTrue(panel.quickActions.contains(.revealInFinder))
        XCTAssertTrue(panel.quickActions.contains(.copyPath))
    }
}
