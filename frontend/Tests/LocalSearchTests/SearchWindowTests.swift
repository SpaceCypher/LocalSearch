import XCTest
import AppKit
@testable import LocalSearch

final class SearchWindowTests: XCTestCase {
    
    func test_window_isNSPanel() {
        let controller = SearchWindowController()
        XCTAssertTrue(controller.window is NSPanel)
    }
    
    func test_window_hasNonActivatingMask() {
        let controller = SearchWindowController()
        let mask = controller.window?.styleMask ?? []
        XCTAssertTrue(mask.contains(.nonactivatingPanel))
    }
    
    func test_window_floatingLevel() {
        let controller = SearchWindowController()
        XCTAssertEqual(controller.window?.level, .floating)
    }
    
    func test_window_appearsOnAllSpaces() {
        let controller = SearchWindowController()
        XCTAssertEqual(
            controller.window?.collectionBehavior.contains(.canJoinAllSpaces), true
        )
    }
    
    func test_window_centeredOnPrimaryDisplay() {
        let controller = SearchWindowController()
        controller.showWindow(nil)
        let frame = controller.window?.frame ?? .zero
        let screenFrame = NSScreen.main?.visibleFrame ?? .zero
        // Window should be horizontally centered
        let centerX = frame.midX
        let screenCenterX = screenFrame.midX
        XCTAssertEqual(centerX, screenCenterX, accuracy: 1.0)
    }
    
    @MainActor
    func test_escape_dismissesWindow() async {
        let controller = SearchWindowController()
        controller.showWindow(nil)
        XCTAssertTrue(controller.window?.isVisible ?? false)
        
        controller.handleEscapeKey()
        
        XCTAssertFalse(controller.window?.isVisible ?? true)
    }
}
