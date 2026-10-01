import XCTest
import SwiftUI
import AppKit
@testable import LocalSearch

/// Renders the app's main states to PNG files so the UI can be reviewed
/// without launching the app:
///
///     LOCALSEARCH_UI_SNAPSHOTS=/tmp/localsearch-ui swift test --filter UISnapshotTests
///
/// Views are hosted in an offscreen window and drawn with AppKit's own
/// display caching, so AppKit-backed controls (text fields, scroll views,
/// progress bars) render as they do in the app. Skipped when the variable
/// is not set.
@MainActor
final class UISnapshotTests: XCTestCase {
    
    private var outputDirectory: URL!
    
    override func setUp() async throws {
        guard let path = ProcessInfo.processInfo.environment["LOCALSEARCH_UI_SNAPSHOTS"], !path.isEmpty else {
            throw XCTSkip("Set LOCALSEARCH_UI_SNAPSHOTS to a directory to render UI snapshots")
        }
        outputDirectory = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: outputDirectory, withIntermediateDirectories: true)
    }
    
    private func render<V: View>(_ view: V, size: CGSize, name: String) throws {
        // The app draws on a dark vibrant material; stand in for it with a dark fill
        let root = view
            .frame(width: size.width, height: size.height)
            .background(Color(white: 0.09))
            .environment(\.colorScheme, .dark)
        let hosting = NSHostingView(rootView: root)
        hosting.frame = CGRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: hosting.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = hosting
        hosting.layoutSubtreeIfNeeded()
        // Let .task / onAppear work and async layout settle
        RunLoop.main.run(until: Date().addingTimeInterval(0.4))
        hosting.layoutSubtreeIfNeeded()
        
        let rep = try XCTUnwrap(hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds))
        hosting.cacheDisplay(in: hosting.bounds, to: rep)
        let png = try XCTUnwrap(rep.representation(using: .png, properties: [:]))
        try png.write(to: outputDirectory.appendingPathComponent("\(name).png"))
    }
    
    /// Real files, so icons, kinds, sizes and dates are real too
    private func makeResults() throws -> (results: [SearchResult], folder: URL) {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("ls-snap-\(UUID().uuidString)")
        let names = [
            "Quarterly report 2025.pdf",
            "report-final-v2 (reviewed by finance and legal, do not edit without asking first).docx",
            "report_generator.py",
            "Reports",
            "expense-report-march.xlsx",
            "screenshot of report.png",
        ]
        try FileManager.default.createDirectory(at: folder.appendingPathComponent("Reports"), withIntermediateDirectories: true)
        var results: [SearchResult] = []
        for (index, name) in names.enumerated() {
            let url = folder.appendingPathComponent(name)
            if name != "Reports" {
                try Data(repeating: 65, count: 1200 * (index + 1)).write(to: url)
            }
            results.append(SearchResult(
                id: "r\(index)",
                filename: name,
                path: url.path,
                rank: Float(10 - index),
                fileKind: FileKind.detect(path: url.path)
            ))
        }
        return (results, folder)
    }
    
    private func panelSize(expanded: Bool, height: CGFloat) -> CGSize {
        CGSize(
            width: SearchContentView.mainWidth + (expanded ? MetadataPanelView.width + SearchContentView.dividerWidth : 0),
            height: height
        )
    }
    
    private func viewModel(query: String, results: [SearchResult], backend: SearchBackendProtocol = MockBackend()) -> SearchViewModel {
        let vm = SearchViewModel(backend: backend)
        vm.queryText = query
        vm.displayResults = results
        vm.queryState = query.isEmpty ? .idle : .complete
        vm.selectedIndex = results.isEmpty ? nil : 0
        return vm
    }
    
    func test_render_searchPanel_states() throws {
        let (results, folder) = try makeResults()
        defer { try? FileManager.default.removeItem(at: folder) }
        let settings = SettingsManager.shared
        let originalDensity = settings.resultDensity
        defer { settings.resultDensity = originalDensity }
        
        // Idle: just the field
        try render(SearchContentView(viewModel: viewModel(query: "", results: [])),
                   size: panelSize(expanded: false, height: 92), name: "01-idle")
        
        // Idle while indexing: field plus a status line
        let indexing = viewModel(
            query: "", results: [],
            backend: ProgressBackend(progress: IndexProgress(phase: "Reading file contents", percent: 0.42, etaMinutes: 3))
        )
        try render(SearchContentView(viewModel: indexing),
                   size: panelSize(expanded: false, height: 132), name: "02-idle-indexing")
        
        // Results, comfortable density, with matching lines for two of them
        settings.resultDensity = .comfortable
        let withResults = viewModel(query: "report", results: results)
        withResults.snippets = [
            "r0": SnippetFormatter.attributed("Revenue grew 12% in the quarter covered by this \u{1}report\u{2}, driven by…"),
            "r2": SnippetFormatter.attributed("def build_\u{1}report\u{2}(rows): # returns the rendered \u{1}report\u{2} as a string"),
        ]
        try render(SearchContentView(viewModel: withResults),
                   size: panelSize(expanded: false, height: 116 + 72 + 5 * 52 + 40), name: "03-results-comfortable")
        
        // Same, compact
        settings.resultDensity = .compact
        try render(SearchContentView(viewModel: withResults),
                   size: panelSize(expanded: false, height: 96 + 56 + 5 * 44 + 40), name: "04-results-compact")
        settings.resultDensity = .comfortable
        
        // Details open, with a short list (the case that used to clip)
        let expanded = viewModel(query: "report", results: Array(results.prefix(2)))
        expanded.expandedResult = results[0]
        try render(SearchContentView(viewModel: expanded),
                   size: panelSize(expanded: true, height: MetadataPanelView.minHeight), name: "05-details")
        
        // No results
        try render(SearchContentView(viewModel: viewModel(query: "zzyzx", results: [])),
                   size: panelSize(expanded: false, height: 290), name: "06-no-results")
        
        // No results because of filters
        let filtered = viewModel(query: "report kind:mp4", results: [])
        filtered.parsedFilters = [.kind("mp4")]
        try render(SearchContentView(viewModel: filtered),
                   size: panelSize(expanded: false, height: 290), name: "07-no-results-filtered")
        
        // Engine failed to load
        let broken = viewModel(
            query: "report", results: [],
            backend: MockBackend(unavailableReason: "The search engine library (liblocalsearch.dylib) could not be loaded. Reinstall LocalSearch, or in a development checkout run cargo build.")
        )
        try render(SearchContentView(viewModel: broken),
                   size: panelSize(expanded: false, height: 290), name: "08-engine-unavailable")
    }
    
    func test_render_loading_and_welcome() throws {
        // Slow first search with nothing to show yet: skeleton rows
        let loading = viewModel(query: "report", results: [])
        loading.queryState = .searchingSlow
        loading.showSkeletons = true
        try render(SearchContentView(viewModel: loading),
                   size: panelSize(expanded: false, height: 116 + 3 * 56 + 40), name: "09-skeletons")
        
        try render(WelcomeView(onContinue: {}), size: CGSize(width: 680, height: 500), name: "20-welcome")
    }
    
    func test_render_settings_tabs() throws {
        let size = CGSize(width: 480, height: 600)
        // Settings writes through to SettingsManager; keep the user's real settings intact
        let ready = ProgressBackend(progress: nil, summary: "Up to date · 59,639 items")
        
        for tab in SettingsTab.allCases {
            try render(SettingsView(initialTab: tab, backend: ready), size: size, name: "10-settings-\(tab.rawValue.lowercased())")
        }
        
        // Tall capture of the Indexing tab so everything below the fold is reviewable
        try render(SettingsView(initialTab: .indexing, backend: ready),
                   size: CGSize(width: 480, height: 1000), name: "11-settings-indexing-full")
        
        try render(
            SettingsView(initialTab: .indexing, backend: ProgressBackend(
                progress: IndexProgress(phase: "Reading file contents", percent: 0.42, etaMinutes: 3)
            )),
            size: size, name: "12-settings-indexing-in-progress"
        )
        try render(
            SettingsView(initialTab: .indexing, backend: MockBackend(unavailableReason: "The search engine library (liblocalsearch.dylib) could not be loaded. Reinstall LocalSearch, or in a development checkout run cargo build.")),
            size: size, name: "13-settings-engine-unavailable"
        )
    }
}

/// A backend that reports a fixed index state
private final class ProgressBackend: SearchBackendProtocol {
    let progress: IndexProgress?
    let summary: String?
    
    init(progress: IndexProgress?, summary: String? = nil) {
        self.progress = progress
        self.summary = summary
    }
    
    func search(query: String, filters: [QueryFilter], scope: SearchScope, cancellationToken: CancellationToken) -> AsyncStream<[SearchResult]> {
        AsyncStream { $0.finish() }
    }
    func systemState() -> AsyncStream<SystemState> { AsyncStream { $0.finish() } }
    func indexProgress() -> AsyncStream<IndexProgress?> {
        let progress = self.progress
        return AsyncStream { continuation in
            continuation.yield(progress)
            continuation.finish()
        }
    }
    func prefetchPrefix(_ prefix: String) async {}
    func spellingSuggestions(for query: String) async -> [String] { [] }
    func indexSummary() -> String? { summary }
}
