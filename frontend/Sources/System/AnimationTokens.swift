import SwiftUI
import AppKit

/// Animation timing tokens from spec §9
/// All spring parameters calibrated for 60fps macOS
public enum AnimationTokens {
    case windowAppear
    case windowDismiss
    case selectionMove
    case resultInsertion
    case metadataSlide
    case spinnerFade
    
    /// Spring animation parameters
    public var animation: Animation {
        switch self {
        case .windowAppear:
            return .spring(response: 0.3, dampingFraction: 0.85)
        case .windowDismiss:
            return .spring(response: 0.2, dampingFraction: 1.0)
        case .selectionMove:
            return .spring(response: 0.15, dampingFraction: 1.0)
        case .resultInsertion:
            return .spring(response: 0.25, dampingFraction: 0.9)
        case .metadataSlide:
            return .spring(response: 0.18, dampingFraction: 0.88)
        case .spinnerFade:
            return .spring(response: 0.12, dampingFraction: 1.0)
        }
    }
    
    /// Returns animation respecting reduce motion preference
    /// Returns nil if reduce motion is enabled (instant transition)
    public func respectingReduceMotion(isReduceMotion: Bool) -> Animation? {
        return isReduceMotion ? nil : animation
    }
    
    /// Check system reduce motion preference
    public static var isReduceMotionEnabled: Bool {
        return NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
    }
}

/// Coordinates concurrent animations to prevent performance issues
/// Max 4 animations can run simultaneously
@MainActor
public class AnimationCoordinator: ObservableObject {
    @Published public private(set) var activeCount: Int = 0
    private var queue: [AnimationTokens] = []
    private let maxConcurrent = 4
    
    public init() {}
    
    /// Enqueue an animation
    /// If under max concurrent, starts immediately
    /// Otherwise queues for later execution
    public func enqueue(_ token: AnimationTokens) {
        if activeCount < maxConcurrent {
            start(token)
        } else {
            queue.append(token)
        }
    }
    
    private func start(_ token: AnimationTokens) {
        activeCount += 1
        // Animation completes after its duration
        Task {
            try? await Task.sleep(nanoseconds: UInt64(token.duration * 1_000_000_000))
            complete()
        }
    }
    
    private func complete() {
        activeCount -= 1
        if !queue.isEmpty {
            let next = queue.removeFirst()
            start(next)
        }
    }
}

extension AnimationTokens {
    /// Approximate duration in seconds for scheduling
    var duration: Double {
        switch self {
        case .windowAppear: return 0.3
        case .windowDismiss: return 0.2
        case .selectionMove: return 0.15
        case .resultInsertion: return 0.25
        case .metadataSlide: return 0.18
        case .spinnerFade: return 0.12
        }
    }
}
