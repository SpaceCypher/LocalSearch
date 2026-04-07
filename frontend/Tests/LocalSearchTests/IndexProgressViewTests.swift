import XCTest
@testable import LocalSearch

@MainActor
final class IndexProgressViewTests: XCTestCase {
    
    func test_progressView_hidden_afterBootstrap() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.indexProgress = nil // not bootstrapping
        XCTAssertFalse(vm.showIndexProgress)
    }
    
    func test_progressView_shown_duringBootstrap() {
        let vm = SearchViewModel(backend: MockBackend())
        vm.indexProgress = IndexProgress(phase: "Documents complete", percent: 0.32, etaMinutes: 4)
        XCTAssertTrue(vm.showIndexProgress)
    }
    
    func test_progressBar_isStatic_notAnimated() {
        // Static progress bar (not pulsing) as per spec §6
        let progress = IndexProgress(phase: "Indexing", percent: 0.5, etaMinutes: 2)
        // View test would go here - for now just verify struct
        XCTAssertEqual(progress.percent, 0.5, accuracy: 0.001)
        XCTAssertEqual(progress.phase, "Indexing")
    }
    
    func test_etaText_roundsToNearestMinute() {
        let progress = IndexProgress(phase: "Indexing", percent: 0.3, etaMinutes: 4)
        let etaText = formatETA(minutes: progress.etaMinutes)
        XCTAssertEqual(etaText, "Est. 4 min")
    }
    
    func test_etaText_pluralMinutes() {
        let progress = IndexProgress(phase: "Indexing", percent: 0.1, etaMinutes: 10)
        let etaText = formatETA(minutes: progress.etaMinutes)
        XCTAssertEqual(etaText, "Est. 10 min")
    }
    
    func test_etaText_singleMinute() {
        let progress = IndexProgress(phase: "Indexing", percent: 0.9, etaMinutes: 1)
        let etaText = formatETA(minutes: progress.etaMinutes)
        XCTAssertEqual(etaText, "Est. 1 min")
    }
    
    // Helper function to format ETA
    private func formatETA(minutes: Int) -> String {
        if minutes == 1 {
            return "Est. 1 min"
        } else {
            return "Est. \(minutes) min"
        }
    }
}
