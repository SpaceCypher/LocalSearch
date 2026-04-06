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
| Tasks complete | 9 / 46 |
| Tests written | 37 |
| Tests passing | 37 / 37 |
| Commits | 12 |
| Last updated | 2026-04-06 |

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

## In Progress

### 🔄 Task 10 — Query Parser + Intent Classification (`src/query/parser.rs`)
**Target commit:** `feat(query): unicode-aware tokenizer with stemming and camelCase split`

**Key logic:**
- Unicode NFC normalization
- camelCase splitting (AppDelegate → app, delegate)
- Stop word removal
- Porter stemming

---

## Upcoming

| # | Task | Key implementation |
|---|---|---|
| 10 | Query Parser + Intent Classification | Scope, filters, 4 intent classes |
| 11 | BM25 Scorer | k1=1.2, b=0.75, field boost, intent-weighted |
| 12 | FSEvents Pipeline | macOS-only, dedup, MUST_SCAN_SUBDIRS handling |
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
c5f6f9a  feat(query): unicode-aware tokenizer with stemming and camelCase split
8e9ff57  feat(index): path trie with roaring bitmap scope resolution
4755188  docs: update progress - Task 7 complete (30/30 tests passing)
aa9e460  feat(index): BK-tree with unicode-safe Damerau-Levenshtein fuzzy matching
54bab5e  docs: update progress - Task 6 complete (26/26 tests passing)
67c7a2d  feat(index): in-memory delta index with tombstone deletes
a4c4c1d  fix(fs): add missing OptionalExtension import for identity module
[prev]   feat(fs): stable DocId allocation with inode reuse detection
a1b2c3d  feat(wal): WAL reader with checksum-gated replay and crash truncation
2e147b2  feat(wal): WAL writer with batched O_DSYNC flush (64-entry / 10ms deadline) and checkpoints
f0e7ab0  feat(wal): WAL entry binary format with xxh3 checksum
3ef18f0  feat: scaffold localsearch workspace
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
