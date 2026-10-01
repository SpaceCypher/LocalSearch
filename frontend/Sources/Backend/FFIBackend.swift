import Foundation
import Darwin

/// Talks to the Rust engine in `liblocalsearch.dylib` (see `src/ffi.rs`).
final class FFIBackend: SearchBackendProtocol {
    private typealias QueryFn = @convention(c) (
        UnsafePointer<CChar>,
        UnsafeMutablePointer<UnsafeMutableRawPointer?>,
        UnsafeMutablePointer<Int>
    ) -> Int32
    private typealias FreeResultsFn = @convention(c) (UnsafeMutableRawPointer?, Int) -> Void
    private typealias StringFn = @convention(c) (UnsafePointer<CChar>) -> Int32
    private typealias StatusFn = @convention(c) (UnsafeMutableRawPointer) -> Int32
    private typealias VoidFn = @convention(c) () -> Void

    private let handle: UnsafeMutableRawPointer
    private let queryFn: QueryFn
    private let freeResultsFn: FreeResultsFn
    private let recordClickFn: StringFn
    private let configureFn: StringFn
    private let indexStatusFn: StatusFn
    private let shutdownFn: VoidFn

    private init(
        handle: UnsafeMutableRawPointer,
        queryFn: @escaping QueryFn,
        freeResultsFn: @escaping FreeResultsFn,
        recordClickFn: @escaping StringFn,
        configureFn: @escaping StringFn,
        indexStatusFn: @escaping StatusFn,
        shutdownFn: @escaping VoidFn
    ) {
        self.handle = handle
        self.queryFn = queryFn
        self.freeResultsFn = freeResultsFn
        self.recordClickFn = recordClickFn
        self.configureFn = configureFn
        self.indexStatusFn = indexStatusFn
        self.shutdownFn = shutdownFn
    }

    deinit {
        dlclose(handle)
    }

    private static let dylibName = "liblocalsearch.dylib"

    /// Where the engine library is looked for, in order: an explicit override,
    /// inside the app bundle (how a packaged app ships), then the cargo output
    /// directories of a development checkout.
    static func dylibCandidates() -> [String] {
        var candidates: [String] = []

        if let explicit = ProcessInfo.processInfo.environment["LOCALSEARCH_DYLIB_PATH"], !explicit.isEmpty {
            candidates.append(explicit)
        }

        if let frameworks = Bundle.main.privateFrameworksPath {
            candidates.append((frameworks as NSString).appendingPathComponent(dylibName))
        }

        // Resolve repo root from this source file path for local development.
        let repoRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // Backend
            .deletingLastPathComponent() // Sources
            .deletingLastPathComponent() // frontend
            .deletingLastPathComponent() // repo root
        for profile in ["release", "debug"] {
            candidates.append("./target/\(profile)/\(dylibName)")
            candidates.append("../target/\(profile)/\(dylibName)")
            candidates.append(repoRoot.appendingPathComponent("target/\(profile)/\(dylibName)").path)
        }

        // Deduplicate while preserving order
        var seen = Set<String>()
        return candidates.filter { seen.insert($0).inserted }
    }

    static func makeIfAvailable() -> FFIBackend? {
        var loadErrors: [String] = []

        for path in dylibCandidates() {
            guard let handle = dlopen(path, RTLD_NOW | RTLD_LOCAL) else {
                let message = dlerror().map { String(cString: $0) } ?? "unknown error"
                loadErrors.append("dlopen failed for \(path): \(message)")
                continue
            }

            guard
                let query = dlsym(handle, "localsearch_query"),
                let freeResults = dlsym(handle, "localsearch_free_results"),
                let recordClick = dlsym(handle, "localsearch_record_click"),
                let configure = dlsym(handle, "localsearch_configure"),
                let indexStatus = dlsym(handle, "localsearch_index_status"),
                let shutdown = dlsym(handle, "localsearch_shutdown")
            else {
                loadErrors.append("\(path) is missing expected symbols (stale build?)")
                dlclose(handle)
                continue
            }

            return FFIBackend(
                handle: handle,
                queryFn: unsafeBitCast(query, to: QueryFn.self),
                freeResultsFn: unsafeBitCast(freeResults, to: FreeResultsFn.self),
                recordClickFn: unsafeBitCast(recordClick, to: StringFn.self),
                configureFn: unsafeBitCast(configure, to: StringFn.self),
                indexStatusFn: unsafeBitCast(indexStatus, to: StatusFn.self),
                shutdownFn: unsafeBitCast(shutdown, to: VoidFn.self)
            )
        }

        fputs("[LocalSearch] Search engine library could not be loaded.\n", stderr)
        for err in loadErrors {
            fputs("[LocalSearch] \(err)\n", stderr)
        }
        return nil
    }

