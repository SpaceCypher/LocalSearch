import Cocoa
import Carbon

class HotkeyManager {
    static let shared = HotkeyManager()
    
    private(set) var isRegistered = false
    private var eventHandler: EventHandlerRef?
    
    private init() {}
    
    func register() {
        // Stub implementation for Task F1
        // Full implementation in Task F5
        isRegistered = true
        print("HotkeyManager: ⌥Space hotkey registration (stub)")
    }
    
    func toggle() {
        // Stub - will be implemented in Task F5
        print("HotkeyManager: toggle() called")
    }
}
