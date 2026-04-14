import Foundation
import Darwin

final class FFIBackend: SearchBackendProtocol {
    private typealias QueryFn = @convention(c) (
        UnsafePointer<CChar>,
        UnsafeMutablePointer<UnsafeMutableRawPointer?>,
        UnsafeMutablePointer<Int>
    ) -> Int32
    private typealias FreeResultsFn = @convention(c) (UnsafeMutableRawPointer?, Int) -> Void

    private let handle: UnsafeMutableRawPointer
    private let queryFn: QueryFn
    private let freeResultsFn: FreeResultsFn

    private init(
        handle: UnsafeMutableRawPointer,
        queryFn: @escaping QueryFn,
        freeResultsFn: @escaping FreeResultsFn
    ) {
        self.handle = handle
        self.queryFn = queryFn
        self.freeResultsFn = freeResultsFn
    }

    deinit {
        dlclose(handle)
    }

    private static func dylibCandidates() -> [String] {
        var candidates: [String] = []

        if let explicit = ProcessInfo.processInfo.environment["LOCALSEARCH_DYLIB_PATH"], !explicit.isEmpty {
            candidates.append(explicit)
        }

        // Runtime cwd-relative fallbacks
        candidates.append(contentsOf: [
            "./target/debug/liblocalsearch.dylib",
            "../target/debug/liblocalsearch.dylib",
            "../../target/debug/liblocalsearch.dylib"
        ])

        // Resolve repo root from this source file path for stable local dev loading.
        let sourceURL = URL(fileURLWithPath: #filePath)
        let repoRoot = sourceURL
            .deletingLastPathComponent() // Backend
            .deletingLastPathComponent() // Sources
            .deletingLastPathComponent() // frontend
            .deletingLastPathComponent() // repo root
        let absolute = repoRoot.appendingPathComponent("target/debug/liblocalsearch.dylib").path
        candidates.append(absolute)

        // Deduplicate while preserving order
        var seen = Set<String>()
        return candidates.filter { seen.insert($0).inserted }
    }

    static func makeIfAvailable() -> FFIBackend? {
        let candidates = dylibCandidates()
        var loadErrors: [String] = []

        for path in candidates {
            guard let handle = dlopen(path, RTLD_NOW | RTLD_LOCAL) else {
                if let cErr = dlerror() {
                    let message = String(cString: cErr)
                    loadErrors.append("dlopen failed for \(path): \(message)")
                } else {
                    loadErrors.append("dlopen failed for \(path): unknown error")
                }
                continue
            }

            guard
                let rawQuery = dlsym(handle, "localsearch_query"),
                let rawFreeResults = dlsym(handle, "localsearch_free_results")
            else {
                dlclose(handle)
                continue
            }

            let queryFn = unsafeBitCast(rawQuery, to: QueryFn.self)
            let freeResultsFn = unsafeBitCast(rawFreeResults, to: FreeResultsFn.self)
            return FFIBackend(handle: handle, queryFn: queryFn, freeResultsFn: freeResultsFn)
        }

        if !loadErrors.isEmpty {
            fputs("[LocalSearch] FFI backend unavailable.\n", stderr)
            for err in loadErrors {
                fputs("[LocalSearch] \(err)\n", stderr)
            }
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

        return AsyncStream<[SearchResult]> { continuation in
            let queryTask = Task(priority: .utility) {
                if Task.isCancelled {
                    continuation.finish()
                    return
                }

                guard let queryCString = query.cString(using: .utf8) else {
                    continuation.finish()
                    return
                }

                var rawResults: UnsafeMutableRawPointer?
                var count = 0

                let status = queryCString.withUnsafeBufferPointer { buffer in
                    queryFn(buffer.baseAddress!, &rawResults, &count)
                }

                if Task.isCancelled {
                    continuation.finish()
                    return
                }

                guard status == 0 else {
                    continuation.finish()
                    return
                }

                defer { freeResultsFn(rawResults, count) }

                guard let rawResults else {
                    continuation.finish()
                    return
                }

                if count <= 0 {
                    continuation.finish()
                    return
                }

                let typedResults = rawResults.assumingMemoryBound(to: CSearchResult.self)
                var batch: [SearchResult] = []
                batch.reserveCapacity(count)

                for index in 0..<count {
                    let cResult = typedResults.advanced(by: index).pointee
                    guard let pathPtr = cResult.path else { continue }

                    let path = String(cString: pathPtr)
                    let fileURL = URL(fileURLWithPath: path)
                    let filename = fileURL.lastPathComponent
                    let kind = Self.detectFileKind(path: path)

                    let result = SearchResult(
                        id: String(cResult.doc_id),
                        filename: filename,
                        path: path,
                        rank: cResult.score,
                        fileKind: kind
                    )

                    batch.append(result)
                }

                continuation.yield(batch)
                continuation.finish()
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

    func indexProgress() -> AsyncStream<IndexProgress?> {
        AsyncStream { continuation in
            continuation.yield(nil)
            continuation.finish()
        }
    }

    func prefetchPrefix(_ prefix: String) async {
        // FFI backend does not maintain a remote prefix cache yet.
    }

    func spellingSuggestions(for query: String) async -> [String] {
        []
    }

    private static func detectFileKind(path: String) -> FileKind {
        var isDir: ObjCBool = false
        if FileManager.default.fileExists(atPath: path, isDirectory: &isDir), isDir.boolValue {
            return .folder
        }

        let ext = URL(fileURLWithPath: path).pathExtension.lowercased()
        if ["md", "txt", "pdf", "doc", "docx", "rtf"].contains(ext) {
            return .document
        }
        if ["png", "jpg", "jpeg", "gif", "webp", "heic"].contains(ext) {
            return .image
        }
        if ["rs", "swift", "ts", "js", "py", "go", "java", "cpp", "c", "h"].contains(ext) {
            return .code
        }
        return .document
    }
}

private struct CSearchResult {
    var doc_id: UInt64
    var path: UnsafePointer<CChar>?
    var score: Float
    var snippet: UnsafePointer<CChar>?
}
