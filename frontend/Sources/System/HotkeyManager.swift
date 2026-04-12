import Cocoa
import Carbon

// Protocol for window controller operations
@objc protocol WindowControllerProtocol: AnyObject {
    func showWindow(_ sender: Any?)
    @objc optional func hideWindow()
}

class HotkeyManager {
    static let shared = HotkeyManager()
    
    private(set) var isRegistered = false
    private var eventHandler: EventHandlerRef?
    private weak var windowController: WindowControllerProtocol?
    private var isWindowVisible = false
    
    private init() {}
    
    // Internal initializer for testing with dependency injection
    internal init(windowController: WindowControllerProtocol) {
        self.windowController = windowController
    }
    
    // Public setter for wiring from AppDelegate (F5)
    func setWindowController(_ controller: WindowControllerProtocol) {
        self.windowController = controller
    }
    
    func register() {
        guard !isRegistered else { return }
        
        // Try CGEventTap first (requires accessibility permission)
        if registerWithCGEventTap() {
            isRegistered = true
            print("HotkeyManager: ⌥Space registered via CGEventTap")
            return
        }
        
        // Fallback to Carbon RegisterEventHotKey (works in sandbox)
        if registerWithCarbon() {
            isRegistered = true
            print("HotkeyManager: ⌥Space registered via Carbon")
            return
        }
        
        print("HotkeyManager: Failed to register hotkey")
    }
    
    func toggle() {
        print("HotkeyManager: toggle() triggered (isVisible: \(isWindowVisible))")
        if isWindowVisible {
            hide()
        } else {
            show()
        }
    }
    
    func show() {
        print("HotkeyManager: show() triggered")
        guard let controller = windowController else { 
            print("HotkeyManager: show() failed - windowController is nil")
            return 
        }
        controller.showWindow(nil)
        isWindowVisible = true
    }
    
    func hide() {
        guard let controller = windowController else { return }
        if let hideMethod = controller.hideWindow {
            hideMethod()
        } else if let nsController = controller as? NSWindowController {
            nsController.window?.orderOut(nil)
        }
        isWindowVisible = false
    }
    
    private func registerWithCGEventTap() -> Bool {
        // CGEventTap requires accessibility permission
        // This is a simplified implementation that sets up the tap
        // In production, you'd need to check AXIsProcessTrusted() first
        
        let eventMask = (1 << CGEventType.keyDown.rawValue)
        
        guard let eventTap = CGEvent.tapCreate(
            tap: .cgSessionEventTap,
            place: .headInsertEventTap,
            options: .defaultTap,
            eventsOfInterest: CGEventMask(eventMask),
            callback: { (proxy, type, event, refcon) -> Unmanaged<CGEvent>? in
                // Check for Option+Space (keyCode 49 = Space, Option flag)
                let keyCode = event.getIntegerValueField(.keyboardEventKeycode)
                let flags = event.flags
                
                if keyCode == 49 && flags.contains(.maskAlternate) {
                    // Trigger toggle on main thread
                    DispatchQueue.main.async {
                        HotkeyManager.shared.toggle()
                    }
                    // Consume the event
                    return nil
                }
                
                return Unmanaged.passRetained(event)
            },
            userInfo: nil
        ) else {
            return false
        }
        
        let runLoopSource = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, eventTap, 0)
        CFRunLoopAddSource(CFRunLoopGetCurrent(), runLoopSource, .commonModes)
        CGEvent.tapEnable(tap: eventTap, enable: true)
        
        return true
    }
    
    private func registerWithCarbon() -> Bool {
        // Carbon hotkey registration (works in sandbox)
        var hotKeyRef: EventHotKeyRef?
        var gMyHotKeyID = EventHotKeyID()
        gMyHotKeyID.signature = OSType(0x4C534348) // 'LSCH'
        gMyHotKeyID.id = 1
        
        let status = RegisterEventHotKey(
            UInt32(49), // Space key
            UInt32(optionKey), // Option modifier
            gMyHotKeyID,
            GetApplicationEventTarget(),
            0,
            &hotKeyRef
        )
        
        if status == noErr {
            // Install event handler
            var eventSpec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
            
            InstallEventHandler(
                GetApplicationEventTarget(),
                { (nextHandler, theEvent, userData) -> OSStatus in
                    DispatchQueue.main.async {
                        HotkeyManager.shared.toggle()
                    }
                    return noErr
                },
                1,
                &eventSpec,
                nil,
                &self.eventHandler
            )
            
            return true
        }
        
        return false
    }
}
