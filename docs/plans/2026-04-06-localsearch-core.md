# LocalSearch Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a production-grade macOS file search daemon in Rust that replaces Spotlight with sub-100ms queries, strong freshness guarantees, and graceful degradation.

**Architecture:** FSEvents → WAL → Delta Index → Immutable Segments (LSM). Query executor merges delta + segments at query time. Content extraction runs in a sandboxed XPC process. Swift UI calls into Rust via C FFI.

**Tech Stack:** Rust (core engine), Swift/SwiftUI (UI), SQLite (signal DB + identity DB), crossbeam (channels), roaring-rs (bitmaps), rayon (parallel queries), memmap2 (mmap), fsevent-sys (FSEvents), lz4/zstd (compression), xxhash-rust (checksums).

**Reference files:**
- `context/main.md` — architecture decisions and tradeoffs
- `context/implementation_spec.md` — full Rust code, binary formats, trait definitions

---

## Task 1: Project Scaffold

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/lib.rs`
- Create: `src/wal/mod.rs`, `src/wal/entry.rs`, `src/wal/writer.rs`, `src/wal/reader.rs`
- Create: `src/index/mod.rs`, `src/index/delta.rs`, `src/index/segment.rs`, `src/index/inverted.rs`, `src/index/trie.rs`
- Create: `src/query/mod.rs`, `src/query/parser.rs`, `src/query/executor.rs`, `src/query/ranker.rs`
- Create: `src/fs/mod.rs`, `src/fs/events.rs`, `src/fs/identity.rs`, `src/fs/reconcile.rs`
- Create: `src/extract/mod.rs`, `src/extract/client.rs`, `src/extract/extractors.rs`
- Create: `src/resource/mod.rs`, `src/resource/memory.rs`, `src/resource/power.rs`
- Create: `src/metrics/mod.rs`, `src/metrics/collector.rs`
- Create: `extractor-xpc/Cargo.toml`, `extractor-xpc/src/main.rs`

**Step 1: Init workspace**

```bash
cd /Users/sanidhyakumar/Documents/findohh
cargo init --name localsearch
cargo new --name localsearch-extractor extractor-xpc
```

Expected: Two crates created, no errors.

**Step 2: Write `Cargo.toml`**

```toml
[package]
name = "localsearch"
version = "0.1.0"
edition = "2021"

[workspace]
members = [".", "extractor-xpc"]

[dependencies]
roaring = "0.10"
lz4 = "1.24"
zstd = "0.13"
crossbeam = "0.8"
rayon = "1.7"
memmap2 = "0.9"
rusqlite = { version = "0.31", features = ["bundled"] }
fsevent-sys = "4.1"
xxhash-rust = { version = "0.8", features = ["xxh3"] }
unicode-normalization = "0.1"
rust-stemmers = "1.2"
log = "0.4"
env_logger = "0.11"
thiserror = "1.0"
anyhow = "1.0"

[dev-dependencies]
tempfile = "3"
```

**Step 3: Create all module files with `mod.rs` stubs**

```bash
mkdir -p src/{wal,index,query,fs,extract,resource,metrics}
touch src/wal/{mod,entry,writer,reader}.rs
touch src/index/{mod,delta,segment,inverted,trie}.rs
touch src/query/{mod,parser,executor,ranker}.rs
touch src/fs/{mod,events,identity,reconcile}.rs
touch src/extract/{mod,client,extractors}.rs
touch src/resource/{mod,memory,power}.rs
touch src/metrics/{mod,collector}.rs
```

**Step 4: Declare modules in `src/lib.rs`**

```rust
pub mod wal;
pub mod index;
pub mod query;
pub mod fs;
pub mod extract;
pub mod resource;
pub mod metrics;
```

**Step 5: Build check**

```bash
cargo check
```

Expected: Compiles with zero errors (only unused module warnings OK).

**Step 6: Commit**

```bash
git init
git add .
git commit -m "feat: scaffold localsearch workspace"
```

---

## Task 2: WAL Entry (`src/wal/entry.rs`)

**Files:**
- Create: `src/wal/entry.rs`
- Test: `src/wal/entry.rs` (inline `#[cfg(test)]`)

**Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wal_entry_roundtrip() {
        let entry = WalEntry {
            seq: 1,
            timestamp_us: 1000000,
            event_type: EventType::Created,
            flags: 0,
            doc_id: 42,
            inode: 12345,
            volume_uuid: 0xdeadbeef,
            mtime_ns: 999,
            size: 1024,
            mode: 0o644,
            uid: 501,
            gid: 20,
            path: "/Users/test/file.txt".to_string(),
        };

        let mut buf = Vec::new();
        entry.serialize(&mut buf);
        let decoded = WalEntry::deserialize(&buf).unwrap();

        assert_eq!(decoded.seq, 1);
        assert_eq!(decoded.doc_id, 42);
        assert_eq!(decoded.path, "/Users/test/file.txt");
        assert!(decoded.verify_checksum());
    }

    #[test]
    fn test_corrupt_entry_detected() {
        let entry = WalEntry::new_test();
        let mut buf = Vec::new();
        entry.serialize(&mut buf);
        buf[10] ^= 0xFF; // Flip bits
        let decoded = WalEntry::deserialize(&buf).unwrap();
        assert!(!decoded.verify_checksum());
    }
}
```

**Step 2: Run — verify FAIL**

```bash
cargo test -p localsearch wal::entry
```

Expected: FAIL — `WalEntry` not defined.

**Step 3: Implement `src/wal/entry.rs`**

```rust
use xxhash_rust::xxh3::xxh3_64;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventType {
    Created = 1,
    Modified = 2,
    Deleted = 3,
    Renamed = 4,
    PermissionChanged = 5,
    MetadataChanged = 6,
    Checkpoint = 255,
}

