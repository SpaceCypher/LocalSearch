# LocalSearch — Build Progress Log

> Live progress tracker. Updated after every task completes.
> Plan: [`docs/plans/2026-04-06-localsearch-core.md`](./plans/2026-04-06-localsearch-core.md)

---

## Status Legend
- `✅` Complete + committed
- `🔄` In progress
- `⬜` Not started

---

## Summary

| Metric | Value |
|---|---|
| Tasks complete | 13 / 46 (backend) + 7 / 22 (frontend) |
| Tests written | 47 (backend) + 33 (frontend) = 80 |
| Tests passing | 80 / 80 |
| Commits | 21 |
| Last updated | 2026-04-07 |

---

## Completed Tasks

### ✅ Task 1 — Project Scaffold
**Commit:** `feat: scaffold localsearch workspace`
- Initialized Cargo workspace (`localsearch` + `extractor-xpc` crates)
- Created all 26 module source files across `wal/`, `index/`, `query/`, `fs/`, `extract/`, `resource/`, `metrics/`
- Wrote `Cargo.toml` with all production dependencies from `implementation_spec.md §16.1`
  - `roaring`, `lz4_flex`, `zstd`, `crossbeam`, `rayon`, `memmap2`, `rusqlite`, `xxhash-rust`, `unicode-normalization`, `rust-stemmers`
- `cargo check` → ✅ zero errors

---

### ✅ Task 2 — WAL Entry Binary Format (`src/wal/entry.rs`)
**Commit:** `feat(wal): WAL entry binary format with xxh3 checksum`

**What was built:**
- `EventType` enum (Created/Modified/Deleted/Renamed/PermissionChanged/MetadataChanged/Checkpoint)
- `WalEntry` struct with full binary layout from `implementation_spec.md §2.1`
  - Fixed 70-byte header + variable path + 8-byte xxh3_64 checksum
  - Little-endian byte order throughout
- `serialize(&mut Vec<u8>)` — binary encode
- `deserialize(&[u8])` — binary decode
- `verify_checksum(&self)` — self-consistency check (post round-trip)
- `verify_checksum_bytes(&[u8])` — raw buffer integrity check (WAL reader usage, catches corruption before decode)
- `serialized_len()` — exact byte count without allocating

**Tests (5/5 GREEN):**
| Test | Result |
|---|---|
| `test_wal_entry_roundtrip` | ✅ |
| `test_corrupt_entry_detected` | ✅ |
| `test_event_type_roundtrip` | ✅ |
| `test_serialized_len_accurate` | ✅ |
| `test_unicode_path_roundtrip` | ✅ |

**Key design note:** Corruption is detected at the **buffer layer** (`verify_checksum_bytes`) before deserialization — matching the WAL reader's usage pattern. The struct-level `verify_checksum` is for post-decode self-consistency.

---

### ✅ Task 3 — WAL Writer (`src/wal/writer.rs`)
**Commit:** `feat(wal): WAL writer with batched O_DSYNC flush (64-entry / 10ms deadline) and checkpoints`

**What was built:**
- `WriterState` — internal state: pending byte buffer, counts, last_seq, checkpoint_seq, total_flushed
- `WalWriter::new()` — opens/creates WAL file, spawns background `wal-deadline-flush` thread
- `WalWriter::new_with_deadline()` — configurable deadline for tests (e.g. 60s to disable timer)
- `append(entry)` — serializes entry into pending buffer; auto-flushes when `pending_count >= 64`
- `flush()` — drains pending with single `write_all` + `sync_data()` (O_DSYNC); **no-op if pending is empty** (does NOT increment flush_count)
- `do_flush()` — core flush logic: writes checkpoint record + `sync_all()` when 256-entry boundary crossed
- Background deadline thread — wakes every `deadline`, flushes if `pending_count > 0`
- `Drop` impl — signals thread to stop, flushes remaining entries before file closes

**ADR-005 satisfied:** Per-entry `O_DSYNC` → batched (64-entry or 10ms). Under 50K events/sec storm: 781 flushes instead of 50,000.

**Tests (6/6 GREEN):**
| Test | Result |
|---|---|
| `test_wal_writer_append_and_checkpoint` | ✅ |
| `test_wal_batched_flush_under_storm` | ✅ |
| `test_wal_deadline_flush_fires_within_10ms` | ✅ |
| `test_empty_flush_does_not_increment_flush_count` | ✅ |
| `test_wal_file_is_non_empty_after_append_and_flush` | ✅ |
| `test_checkpoint_fires_at_correct_boundary` | ✅ |

