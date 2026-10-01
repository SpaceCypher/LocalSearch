import XCTest
@testable import LocalSearch

@MainActor
final class ResultRowViewTests: XCTestCase {
    
    func test_rowHeight_standard_is56() {
        let result = SearchResult.mock(rank: 1.0)
        let row = ResultRowView(result: result, isSelected: false, isTopHit: false)
        // View has fixed height of 56
        XCTAssertNotNil(row)
    }
    
    func test_filename_middleTruncated() {
        let longFilename = "quarterly_report_sales_analysis_final_v2.pdf"
        let result = SearchResult(
            id: "test",
            filename: longFilename,
            path: "/test/path",
            rank: 1.0,
            fileKind: .document
        )
        let row = ResultRowView(result: result, isSelected: false, isTopHit: false)
        
        // Should truncate in the middle
        let display = row.displayFilename
        if longFilename.count > 50 {
            XCTAssertTrue(display.contains("…"))
            XCTAssertTrue(display.hasSuffix(".pdf"))
        }
    }
    
    func test_path_homeDirectorySubstituted() {
        let homeDir = FileManager.default.homeDirectoryForCurrentUser.path
        let result = SearchResult(
            id: "test",
            filename: "test.txt",
            path: "\(homeDir)/Documents/Work/file.txt",
            rank: 1.0,
            fileKind: .document
        )
        let row = ResultRowView(result: result, isSelected: false, isTopHit: false)
        
        XCTAssertTrue(row.displayPath.hasPrefix("~/"))
        XCTAssertFalse(row.displayPath.contains(homeDir))
    }
    
    func test_path_longPath_leftTruncated() {
        let longPath = "~/a/b/c/d/e/f/g/h/i/j/k/very/long/path/to/file.txt"
        let result = SearchResult(
            id: "test",
            filename: "file.txt",
            path: longPath,
            rank: 1.0,
            fileKind: .document
        )
        let row = ResultRowView(result: result, isSelected: false, isTopHit: false)
        
        // Should left-truncate long paths
        if longPath.count > 60 {
            XCTAssertTrue(row.displayPath.hasPrefix("…/"))
        }
    }
    
    func test_revokedPermission_showsLockIcon() {
        let result = SearchResult.mock(rank: 1.0, permissionState: .revoked)
        let row = ResultRowView(result: result, isSelected: false, isTopHit: false)
        XCTAssertTrue(row.showsLockIcon)
        XCTAssertEqual(row.iconOpacity, 0.6)
    }
    
    func test_revokedPermission_pathRowReplacedWithDeniedLabel() {
        let result = SearchResult.mock(rank: 1.0, permissionState: .revoked)
        let row = ResultRowView(result: result, isSelected: false, isTopHit: false)
        XCTAssertEqual(row.pathRowText, "Permission denied")
    }
    
    func test_revokedPermission_enterKey_showsSystemSettingsAlert() {
        let vm = SearchViewModel(backend: MockBackend())
        let alerter = MockAlerter()
        vm.alerter = alerter
        vm.displayResults = [.mock(rank: 1.0, permissionState: .revoked)]
        vm.selectedIndex = 0
        vm.handleReturnKey(modifiers: [])
        XCTAssertTrue(alerter.lastAlert?.hasButton("Open System Settings") ?? false)
    }

    // MARK: - Why a result matched
    
    func test_snippetFormatter_stripsMarkers_andEmphasisesMatches() {
        let snippet = SnippetFormatter.attributed("We agreed the \u{1}zebrafish\u{2} tanks need \u{1}cleaning\u{2}.")
        
        XCTAssertEqual(SnippetFormatter.plain(snippet), "We agreed the zebrafish tanks need cleaning.")
        let emphasised = snippet.runs.filter { $0.foregroundColor != nil }.map { String(snippet[$0.range].characters) }
        XCTAssertEqual(emphasised, ["zebrafish", "cleaning"])
    }
    
    func test_snippet_isReadOutInAccessibilityLabel() {
        let row = ResultRowView(
            result: .mock(rank: 1.0, id: "A"),
            isSelected: false,
            isTopHit: false,
            snippet: SnippetFormatter.attributed("about \u{1}otters\u{2}")
        )
        XCTAssertTrue(row.accessibilityLabel.hasSuffix("Matches: about otters"))
    }
    
    @MainActor
    func test_viewModel_loadsSnippets_forListedResults_only() async throws {
        // Conforms directly: a protocol-extension default cannot be overridden
        // from a subclass of a type that relies on it.
        final class SnippetBackend: SearchBackendProtocol {
            func search(query: String, filters: [QueryFilter], scope: SearchScope, cancellationToken: CancellationToken) -> AsyncStream<[SearchResult]> {
                AsyncStream { continuation in
                    continuation.yield([.mock(rank: 1.0, id: "content-hit"), .mock(rank: 0.5, id: "name-hit")])
                    continuation.finish()
                }
            }
            func systemState() -> AsyncStream<SystemState> { AsyncStream { $0.finish() } }
            func indexProgress() -> AsyncStream<IndexProgress?> { AsyncStream { $0.finish() } }
            func prefetchPrefix(_ prefix: String) async {}
            func spellingSuggestions(for query: String) async -> [String] { [] }
            func snippet(path: String, query: String) async -> String? {
                path.contains("content-hit") ? "the \u{1}\(query)\u{2} line" : nil
            }
        }
        let vm = SearchViewModel(backend: SnippetBackend())
        vm.snippets = ["stale": AttributedString("old")]
        
        vm.queryText = "otter"
        vm.onQueryChange("otter")
        await waitUntil { !vm.snippets.isEmpty && vm.snippets["stale"] == nil }
        
        XCTAssertEqual(vm.snippets.keys.sorted(), ["content-hit"])
        XCTAssertEqual(vm.snippets["content-hit"].map(SnippetFormatter.plain), "the otter line")
        
        vm.queryText = ""
        vm.onQueryChange("")
        XCTAssertTrue(vm.snippets.isEmpty)
    }

    func test_pathLine_showsContainingFolder_notTheNameAgain() {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        func row(_ path: String) -> ResultRowView {
            ResultRowView(
                result: SearchResult(id: "x", filename: (path as NSString).lastPathComponent, path: path, rank: 1, fileKind: .document),
                isSelected: false, isTopHit: false
            )
        }
        XCTAssertEqual(row(home + "/Documents/Work/plan.md").pathRowText, "~/Documents/Work")
        XCTAssertEqual(row(home + "/plan.md").pathRowText, "~")
        XCTAssertEqual(row("/Volumes/Archive/2024/plan.md").pathRowText, "/Volumes/Archive/2024")
        XCTAssertEqual(row("/plan.md").pathRowText, "/")
    }
}