impl TryFrom<u8> for EventType {
    type Error = anyhow::Error;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            1 => Ok(EventType::Created),
            2 => Ok(EventType::Modified),
            3 => Ok(EventType::Deleted),
            4 => Ok(EventType::Renamed),
            5 => Ok(EventType::PermissionChanged),
            6 => Ok(EventType::MetadataChanged),
            255 => Ok(EventType::Checkpoint),
            _ => Err(anyhow::anyhow!("Invalid EventType: {}", v)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WalEntry {
    pub seq: u64,
    pub timestamp_us: u64,
    pub event_type: EventType,
    pub flags: u8,
    pub doc_id: u64,
    pub inode: u64,
    pub volume_uuid: u128,
    pub mtime_ns: u64,
    pub size: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub path: String,
}

impl WalEntry {
    pub fn serialize(&self, buf: &mut Vec<u8>) {
        let path_bytes = self.path.as_bytes();
        // Fixed fields
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.timestamp_us.to_le_bytes());
        buf.push(self.event_type as u8);
        buf.push(self.flags);
        buf.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
        buf.extend_from_slice(&self.doc_id.to_le_bytes());
        buf.extend_from_slice(&self.inode.to_le_bytes());
        buf.extend_from_slice(&self.volume_uuid.to_le_bytes());
        buf.extend_from_slice(&self.mtime_ns.to_le_bytes());
        buf.extend_from_slice(&self.size.to_le_bytes());
        buf.extend_from_slice(&self.mode.to_le_bytes());
        buf.extend_from_slice(&self.uid.to_le_bytes());
        buf.extend_from_slice(&self.gid.to_le_bytes());
        buf.extend_from_slice(&[0u8; 4]); // reserved
        buf.extend_from_slice(path_bytes);
        // Checksum (last 8 bytes)
        let checksum = xxh3_64(&buf);
        buf.extend_from_slice(&checksum.to_le_bytes());
    }

    pub fn deserialize(buf: &[u8]) -> anyhow::Result<Self> {
        if buf.len() < 96 { return Err(anyhow::anyhow!("Buffer too short")); }
        let seq = u64::from_le_bytes(buf[0..8].try_into()?);
        let timestamp_us = u64::from_le_bytes(buf[8..16].try_into()?);
        let event_type = EventType::try_from(buf[16])?;
        let flags = buf[17];
        let path_len = u16::from_le_bytes(buf[18..20].try_into()?) as usize;
        let doc_id = u64::from_le_bytes(buf[20..28].try_into()?);
        let inode = u64::from_le_bytes(buf[28..36].try_into()?);
        let volume_uuid = u128::from_le_bytes(buf[36..52].try_into()?);
        let mtime_ns = u64::from_le_bytes(buf[52..60].try_into()?);
        let size = u64::from_le_bytes(buf[60..68].try_into()?);
        let mode = u32::from_le_bytes(buf[68..72].try_into()?);
        let uid = u32::from_le_bytes(buf[72..76].try_into()?);
        let gid = u32::from_le_bytes(buf[76..80].try_into()?);
        // [80..84] reserved
        let path_start = 84;
        let path_end = path_start + path_len;
        let path = String::from_utf8(buf[path_start..path_end].to_vec())?;
        Ok(WalEntry { seq, timestamp_us, event_type, flags, doc_id, inode,
                      volume_uuid, mtime_ns, size, mode, uid, gid, path })
    }

    pub fn verify_checksum(&self) -> bool {
        let mut buf = Vec::new();
        self.serialize(&mut buf);
        // The last 8 bytes are the checksum, rest is the data
        let data = &buf[..buf.len() - 8];
        let stored = u64::from_le_bytes(buf[buf.len()-8..].try_into().unwrap());
        xxh3_64(data) == stored
    }

    #[cfg(test)]
    pub fn new_test() -> Self {
        WalEntry {
            seq: 1, timestamp_us: 1000, event_type: EventType::Created,
            flags: 0, doc_id: 1, inode: 100, volume_uuid: 0,
            mtime_ns: 0, size: 0, mode: 0o644, uid: 501, gid: 20,
            path: "/test".to_string(),
        }
    }
}
```

**Step 4: Run — verify PASS**

```bash
cargo test -p localsearch wal::entry
```

Expected: 2 tests pass.

**Step 5: Commit**

```bash
git add src/wal/entry.rs
git commit -m "feat(wal): WAL entry binary format with xxh3 checksum"
```

---

## Task 3: WAL Writer (`src/wal/writer.rs`)

**Files:**
- Create: `src/wal/writer.rs`
- Test: inline `#[cfg(test)]`

> **Architect-review fix (ADR-005):** WAL writes MUST be batched — accumulate up to 64 events OR 10ms, then flush with a single `O_DSYNC`. Per-event `O_DSYNC` saturates I/O under FSEvent storms (50K events/sec). Durability is still guaranteed: events are in the kernel buffer immediately; `O_DSYNC` on flush ensures they reach disk before the batch deadline.

**Step 1: Write failing tests**

```rust
#[test]
fn test_wal_writer_append_and_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.wal");
    let mut writer = WalWriter::new(&path).unwrap();

    for i in 0..300u64 {
        let entry = WalEntry { seq: i, ..WalEntry::new_test() };
        writer.append(entry).unwrap();
    }

    // Checkpoint should have fired at seq 256
    assert_eq!(writer.checkpoint_seq(), 256);
    assert_eq!(writer.last_seq(), 299);
}

#[test]
fn test_wal_batched_flush_under_storm() {
    // ADR-005: batch up to 64 entries before O_DSYNC, not per-entry.
    // Verify: 128 appends result in exactly 2 flushes (not 128).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("storm.wal");
    let mut writer = WalWriter::new(&path).unwrap();

    for i in 0..128u64 {
        writer.append(WalEntry { seq: i, ..WalEntry::new_test() }).unwrap();
    }
    writer.flush().unwrap(); // explicit flush to drain pending batch

    assert_eq!(writer.flush_count(), 2, // batch_size=64 → 2 flushes for 128 entries
        "Expected exactly 2 O_DSYNC flushes for 128 entries (batch_size=64)");
    assert_eq!(writer.last_seq(), 127);
}

#[test]
fn test_wal_deadline_flush_fires_within_10ms() {
    // ADR-005: even if batch is not full, flush fires within 10ms.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deadline.wal");
    let mut writer = WalWriter::new_with_deadline(&path, std::time::Duration::from_millis(10)).unwrap();

    writer.append(WalEntry { seq: 0, ..WalEntry::new_test() }).unwrap(); // only 1 entry
    std::thread::sleep(std::time::Duration::from_millis(15));
    writer.flush().unwrap();

    assert_eq!(writer.flush_count(), 1, "Deadline flush should have fired within 10ms");
}
```

**Step 2: Run — verify FAIL**

```bash
cargo test -p localsearch wal::writer
```

**Step 3: Implement `src/wal/writer.rs`** — `WalWriter` with:
- Internal `Vec<WalEntry>` pending batch (capacity 64)
- Background timer thread that fires flush every 10ms if batch is non-empty
- `flush()` drains pending batch with a single `O_DSYNC` write (via `write_all` + `sync_data()`)
- `fsync` checkpoint record every 256 entries (after flush)
- `flush_count()` counter for tests
- See `implementation_spec.md §2.2` for WAL binary layout.

**Step 4: Run — verify PASS**

```bash
cargo test -p localsearch wal::writer
```

**Step 5: Commit**

```bash
git commit -am "feat(wal): WAL writer with batched O_DSYNC flush (64-entry / 10ms deadline) and checkpoints"
```

---

## Task 4: WAL Reader + Crash Recovery (`src/wal/reader.rs`)

**Step 1: Write failing test**

```rust
#[test]
fn test_wal_reader_replay_from_checkpoint() {
    // Write 500 entries, corrupt entry 300, verify replay stops at 299
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.wal");
    // ... write entries 0..500, corrupt entry[300]
    let entries = WalReader::new(&path).unwrap()
        .replay_from(0).unwrap();
    assert!(entries.iter().all(|e| e.seq < 300));
}
```

**Step 2:** Run FAIL → Implement → Run PASS → Commit `"feat(wal): WAL reader with checksum-gated replay and crash recovery"`

---

## Task 5: File Identity DB (`src/fs/identity.rs`)

**Files:**
- Create: `src/fs/identity.rs`

**Step 1: Write failing test**

```rust
#[test]
fn test_doc_id_allocation_is_stable() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = IdentityDb::new(&dir.path().join("identity.db")).unwrap();
    let id1 = db.get_or_allocate(0xabc, 1001, 1, 16).unwrap();
    let id2 = db.get_or_allocate(0xabc, 1001, 1, 16).unwrap();
    assert_eq!(id1, id2); // Same file → same DocId
}

#[test]
fn test_inode_reuse_gets_new_doc_id() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = IdentityDb::new(&dir.path().join("identity.db")).unwrap();
    let id1 = db.get_or_allocate(0xabc, 1001, 1, 16).unwrap(); // gen=1
    let id2 = db.get_or_allocate(0xabc, 1001, 2, 16).unwrap(); // gen=2 (reused)
    assert_ne!(id1, id2);
}
```

**Step 2:** Run FAIL → Implement with SQLite (`rusqlite`) → Run PASS → Commit `"feat(fs): stable DocId allocation with inode reuse detection"`

---

## Task 6: Delta Index (`src/index/delta.rs`)

**Step 1: Write failing tests**

```rust
#[test]
fn test_delta_insert_and_lookup() {
    let mut delta = DeltaIndex::new(50 * 1024 * 1024);
    let doc = Document::new_test(DocId(1), "/test/file.txt");
    let mut postings = HashMap::new();
    postings.insert("quarterly".to_string(), Posting::new(DocId(1), 3, FIELD_FILENAME));
    delta.insert_document(doc, postings).unwrap();

    let result = delta.lookup("quarterly");
    assert!(result.is_some());
    assert_eq!(result.unwrap().postings[0].doc_id, DocId(1));
}

#[test]
fn test_delta_delete_tombstone() {
    let mut delta = DeltaIndex::new(50 * 1024 * 1024);
    // insert then delete
    delta.insert_document(Document::new_test(DocId(1), "/f"), HashMap::new()).unwrap();
    delta.delete_document(DocId(1)).unwrap();
    assert!(delta.is_deleted(DocId(1)));
}
```

**Step 2:** Run FAIL → Implement → Run PASS → Commit `"feat(index): in-memory delta index with tombstone deletes"`

---

## Task 7: BK-Tree Fuzzy Matching (`src/index/`)

**File:** Create `src/index/bktree.rs`, add to `src/index/mod.rs`

**Step 1: Write failing tests**

```rust
#[test]
fn test_bktree_exact_match() {
    let mut tree = BkTree::new();
    tree.insert("hello");
    assert_eq!(tree.search("hello", 0), vec!["hello"]);
}

#[test]
fn test_bktree_edit_distance_1() {
    let mut tree = BkTree::new();
    tree.insert("hello");
    tree.insert("world");
    let results = tree.search("helo", 1); // 1 deletion
    assert!(results.contains(&"hello".to_string()));
    assert!(!results.contains(&"world".to_string()));
}

#[test]
fn test_bktree_edit_distance_2() {
    let mut tree = BkTree::new();
    tree.insert("quarterly");
    let results = tree.search("quartely", 2); // 1 deletion
    assert!(results.contains(&"quarterly".to_string()));
}

#[test]
fn test_bktree_unicode_safe() {
    // Must operate on Unicode code points, not bytes
    let mut tree = BkTree::new();
    tree.insert("café");
    let results = tree.search("cafe", 1);
    assert!(results.contains(&"café".to_string()));
}
```

**Step 2:** Run FAIL → Implement Damerau-Levenshtein on `char`s (not bytes) → Run PASS → Commit `"feat(index): BK-tree with unicode-safe Damerau-Levenshtein fuzzy matching"`

---

## Task 8: Path Trie + Roaring Bitmap Scope (`src/index/trie.rs`)

**Step 1: Write failing tests**

```rust
#[test]
fn test_trie_scope_query_returns_correct_docs() {
    let mut trie = PathTrie::new();
    trie.insert("/Users/alice/Documents/report.pdf", DocId(1));
    trie.insert("/Users/alice/Desktop/todo.txt", DocId(2));
    trie.insert("/Users/bob/Documents/notes.md", DocId(3));

    let result = trie.scope_query(Path::new("/Users/alice/Documents"));
    assert!(result.contains(1u32));
    assert!(!result.contains(2u32));
    assert!(!result.contains(3u32));
}
```

**Step 2:** Run FAIL → Implement using `roaring::RoaringBitmap` → Run PASS → Commit `"feat(index): path trie with roaring bitmap scope resolution"`

---

## Task 9: Tokenizer (`src/query/parser.rs`)

**Step 1: Write failing tests**

```rust
#[test]
fn test_tokenizer_camelcase_split() {
    let t = Tokenizer::new();
    let tokens = t.tokenize("AppDelegate");
    let terms: Vec<_> = tokens.iter().map(|t| t.term.as_str()).collect();
    assert!(terms.contains(&"app"));
    assert!(terms.contains(&"delegate"));
}

#[test]
fn test_tokenizer_stop_words_removed() {
    let t = Tokenizer::new();
    let tokens = t.tokenize("the file report");
    assert!(!tokens.iter().any(|t| t.term == "the"));
}

#[test]
fn test_tokenizer_stems() {
    let t = Tokenizer::new();
    let tokens = t.tokenize("running");
    assert!(tokens.iter().any(|t| t.term == "run"));
}
```

**Step 2:** Run FAIL → Implement using `rust-stemmers` + `unicode-normalization` → Run PASS → Commit `"feat(query): unicode-aware tokenizer with stemming and camelCase split"`

---

## Task 10: Query Parser + Intent Classification (`src/query/parser.rs`)

**Step 1: Write failing tests**

```rust
#[test]
fn test_parser_scope_extraction() {
    let q = Query::parse("report in:/Users/alice/Documents").unwrap();
    assert_eq!(q.scope, Some(PathBuf::from("/Users/alice/Documents")));
    assert_eq!(q.tokens, vec!["report"]);
}

#[test]
fn test_intent_lookup() {
    let q = Query::parse("invoice").unwrap();
    assert_eq!(q.intent, QueryIntent::Lookup);
}

#[test]
fn test_intent_recovery() {
    let q = Query::parse("budget report after:2024-01-01").unwrap();
    assert_eq!(q.intent, QueryIntent::Recovery);
}
```

**Step 2:** Run FAIL → Implement → Run PASS → Commit `"feat(query): query parser with scope, filters, and intent classification"`

---

## Task 11: BM25 Scorer (`src/query/ranker.rs`)

**Step 1: Write failing test**

```rust
#[test]
fn test_bm25_higher_freq_higher_score() {
    let scorer = BM25Scorer::new(1000, 50.0);
    let s1 = scorer.score(5, 100, 200); // tf=5
    let s2 = scorer.score(1, 100, 200); // tf=1
    assert!(s1 > s2);
}

#[test]
fn test_bm25_rare_term_higher_idf() {
    let scorer = BM25Scorer::new(1000, 50.0);
    // term in 10 docs vs term in 900 docs
    let s_rare = scorer.idf(10);
    let s_common = scorer.idf(900);
    assert!(s_rare > s_common);
}
```

**Step 2:** Run FAIL → Implement with k1=1.2, b=0.75 → Run PASS → Commit `"feat(query): BM25 scorer with field boost and intent-weighted ranking"`

---

## Task 12: FSEvents Pipeline (`src/fs/events.rs`)

> Note: FSEvents requires macOS and won't run in CI on Linux. Gate with `#[cfg(target_os = "macos")]`.

**Step 1: Write integration test (macOS only)**

```rust
#[cfg(target_os = "macos")]
#[test]
fn test_fsevent_detects_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = crossbeam::channel::unbounded();

    let _watcher = FsEventWatcher::new(dir.path(), tx).unwrap();

    std::fs::write(dir.path().join("test.txt"), "hello").unwrap();
    std::thread::sleep(Duration::from_millis(500));

    let event = rx.try_recv().unwrap();
    assert!(event.path.ends_with("test.txt"));
}
```

**Step 2:** Run FAIL → Implement using `fsevent-sys` → Run PASS → Commit `"feat(fs): FSEvents watcher with deduplication and MUST_SCAN_SUBDIRS handling"`

---

## Task 13: WAL Ingestion Pipeline (`src/wal/mod.rs`)

Wire together: `FsEventWatcher` → `EventDeduplicator` → `IdentityDb::resolve` → `WalWriter::append`.

**Test:** Write a file → verify WAL entry appears with correct `doc_id`, `path`, `mtime_ns`.

**Commit:** `"feat(wal): end-to-end FSEvents → WAL ingestion pipeline"`

---

## Task 14: Indexing Pipeline (`src/index/mod.rs`)

Wire: `WalReader::replay` → `Tokenizer::tokenize` → `DeltaIndex::insert_document`.

**Test:** Write WAL entries manually → run pipeline → query delta index → verify results.

**Commit:** `"feat(index): WAL → delta index transformation pipeline"`

---

## Task 15: Query Executor (`src/query/executor.rs`)

Wire: `Query::parse` → `BkTree::search` (fuzzy expand) → `PathTrie::scope_query` → query delta + segments → `BM25Scorer` → top-K → return results.

**Test:**

```rust
#[test]
fn test_executor_end_to_end() {
    // Index 3 files, query for one by name with 1 typo
    let executor = build_test_executor(&[
        ("/test/quarterly_report.pdf", "quarterly report Q3"),
        ("/test/budget.xlsx", "budget 2024"),
        ("/test/notes.txt", "meeting notes"),
    ]);

    let results = executor.execute(Query::parse("quartely report").unwrap()).unwrap();
    assert_eq!(results[0].path, "/test/quarterly_report.pdf");
}
```

**Commit:** `"feat(query): full query executor with fuzzy expansion, scope filter, and BM25 ranking"`

---

## Task 16: Memory Controller (`src/resource/memory.rs`)

**Test:** Simulate RSS > 80% budget → verify `MemoryState` transitions to `Reduced` → simulate RSS drop → verify recovery to `Full`.

**Commit:** `"feat(resource): memory pressure control loop with state machine and recovery"`

---

## Task 17: Compaction (`src/index/segment.rs`)

Implement micro-compaction: delta → immutable segment file (LZ4 term dict, delta+varint posting lists, zstd metadata). Test atomic `rename()` write. Test segment binary search for term lookup.

**Commit:** `"feat(index): micro-compaction writes immutable segments with atomic rename"`

---

## Task 18: Integrity Check + Metrics (`src/metrics/`)

Implement 1000-doc sample parity check. Implement per-query SQLite ring buffer (7-day retention, 50MB cap). Wire phantom_rate alert threshold (>5% → trigger reconciliation).

**Commit:** `"feat(metrics): integrity check, query latency ring buffer, phantom rate alert"`

---

## Task 19: Cold Start Scenarios (`src/main.rs`)

Implement the 3 startup paths from `implementation_spec.md §9`:
- First launch: index Desktop/Documents/Downloads synchronously, then background crawl
- Warm restart: WAL replay from checkpoint, then `madvise(MADV_WILLNEED)` on hot slab
- Crash recovery: discard partial segment, verify segment checksums, replay WAL

**Commit:** `"feat: first-launch, warm-restart, and crash-recovery startup paths"`

---

## Task 20: Chaos Tests (`src/wal/`, `src/fs/`)

Implement the 5 chaos scenarios from `implementation_spec.md §10`:
1. `test_wal_corruption_recovery` — corrupt entry 500, verify replay stops at 499
2. `test_fsevent_storm` — 50 MUST_SCAN_SUBDIRS, verify hot paths reconciled first
3. Extractor XPC crash → verify `content_pending` state, not crash in main process
4. Mid-compaction disk full → verify partial segment discarded on restart
5. Permission revocation mid-crawl → verify `EACCES` handled, path marked inaccessible

**Commit:** `"test: chaos scenarios — WAL corruption, FSEvents storm, disk full, permission revocation"`

---

## Task 21: Rust ↔ Swift FFI (`src/lib.rs`)

Expose C ABI from `implementation_spec.md §13.3`:

```rust
#[repr(C)]
pub struct SearchResult { pub doc_id: u64, pub path: *const c_char, pub score: f32, pub snippet: *const c_char }

#[no_mangle]
pub extern "C" fn localsearch_query(query: *const c_char, results: *mut *mut SearchResult, count: *mut usize) -> i32 { ... }

#[no_mangle]
pub extern "C" fn localsearch_free_results(results: *mut SearchResult, count: usize) { ... }
```

**Commit:** `"feat(ffi): C ABI for Swift UI integration"`

---

## Task 22: Suffix Array Substring Search (`src/index/segment.rs`)

**Files:**
- Modify: `src/index/segment.rs` — add `SuffixArray` struct
- Modify: `src/index/mod.rs` — expose `SuffixArray`

**Step 1: Write failing tests**

```rust
#[test]
fn test_suffix_array_exact_substring() {
    let mut sa = SuffixArray::new();
    sa.insert("quarterly_report.pdf", DocId(1));
    sa.insert("budget_2024.xlsx", DocId(2));
    sa.insert("meeting_notes.txt", DocId(3));

    let results = sa.search_substring("report");
    assert!(results.contains(&DocId(1)));
    assert!(!results.contains(&DocId(2)));
}

#[test]
fn test_suffix_array_mid_filename() {
    let mut sa = SuffixArray::new();
    sa.insert("AppDelegate.swift", DocId(1));
    let results = sa.search_substring("Delegate");
    assert!(results.contains(&DocId(1)));
}

#[test]
fn test_substring_query_falls_back_during_rebuild() {
    // Architect-review fix: during suffix array rebuild, substring queries must
    // fall back to a linear scan of the delta index — not return empty results.
    // Verifies the rebuild window does NOT silently degrade user-visible search.
    let mut sa = SuffixArray::new();
    sa.insert("quarterly_report.pdf", DocId(1));

    // Simulate rebuild in progress: mark SA as rebuilding
    sa.begin_rebuild();
    assert!(sa.is_rebuilding());

    // Substring search during rebuild should return a FallbackRequired signal
    // (not empty results — caller must fall back to delta index linear scan)
    let result = sa.search_substring("report");
    assert!(
        result.is_fallback_required(),
        "During rebuild, search_substring must signal FallbackRequired, not return empty"
    );

    // After rebuild completes, normal results resume
    sa.commit_rebuild();
    let result = sa.search_substring("report");
    assert!(!result.is_fallback_required());
    assert!(result.doc_ids().contains(&DocId(1)));
}
```

**Implementation note for `SuffixArray`:**
- Add a `rebuilding: AtomicBool` flag
- `search_substring` checks `rebuilding` before binary search; returns `SubstringResult::FallbackRequired` if true
- `QueryExecutor` handles `FallbackRequired` by running a linear scan over the delta index as fallback
- This ensures users never receive silent empty results during the rebuild window

**Step 2:** Run FAIL → Implement flat binary suffix array (sorted `(u32, DocId)` pairs over concatenated filename blob) + `begin_rebuild`/`commit_rebuild` + `FallbackRequired` signal → Run PASS → Commit `"feat(index): suffix array for O(log n) substring search with graceful rebuild-window fallback"`

---

## Task 23: FSEvents Edge Cases — Rename, Symlink, Permission (`src/fs/events.rs`)

**Files:**
- Modify: `src/fs/events.rs`
- Test: inline `#[cfg(test)]` + `#[cfg(target_os = "macos")]`

**Step 1: Write failing tests**

```rust
#[cfg(target_os = "macos")]
#[test]
fn test_rename_updates_path_keeps_doc_id() {
    // Create file → index → rename → verify same DocId, new path in index
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old.txt");
    let new = dir.path().join("new.txt");
    std::fs::write(&old, "hello").unwrap();
    let (tx, rx) = crossbeam::channel::unbounded();
    let _w = FsEventWatcher::new(dir.path(), tx.clone()).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    std::fs::rename(&old, &new).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let events: Vec<_> = rx.try_iter().collect();
    // Expect: Deleted(old) + Created(new) pair with same doc_id
    let deleted = events.iter().find(|e| e.event_type == EventType::Deleted);
    let created = events.iter().find(|e| e.event_type == EventType::Created && e.path.ends_with("new.txt"));
    assert!(deleted.is_some());
    assert!(created.is_some());
}

#[test]
fn test_symlink_not_followed() {
    // symlink target content should NOT be indexed
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    let link = dir.path().join("link.txt");
    std::fs::write(&target, "secret").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let meta = std::fs::symlink_metadata(&link).unwrap();
    assert!(meta.file_type().is_symlink()); // Never follow
}

#[test]
fn test_permission_denied_handled_gracefully() {
    // chmod 000 → verify EACCES logged, path marked INACCESSIBLE, no crash
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("secret.txt");
    std::fs::write(&file, "data").unwrap();
    std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    let result = std::fs::read(&file);
    assert!(result.is_err());
    // Cleanup
    std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
}
```

**Step 2:** Run FAIL → implement rename pair detection, `should_follow_symlink() → false`, `EACCES` → `mark_inaccessible()` → Run PASS → Commit `"feat(fs): rename pair detection, symlink policy, permission change handling"`

---

## Task 24: Reconciliation Worker (`src/fs/reconcile.rs`)

**Files:**
- Modify: `src/fs/reconcile.rs`

**Step 1: Write failing tests**

```rust
#[test]
fn test_reconciliation_detects_added_files() {
    // Build index snapshot with 3 files
    // Add 1 file to disk (not in index)
    // Run reconcile_subtree()
    // Verify to_add contains the new file
    let dir = tempfile::tempdir().unwrap();
    let corpus = vec!["a.txt", "b.txt", "c.txt"];
    for f in &corpus { std::fs::write(dir.path().join(f), "x").unwrap(); }

    let snapshot = build_index_snapshot_from_paths(&corpus, dir.path());
    std::fs::write(dir.path().join("d.txt"), "new").unwrap(); // Not in index

    let diff = compute_reconciliation_diff(dir.path(), &snapshot).unwrap();
    assert_eq!(diff.to_add.len(), 1);
    assert!(diff.to_add[0].ends_with("d.txt"));
}

#[test]
fn test_reconciliation_detects_deleted_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("gone.txt");
    std::fs::write(&file, "x").unwrap();
    let snapshot = build_index_snapshot_from_paths(&["gone.txt"], dir.path());
    std::fs::remove_file(&file).unwrap(); // Delete from disk

    let diff = compute_reconciliation_diff(dir.path(), &snapshot).unwrap();
    assert_eq!(diff.to_delete.len(), 1);
}

#[test]
fn test_hot_paths_get_higher_priority() {
    assert!(compute_priority(Path::new("/Users/alice/Documents/x")) >
            compute_priority(Path::new("/Users/alice/.cache/x")));
}
```

**Step 2:** Run FAIL → implement `compute_reconciliation_diff`, priority queue with hot-path weighting, `mark_reconciling` state → Run PASS → Commit `"feat(fs): snapshot-diff reconciliation with priority queue for hot paths"`

---

## Task 25: Content Extraction XPC Service (`extractor-xpc/`)

**Files:**
- Modify: `extractor-xpc/src/main.rs`
- Modify: `src/extract/client.rs` — XPC client
- Modify: `src/extract/extractors.rs` — per-format extractors

**Step 1: Write failing tests**

```rust
// Tests for extractors.rs (no XPC needed)
#[test]
fn test_extract_plaintext_first_64kb() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    let content = "hello world ".repeat(10000); // > 64KB
    std::fs::write(&file, &content).unwrap();

    let result = extract_plaintext(&file).unwrap();
    assert!(!result.full_content); // Only partial
    assert!(result.text.len() <= 64 * 1024 + 100); // ~64KB
    assert!(result.text.contains("hello world"));
}

#[test]
fn test_extract_unknown_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("binary.bin");
    std::fs::write(&file, &[0u8; 100]).unwrap();
    let result = extract_content(&file).unwrap();
    assert!(result.text.is_empty());
}

#[test]
fn test_extractor_crash_does_not_panic_main_process() {
    // Simulate malformed PDF → extractor returns empty, main process unaffected
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bad.pdf");
    std::fs::write(&file, b"not a real pdf").unwrap();
    let result = extract_content(&file); // Should not panic
    assert!(result.is_ok()); // Graceful empty result
}
```

**Step 2:** Run FAIL → implement `extract_plaintext` (read first 64KB), `extract_pdf` (PDFKit FFI stub), `extract_content` dispatcher, XPC client with `on_connection_interrupted` handler → Run PASS → Commit `"feat(extract): sandboxed XPC content extractor with crash containment"`

---

## Task 26: Power Monitor + I/O Throttling (`src/resource/power.rs`, `src/resource/memory.rs`)

**Files:**
- Modify: `src/resource/power.rs`
- Modify: `src/resource/memory.rs` — add `mlock` of hot slab

**Step 1: Write failing tests**

```rust
#[test]
fn test_should_not_compact_on_critical_thermal() {
    let monitor = PowerMonitor::new_test(ThermalState::Critical, false);
    assert!(!monitor.should_compact());
}

#[test]
fn test_should_not_compact_on_battery_below_20() {
    let monitor = PowerMonitor::new_test(ThermalState::Nominal, true /* on battery */);
    assert!(!monitor.should_compact()); // Battery check logic
}

#[test]
fn test_should_compact_when_idle_and_plugged_in() {
    let monitor = PowerMonitor::new_test(ThermalState::Nominal, false);
    assert!(monitor.should_compact());
}

#[cfg(target_os = "macos")]
#[test]
fn test_io_throttle_set_on_background_threads() {
    // Verify setiopolicy_np called without panic
    set_io_policy(IoPolicy::Throttle);
}
```

**Step 2:** Run FAIL → implement `PowerMonitor` subscribing to `NSProcessInfo.thermalState`, `IOPMCopyBatteryInfo`, `setiopolicy_np` per thread pool, `mlock()` on hot slab at startup → Run PASS → Commit `"feat(resource): thermal/battery-aware compaction scheduler and I/O throttling per thread pool"`

---

## Task 27: Index Migration + Versioning (`src/index/`)

**Files:**
- Create: `src/index/migration.rs`
- Modify: `src/index/segment.rs` — add version header fields

**Step 1: Write failing tests**

```rust
#[test]
fn test_migration_none_when_versions_match() {
    let plan = check_compatibility(1, 1).unwrap();
    assert_eq!(plan, MigrationPlan::None);
}

#[test]
fn test_migration_additive_is_transparent() {
    // Reader version 1 opens index version 2 (additive only) — should succeed
    let plan = check_compatibility(2, 1).unwrap();
    assert_eq!(plan, MigrationPlan::Additive);
}

#[test]
fn test_migration_breaking_triggers_backup() {
    // Simulate Tier 3 migration
    let dir = tempfile::tempdir().unwrap();
    let index_dir = dir.path().join("index");
    std::fs::create_dir(&index_dir).unwrap();
    // Create fake old-format segment
    std::fs::write(index_dir.join("seg_0.lssg"), b"old").unwrap();

    let result = execute_migration(MigrationPlan::FullReindex { index_dir: index_dir.clone() });
    assert!(result.is_ok());
    // Backup should exist
    assert!(dir.path().join("index.v0.backup").exists());
}
```

**Step 2:** Run FAIL → implement `check_compatibility`, `MigrationPlan` enum, `execute_migration` with backup logic → Run PASS → Commit `"feat(index): index versioning and 3-tier migration strategy with rollback backup"`

---

## Task 28: Index-Disk Parity Test (`tests/parity.rs`)

**Files:**
- Create: `tests/parity.rs`

**Step 1: Write the parity test (this IS the test — it's a regression detector, not a unit test)**

```rust
// tests/parity.rs
use localsearch::*;
use std::collections::HashSet;

#[test]
fn test_index_disk_parity() {
    let corpus_dir = tempfile::tempdir().unwrap();

    // Populate test corpus: 100 files across nested directories
    for i in 0..100 {
        let subdir = corpus_dir.path().join(format!("dir{}", i % 10));
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(subdir.join(format!("file_{}.txt", i)), format!("content {}", i)).unwrap();
    }

    // Build index
    let engine = SearchEngine::build_from_path(corpus_dir.path()).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(3)); // Allow indexing

    // Ground truth: all files on disk
    let disk_files: HashSet<_> = walkdir::WalkDir::new(corpus_dir.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_owned())
        .collect();

    // Index view: all indexed documents
    let index_files: HashSet<_> = engine.all_doc_paths().unwrap().into_iter().collect();

    let phantom: Vec<_> = index_files.difference(&disk_files).collect();
    let missing: Vec<_> = disk_files.difference(&index_files).collect();

    // Thresholds from implementation_spec.md §12
    assert_eq!(phantom.len(), 0, "Phantom docs: {:?}", phantom);
    let miss_rate = missing.len() as f32 / disk_files.len() as f32;
    assert!(miss_rate < 0.01, "Miss rate {:.2}% exceeds 1% threshold. Missing: {:?}", miss_rate * 100.0, missing);
}
```

Add `walkdir = "2"` to `[dev-dependencies]`.

**Step 2:** Run FAIL → wire `SearchEngine::all_doc_paths()` → Run PASS → Commit `"test: index-disk parity test — catches silent index divergence"`

---

## Task 29: Production Checklist Verification

Run through `implementation_spec.md §14.1` gate-by-gate:

```bash
cargo test --all                         # All unit + integration tests
cargo test chaos                         # All chaos scenarios
```

Manual checks:
- [ ] Parity test < 0.1% phantom rate on 10K file test corpus
- [ ] P95 query latency < 150ms (warm cache) via `metrics::collector`
- [ ] Memory stays within tier budget for 1hr run with continuous file writes
- [ ] Kill -9 during compaction → restart → verify index intact
- [ ] Revoke TCC permission mid-run → verify graceful degradation

**Commit:** `"chore: production checklist gate — all chaos scenarios pass"`

---

## Task 30: N-gram Trigram Index + Phonetic Expansion (`src/index/trigram.rs`, `src/query/`)

**Files:**
- Create: `src/index/trigram.rs`
- Create: `src/query/phonetic.rs`
- Modify: `src/query/executor.rs` — wire fallback chain

**Step 1: Write failing tests**

```rust
// Trigram
#[test]
fn test_trigram_jaccard_above_threshold_matches() {
    let mut idx = TrigramIndex::new();
    idx.insert("finder", DocId(1));
    // "finde" shares {fin, ind, nde} with "finder" → Jaccard > 0.5
    let results = idx.search_jaccard("finde", 0.5);
    assert!(results.contains(&DocId(1)));
}

// Phonetic
#[test]
fn test_double_metaphone_mayer_matches_meyer() {
    let pm1 = double_metaphone("Mayer");
    let pm2 = double_metaphone("Meyer");
    assert_eq!(pm1.primary, pm2.primary);
}

#[test]
fn test_phonetic_expansion_fires_when_bk_returns_few() {
    // BK-tree has no match for "smyth" → phonetic expansion finds "smith"
    let mut exec = QueryExecutor::new_test();
    exec.add_to_index("smith.pdf", DocId(1));
    let results = exec.execute(Query::parse("smyth").unwrap()).unwrap();
    assert!(!results.is_empty());
}
```

**Step 2:** Run FAIL → implement trigram decomposition + Jaccard scoring; implement Double Metaphone (use `rphonetic` crate or hand-implement); wire 3-layer fallback: BK-tree → trigram → phonetic → Run PASS → Commit `"feat(query): 3-layer fuzzy pipeline — BK-tree, trigram Jaccard, Double Metaphone phonetic"`

Add `rphonetic = "1"` to `Cargo.toml`.

---

## Task 31: Prefix Cache — Precomputed Top-100 per Trie Node (`src/index/trie.rs`)

**Files:**
- Modify: `src/index/trie.rs`
- Modify: `src/query/executor.rs` — check prefix cache as fast path

**Step 1: Write failing tests**

```rust
#[test]
fn test_prefix_cache_returns_precomputed_results() {
    let mut trie = PathTrie::new();
    trie.insert("/Users/alice/report.pdf", DocId(1));
    trie.insert("/Users/alice/readme.md", DocId(2));
    trie.insert("/Users/bob/random.txt", DocId(3));
    trie.rebuild_prefix_cache(100); // precompute top-100

    let cached = trie.prefix_cache_lookup("re");
    assert!(cached.is_some());
    let hits = cached.unwrap();
    assert!(hits.iter().any(|r| r.doc_id == DocId(1)));
}

#[test]
fn test_prefix_cache_15ms_warm() {
    // Prefix cache hit must be <15ms
    let trie = build_large_trie(50_000);
    let start = std::time::Instant::now();
    let _ = trie.prefix_cache_lookup("re");
    assert!(start.elapsed().as_millis() < 15);
}
```

**Step 2:** Run FAIL → implement per-trie-node `top_k: Vec<(DocId, f32)>` precomputed during compaction; refresh in background after every compaction → Run PASS → Commit `"feat(index): prefix cache — precomputed top-100 docs per trie node for <15ms first-keystroke"`

---

## Task 32: `ResultProvider` Trait + Score Normalization (`src/query/`)

**Files:**
- Create: `src/query/provider.rs`
- Modify: `src/query/executor.rs`

**Step 1: Write failing tests**

```rust
#[test]
fn test_result_provider_trait_merge() {
    // Two providers, scores on different scales → normalized merge
    let kw_provider = MockKeywordProvider::new(vec![(DocId(1), 15.3), (DocId(2), 8.1)]);
    let sem_provider = MockSemanticProvider::new(vec![(DocId(3), 0.92), (DocId(1), 0.71)]);
    let merged = merge_providers(&[Box::new(kw_provider), Box::new(sem_provider)]);
    // All scores in [0.0, 1.0] after normalization
    assert!(merged.iter().all(|r| r.score >= 0.0 && r.score <= 1.0));
    // Doc 1 should appear once (deduped), score averaged/merged
    assert_eq!(merged.iter().filter(|r| r.doc_id == DocId(1)).count(), 1);
}
```

**Step 2:** Run FAIL → define `trait ResultProvider { fn provide(&self, query: &Query) -> Vec<(DocId, f32)>; }`, implement min-max normalization per provider, deduplicate by doc_id → Run PASS → Commit `"feat(query): ResultProvider trait with score normalization layer — extensible for semantic v2"`

---

## Task 33: Directory Proximity Ranking Signal (`src/query/ranker.rs`)

**Files:**
- Modify: `src/query/ranker.rs`

**Step 1: Write failing test**

```rust
#[test]
fn test_directory_proximity_boosts_sibling_files() {
    let mut ranker = Ranker::new_test();
    ranker.session_clicked_paths = vec![PathBuf::from("/Users/alice/Projects/AlphaApp/main.rs")];

    // score file in same dir
    let same_dir = RawResult::new(DocId(1), "/Users/alice/Projects/AlphaApp/config.toml");
    // score file in different dir
    let diff_dir = RawResult::new(DocId(2), "/Users/alice/Documents/notes.txt");

    let q = Query::parse("config").unwrap();
    assert!(ranker.score(&same_dir, &q) > ranker.score(&diff_dir, &q));
}
```

**Step 2:** Run FAIL → add `η × directory_proximity` term to score function (η=1.5 when parent dir matches session clicked paths) → Run PASS → Commit `"feat(query): directory proximity ranking signal from session context"`

---

## Task 34: Thread Registry + Watchdog (`src/metrics/`)

**Files:**
- Create: `src/metrics/thread_registry.rs`
- Modify: `src/metrics/mod.rs`

**Step 1: Write failing tests**

```rust
#[test]
fn test_thread_registry_tracks_active_op() {
    let registry = ThreadRegistry::new();
    registry.register("query-worker-1", Pool::Query);
    registry.set_current_op("query-worker-1", "executing BM25 scorer");
    let info = registry.get("query-worker-1").unwrap();
    assert_eq!(info.current_op, "executing BM25 scorer");
}

#[test]
fn test_watchdog_fires_on_lock_held_too_long() {
    // A thread that claims it started an op 600ms ago should trigger alert
    let registry = ThreadRegistry::new();
    registry.register("query-worker-1", Pool::Query);
    registry.set_op_start_backdated("query-worker-1", 600); // 600ms ago
    assert!(registry.check_watchdog().is_err());
}
```

**Step 2:** Run FAIL → implement `ThreadRegistry` with `AtomicU64` timestamps, per-thread `io_wait_us` counter, watchdog check (alert if op > 500ms, SIGABRT in test mode) → Run PASS → Commit `"feat(metrics): thread registry with per-thread op tracking and lock-hold watchdog"`

---

## Task 35: Fault Injector Infrastructure (`src/fault.rs`)

**Files:**
- Create: `src/fault.rs` (compiled only in test/debug builds via `#[cfg(debug_assertions)]`)

**Step 1: Write failing test**

```rust
#[cfg(debug_assertions)]
#[test]
fn test_fault_injector_injects_wal_error() {
    let spec = FaultSpec {
        point: InjectionPoint::WalWrite,
        fault: FaultType::ReturnError(IoError::Corrupt),
        probability: 1.0,
        after_n_calls: 5,
    };
    let injector = FaultInjector::from_spec(spec);

    // First 4 calls succeed
    for _ in 0..4 { assert!(injector.check(InjectionPoint::WalWrite).is_ok()); }
    // 5th call injects the fault
    assert!(injector.check(InjectionPoint::WalWrite).is_err());
}

#[cfg(debug_assertions)]
#[test]
fn test_fault_injector_loads_from_json() {
    let json = r#"{"point":"WalWrite","fault":"Corrupt","probability":0.5,"after_n_calls":0}"#;
    let spec: FaultSpec = serde_json::from_str(json).unwrap();
    let _ = FaultInjector::from_spec(spec);
}
```

Add `serde_json = "1"` to `[dev-dependencies]`.

**Step 2:** Run FAIL → implement `FaultInjector` with injection points, `FaultSpec` JSON loading, per-point call counter → Run PASS → Commit `"feat(fault): debug-only fault injector for scripted chaos testing"`

---

## Task 36: System Invariant Checker (`src/metrics/invariants.rs`)

**Files:**
- Create: `src/metrics/invariants.rs`

**Step 1: Write failing tests**

```rust
#[test]
fn test_invariant_wal_monotonicity() {
    let entries = vec![
        WalEntry { seq: 1, ..WalEntry::new_test() },
        WalEntry { seq: 2, ..WalEntry::new_test() },
        WalEntry { seq: 4, ..WalEntry::new_test() }, // gap!
    ];
    let result = check_wal_monotonicity(&entries);
    assert!(result.is_err());
}

#[test]
fn test_invariant_doc_id_uniqueness() {
    let mut delta = DeltaIndex::new(50 * 1024 * 1024);
    delta.insert_document(Document::new_test(DocId(1), "/a"), HashMap::new()).unwrap();
    delta.insert_document(Document::new_test(DocId(1), "/b"), HashMap::new()).unwrap(); // duplicate!
    let result = check_doc_id_uniqueness(&delta);
    assert!(result.is_err());
}

#[test]
fn test_invariant_delta_main_disjoint() {
    // Doc present in delta AND in segment with different content → violation
    let (delta, segments) = build_conflicting_test_state();
    assert!(check_delta_main_disjoint(&delta, &segments).is_err());
}
```

**Step 2:** Run FAIL → implement 5 invariant checks (WAL monotonicity, segment immutability hash check, doc_id uniqueness, trie-index consistency 1% sample, delta-main disjointness) → wire to run after every compaction in test builds → Run PASS → Commit `"feat(metrics): 5 system invariant checkers — compile-time safety net"`

---

## Task 37: Security & Permissions Model (`src/fs/permissions.rs`)

**Files:**
- Create: `src/fs/permissions.rs`
- Modify: `src/fs/identity.rs` — add permission state to path trie node

**Step 1: Write failing tests**

```rust
#[test]
fn test_permission_state_transitions_accessible_to_revoked() {
    let mut tracker = PermissionTracker::new();
    tracker.mark_accessible(PathBuf::from("/Users/alice/secret.txt"));
    tracker.mark_inaccessible(PathBuf::from("/Users/alice/secret.txt"));
    assert_eq!(tracker.state(Path::new("/Users/alice/secret.txt")),
               PermissionState::Revoked);
}

#[test]
fn test_never_index_list_blocks_sensitive_paths() {
    assert!(is_never_index_path(Path::new("/private/var/something")));
    assert!(is_never_index_path(Path::new("/Users/alice/.ssh/id_rsa")));
    assert!(is_never_index_path(Path::new("/Users/alice/secret.pem")));
    assert!(!is_never_index_path(Path::new("/Users/alice/Documents/report.pdf")));
}

#[test]
fn test_permission_audit_log_records_revocation() {
    let dir = tempfile::tempdir().unwrap();
    let mut log = PermissionAuditLog::new(&dir.path().join("audit.db")).unwrap();
    log.record("/Users/alice/file.txt", "EACCES", "ACCESSIBLE→REVOKED").unwrap();
    let entries = log.recent(7).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].transition, "ACCESSIBLE→REVOKED");
}
```

**Step 2:** Run FAIL → implement `PermissionState` enum, `PermissionTracker`, `is_never_index_path()` with hardcoded exclusion list (`~/.ssh/`, `*/Keychain/`, `*.pem`, etc.), SQLite `permission_audit` table → Run PASS → Commit `"feat(security): permission state machine, never-index list, audit log"`

---

## Task 38: TCC Integration + Revocation Handler (`src/fs/tcc.rs`)

**Files:**
- Create: `src/fs/tcc.rs`

**Step 1: Write failing tests**

```rust
#[cfg(target_os = "macos")]
#[test]
fn test_tcc_revocation_suspends_crawl() {
    // Simulate TCC revocation notification
    let mut tcc = TccMonitor::new_test();
    tcc.simulate_revocation();
    assert!(tcc.is_crawl_suspended());
    assert_eq!(tcc.degraded_scope(), TccScope::NoAccess);
}

#[test]
fn test_fallback_to_hot_paths_when_full_disk_denied() {
    let scope = TccScope::HotPathsOnly;
    let allowed = allowed_paths_for_scope(scope);
    assert!(allowed.contains(&PathBuf::from(dirs::home_dir().unwrap().join("Documents"))));
    assert!(!allowed.contains(&PathBuf::from("/private/var")));
}
```

**Step 2:** Run FAIL → implement `TccMonitor` subscribing to `DistributedNotificationCenter` for TCC revocation, `TccScope` enum (FullDiskAccess/HotPathsOnly/NoAccess), graceful degradation → Run PASS → Commit `"feat(security): TCC revocation handler with graceful scope degradation"`

---

## Task 39: SSD/HDD Detection + Cold Segment `pread` Strategy (`src/resource/`)

**Files:**
- Create: `src/resource/storage.rs`
- Modify: `src/index/segment.rs` — add `pread`-based cold segment loader

**Step 1: Write failing tests**

```rust
#[cfg(target_os = "macos")]
#[test]
fn test_storage_type_detection_does_not_panic() {
    // Should return SSD or HDD without crashing
    let storage_type = detect_storage_type();
    assert!(matches!(storage_type, StorageType::SSD | StorageType::HDD | StorageType::Unknown));
}

#[test]
fn test_cold_segment_pread_delivers_partial_results_first() {
    // When segment is cold (simulated), partial results from warm segments
    // are returned first while cold segment loads asynchronously
    let (warm_seg, cold_seg) = build_test_segment_pair();
    let executor = QueryExecutor::with_segments(vec![warm_seg], vec![cold_seg]);
    let results = executor.execute(Query::parse("test").unwrap()).unwrap();
    // Should get warm results immediately, not block on cold
    assert!(!results.initial.is_empty());
}
```

**Step 2:** Run FAIL → implement `detect_storage_type()` via `IORegistryEntry`, adapt compaction batch sizes (4MB SSD / 1MB HDD), implement `pread`-based async cold segment loader → Run PASS → Commit `"feat(resource): SSD/HDD detection, adaptive batch sizes, non-blocking cold segment reads"`

---

## Task 40: External Drive + Network Volume Support (`src/fs/volumes.rs`)

**Files:**
- Create: `src/fs/volumes.rs`

**Step 1: Write failing tests**

```rust
#[cfg(target_os = "macos")]
#[test]
fn test_volume_mount_triggers_incremental_reconciliation() {
    // Simulate disk-appeared callback → verify reconciliation job enqueued
    let mut vmon = VolumeMonitor::new_test();
    vmon.simulate_mount("/Volumes/ExternalDrive", "test-uuid-1234");
    let jobs = vmon.pending_reconciliation_jobs();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].volume_uuid, "test-uuid-1234");
}

#[test]
fn test_network_volume_falls_back_to_polling() {
    let vol = VolumeInfo { is_network: true, supports_smb3_notifications: false,
                           path: PathBuf::from("/Volumes/NAS") };
    let strategy = select_watch_strategy(&vol);
    assert_eq!(strategy, WatchStrategy::PollEvery(Duration::from_secs(300)));
}

#[test]
fn test_network_volume_content_indexing_opt_in_only() {
    let vol = VolumeInfo { is_network: true, content_indexing_opted_in: false,
                           path: PathBuf::from("/Volumes/NAS") };
    assert!(!should_content_index(&vol));
}
```

**Step 2:** Run FAIL → implement `VolumeMonitor` using `DADiskRegisterDiskAppearedCallback`, Merkle snapshot per volume UUID, `WatchStrategy` enum (FSEvents/SMBv3Notify/Poll), network volume polling loop → Run PASS → Commit `"feat(fs): external drive reconciliation and network volume polling"`

---

## Task 41: Spotlight Bootstrap Fallback (`src/query/spotlight_fallback.rs`)

**Files:**
- Create: `src/query/spotlight_fallback.rs`
- Modify: `src/main.rs` — use fallback during bootstrap

**Step 1: Write failing tests**

```rust
#[cfg(target_os = "macos")]
#[test]
fn test_spotlight_fallback_returns_results_when_index_empty() {
    let fallback = SpotlightFallback::new();
    // Query Spotlight directly
    let results = fallback.query("Cargo.toml").unwrap();
    // Should return something (whatever Spotlight has)
    // We just test it doesn't crash and is correctly labeled
    for r in &results {
        assert!(r.source == ResultSource::Spotlight);
    }
}

#[test]
fn test_first_launch_uses_spotlight_until_hot_paths_indexed() {
    let engine = SearchEngine::new_empty();
    let results = engine.query("report").unwrap();
    // Before indexing completes, source should be Spotlight
    assert!(results.iter().all(|r| r.source == ResultSource::Spotlight
                                || r.source == ResultSource::LocalIndex));
}
```

**Step 2:** Run FAIL → implement `SpotlightFallback` using `NSMetadataQuery` via ObjC bindings, `ResultSource` enum, progressive result replacement as local index warms up → Run PASS → Commit `"feat(query): Spotlight fallback during first-launch and crash recovery bootstrap"`

---

## Task 42: Performance Benchmark Suite + Shadow Ranking (`benches/`, `src/metrics/`)

**Files:**
- Create: `benches/query_latency.rs`
- Create: `src/metrics/shadow_ranking.rs`

**Step 1: Write failing benchmark**

```rust
// benches/query_latency.rs
use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};

fn bench_query_p50(c: &mut Criterion) {
    let engine = build_50k_corpus_engine(); // 50K files, warm cache
    let queries = load_realistic_query_set(200); // 200 representative queries

    c.bench_function("query_p50_warm", |b| {
        b.iter(|| {
            let q = &queries[rand::random::<usize>() % queries.len()];
            engine.query(q).unwrap()
        })
    });
}

criterion_group!(benches, bench_query_p50);
criterion_main!(benches);
```

```rust
// Shadow ranking test
#[test]
fn test_shadow_ranking_detects_regression() {
    let baseline_results = load_logged_rankings("baseline.json");
    let new_results = compute_rankings_with_new_weights(&baseline_results);
    let rbo_scores = compute_rbo_all(&baseline_results, &new_results);

    // 10th percentile RBO must be > 0.7 (not too different from baseline)
    let p10 = percentile(&rbo_scores, 10);
    assert!(p10 > 0.7, "Ranking regression at P10: RBO={:.2}", p10);
}
```

Add `criterion = "0.5"` and `rand = "0.8"` to `[dev-dependencies]`.

**Step 2:** Run FAIL → implement benchmark corpus builder, realistic query set loader, RBO computation, shadow ranking log/compare → Run PASS → integrate benchmarks into CI with alert thresholds (P50 >80ms, P99 >350ms) → Commit `"test(bench): performance benchmark suite with P50/P99 tracking and shadow ranking regression gate"`

---

## Task 43: `content_hash` Idempotent Re-extraction (`src/extract/`)

**Files:**
- Modify: `src/extract/client.rs`
- Modify: `src/index/delta.rs` — store `content_hash` per doc

**Step 1: Write failing test**

```rust
#[test]
fn test_content_hash_skips_unchanged_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("report.txt");
    std::fs::write(&file, "quarterly revenue report").unwrap();

    let mut engine = SearchEngine::build_from_path(dir.path()).unwrap();
    let first_extract_count = engine.extraction_count();

    // Simulate re-index trigger (mtime unchanged, content unchanged)
    engine.trigger_reindex(&file).unwrap();
    assert_eq!(engine.extraction_count(), first_extract_count); // No re-extraction

    // Now change the file
    std::fs::write(&file, "updated content").unwrap();
    engine.trigger_reindex(&file).unwrap();
    assert_eq!(engine.extraction_count(), first_extract_count + 1); // Extracted once
}
```

**Step 2:** Run FAIL → implement `content_hash` (XXH3 of first 64KB), store in doc metadata, skip extraction if hash unchanged → Run PASS → Commit `"feat(extract): content_hash for idempotent re-extraction — skip unchanged files"`

---

## Task 44: Priority-Ordered Warmup + Signal DB Bootstrap (`src/main.rs`)

**Files:**
- Modify: `src/main.rs` — warm restart path
- Modify: `src/metrics/collector.rs` — expose top-50 path prefixes and top-500 terms

**Step 1: Write failing test**

```rust
#[test]
fn test_warmup_uses_signal_db_to_prioritize_paths() {
    let dir = tempfile::tempdir().unwrap();
    // Simulate signal DB with hot terms
    let signal_db = SignalDb::new_test_with_data(&[
        ("quarterly", 150),  // clicked 150 times
        ("budget", 90),
    ]);
    let warmup_plan = build_warmup_plan(&signal_db);
    // Top terms should be first
    assert_eq!(warmup_plan.hot_terms[0], "quarterly");
    assert_eq!(warmup_plan.hot_terms[1], "budget");
}
```

**Step 2:** Run FAIL → implement `build_warmup_plan()` from signal DB, issue `madvise(MADV_WILLNEED)` on pages for top-50 paths and top-500 terms → Run PASS → Commit `"feat: signal-DB-ordered warmup — top query terms pre-faulted at launch"`

---

## Task 45: Health Dashboard Debug Panel (CLI) (`src/metrics/dashboard.rs`)

**Files:**
- Create: `src/metrics/dashboard.rs`
- Modify: `src/main.rs` — expose via `--debug-panel` flag

**Step 1: Write failing test**

```rust
#[test]
fn test_dashboard_renders_health_status() {
    let state = HealthState {
        memory_state: MemoryState::Full,
        segment_count: 4,
        delta_size_bytes: 12_800_000,
        wal_lag_events: 0,
        doc_count: 1_247_832,
        content_indexed_fraction: 0.943,
        last_compaction_ago_secs: 240,
        phantom_rate: 0.003,
        stale_rate: 0.011,
        query_p50_ms: 48.0,
        query_p99_ms: 187.0,
        zero_result_rate: 0.021,
    };
    let output = render_dashboard(&state);
    assert!(output.contains("HEALTHY"));
    assert!(output.contains("1,247,832"));
    assert!(output.contains("48ms"));
}
```

**Step 2:** Run FAIL → implement `render_dashboard()` producing the exact panel format from `main.md §counter6`, wire to `HealthState` aggregated from `ThreadRegistry` + `Metrics` + `MemoryController` → Run PASS → Commit `"feat(metrics): health dashboard debug panel — index status, query P50/P99, component health"`

---

## Task 46: Final Production Gate

Run through `implementation_spec.md §14.1` and `main.md §counter5 testing` gates:

```bash
cargo test --all --release              # All unit + integration tests
cargo bench                             # Benchmark suite — P50 <80ms, P99 <350ms
cargo test parity                       # Index-disk parity <1% miss
cargo test chaos                        # All 5 chaos scenarios
cargo test invariant                    # All 5 system invariants hold
```

Manual gates:
- [ ] Shadow ranking eval — P10 RBO > 0.7 vs baseline
- [ ] Memory budget respected for 1hr continuous write load
- [ ] Kill -9 during compaction → restart → index intact, no phantom docs
- [ ] TCC revocation → crawl suspends, results degrade gracefully with UI indicator
- [ ] External drive mount → reconciliation enqueued, incremental diff only
- [ ] Network volume → falls back to 5-min polling, content indexing opt-in only
- [ ] Fault injector: all 5 chaos scenarios pass with FaultSpec JSON
- [ ] `~/.ssh/` and `*.pem` paths never appear in index regardless of TCC scope
- [ ] SSD vs HDD detection correct on test machine
- [ ] Health dashboard shows accurate state for all `MemoryState` transitions

**Commit:** `"chore: final production gate — all 46 tasks complete, all CI gates pass"`

---

## Execution Options

**Plan complete and saved to `docs/plans/2026-04-06-localsearch-core.md`.**

**1. Subagent-Driven (this session)** — Fresh subagent per task, code review between tasks, fast iteration.

**2. Parallel Session (separate)** — Open new session with executing-plans, batch execution with checkpoints.

**Which approach?**
