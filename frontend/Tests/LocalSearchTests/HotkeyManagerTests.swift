import XCTest
@testable import LocalSearch

final class HotkeyManagerTests: XCTestCase {
    
    func test_hotkeyManager_isSingleton() {
        let a = HotkeyManager.shared
        let b = HotkeyManager.shared
        XCTAssertTrue(a === b)
    }
    
    func test_register_doesNotThrow() {
        // Should not crash on registration — real CGEventTap requires accessibility permission
        // Test in sandbox: verify it sets the registered flag
        XCTAssertNoThrow(HotkeyManager.shared.register())
        XCTAssertTrue(HotkeyManager.shared.isRegistered)
    }
    
    func test_toggle_callsShowOrHide() {
        let controller = MockWindowController()
        let manager = HotkeyManager(windowController: controller)
        
        // First toggle should show
        manager.toggle()
        XCTAssertTrue(controller.showCalled)
        XCTAssertFalse(controller.hideCalled)
        
        // Second toggle should hide
        controller.showCalled = false
        manager.toggle()
        XCTAssertFalse(controller.showCalled)
        XCTAssertTrue(controller.hideCalled)
    }
}

// Mock window controller for testing
@objc class MockWindowController: NSObject, WindowControllerProtocol {
    var showCalled = false
    var hideCalled = false
    
    func showWindow(_ sender: Any?) {
        showCalled = true
    }
    
    @objc func hideWindow() {
        hideCalled = true
    }
}