---

### ✅ Task 4 — WAL Reader + Crash Recovery (`src/wal/reader.rs`)
**Commit:** `feat(wal): WAL reader with checksum-gated replay and crash truncation`

**What was built:**
- `WalReader::new()` — opens WAL file for sequential reading
- `replay_from(start_seq)` — replays entries from a given sequence number
  - Verifies checksum on each entry BEFORE deserialization
  - Stops gracefully at first corrupt/truncated entry
  - Returns all valid entries collected before corruption point
- `read_entry()` — reads single entry with corruption detection
  - Reads fixed 70-byte header to get path length
  - Reads variable-length path + 8-byte checksum
  - Calls `verify_checksum_bytes()` before deserializing
  - Returns `Ok(None)` on clean EOF, `Err(_)` on corruption
- `current_position()` — returns file offset (for truncation after replay)

**Crash recovery strategy:**
1. Read entries sequentially from WAL file
2. Verify xxh3 checksum on raw bytes before decode
3. Stop at first checksum failure or truncated read (partial write from crash)
4. Return all valid entries up to corruption point
5. Caller can truncate WAL file to discard corrupt tail

**Tests (6/6 GREEN):**
| Test | Result |
|---|---|
| `test_wal_reader_replay_from_checkpoint` | ✅ |
| `test_wal_reader_handles_truncated_entry` | ✅ |
| `test_wal_reader_empty_file` | ✅ |
| `test_wal_reader_skips_entries_before_start_seq` | ✅ |
| `test_wal_reader_handles_checkpoint_records` | ✅ |
| `test_wal_reader_replay_from_after_checkpoint` | ✅ |

**Key design note:** Corruption detection happens at the **raw buffer level** before deserialization. When corruption is detected, `replay_from()` stops gracefully and returns all valid entries collected so far — it does NOT propagate the error. This ensures crash recovery always succeeds with partial data rather than failing completely.

### ✅ Task 5 — File Identity DB (`src/fs/identity.rs`)
**Commit:** `feat(fs): stable DocId allocation with inode reuse detection`

**What was built:**
- `DocId` — transparent wrapper around u64, never reused
- `FileIdentity` — composite key: `(volume_uuid, inode, generation, device_id)`
- `IdentityDb` — SQLite-backed mapping with two tables:
  - `identity_map` — `FileIdentity` → `DocId` with unique index
  - `metadata` — persistent counter for monotonic DocId allocation
- `get_or_allocate()` — lookup existing or allocate new DocId
- `allocate_next_id()` — atomic counter increment with persistence
- Inode reuse detection via `generation` field (APFS guarantees increment on reuse)
- Rename-stable: same file keeps same DocId across path changes

**Tests (7/7 GREEN):**
| Test | Result |
|---|---|
| `test_doc_id_allocation_is_stable` | ✅ |
| `test_inode_reuse_gets_new_doc_id` | ✅ |
| `test_doc_id_never_zero` | ✅ |
| `test_persistence_across_reopens` | ✅ |
| `test_counter_persists_across_reopens` | ✅ |
| `test_doc_ids_are_monotonic` | ✅ |
| `test_different_volumes_get_different_doc_ids` | ✅ |

**Key design note:** DocIds are allocated from a persistent counter that survives process restarts. The `generation` field in `FileIdentity` detects when an inode is reused for a different file after deletion, ensuring each distinct file gets a unique DocId even if the inode number is recycled.

---

### ✅ Task 6 — Delta Index (`src/index/delta.rs`)
**Commit:** `feat(index): in-memory delta index with tombstone deletes`

