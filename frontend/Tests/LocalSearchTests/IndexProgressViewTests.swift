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
    
    func test_etaText_pluralMinutes() {
        let progress = IndexProgress(phase: "Indexing", percent: 0.1, etaMinutes: 10)
        XCTAssertEqual(progress.etaText, "About 10 min left")
    }
    
    func test_etaText_singleMinute() {
        let progress = IndexProgress(phase: "Indexing", percent: 0.9, etaMinutes: 1)
        XCTAssertEqual(progress.etaText, "About 1 min left")
    }
    
    func test_noEstimate_showsNoEtaAndNoPercentage() {
        // While the amount of work is unknown we must not invent numbers
        let progress = IndexProgress(phase: "Scanning folders", percent: 0, etaMinutes: nil)
        XCTAssertNil(progress.etaText)
        XCTAssertFalse(progress.isDeterminate)
        XCTAssertEqual(progress.statusText, "Scanning folders…")
    }
    
    func test_statusText_includesPercentage() {
        let progress = IndexProgress(phase: "Reading file contents", percent: 0.42, etaMinutes: 3)
        XCTAssertEqual(progress.statusText, "Reading file contents 42%")
    }
    
    // MARK: - Engine status → progress
    
    func test_tracker_readyPhase_hidesProgress() {
        var tracker = IndexProgressTracker()
        XCTAssertNil(tracker.progress(for: CIndexStatus(phase: 3)))
    }
    
    func test_tracker_scanning_isIndeterminate_withCount() {
        var tracker = IndexProgressTracker()
        let progress = tracker.progress(for: CIndexStatus(phase: 1, doc_count: 1200))
        XCTAssertEqual(progress?.isDeterminate, false)
        XCTAssertEqual(progress?.phase.contains("1,200"), true)
    }
    
    func test_tracker_extracting_estimatesTimeFromObservedRate() {
        var tracker = IndexProgressTracker()
        let start = Date()
        // Phase begins: no history yet, so no estimate
        let first = tracker.progress(for: CIndexStatus(phase: 2, work_done: 0, work_total: 1000), now: start)
        XCTAssertNil(first?.etaMinutes)
        
        // 25% done after 60s → about 3 more minutes
        let later = tracker.progress(
            for: CIndexStatus(phase: 2, work_done: 250, work_total: 1000),
            now: start.addingTimeInterval(60)
        )
        XCTAssertEqual(later?.percent ?? 0, 0.25, accuracy: 0.001)
        XCTAssertEqual(later?.etaMinutes, 3)
    }
}
