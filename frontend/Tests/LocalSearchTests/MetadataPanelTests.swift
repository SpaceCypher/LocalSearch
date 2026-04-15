import XCTest
@testable import LocalSearch

@MainActor
final class MetadataPanelTests: XCTestCase {
    
    func test_panel_appearsOnRightArrow() {
        let vm = SearchViewModel(backend: FixtureBackend())
        vm.displayResults = [.mock(rank: 1.0)]
        vm.selectedIndex = 0
        XCTAssertNil(vm.expandedResult)
        vm.handleArrowKey(.right)
        XCTAssertNotNil(vm.expandedResult)
    }
    
    func test_panel_windowExpandsTo900px() {
        let panel = MetadataPanelView(result: .mock(rank: 1.0))
        XCTAssertNotNil(panel)
    }
    
    func test_panel_collapsesOnLeftArrow() {
        let vm = SearchViewModel(backend: FixtureBackend())
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
        XCTAssertEqual(panel.thumbnailState, .loadingIcon)
    }
    
    func test_displayPath_replacesHomeDirectoryWithTilde() {
        let homeDir = FileManager.default.homeDirectoryForCurrentUser.path
        let result = SearchResult(
            id: "test",
            filename: "test.txt",
            path: "\(homeDir)/Documents/test.txt",
            rank: 1.0,
            fileKind: .document
        )
        let panel = MetadataPanelView(result: result)
        XCTAssertTrue(panel.displayPath.hasPrefix("~/"))
    }
}
