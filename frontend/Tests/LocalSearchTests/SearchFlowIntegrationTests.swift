import XCTest
@testable import LocalSearch

@MainActor
final class SearchFlowIntegrationTests: XCTestCase {
    func test_fullFlow_typeQuery_seeResults() async throws {
        let backend = ImmediateBackend(results: [
            .mock(rank: 1.0, id: "report", fileKind: .document),
            .mock(rank: 0.8, id: "notes", fileKind: .document),
        ])
        let vm = SearchViewModel(backend: backend)

        vm.onQueryChange("report")
        try await Task.sleep(nanoseconds: 220_000_000)

        XCTAssertEqual(vm.queryState, .complete)
        XCTAssertFalse(vm.displayResults.isEmpty)
        XCTAssertEqual(vm.displayResults.first?.id, "report")
    }

    func test_fullFlow_zeroResults_suggestionsAppear() async throws {
        let backend = FixtureBackend()
        backend.fixtureSuggestions = ["receipt", "recite"]
        let vm = SearchViewModel(backend: backend)

        vm.onQueryChange("receit")
        try await waitUntil(timeoutNanoseconds: 1_000_000_000) {
            vm.queryState == .complete
        }

        XCTAssertEqual(vm.queryState, .complete)
        XCTAssertEqual(vm.spellingSuggestions, ["receipt", "recite"])
    }

    func test_fullFlow_systemStateAndIndexProgress_streamIn() async throws {
        let backend = FixtureBackend()
        backend.fixtureSystemStates = [.nominal, .fuzzyPaused]
        backend.fixtureIndexProgress = [IndexProgress(phase: "Bootstrap", percent: 0.45, etaMinutes: 3)]

        let vm = SearchViewModel(backend: backend)
        try await Task.sleep(nanoseconds: 40_000_000)

        XCTAssertEqual(vm.systemState, .fuzzyPaused)
        XCTAssertEqual(vm.indexProgress?.phase, "Bootstrap")
        XCTAssertEqual(vm.indexProgress?.percent ?? 0.0, 0.45, accuracy: 0.0001)
    }

    private func waitUntil(
        timeoutNanoseconds: UInt64,
        checkEveryNanoseconds: UInt64 = 20_000_000,
        condition: @escaping () -> Bool
    ) async throws {
        let deadline = DispatchTime.now().uptimeNanoseconds + timeoutNanoseconds
        while DispatchTime.now().uptimeNanoseconds < deadline {
            if condition() {
                return
            }
            try await Task.sleep(nanoseconds: checkEveryNanoseconds)
        }
        XCTFail("Timed out waiting for condition")
    }
}