    func search(
        query: String,
        filters: [QueryFilter],
        scope: SearchScope,
        cancellationToken: CancellationToken
    ) -> AsyncStream<[SearchResult]> {
        let queryFn = self.queryFn
        let freeResultsFn = self.freeResultsFn
        let request = SearchRequest(query: query)

        return AsyncStream<[SearchResult]> { continuation in
            Task.detached(priority: .userInitiated) {
                defer { continuation.finish() }
                guard !request.text.isEmpty else {
                    continuation.yield([])
                    return
                }

                var rawResults: UnsafeMutableRawPointer?
                var count = 0
                let status = request.text.withCString { queryFn($0, &rawResults, &count) }
                guard status == 0 else { return }
                defer { freeResultsFn(rawResults, count) }

                guard let rawResults, count > 0 else {
                    continuation.yield([])
                    return
                }

                let typedResults = rawResults.assumingMemoryBound(to: CSearchResult.self)
                var batch: [SearchResult] = []
                batch.reserveCapacity(count)

                for index in 0..<count {
                    let cResult = typedResults.advanced(by: index).pointee
                    guard let pathPtr = cResult.path else { continue }

                    let path = String(cString: pathPtr)
                    guard request.matches(path: path) else { continue }

                    batch.append(SearchResult(
                        id: String(cResult.doc_id),
                        filename: URL(fileURLWithPath: path).lastPathComponent,
                        path: path,
                        rank: cResult.score,
                        fileKind: FileKind.detect(path: path)
                    ))
                }

                continuation.yield(batch)
            }
        }
    }

    func systemState() -> AsyncStream<SystemState> {
        AsyncStream { continuation in
            continuation.yield(.nominal)
            continuation.finish()
        }
    }

