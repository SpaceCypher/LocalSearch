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
    private typealias SnippetFn = @convention(c) (UnsafePointer<CChar>, UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?
    private typealias FreeStringFn = @convention(c) (UnsafeMutablePointer<CChar>?) -> Void

    private let handle: UnsafeMutableRawPointer
    private let queryFn: QueryFn
    private let freeResultsFn: FreeResultsFn
    private let recordClickFn: StringFn
    private let configureFn: StringFn
    private let indexStatusFn: StatusFn
    private let shutdownFn: VoidFn
    private let snippetFn: SnippetFn
    private let freeStringFn: FreeStringFn

    private init(
        handle: UnsafeMutableRawPointer,
        queryFn: @escaping QueryFn,
        freeResultsFn: @escaping FreeResultsFn,
        recordClickFn: @escaping StringFn,
        configureFn: @escaping StringFn,
        indexStatusFn: @escaping StatusFn,
        shutdownFn: @escaping VoidFn,
        snippetFn: @escaping SnippetFn,
        freeStringFn: @escaping FreeStringFn
    ) {
        self.handle = handle
        self.queryFn = queryFn
        self.freeResultsFn = freeResultsFn
        self.recordClickFn = recordClickFn
        self.configureFn = configureFn
        self.indexStatusFn = indexStatusFn
        self.shutdownFn = shutdownFn
        self.snippetFn = snippetFn
        self.freeStringFn = freeStringFn
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
                let shutdown = dlsym(handle, "localsearch_shutdown"),
                let snippet = dlsym(handle, "localsearch_snippet"),
                let freeString = dlsym(handle, "localsearch_free_string")
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
                shutdownFn: unsafeBitCast(shutdown, to: VoidFn.self),
                snippetFn: unsafeBitCast(snippet, to: SnippetFn.self),
                freeStringFn: unsafeBitCast(freeString, to: FreeStringFn.self)
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
            let queryTask = Task.detached(priority: .userInitiated) {
                defer { continuation.finish() }
                // The user has already typed something else
                if Task.isCancelled { return }
                guard !request.text.isEmpty else {
                    continuation.yield([])
                    return
                }

                var rawResults: UnsafeMutableRawPointer?
                var count = 0
                let status = request.text.withCString { queryFn($0, &rawResults, &count) }
                guard status == 0 else { return }
                defer { freeResultsFn(rawResults, count) }
                if Task.isCancelled { return }

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

            continuation.onTermination = { _ in
                queryTask.cancel()
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

    /// Reads the file, so it runs off the main actor.
    func snippet(path: String, query: String) async -> String? {
        let snippetFn = self.snippetFn
        let freeStringFn = self.freeStringFn
        let text = SearchRequest(query: query).text
        guard !text.isEmpty else { return nil }
        return await Task.detached(priority: .utility) {
            let raw = path.withCString { pathPtr in
                text.withCString { queryPtr in snippetFn(pathPtr, queryPtr) }
            }
            guard let raw else { return nil as String? }
            defer { freeStringFn(raw) }
            return String(cString: raw)
        }.value
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
/// comes back: `kind:pdf`, `in:folder`, `-word`, `after:2025-01-31`,
/// `before:2025-06`, `size:>10mb`, `tag:work`.
struct SearchRequest {
    /// Query text with filter tokens removed
    let text: String
    private var kinds: [String] = []
    private var folders: [String] = []
    private var excluded: [String] = []
    private var tags: [String] = []
    /// Modified on or after this instant
    private var modifiedFrom: Date?
    /// Modified before this instant
    private var modifiedBefore: Date?
    private var minBytes: Int64?
    private var maxBytes: Int64?
    /// Filter tokens that could not be understood (e.g. `after:yesterday`).
    /// They are dropped from the query rather than searched for as words.
    private(set) var unrecognised: [String] = []

    private static let filterPrefixes = ["kind:", "in:", "after:", "before:", "size:", "tag:"]

    /// True when a filter needs the file's attributes, which costs a stat
    private var needsAttributes: Bool {
        modifiedFrom != nil || modifiedBefore != nil || minBytes != nil || maxBytes != nil || !tags.isEmpty
    }

    /// `2025`, `2025-03` or `2025-03-09`, in the user's time zone
    static func parseDate(_ text: String) -> Date? {
        let parts = text.split(separator: "-").compactMap { Int($0) }
        guard (1...3).contains(parts.count), parts.count == text.split(separator: "-").count,
              (1970...9999).contains(parts[0]) else { return nil }
        var components = DateComponents()
        components.year = parts[0]
        components.month = parts.count > 1 ? parts[1] : 1
        components.day = parts.count > 2 ? parts[2] : 1
        guard (1...12).contains(components.month!), (1...31).contains(components.day!) else { return nil }
        return Calendar.current.date(from: components)
    }

    /// `>10mb`, `<500kb`, `>2gb` → (isMinimum, bytes)
    static func parseSize(_ text: String) -> (isMinimum: Bool, bytes: Int64)? {
        guard let comparator = text.first, comparator == ">" || comparator == "<" else { return nil }
        let rest = text.dropFirst().lowercased()
        let digits = rest.prefix { $0.isNumber }
        guard let amount = Int64(digits) else { return nil }
        let multiplier: Int64
        switch rest.dropFirst(digits.count) {
        case "b", "": multiplier = 1
        case "kb": multiplier = 1_000
        case "mb": multiplier = 1_000_000
        case "gb": multiplier = 1_000_000_000
        default: return nil
        }
        return (comparator == ">", amount * multiplier)
    }

    init(query: String) {
        var words: [String] = []
        for token in query.split(separator: " ").map(String.init) {
            let lower = token.lowercased()
            if lower.hasPrefix("kind:") {
                kinds.append(String(lower.dropFirst(5)))
            } else if lower.hasPrefix("in:") {
                folders.append(String(lower.dropFirst(3)).replacingOccurrences(of: "~", with: ""))
            } else if lower.hasPrefix("after:") {
                if let date = Self.parseDate(String(lower.dropFirst(6))) { modifiedFrom = date } else { unrecognised.append(token) }
            } else if lower.hasPrefix("before:") {
                if let date = Self.parseDate(String(lower.dropFirst(7))) { modifiedBefore = date } else { unrecognised.append(token) }
            } else if lower.hasPrefix("size:") {
                if let size = Self.parseSize(String(lower.dropFirst(5))) {
                    if size.isMinimum { minBytes = size.bytes } else { maxBytes = size.bytes }
                } else {
                    unrecognised.append(token)
                }
            } else if lower.hasPrefix("tag:") {
                tags.append(String(lower.dropFirst(4)))
            } else if lower.hasPrefix("-"), lower.count > 1 {
                excluded.append(String(lower.dropFirst()))
            } else {
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
        guard needsAttributes else { return true }

        // A file we cannot stat cannot be shown to satisfy a date or size filter
        let fileURL = URL(fileURLWithPath: path)
        guard let values = try? fileURL.resourceValues(forKeys: [.contentModificationDateKey, .fileSizeKey, .tagNamesKey]) else {
            return false
        }
        if let modifiedFrom, (values.contentModificationDate ?? .distantPast) < modifiedFrom { return false }
        if let modifiedBefore, (values.contentModificationDate ?? .distantFuture) >= modifiedBefore { return false }
        if minBytes != nil || maxBytes != nil {
            // Size filters are about files; a folder has no size of its own
            guard let size = values.fileSize.map(Int64.init) else { return false }
            if let minBytes, size <= minBytes { return false }
            if let maxBytes, size >= maxBytes { return false }
        }
        if !tags.isEmpty {
            let fileTags = (values.tagNames ?? []).map { $0.lowercased() }
            if !tags.allSatisfy(fileTags.contains) { return false }
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