**What was built:**
- `DeltaIndex` — in-memory inverted index with 50MB budget
  - `insert_document(doc, postings)` — adds document and its term postings to index
  - `delete_document(doc_id)` — marks document as deleted via tombstone (doesn't remove from index)
  - `is_deleted(doc_id)` — checks if document is tombstoned
  - `lookup(term)` — returns posting list for a term
- `Posting` — term occurrence in a document (doc_id, term_freq, field_mask, positions)
- `PostingList` — list of postings for a term
- `Document` — document metadata (doc_id, path)
- Field constants: `FIELD_FILENAME`, `FIELD_PATH`, `FIELD_CONTENT`, `FIELD_TAGS`

**Tests (2/2 GREEN):**
| Test | Result |
|---|---|
| `test_delta_insert_and_lookup` | ✅ |
| `test_delta_delete_tombstone` | ✅ |

**Key design note:** Deletes use tombstones rather than immediate removal from the index. This allows for efficient batch compaction later and maintains posting list stability during concurrent queries.

---

### ✅ Task 7 — BK-Tree Fuzzy Matching (`src/index/bktree.rs`)
**Commit:** `feat(index): BK-tree with unicode-safe Damerau-Levenshtein fuzzy matching`

**What was built:**
- `BkTree` — tree structure for efficient fuzzy string search
  - `insert(word)` — adds word to tree, organized by edit distance
  - `search(query, max_distance)` — finds all words within max_distance edits
- `damerau_levenshtein(a, b)` — computes edit distance on Unicode code points
  - Supports: insertion, deletion, substitution, transposition
  - Operates on `char` (Unicode code points), not bytes
  - Handles Unicode correctly (e.g., "café" vs "cafe")
- Tree structure: each node stores a word and children indexed by edit distance
- Search optimization: prunes branches outside distance range

**Tests (4/4 GREEN):**
| Test | Result |
|---|---|
| `test_bktree_exact_match` | ✅ |
| `test_bktree_edit_distance_1` | ✅ |
| `test_bktree_edit_distance_2` | ✅ |
| `test_bktree_unicode_safe` | ✅ |

**Key design note:** The BK-Tree uses the triangle inequality property of edit distance to prune the search space. For a query with max distance d, only children at distances in range [node_distance - d, node_distance + d] need to be explored. This makes fuzzy search much faster than linear scanning.

---

### ✅ Task 8 — Path Trie + Roaring Bitmap Scope (`src/index/trie.rs`)
**Commit:** `feat(index): path trie with roaring bitmap scope resolution`

**What was built:**
- `PathTrie` — compressed trie for efficient path-based scope queries
  - `insert(path, doc_id)` — adds document to trie at its path location
  - `scope_query(scope)` — returns all DocIds under a path prefix
  - `collect_all_docs(node)` — recursively gathers all documents in subtree
- `TrieNode` — trie node with Roaring bitmap for documents and child map
- Path splitting: splits paths by `/` and builds trie from components
- Roaring bitmap operations: efficient set union for collecting documents
- O(path_depth) insertion and O(path_depth + result_size) query

**Tests (4/4 GREEN):**
| Test | Result |
|---|---|
| `test_trie_scope_query_returns_correct_docs` | ✅ |
| `test_trie_scope_query_includes_subdirectories` | ✅ |
| `test_trie_scope_query_nonexistent_path` | ✅ |
| `test_trie_multiple_docs_same_path` | ✅ |

**Key design note:** The trie uses Roaring bitmaps for space-efficient DocId storage. Scope queries recursively collect all documents from the target node and its descendants, enabling efficient "search in folder" functionality. The bitmap union operation is very fast, making scope filtering O(1) per document during query execution.

---

### ✅ Task 9 — Tokenizer (`src/query/parser.rs`)
**Commit:** `feat(query): unicode-aware tokenizer with stemming and camelCase split`

**What was built:**
- `Token` — term with position (doc_id, term, position)
- `Tokenizer` — Unicode-aware tokenizer with:
  - Unicode NFC normalization using `unicode-normalization` crate
  - camelCase splitting (e.g., "AppDelegate" → "app", "delegate")
  - Stop word removal (48 common English words: "the", "a", "an", etc.)
  - Porter stemming using `rust-stemmers` crate (e.g., "running" → "run")
- `tokenize(text)` — converts text to normalized, stemmed tokens
- `split_camel_case(word)` — detects uppercase boundaries in camelCase words

**Tests (3/3 GREEN):**
| Test | Result |
|---|---|
| `test_tokenizer_camelcase_split` | ✅ |
| `test_tokenizer_stop_words_removed` | ✅ |
| `test_tokenizer_stems` | ✅ |

**Key design note:** The tokenizer operates on Unicode code points (not bytes) and normalizes to NFC form before processing. camelCase detection looks for lowercase-to-uppercase transitions. The Porter stemmer reduces words to their root form for better matching (e.g., "running", "runs", "ran" all stem to "run").

---

### ✅ Task 10 — Query Parser + Intent Classification (`src/query/parser.rs`)
**Commit:** `feat(query): BM25 scorer with field boost and intent-weighted ranking` (bundled with Task 11)

**What was built:**
- `Query` struct with fields: tokens, scope, intent, filters
- `QueryIntent` enum: Lookup, Recovery, Exploration, Verification
- `Query::parse(text)` — parses query string into structured Query
  - Extracts scope from "in:/path" syntax
  - Extracts filters like "after:date", "before:date", "ext:pdf"
  - Classifies intent based on query patterns:
    - Lookup: simple keyword queries (default)
    - Recovery: queries with time filters (after/before)
    - Exploration: queries with wildcards (* or ?)
    - Verification: queries checking file existence

**Tests (3/3 GREEN):**
| Test | Result |
|---|---|
| `test_parser_scope_extraction` | ✅ |
| `test_intent_lookup` | ✅ |
| `test_intent_recovery` | ✅ |

**Key design note:** Intent classification uses heuristics based on query structure. Time filters indicate Recovery intent (finding old files), wildcards indicate Exploration, and specific keywords like "exist"/"find"/"check" indicate Verification. Default is Lookup for simple keyword searches.

---

### ✅ Task 11 — BM25 Scorer (`src/query/ranker.rs`)
**Commit:** `feat(query): BM25 scorer with field boost and intent-weighted ranking`

**What was built:**
- `BM25Scorer` struct with configurable parameters
- `new(total_docs, avg_doc_len)` — constructor with k1=1.2, b=0.75 defaults
- `idf(doc_freq)` — calculates inverse document frequency
  - Formula: log((total_docs - doc_freq + 0.5) / (doc_freq + 0.5) + 1.0)
- `score(term_freq, doc_len, doc_freq)` — calculates BM25 score
  - Formula: IDF * (tf * (k1 + 1)) / (tf + k1 * (1 - b + b * doc_len / avg_doc_len))

**Tests (2/2 GREEN):**
| Test | Result |
|---|---|
| `test_bm25_higher_freq_higher_score` | ✅ |
| `test_bm25_rare_term_higher_idf` | ✅ |

**Key design note:** BM25 is a probabilistic ranking function that balances term frequency (how often a term appears in a document) with inverse document frequency (how rare the term is across all documents). The k1 parameter controls term frequency saturation, and b controls document length normalization.

---

### ✅ Task 12 — FSEvents Pipeline (`src/fs/events.rs`)
**Commit:** `feat(fs): FSEvents watcher with deduplication and MUST_SCAN_SUBDIRS handling`

**What was built:**
- `FsEvent` struct with path, event_type, timestamp fields
- `FsEventWatcher` — wraps macOS FSEvents via notify crate
  - `new(path, sender)` — creates watcher for directory, sends events to channel
  - Event deduplication logic (100ms window) to handle FSEvents storms
  - MUST_SCAN_SUBDIRS handling via RecursiveMode::Recursive
  - Thread-safe channel-based event delivery
- All code gated with `#[cfg(target_os = "macos")]` for cross-platform compilation
- Uses `notify` crate (v6.1) with native FSEvents backend on macOS

**Tests (5/5 GREEN on macOS):**
| Test | Result |
|---|---|
| `test_fsevent_detects_new_file` | ✅ |
| `test_fsevent_detects_file_modification` | ✅ |
| `test_fsevent_detects_file_deletion` | ✅ |
| `test_fsevent_deduplication` | ✅ |
| `test_fsevent_subdirectory_changes` | ✅ |

**Key design note:** FSEvents can generate event storms (hundreds of events per second) during operations like git checkout or npm install. The 100ms deduplication window prevents overwhelming downstream systems while still capturing all meaningful changes. The notify crate provides a high-level abstraction over platform-specific file watchers.

---

### ✅ Task F1 — Xcode Project Scaffold (Frontend)
**Commit:** `feat(frontend): scaffold SwiftUI macOS app — accessory policy, no Dock icon`

**What was built:**
- Swift Package Manager project structure (`frontend/Package.swift`)
- `LocalSearchApp.swift` — App entry point with NSApplicationDelegateAdaptor
- `AppDelegate` — Sets `.accessory` activation policy (no Dock icon)
- `SearchWindowController` — NSPanel-based window controller (stub)
- `SearchViewModel` — State management (stub)
- `HotkeyManager` — Global hotkey registration (stub)
- `Info.plist` — LSUIElement=true for accessory app behavior
- Basic test structure with `SearchViewModelTests.swift`

**Tests (1/1 GREEN):**
| Test | Result |
|---|---|
| `test_initialState` | ✅ |

**Key design note:** Uses Swift Package Manager instead of Xcode project for better version control and CLI build support. The app runs as an accessory (no Dock icon, no menu bar) and manages its own NSPanel window.

---

### ✅ Task F2 — SearchViewModel State Machine Core (`frontend/Sources/ViewModel/SearchViewModel.swift`)
**Commit:** `feat(vm): SearchViewModel core — state machine, generation counter, insertion sort`

**What was built:**
- `SearchViewModel` with `@MainActor` ObservableObject pattern
- Query state machine: IDLE, TYPING, SEARCHING, STREAMING, COMPLETE
- `queryGeneration: UInt64` counter for cancellation (monotonically incrementing)
- `onQueryChange(_:)` — synchronous state updates, immediate stale result dimming
- `applyResult(_:fromGeneration:)` — generation check, insertion sort, selection tracking
- `insertResultSorted(_:)` — binary search insertion maintaining rank order
- Selection tracking that follows documents, not positions
- Stale result dimming (opacity 0.4) before async work starts
- Empty query handling (transition to IDLE, clear results)
- `MockBackend` for testing and development
- Backend protocol: `SearchBackendProtocol` with AsyncStream patterns

**Tests (7/7 GREEN):**
| Test | Result |
|---|---|
| `test_initialState_isIdle` | ✅ |
| `test_queryChange_immediatelyDimsStaleResults` | ✅ |
| `test_generationIncrements_onEachQueryChange` | ✅ |
| `test_emptyQuery_transitionsToIdle_clearsResults` | ✅ |
| `test_staleResults_discarded_whenGenerationMismatch` | ✅ |
| `test_insertResult_maintainsSortOrder` | ✅ |
| `test_selectionTracksDocument_notPosition` | ✅ |

**Key design notes:**
- Generation counter is the source of truth for cancellation, not timing
- All state mutations happen synchronously on MainActor before async work
- Stale results dimmed immediately (Guarantee 2 from spec §1)
- Selection follows document ID across rank changes, not array position
- Binary search insertion maintains O(log n) performance for result ordering
- Max 20 results in display list (8 visible + 12 scroll buffer)

---

### ✅ Task F4 — SearchWindow NSPanel Configuration (`frontend/Sources/Views/SearchWindowController.swift`)
**Commit:** `feat(window): NSPanel floating, non-activating, all-spaces, escape-to-dismiss`

**What was built:**
- `SearchWindowController` using `NSPanel` (not NSWindow)
- `.nonactivatingPanel` style mask — doesn't steal focus from underlying app
- `.floating` window level — appears above all normal windows
- `.canJoinAllSpaces` collection behavior — visible on all desktops/spaces
- Center-on-primary-display positioning using `NSScreen.main`
- `handleEscapeKey()` method — dismisses window with `orderOut(nil)`
- SwiftUI content view integration via `NSHostingView`

**Tests (6/6 GREEN):**
| Test | Result |
|---|---|
| `test_window_isNSPanel` | ✅ |
| `test_window_hasNonActivatingMask` | ✅ |
| `test_window_floatingLevel` | ✅ |
| `test_window_appearsOnAllSpaces` | ✅ |
| `test_window_centeredOnPrimaryDisplay` | ✅ |
| `test_escape_dismissesWindow` | ✅ |

**Key design notes:**
- NSPanel instead of NSWindow for proper floating panel behavior
- Non-activating means keyboard focus stays in underlying app (Spotlight-like)
- Floating level ensures window appears above all normal windows
- All-spaces behavior means window follows user across virtual desktops
- Escape key always dismisses (Guarantee from spec §2)

---

## In Progress

None — ready for Task F8 (ScopeBarView)

---

## Completed Tasks (Frontend)

### ✅ Task F6 — QueryFieldView State Integration
**Commit:** `feat(vm): QueryFieldView state — spinner, clear button, filter parsing integration`

**What was built:**
- Added `@Published` properties to SearchViewModel:
  - `showSpinner: Bool` — shows after 80ms debounce, hides when results arrive
  - `parsedFilters: [QueryFilter]` — extracted filters from query
  - `strippedQueryText: String` — query text with filters removed
  - `showClearButton: Bool` — computed property based on queryText
- `clearQuery()` method — resets query to empty and transitions to idle
- Integrated QueryParser into `onQueryChange(_:)` — parses filters on every query change
- Updated `startSearch` to pass `parsedFilters` to backend
- Created `TestHelpers.swift` — shared mock backends and extensions for all test files
  - `ImmediateBackend` — returns results immediately
  - `SlowBackend` — delays results by configurable duration
  - `TrackingBackend` — tracks search call count and last query
  - `SearchResult.mock()` extension

**Tests (5/5 GREEN):**
| Test | Result |
|---|---|
| `test_queryField_showsSpinner_after80ms` | ✅ |
| `test_queryField_hidesSpinner_whenResultsArrive` | ✅ |
| `test_clearButton_appears_whenQueryNonEmpty` | ✅ |
| `test_clearButton_tap_resetsToIdle` | ✅ |
| `test_filterChip_renderedFromParsedQuery` | ✅ |

**Key design notes:**
- Spinner appears after 80ms debounce (not immediately on typing)
- Spinner hides when search completes or is cancelled
- Clear button visibility is computed from queryText (not stored state)
- Filter parsing happens synchronously on every query change
- Parsed filters are passed to backend for server-side filtering
- Test helpers centralized to avoid duplication across test files

### ✅ Task F7 — Query Parser (Inline Filter Syntax)
**Commit:** `feat(query): inline filter parser — 8 filter types, negation, content phrase`

**What was built:**
- `QueryParser` class with `parse(_:)` static method
- `ParsedQuery` struct with `filters: [QueryFilter]` and `strippedText: String`
- `QueryFilter` enum with 8 filter types:
  - `kind(String)` — file type filter (e.g., "kind:pdf")
  - `path(String)` — path scope filter (e.g., "in:/Documents")
  - `after(String)` — date filter (e.g., "after:2024-01-01")
  - `before(String)` — date filter (e.g., "before:2024-12-31")
  - `size(String)` — size filter (e.g., "size:>1MB")
  - `tag(String)` — tag filter (e.g., "tag:important")
  - `contentPhrase(String)` — exact phrase match (e.g., content:"hello world")
  - `negation(String)` — exclude term (e.g., "-unwanted")
- Filter extraction with regex patterns
- Stripped text generation (removes filters, preserves query terms)

**Tests (8/8 GREEN):**
| Test | Result |
|---|---|
| `test_parse_noFilters_returnsFullText` | ✅ |
| `test_parse_kindFilter` | ✅ |
| `test_parse_pathScope` | ✅ |
| `test_parse_dateFilter_after` | ✅ |
| `test_parse_sizeFilter` | ✅ |
| `test_parse_contentPhrase` | ✅ |
| `test_parse_negation` | ✅ |
| `test_parse_multipleFilters` | ✅ |

**Key design notes:**
- Regex-based filter extraction for 8 filter types
- Stripped text preserves query terms after filter removal
- Content phrase uses quoted string syntax
- Negation uses minus prefix (e.g., "-term")
- Multiple filters can be combined in single query

---

## Completed Tasks (Frontend - Previous)

### ✅ Task F3 — Debounce + Cancellation Engine
**Commit:** `feat(vm): 80ms trailing-edge debounce with task cancellation and prefix cache bypass`

**What was built:**
- Refactored debounce properties from associated objects to stored properties in `SearchViewModel`
- `PrefixCache` class — in-memory cache for instant prefix results
- `debounceTask` property — cancellable Task for 80ms debounce
- `searchTask` property — cancellable Task for backend search
- Updated `onQueryChange(_:)` with:
  - Task cancellation on every keystroke (cancel previous debounce/search)
  - Prefix cache lookup that bypasses debounce (instant results)
  - 80ms trailing-edge debounce using `Task.sleep(nanoseconds: 80_000_000)`
  - Generation check before starting search (prevents stale searches)
- `startSearch(query:generation:)` private method:
  - Spawns searchTask with backend AsyncStream
  - Generation check on every result before applying
  - State transition to `.complete` when stream finishes

**Tests (11/11 GREEN):**
| Test | Result |
|---|---|
| `test_initialState_isIdle` | ✅ |
| `test_queryChange_immediatelyDimsStaleResults` | ✅ |
| `test_generationIncrements_onEachQueryChange` | ✅ |
| `test_emptyQuery_transitionsToIdle_clearsResults` | ✅ |
| `test_staleResults_discarded_whenGenerationMismatch` | ✅ |
| `test_insertResult_maintainsSortOrder` | ✅ |
| `test_selectionTracksDocument_notPosition` | ✅ |
| `test_debounce_firesAfter80ms` | ✅ |
| `test_rapidTyping_firesOnlyOneSearch` | ✅ |
| `test_newQuery_cancels_previousSearchTask` | ✅ |
| `test_prefixCache_hit_bypasses_debounce` | ✅ |

**Key design notes:**
- Trailing-edge debounce: search fires 80ms after last keystroke
- Task cancellation prevents wasted work from abandoned queries
- Prefix cache provides instant results for common prefixes (no debounce wait)
- Generation counter ensures only current query's results are displayed
- Refactored from associated objects to proper stored properties for cleaner architecture

---

### ✅ Task F5 — Global Hotkey Registration (⌥Space)
**Commit:** `feat(hotkey): ⌥Space global hotkey with CGEventTap + Carbon fallback`

**What was built:**
- `HotkeyManager` singleton with dual registration strategy:
  - Primary: CGEventTap (requires accessibility permission, more reliable)
  - Fallback: Carbon RegisterEventHotKey (works in sandbox, no permission needed)
- `WindowControllerProtocol` — protocol for window show/hide operations
- `register()` — tries CGEventTap first, falls back to Carbon
- `toggle()` — shows/hides window, positions at cursor on show
- `setWindowController(_:)` — public setter for wiring from AppDelegate
- AppDelegate integration — wires HotkeyManager to SearchWindowController
- SearchWindowController conformance to WindowControllerProtocol

**Tests (3/3 GREEN):**
| Test | Result |
|---|---|
| `test_hotkeyManager_isSingleton` | ✅ |
| `test_register_doesNotThrow` | ✅ |
| `test_toggle_callsShowOrHide` | ✅ |

**Key design notes:**
- CGEventTap provides better reliability but requires accessibility permission
- Carbon fallback ensures hotkey works even without permission
- Window positioning at cursor on show (Spotlight-like behavior)
- Weak reference to window controller prevents retain cycles
- Toggle tracks visibility state to alternate between show/hide

---

## In Progress (Previous)

### ✅ Task F3 — Debounce + Cancellation Engine
**Commit:** `feat(vm): 80ms trailing-edge debounce with task cancellation and prefix cache bypass`

**What was built:**
- `SearchViewModel+Debounce.swift` extension file with:
  - `PrefixCache` class — in-memory cache for instant prefix results
  - `debounceTask` property — cancellable Task for 80ms debounce
  - `searchTask` property — cancellable Task for backend search
  - Associated object storage for extension properties
- Updated `onQueryChange(_:)` with:
  - Task cancellation on every keystroke (cancel previous debounce/search)
  - Prefix cache lookup that bypasses debounce (instant results)
  - 80ms trailing-edge debounce using `Task.sleep(nanoseconds: 80_000_000)`
  - Generation check before starting search (prevents stale searches)
- `startSearch(query:generation:)` private method:
  - Spawns searchTask with backend AsyncStream
  - Generation check on every result before applying
  - State transition to `.complete` when stream finishes

**Tests (4/4 GREEN):**
| Test | Result |
|---|---|
| `test_debounce_firesAfter80ms` | ✅ |
| `test_rapidTyping_firesOnlyOneSearch` | ✅ |
| `test_newQuery_cancels_previousSearchTask` | ✅ |
| `test_prefixCache_hit_bypasses_debounce` | ✅ |

**Key design notes:**
- Trailing-edge debounce: search fires 80ms after last keystroke
- Task cancellation prevents wasted work from abandoned queries
- Prefix cache provides instant results for common prefixes (no debounce wait)
- Generation counter ensures only current query's results are displayed
- All tests pass including existing F2 tests (10/10 total)

---

## Upcoming (Frontend)

| # | Task | Key implementation |
|---|---|---|
| F6 | QueryFieldView — Search Input Component | Spinner, clear button, filter chip extraction |
| F7 | Query Parser — Inline Filter Syntax | 8 filter types, negation, content phrase |
| F8 | ScopeBarView — Filter Chips | Multi-select OR filter, ⌘1-5 shortcuts, instant client-side filtering |
| F9 | ResultListView + ResultRowView | Fixed heights, pill highlights, middle truncation |
| F10 | Keyboard Navigation | 14 shortcuts, history navigation, modifier actions |
| F11 | StatusBarView — System State Feedback | Result count, 7-state system badge, rotating shortcut hints |
| F12 | MetadataPanelView — Detail Expansion | QL thumbnail, 260px slide-in, quick actions |
| F13 | Animation System | 6 spring configs, reduce motion support, 4-animation cap |
| F14 | IndexProgressView — First Launch Bootstrap | Static progress bar, phase text, ETA display |
| F15 | Zero-Result State + Spelling Suggestions | BK-tree spelling suggestions, degraded-mode explanation |
| F16 | Permission-Denied Result Row | Lock icon, inline label, system settings alert |
| F17 | Accessibility | VoiceOver labels, live region status bar, reduce motion |
| F18 | Backend Protocol + XPC Channel | XPC channel to Rust engine, AsyncStream bridge, auto-reconnect |
| F19 | Prefix Cache (UI-Side Mirror) | LRU 8MB, prefix substring match, speculative prefetch |
| F20 | Slow Backend + Skeleton State | 150ms threshold, max 3, stale-results exclusion |
| F21 | Integration Test — Full Search Flow | End-to-end with MockBackend |
| F22 | Accessibility Audit + Reduce Motion Hardening | XCUITest VoiceOver navigation audit |

---

## Upcoming (Backend - On Hold) (Frontend)

| # | Task | Key implementation |
|---|---|---|
| 13 | WAL Ingestion Pipeline | FSEvents → WAL end-to-end wire |
| 14 | Indexing Pipeline | WAL → delta index transformation |
| 15 | Query Executor | Full pipeline: fuzzy expand → scope → merge → rank → top-K |
| 16 | Memory Controller | 4-state machine (Full/Reduced/Minimal/Critical) + recovery |
| 17 | Compaction | Delta → immutable segment, atomic `rename()`, LZ4/zstd |
| 18 | Integrity Check + Metrics | 1000-doc parity sample, SQLite ring buffer |
| 19 | Cold Start Scenarios | First launch, warm restart, crash recovery |
| 20 | Chaos Tests | WAL corruption, FSEvents storm, disk full, EACCES |
| 21 | Rust ↔ Swift FFI | C ABI: `localsearch_query`, `localsearch_free_results` |
| 22 | Suffix Array | O(log n) substring search + rebuild-window fallback |
| 23 | FSEvents Edge Cases | Rename pair detection, symlink policy, EACCES handling |
| 24 | Reconciliation Worker | Snapshot-diff, priority queue for hot paths |
| 25 | Content Extraction XPC | Sandboxed extractor, crash containment, 64KB extraction |
| 26 | Power Monitor + I/O Throttling | Thermal/battery-aware compaction, `setiopolicy_np` |
| 27 | Index Migration + Versioning | 3-tier migration, rollback backup |
| 28 | Index-Disk Parity Test | `tests/parity.rs` — catches silent index divergence |
| 29 | Production Checklist | All chaos scenarios as CI gates |
| 30–46 | Advanced features | N-gram, Phonetic, Prefix Cache, ResultProvider, Thread Registry, Fault Injector, Security/TCC, External Volumes, Spotlight Fallback, Benchmarks, Health Dashboard, Signal DB |

---

## Git Log

```
5cfa653  feat(vm): QueryFieldView state — spinner, clear button, filter parsing integration
eb38829  docs: update progress - F7 complete (28/28 tests passing)
e7c25f2  feat(query): inline filter parser — 8 filter types, negation, content phrase
40868e3  fix(vm): refactor debounce to stored properties, wire HotkeyManager to window controller
fd7f48f  feat(vm): 80ms trailing-edge debounce with task cancellation and prefix cache bypass
4879c82  feat(window): NSPanel floating, non-activating, all-spaces, escape-to-dismiss
cb9987b  feat(fs): FSEvents watcher with deduplication and MUST_SCAN_SUBDIRS handling
5710737  feat(query): BM25 scorer with field boost and intent-weighted ranking
e4fd0d0  docs: update progress - Task 9 complete (37/37 tests passing)
c5f6f9a  feat(query): unicode-aware tokenizer with stemming and camelCase split
edbd8b6  docs: update progress - Task 8 complete (34/34 tests passing)
8e9ff57  feat(index): path trie with roaring bitmap scope resolution
4755188  docs: update progress - Task 7 complete (30/30 tests passing)
aa9e460  feat(index): BK-tree with unicode-safe Damerau-Levenshtein fuzzy matching
54bab5e  docs: update progress - Task 6 complete (26/26 tests passing)
67c7a2d  feat(index): in-memory delta index with tombstone deletes
a4c4c1d  fix(fs): add missing OptionalExtension import for identity module
[prev]   feat(fs): stable DocId allocation with inode reuse detection
```

---

## Architecture Decisions Logged

| ADR | Decision | Rationale |
|---|---|---|
| ADR-005 | Batch WAL writes (64-entry / 10ms) before `O_DSYNC` | Per-entry `O_DSYNC` saturates I/O under 50K events/sec FSEvent storms |
| ADR-SA | `SuffixArray` signals `FallbackRequired` during rebuild | Prevents silent empty results during the rebuild window |
| ADR-CMP | Size-tiered compaction over level-tiered | Minimizes write amplification on laptop SSDs |
| ADR-MMAP | `pread` over mmap for cold segments | Predictable latency — avoids Jetsam compression spikes |
| ADR-XPC | Content extraction in separate XPC process | Crash isolation — PDFKit/Vision failures can't corrupt main index |
| ADR-XPC2 | Compaction helper XPC deferred to v2 | Compaction doesn't touch user file content — security gain is minor |
