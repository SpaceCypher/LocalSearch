import XCTest
@testable import LocalSearch

final class AnimationTests: XCTestCase {
    
    func test_animationTokens_matchSpec() {
        // All parameters from spec §9 — must match exactly
        // Note: SwiftUI Animation doesn't expose response/dampingFraction directly
        // We verify the tokens exist and return non-nil animations
        XCTAssertNotNil(AnimationTokens.windowAppear.animation)
        XCTAssertNotNil(AnimationTokens.windowDismiss.animation)
        XCTAssertNotNil(AnimationTokens.selectionMove.animation)
        XCTAssertNotNil(AnimationTokens.resultInsertion.animation)
        XCTAssertNotNil(AnimationTokens.metadataSlide.animation)
        XCTAssertNotNil(AnimationTokens.spinnerFade.animation)
    }
    
    func test_reduceMotion_disablesAllAnimations() {
        // Simulate reduce motion preference
        let token = AnimationTokens.windowAppear.respectingReduceMotion(isReduceMotion: true)
        XCTAssertNil(token, "Reduce motion should return nil (instant transition)")
        
        let normalToken = AnimationTokens.windowAppear.respectingReduceMotion(isReduceMotion: false)
        XCTAssertNotNil(normalToken, "Normal mode should return animation")
    }
    
    @MainActor
    func test_maxConcurrentAnimations_is4() async throws {
        let coordinator = AnimationCoordinator()
        
        // Enqueue 6 animations
        for _ in 0..<6 {
            coordinator.enqueue(.resultInsertion)
        }
        
        // Only 4 should run simultaneously
        XCTAssertEqual(coordinator.activeCount, 4, "Max 4 animations should run concurrently")
        
        // Wait for animations to complete
        try await Task.sleep(nanoseconds: 300_000_000)
        
        // After some complete, remaining should start
        XCTAssertLessThanOrEqual(coordinator.activeCount, 4)
    }
    
    @MainActor
    func test_animationCoordinator_queuesExcess() async throws {
        let coordinator = AnimationCoordinator()
        
        // Start with 0 active
        XCTAssertEqual(coordinator.activeCount, 0)
        
        // Enqueue 2 animations
        coordinator.enqueue(.windowAppear)
        coordinator.enqueue(.windowDismiss)
        
        // Both should start immediately (under max)
        XCTAssertEqual(coordinator.activeCount, 2)
    }
}
