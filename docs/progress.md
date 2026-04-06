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
| Tasks complete | 5 / 46 |
| Tests written | 24 |
| Tests passing | 24 / 24 |
| Commits | 5 |
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

## In Progress

### 🔄 Task 6 — Delta Index (`src/index/delta.rs`)
**Target commit:** `feat(index): in-memory delta index with tombstone deletes`

**Key logic:**
- In-memory inverted index holding recent changes
- HashMap-based term → posting list structure
- Tombstone deletes (HashSet of deleted DocIds)
- Size tracking with configurable budget (default 50MB)
- Triggers compaction when size exceeds limit

---

## Upcoming

| # | Task | Key implementation |
|---|---|---|
| 6 | Delta Index | In-memory inverted index, tombstone deletes, 50MB budget |
| 7 | BK-Tree Fuzzy Matching | Damerau-Levenshtein on Unicode code points, edit distance ≤ 2 |
| 8 | Path Trie + Roaring Bitmap Scope | O(1) scope filter per document |
| 9 | Tokenizer | Unicode NFC, camelCase split, stop words, Porter stemmer |
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