    /// Polls the engine. Yields nil whenever the index is up to date.
    func indexProgress() -> AsyncStream<IndexProgress?> {
        let indexStatusFn = self.indexStatusFn
        return AsyncStream { continuation in
            let task = Task.detached(priority: .utility) {
                var tracker = IndexProgressTracker()
                while !Task.isCancelled {
                    let progress = Self.readStatus(indexStatusFn).flatMap { tracker.progress(for: $0) }
                    continuation.yield(progress)
                    // Poll quickly while there is something to show, lazily otherwise
                    try? await Task.sleep(for: progress == nil ? .seconds(2) : .milliseconds(400))
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    func prefetchPrefix(_ prefix: String) async {
        // Queries are answered from the engine's in-memory index; nothing to warm.
    }

    func spellingSuggestions(for query: String) async -> [String] {
        []
    }

    func recordOpen(path: String) {
        let recordClickFn = self.recordClickFn
        Task.detached(priority: .utility) {
            _ = path.withCString { recordClickFn($0) }
        }
    }

    func applyIndexingConfig(_ json: String) {
        let configureFn = self.configureFn
        Task.detached(priority: .utility) {
            let status = json.withCString { configureFn($0) }
            if status != 0 {
                fputs("[LocalSearch] Could not apply indexing settings (code \(status)).\n", stderr)
            }
        }
    }

    func indexSummary() -> String? {
        guard let status = Self.readStatus(indexStatusFn) else { return nil }
        let count = status.doc_count.formatted()
        switch status.phase {
        case 0: return "Loading saved index…"
        case 1: return "Scanning folders · \(count) items so far"
        case 2:
            let done = status.work_done.formatted()
            let total = status.work_total.formatted()
            return "Reading file contents · \(done) of \(total)"
        default:
            let note = status.budget_exhausted != 0 ? " · content limit reached" : ""
            return "Up to date · \(count) items\(note)"
        }
    }

    func shutdown() {
        shutdownFn()
    }

    private static func readStatus(_ indexStatusFn: StatusFn) -> CIndexStatus? {
        var status = CIndexStatus()
        let code = withUnsafeMutablePointer(to: &status) { indexStatusFn(UnsafeMutableRawPointer($0)) }
        return code == 0 ? status : nil
    }
}

/// Mirrors `SearchResult` in `src/ffi.rs`
private struct CSearchResult {
    var doc_id: UInt64
    var path: UnsafePointer<CChar>?
    var score: Float
    var snippet: UnsafePointer<CChar>?
}

/// Mirrors `IndexStatus` in `src/ffi.rs`
struct CIndexStatus {
    var phase: UInt32 = 0
    var budget_exhausted: UInt32 = 0
    var doc_count: UInt64 = 0
    var work_done: UInt64 = 0
    var work_total: UInt64 = 0
    var elapsed_secs: UInt64 = 0
}

/// Turns raw engine status into what the UI shows, estimating time left from
/// the rate observed since the current phase began.
struct IndexProgressTracker {
    private var phase: UInt32?
    private var phaseStart = Date()

    mutating func progress(for status: CIndexStatus, now: Date = Date()) -> IndexProgress? {
        if status.phase != phase {
            phase = status.phase
            phaseStart = now
        }

        switch status.phase {
        case 0:
            return IndexProgress(phase: "Loading index", percent: 0, etaMinutes: nil)
        case 1:
            let count = status.doc_count.formatted()
            return IndexProgress(phase: "Scanning folders · \(count) items", percent: 0, etaMinutes: nil)
        case 2:
            guard status.work_total > 0 else {
                return IndexProgress(phase: "Reading file contents", percent: 0, etaMinutes: nil)
            }
            let fraction = min(1, Double(status.work_done) / Double(status.work_total))
            let elapsed = now.timeIntervalSince(phaseStart)
            var eta: Int?
            if fraction > 0.02, elapsed > 3 {
                let remaining = elapsed * (1 - fraction) / fraction
                eta = max(1, Int((remaining / 60).rounded(.up)))
            }
            return IndexProgress(phase: "Reading file contents", percent: fraction, etaMinutes: eta)
        default:
            return nil
        }
    }
}

/// The query as sent to the engine, plus the inline filters applied to what
/// comes back. Supported: `kind:pdf`, `in:folder`, and `-word` exclusions.
struct SearchRequest {
    /// Query text with filter tokens removed
    let text: String
    private var kinds: [String] = []
    private var folders: [String] = []
    private var excluded: [String] = []

    private static let filterPrefixes = ["kind:", "in:", "after:", "before:", "size:", "tag:"]

    init(query: String) {
        var words: [String] = []
        for token in query.split(separator: " ").map(String.init) {
            let lower = token.lowercased()
            if lower.hasPrefix("kind:") {
                kinds.append(String(lower.dropFirst(5)))
            } else if lower.hasPrefix("in:") {
                folders.append(String(lower.dropFirst(3)).replacingOccurrences(of: "~", with: ""))
            } else if lower.hasPrefix("-"), lower.count > 1 {
                excluded.append(String(lower.dropFirst()))
            } else if !Self.filterPrefixes.contains(where: lower.hasPrefix) {
                words.append(token)
            }
        }
        text = words.joined(separator: " ")
    }

    func matches(path: String) -> Bool {
        let lower = path.lowercased()
        let url = URL(fileURLWithPath: lower)
        let ext = url.pathExtension

        for kind in kinds where !kind.isEmpty {
            let kindMatches = ext == kind || FileKind.detect(path: path).filterNames.contains(kind)
            if !kindMatches { return false }
        }
        let directory = url.deletingLastPathComponent().path
        for folder in folders where !folder.isEmpty {
            if !directory.contains(folder) { return false }
        }
        for word in excluded {
            if url.lastPathComponent.contains(word) { return false }
        }
        return true
    }
}

extension FileKind {
    static func detect(path: String) -> FileKind {
        var isDir: ObjCBool = false
        if FileManager.default.fileExists(atPath: path, isDirectory: &isDir), isDir.boolValue {
            return .folder
        }

        let ext = URL(fileURLWithPath: path).pathExtension.lowercased()
        if ["png", "jpg", "jpeg", "gif", "webp", "heic", "tiff", "bmp", "svg"].contains(ext) {
            return .image
        }
        if ["rs", "swift", "ts", "tsx", "js", "jsx", "py", "go", "java", "kt", "cpp", "c", "h", "rb", "sh", "sql"].contains(ext) {
            return .code
        }
        return .document
    }

    /// Words accepted by the `kind:` filter for this kind
    var filterNames: [String] {
        switch self {
        case .document: return ["document", "doc"]
        case .image: return ["image", "photo", "picture"]
        case .code: return ["code", "source"]
        case .folder: return ["folder", "directory", "dir"]
        }
    }
}
