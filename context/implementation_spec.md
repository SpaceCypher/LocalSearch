# LocalSearch: Production Implementation Specification

**Document Version:** 1.0  
**Target Platform:** macOS 13+ (Ventura and later)  
**Implementation Language:** Rust (core) + Swift (UI/system integration)  
**Status:** Implementation-ready specification

---

## 1. System Invariants & Guarantees

### 1.1 Correctness Guarantees

**Strong Guarantees (MUST hold):**

1. **Index-Disk Consistency**: For any file F that exists on disk and is readable:
   - F appears in query results within 2 seconds of creation/modification
   - F disappears from results within 2 seconds of deletion
   - Phantom rate (indexed but not on disk) < 0.1% at any time

2. **Query Determinism**: Given identical index state, identical queries return identical results in identical order

3. **Durability**: WAL writes are durable before acknowledgment. System crash cannot lose committed events.

4. **Atomicity**: Index updates are atomic per-file. No partial file state is ever queryable.

5. **Isolation**: Concurrent queries see a consistent snapshot. No torn reads across index structures.

**Weak Guarantees (best-effort):**

1. **Content Freshness**: File content indexed within 30 seconds for hot paths, 5 minutes for cold paths
2. **Ranking Stability**: Result order stable across queries unless click signal changes
3. **Resource Limits**: Memory usage stays within tier budget 99% of the time

### 1.2 Latency SLOs

```
Operation                P50      P95      P99      Max
─────────────────────────────────────────────────────────
Keyword query (warm)     25ms     80ms    150ms    400ms
Keyword query (cold)     60ms    200ms    400ms    1000ms
Prefix query (cached)    10ms     20ms     40ms    100ms
Semantic query (parallel)   180ms    400ms    600ms    2000ms
Index event processing      <1ms     5ms     20ms     100ms
WAL write                   <1ms     2ms      5ms      20ms
Compaction (per segment)    N/A      N/A      N/A     5000ms
```

**Latency Budget Breakdown (P95 keyword query, 80ms total):**

```
Component                    Budget    Rationale
──────────────────────────────────────────────────────────
Tokenization + parse         2ms       Simple state machine
Fuzzy expansion (BK-tree)    8ms       Tree traversal, edit distance
Inverted index lookup        35ms      Posting list merge, BM25 scoring
Delta index scan             5ms       Small, always in RAM
Scope bitmap AND             2ms       Roaring bitmap intersection
Ranking + rerank             15ms      Score computation, top-K heap
Snippet extraction           10ms      Position offset lookup
IPC overhead                 3ms       Query thread → UI thread
```

### 1.3 Freshness Guarantees

**Metadata Freshness (strong):**
- FSEvents → WAL: <100ms (kernel delivery)
- WAL → Delta Index: <500ms (processing latency)
- Delta Index → Queryable: <1000ms (merge at query time)
- **Total: <2s from disk write to query result**

**Content Freshness (eventual):**
- Hot paths (Desktop, Documents, Downloads): <30s
- Warm paths (recently queried): <5min
- Cold paths (deep directories): <1hr
- Network volumes: <5min (if mounted), polling-based

**Staleness Detection:**
Every index entry carries `metadata_indexed_at` and `content_indexed_at` timestamps. Query results include staleness indicator if `now() - content_indexed_at > 5min`.

### 1.4 Failure Handling Principles

**Principle 1: Fail Visible, Not Silent**
- Degraded states are surfaced to UI with specific reason
- Never return stale results without disclosure
- Errors are logged with full context (path, operation, errno)

**Principle 2: Isolate Blast Radius**
- Content extraction crashes contained to XPC process
- Index corruption affects only corrupted segment, not entire index
- Permission failures affect only inaccessible paths

**Principle 3: Graceful Degradation**
- Memory pressure → evict optional components (HNSW, suffix array)
- Thermal pressure → suspend compaction, continue queries
- I/O contention → serve partial results from warm data

**Principle 4: Automatic Recovery**
- WAL replay on crash
- Incremental reconciliation on FSEvents overflow
- Self-healing via periodic integrity checks

**Principle 5: No Data Loss**
- WAL is source of truth
- Segments are immutable after finalization
- Deletes are tombstones, not immediate removal

---

## 2. Data Model & Storage Engine

### 2.1 File Identity Model

**Problem**: macOS file identity is complex. Inodes are reused. Paths change on rename. Volumes mount/unmount.

**Solution**: Composite identity with stable doc_id allocation.

```rust
/// Stable document identifier, never reused
#[repr(transparent)]
struct DocId(u64);

/// Filesystem identity tuple (not stable across renames)
#[repr(C)]
struct FileIdentity {
    volume_uuid: u128,        // APFS volume UUID (stable)
    inode: u64,               // inode number (reused after delete)
    generation: u32,          // inode generation counter
    device_id: u32,           // st_dev from stat()
}

/// Mapping table: FileIdentity → DocId
/// Stored in: identity.db (SQLite)
/// Schema:
///   CREATE TABLE identity_map (
///     volume_uuid BLOB NOT NULL,
///     inode INTEGER NOT NULL,
///     generation INTEGER NOT NULL,
///     device_id INTEGER NOT NULL,
///     doc_id INTEGER PRIMARY KEY,
///     created_at INTEGER NOT NULL,
///     PRIMARY KEY (volume_uuid, inode, generation, device_id)
///   );
///   CREATE INDEX idx_lookup ON identity_map(volume_uuid, inode, generation);

/// DocId allocation strategy:
/// - Monotonically increasing counter
/// - Persisted in identity.db metadata table
/// - Never reused, even after file deletion
/// - 64-bit space = 18 quintillion IDs (never exhausted)
```

**Identity Resolution Algorithm:**

```rust
fn resolve_doc_id(path: &Path) -> Result<DocId> {
    let stat = fs::metadata(path)?;
    let identity = FileIdentity {
        volume_uuid: get_volume_uuid(stat.st_dev)?,
        inode: stat.st_ino,
        generation: stat.st_gen,
        device_id: stat.st_dev,
    };
    
    // Lookup in identity_map
    if let Some(doc_id) = identity_db.lookup(&identity)? {
        return Ok(doc_id);
    }
    
    // Allocate new DocId
    let doc_id = identity_db.allocate_next_id()?;
    identity_db.insert(identity, doc_id)?;
    Ok(doc_id)
}
```

**Rename Handling:**
- Rename does NOT change DocId
- Inode stays same, path changes
- Index update: modify path field, keep DocId
- Identity map unchanged

**Inode Reuse Detection:**
- After delete, inode may be reused for new file
- `generation` counter increments on reuse (APFS guarantees this)
- New file gets new DocId even if inode matches

### 2.2 WAL Format (Write-Ahead Log)

**Purpose**: Durable, ordered log of all filesystem events. Source of truth for index state.

**File Layout:**

```
WAL file structure:
┌─────────────────────────────────────┐
│ Header (4096 bytes, page-aligned)   │
├─────────────────────────────────────┤
│ Entry 0                             │
│ Entry 1                             │
│ ...                                 │
│ Entry N                             │
├─────────────────────────────────────┤
│ Checkpoint Record                   │
└─────────────────────────────────────┘
```

**Header Format:**

```rust
#[repr(C, packed)]
struct WalHeader {
    magic: u32,              // 0x4C53_5741 ("LSWA")
    format_version: u16,     // Current: 1
    min_reader_version: u16, // Minimum version that can read this WAL
    flags: u32,              // Reserved
    page_size: u32,          // 4096
    first_seq: u64,          // Sequence number of first entry
    last_seq: u64,           // Sequence number of last entry (updated on append)
    checkpoint_seq: u64,     // Last checkpointed sequence number
    created_at: u64,         // Unix timestamp (microseconds)
    writer_pid: u32,         // PID of writer process
    reserved: [u8; 4040],    // Pad to 4096 bytes
    checksum: u64,           // XXHash3 of header (excluding this field)
}
```

**Entry Format:**

```rust
#[repr(C, packed)]
struct WalEntry {
    seq: u64,                // Monotonic sequence number
    timestamp_us: u64,       // Event timestamp (microseconds since epoch)
    event_type: u8,          // EventType enum
    flags: u8,               // Entry flags
    path_len: u16,           // Length of path in bytes
    doc_id: u64,             // Resolved DocId
    inode: u64,              // Inode number
    volume_uuid: u128,       // Volume UUID
    mtime_ns: u64,           // Modification time (nanoseconds)
    size: u64,               // File size in bytes
    mode: u32,               // File mode (permissions)
    uid: u32,                // Owner UID
    gid: u32,                // Owner GID
    reserved: u32,           // Padding
    path: [u8; path_len],    // UTF-8 encoded path (variable length)
    checksum: u64,           // XXHash3 of entire entry (excluding this field)
}

// Total entry size: 96 + path_len + 8 bytes
// Entries are NOT page-aligned (packed sequentially)
```

**EventType Enum:**

```rust
#[repr(u8)]
enum EventType {
    Created = 1,
    Modified = 2,
    Deleted = 3,
    Renamed = 4,        // Two entries: old path (Deleted) + new path (Created)
    PermissionChanged = 5,
    MetadataChanged = 6,
    Checkpoint = 255,   // Special: marks a checkpoint boundary
}
```

**Write Path:**

```rust
impl WalWriter {
    fn append(&mut self, entry: WalEntry) -> Result<()> {
        // 1. Serialize entry
        let mut buf = Vec::with_capacity(entry.size());
        entry.serialize(&mut buf)?;
        
        // 2. Write to file with O_DSYNC (data sync, not metadata)
        self.file.write_all(&buf)?;
        
        // 3. Update header.last_seq (in-memory)
        self.header.last_seq = entry.seq;
        
        // 4. Every 256 entries, fsync() and write checkpoint
        if entry.seq % 256 == 0 {
            self.file.sync_data()?;  // fsync()
            self.write_checkpoint(entry.seq)?;
        }
        
        Ok(())
    }
    
    fn write_checkpoint(&mut self, seq: u64) -> Result<()> {
        // Update header on disk
        self.header.checkpoint_seq = seq;
        self.header.checksum = self.header.compute_checksum();
        
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(self.header.as_bytes())?;
        self.file.sync_all()?;  // fsync() including metadata
        
        Ok(())
    }
}
```

**Read Path (Replay):**

```rust
impl WalReader {
    fn replay_from(&mut self, start_seq: u64) -> Result<Vec<WalEntry>> {
        let mut entries = Vec::new();
        self.file.seek(SeekFrom::Start(4096))?; // Skip header
        
        loop {
            let entry = match self.read_entry()? {
                Some(e) if e.seq >= start_seq => e,
                Some(_) => continue,  // Skip entries before start_seq
                None => break,        // EOF
            };
            
            // Verify checksum
            if !entry.verify_checksum() {
                return Err(Error::CorruptEntry(entry.seq));
            }
            
            entries.push(entry);
        }
        
        Ok(entries)
    }
}
```

**Crash Recovery:**

```rust
fn recover_wal() -> Result<u64> {
    let header = read_wal_header()?;
    let last_checkpoint = header.checkpoint_seq;
    
    // Replay from last checkpoint
    let entries = WalReader::new()?.replay_from(last_checkpoint)?;
    
    // Apply entries to delta index
    for entry in entries {
        delta_index.apply(entry)?;
    }
    
    Ok(header.last_seq)
}
```

**Durability Guarantee:**
- `O_DSYNC` on writes: data reaches disk before `write()` returns
- `fsync()` every 256 entries: checkpoint boundary
- Crash loses at most 255 uncommitted entries
- Those entries will be re-delivered by FSEvents on next stream start

### 2.3 Segment Format (Immutable Index Segments)

**Purpose**: Immutable, compressed, on-disk representation of inverted index.

**File Layout:**

```
Segment file structure:
┌─────────────────────────────────────┐
│ Segment Header (4096 bytes)         │
├─────────────────────────────────────┤
│ Term Dictionary (compressed)        │
├─────────────────────────────────────┤
│ Posting Lists (compressed)          │
├─────────────────────────────────────┤
│ Document Metadata (compressed)      │
├─────────────────────────────────────┤
│ Footer (offsets + checksum)         │
└─────────────────────────────────────┘
```

**Segment Header:**

```rust
#[repr(C, packed)]
struct SegmentHeader {
    magic: u32,              // 0x4C53_5347 ("LSSG")
    format_version: u16,
    min_reader_version: u16,
    flags: u32,
    doc_count: u32,          // Number of documents in this segment
    term_count: u32,         // Number of unique terms
    created_at: u64,
    min_doc_id: u64,         // Smallest DocId in segment
    max_doc_id: u64,         // Largest DocId in segment
    compression: u8,         // CompressionType enum
    reserved: [u8; 4039],
    checksum: u64,
}
```

**Term Dictionary Format:**

```
Term dictionary is a sorted array of (term, posting_offset) pairs.
Binary searchable for O(log n) term lookup.

Entry format:
┌──────────────────────────────────┐
│ term_len: u16                    │
│ term_bytes: [u8; term_len]       │
│ posting_offset: u64              │ ← offset into posting lists section
│ posting_len: u32                 │ ← length of posting list in bytes
│ doc_freq: u32                    │ ← number of docs containing this term
└──────────────────────────────────┘
```

**Posting List Format:**

```rust
// Posting list for a single term
// Compressed using delta encoding + varint
struct PostingList {
    doc_ids: Vec<DocId>,         // Delta-encoded, varint-compressed
    term_freqs: Vec<u32>,        // Parallel array, varint-compressed
    field_masks: Vec<u8>,        // Parallel array, which fields term appears in
}

// Field mask bits:
const FIELD_FILENAME: u8 = 1 << 0;
const FIELD_PATH: u8 = 1 << 1;
const FIELD_CONTENT: u8 = 1 << 2;
const FIELD_TAGS: u8 = 1 << 3;
```

**Document Metadata Format:**

```rust
#[repr(C, packed)]
struct DocMetadata {
    doc_id: u64,
    path_offset: u32,        // Offset into string table
    path_len: u16,
    filename_offset: u32,
    filename_len: u16,
    size: u64,
    mtime_ns: u64,
    mode: u32,
    content_hash: u64,       // SHA256 truncated to 64 bits
    content_indexed_at: u64,
    metadata_indexed_at: u64,
}
```

**Compression Strategy:**

- Term dictionary: LZ4 (fast decompression, moderate ratio)
- Posting lists: Delta encoding + varint (optimal for sorted integers)
- Document metadata: Zstd level 3 (good ratio, acceptable speed)
- String table (paths): Zstd level 5 (high redundancy in paths)

**Read Path:**

```rust
impl Segment {
    fn lookup_term(&self, term: &str) -> Result<Option<PostingList>> {
        // 1. Binary search term dictionary (mmap'd, no decompression needed)
        let dict_entry = self.term_dict.binary_search(term)?;
        
        // 2. Read compressed posting list from disk
        let compressed = self.read_at(dict_entry.posting_offset, dict_entry.posting_len)?;
        
        // 3. Decompress
        let posting_list = PostingList::decode(&compressed)?;
        
        Ok(Some(posting_list))
    }
    
    fn get_doc_metadata(&self, doc_id: DocId) -> Result<DocMetadata> {
        // Binary search doc metadata array (sorted by doc_id)
        let metadata = self.doc_metadata.binary_search_by_key(&doc_id, |m| m.doc_id)?;
        Ok(metadata)
    }
}
```

**Write Path (Compaction):**

```rust
fn compact_segments(segments: &[Segment]) -> Result<Segment> {
    let mut builder = SegmentBuilder::new();
    
    // Merge posting lists for each term
    let all_terms = merge_term_dicts(segments);
    
    for term in all_terms {
        let mut merged_postings = PostingList::new();
        
        for segment in segments {
            if let Some(postings) = segment.lookup_term(&term)? {
                merged_postings.merge(postings);
            }
        }
        
        builder.add_term(term, merged_postings);
    }
    
    // Write to temp file, then rename atomically
    let temp_path = builder.finalize_to_temp()?;
    fs::rename(temp_path, final_path)?;
    
    Ok(Segment::open(final_path)?)
}
```

### 2.4 Memory Layout & mmap Strategy

**Hot Slab (mlock'd, never evicted):**

```
Core inverted index hot slab: ~60MB
├─ Term dictionary (top 10K terms)      20MB
├─ Posting lists (top 10K terms)        30MB
└─ Path trie (root + 3 levels)          10MB

Allocated at startup, mlock()'d to prevent paging.
```

**Warm Data (mmap'd, OS-managed):**

```
├─ Full term dictionary                 ~100MB
├─ Full posting lists                   ~500MB
├─ Document metadata                    ~200MB
└─ String table (paths)                 ~150MB

mmap()'d with MADV_RANDOM (no prefetch).
OS manages page residency.
```

**Cold Data (on-demand pread):**

```
├─ Suffix array                         ~200MB
├─ BK-tree                              ~50MB
└─ Old segments (not in active set)     Variable

Read via pread() on background threads.
Never blocks query thread.
```

**Memory Pressure Response:**

```rust
enum MemoryState {
    Full,       // All components in RAM
    Reduced,    // BK-tree evicted
    Minimal,    // BK-tree + trigram evicted
    Critical,   // Only hot slab + delta index
}

fn handle_memory_pressure(level: MemoryPressureLevel) {
    match level {
        MemoryPressureLevel::Warning => {
            evict_component(Component::HnswIndex);
            state.store(MemoryState::Reduced);
        }
        MemoryPressureLevel::Critical => {
            evict_component(Component::BkTree);
            evict_component(Component::TrigramIndex);
            flush_delta_to_temp_segment();
            state.store(MemoryState::Minimal);
        }
    }
}
```

### 2.5 Index Structures

**Inverted Index (Core):**

```rust
struct InvertedIndex {
    // Immutable segments (LSM-style)
    segments: Arc<RwLock<Vec<Segment>>>,
    
    // Hot slab (mlock'd)
    hot_terms: HashMap<String, PostingList>,
    
    // Statistics for BM25
    total_docs: u64,
    avg_doc_length: f32,
    term_doc_freqs: HashMap<String, u32>,
}
```

**Path Trie (Scope Resolution):**

```rust
struct PathTrie {
    root: TrieNode,
}

struct TrieNode {
    component: String,           // Path component (e.g., "Documents")
    doc_ids: RoaringBitmap,      // All docs under this path
    children: HashMap<String, Box<TrieNode>>,
}

impl PathTrie {
    fn scope_query(&self, prefix: &Path) -> RoaringBitmap {
        let mut node = &self.root;
        
        for component in prefix.components() {
            node = match node.children.get(component.as_os_str().to_str()?) {
                Some(n) => n,
                None => return RoaringBitmap::new(), // Empty result
            };
        }
        
        node.doc_ids.clone()
    }
}
```

**Roaring Bitmap (DocId Sets):**

```rust
// Use roaring-rs crate
// Provides compressed bitmap with fast set operations
use roaring::RoaringBitmap;

fn intersect_postings(lists: Vec<PostingList>) -> RoaringBitmap {
    let mut result = RoaringBitmap::from_iter(lists[0].doc_ids.iter());
    
    for list in &lists[1..] {
        let bitmap = RoaringBitmap::from_iter(list.doc_ids.iter());
        result &= bitmap;
    }
    
    result
}
```

**BK-Tree (Fuzzy Matching):**

```rust
struct BkTree {
    root: Option<Box<BkNode>>,
}

struct BkNode {
    term: String,
    children: HashMap<u32, Box<BkNode>>,  // Key: edit distance
}

impl BkTree {
    fn search(&self, query: &str, max_distance: u32) -> Vec<String> {
        let mut results = Vec::new();
        self.search_recursive(&self.root, query, max_distance, &mut results);
        results
    }
    
    fn search_recursive(&self, node: &Option<Box<BkNode>>, query: &str, 
                        max_dist: u32, results: &mut Vec<String>) {
        let node = match node {
            Some(n) => n,
            None => return,
        };
        
        let dist = damerau_levenshtein(&node.term, query);
        
        if dist <= max_dist {
            results.push(node.term.clone());
        }
        
        // Prune search space using triangle inequality
        let min_child_dist = dist.saturating_sub(max_dist);
        let max_child_dist = dist + max_dist;
        
        for (child_dist, child) in &node.children {
            if *child_dist >= min_child_dist && *child_dist <= max_child_dist {
                self.search_recursive(&Some(child.clone()), query, max_dist, results);
            }
        }
    }
}
```

**Suffix Array (Substring Search):**

```rust
struct SuffixArray {
    // Sorted array of (suffix_start_pos, doc_id) pairs
    suffixes: Vec<(u32, DocId)>,
    
    // Concatenated string of all filenames
    text: String,
}

impl SuffixArray {
    fn search_substring(&self, query: &str) -> Vec<DocId> {
        // Binary search for first suffix starting with query
        let start = self.suffixes.binary_search_by(|(pos, _)| {
            self.text[*pos as usize..].cmp(query)
        }).unwrap_or_else(|e| e);
        
        // Collect all matching suffixes
        let mut results = Vec::new();
        for (pos, doc_id) in &self.suffixes[start..] {
            if !self.text[*pos as usize..].starts_with(query) {
                break;
            }
            results.push(*doc_id);
        }
        
        results
    }
}
```

---

## 3. Filesystem Integration Layer

### 3.1 FSEvents Pipeline

**FSEvents Configuration:**

```rust
use fsevent_sys::*;

fn create_event_stream() -> Result<FSEventStreamRef> {
    let paths = vec!["/Users"];  // Watch all user files
    
    let stream = unsafe {
        FSEventStreamCreate(
            kCFAllocatorDefault,
            event_callback,
            ptr::null_mut(),
            paths.as_ptr(),
            kFSEventStreamEventIdSinceNow,
            0.1,  // 100ms latency
            kFSEventStreamCreateFlagFileEvents |  // File-level events
            kFSEventStreamCreateFlagNoDefer |     // Don't coalesce
            kFSEventStreamCreateFlagWatchRoot     // Watch mount/unmount
        )
    };
    
    Ok(stream)
}
```

**Event Callback:**

```rust
extern "C" fn event_callback(
    stream: FSEventStreamRef,
    context: *mut c_void,
    num_events: usize,
    event_paths: *mut c_void,
    event_flags: *const FSEventStreamEventFlags,
    event_ids: *const FSEventStreamEventId,
) {
    let paths = unsafe {
        slice::from_raw_parts(event_paths as *const *const c_char, num_events)
    };
    let flags = unsafe {
        slice::from_raw_parts(event_flags, num_events)
    };
    
    for i in 0..num_events {
        let path = unsafe { CStr::from_ptr(paths[i]) }.to_str().unwrap();
        let flag = flags[i];
        
        // Check for overflow flag
        if flag & kFSEventStreamEventFlagMustScanSubDirs != 0 {
            enqueue_reconciliation(path);
            continue;
        }
        
        // Normal event processing
        let event_type = classify_event(flag);
        enqueue_wal_write(path, event_type);
    }
}
```

### 3.2 Event Deduplication

**Problem**: FSEvents can deliver duplicate events (same path, same operation, within 1 second).

**Solution**: Deduplication cache with time-based expiry.

```rust
struct EventDeduplicator {
    // Key: (path_hash, event_type), Value: timestamp
    seen: HashMap<(u64, EventType), Instant>,
    
    // Expiry: 2 seconds
    expiry: Duration,
}

impl EventDeduplicator {
    fn is_duplicate(&mut self, path: &Path, event_type: EventType) -> bool {
        let key = (hash_path(path), event_type);
        let now = Instant::now();
        
        if let Some(last_seen) = self.seen.get(&key) {
            if now.duration_since(*last_seen) < self.expiry {
                return true;  // Duplicate
            }
        }
        
        self.seen.insert(key, now);
        false
    }
    
    // Periodic cleanup of expired entries
    fn cleanup(&mut self) {
        let now = Instant::now();
        self.seen.retain(|_, timestamp| {
            now.duration_since(*timestamp) < self.expiry
        });
    }
}
```

### 3.3 WAL Ingestion Pipeline

```rust
struct WalIngestionPipeline {
    // FSEvents → WAL writer
    event_queue: crossbeam::channel::Receiver<FsEvent>,
    wal_writer: WalWriter,
    deduplicator: EventDeduplicator,
    identity_db: IdentityDb,
}

impl WalIngestionPipeline {
    fn run(&mut self) {
        loop {
            let event = self.event_queue.recv().unwrap();
            
            // 1. Deduplicate
            if self.deduplicator.is_duplicate(&event.path, event.event_type) {
                continue;
            }
            
            // 2. Resolve DocId
            let doc_id = match self.identity_db.resolve_doc_id(&event.path) {
                Ok(id) => id,
                Err(e) => {
                    log::warn!("Failed to resolve doc_id for {:?}: {}", event.path, e);
                    continue;
                }
            };
            
            // 3. Stat file for metadata
            let metadata = match fs::metadata(&event.path) {
                Ok(m) => m,
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    // File deleted between event and stat
                    self.write_delete_entry(doc_id)?;
                    continue;
                }
                Err(e) => {
                    log::warn!("Failed to stat {:?}: {}", event.path, e);
                    continue;
                }
            };
            
            // 4. Build WAL entry
            let entry = WalEntry {
                seq: self.wal_writer.next_seq(),
                timestamp_us: SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros() as u64,
                event_type: event.event_type,
                doc_id,
                inode: metadata.ino(),
                volume_uuid: get_volume_uuid(metadata.dev())?,
                mtime_ns: metadata.mtime_nsec() as u64,
                size: metadata.len(),
                mode: metadata.mode(),
                path: event.path.to_string_lossy().into_owned(),
                // ... other fields
            };
            
            // 5. Write to WAL (durable)
            self.wal_writer.append(entry)?;
        }
    }
}
```

### 3.4 Handling MUST_SCAN_SUBDIRS

**Trigger**: `kFSEventStreamEventFlagMustScanSubDirs` flag set.

**Meaning**: FSEvents queue overflowed. Events were dropped. Must reconcile.

**Algorithm**:

```rust
struct ReconciliationJob {
    path: PathBuf,
    priority: u32,
}

fn enqueue_reconciliation(path: &Path) {
    let priority = compute_priority(path);
    
    RECONCILIATION_QUEUE.push(ReconciliationJob {
        path: path.to_owned(),
        priority,
    });
}

fn compute_priority(path: &Path) -> u32 {
    let mut priority = 100;
    
    // Hot paths get highest priority
    if path.starts_with("/Users/*/Desktop") ||
       path.starts_with("/Users/*/Documents") ||
       path.starts_with("/Users/*/Downloads") {
        priority += 200;
    }
    
    // Recently queried paths
    if was_queried_recently(path) {
        priority += 100;
    }
    
    // Shallow paths (less work)
    let depth = path.components().count();
    priority += (10 - depth.min(10)) as u32 * 10;
    
    priority
}
```

**Reconciliation Worker:**

```rust
fn reconcile_subtree(root: &Path) -> Result<()> {
    // 1. Mark subtree as reconciling
    mark_reconciling(root);
    
    // 2. Build snapshot of current index state
    let index_snapshot = build_index_snapshot(root)?;
    
    // 3. Walk filesystem
    let mut disk_snapshot = HashMap::new();
    
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        let path = entry.path();
        
        let metadata = match fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                mark_permission_denied(path);
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        
        disk_snapshot.insert(
            path.to_owned(),
            (metadata.ino(), metadata.mtime()),
        );
    }
    
    // 4. Compute diff
    let mut to_add = Vec::new();
    let mut to_update = Vec::new();
    let mut to_delete = Vec::new();
    
    for (path, (inode, mtime)) in &disk_snapshot {
        match index_snapshot.get(path) {
            Some((idx_inode, idx_mtime)) if inode == idx_inode && mtime == idx_mtime => {
                // Unchanged
            }
            Some(_) => {
                to_update.push(path.clone());
            }
            None => {
                to_add.push(path.clone());
            }
        }
    }
    
    for path in index_snapshot.keys() {
        if !disk_snapshot.contains_key(path) {
            to_delete.push(path.clone());
        }
    }
    
    // 5. Apply changes
    for path in to_add {
        enqueue_index_event(path, EventType::Created);
    }
    for path in to_update {
        enqueue_index_event(path, EventType::Modified);
    }
    for path in to_delete {
        enqueue_index_event(path, EventType::Deleted);
    }
    
    // 6. Unmark reconciling
    unmark_reconciling(root);
    
    Ok(())
}
```

### 3.5 Edge Cases

**Rename Handling:**

```rust
// FSEvents delivers rename as two events:
// 1. kFSEventStreamEventFlagItemRenamed on old path
// 2. kFSEventStreamEventFlagItemRenamed on new path
// Both have same event_id

fn handle_rename(old_path: &Path, new_path: &Path, doc_id: DocId) -> Result<()> {
    // 1. Update path in index (DocId stays same)
    index.update_path(doc_id, new_path)?;
    
    // 2. Update path trie
    path_trie.remove(old_path, doc_id)?;
    path_trie.insert(new_path, doc_id)?;
    
    // 3. Write WAL entries
    wal.append(WalEntry {
        event_type: EventType::Deleted,
        path: old_path.to_owned(),
        doc_id,
        // ...
    })?;
    
    wal.append(WalEntry {
        event_type: EventType::Created,
        path: new_path.to_owned(),
        doc_id,
        // ...
    })?;
    
    Ok(())
}
```

**Symlink Handling:**

```rust
// Policy: Index symlink metadata, not target
// Rationale: User searches for symlink name, not target

fn should_follow_symlink(path: &Path) -> bool {
    false  // Never follow
}

fn index_symlink(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;  // Don't follow
    
    // Index as regular file with special flag
    let entry = WalEntry {
        event_type: EventType::Created,
        flags: FLAG_SYMLINK,
        // ...
    };
    
    wal.append(entry)?;
    Ok(())
}
```

**Permission Changes:**

```rust
fn handle_permission_change(path: &Path, doc_id: DocId) -> Result<()> {
    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
            // Lost access
            mark_inaccessible(doc_id)?;
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    
    // Update permission state
    update_permission_state(doc_id, metadata.mode())?;
    
    Ok(())
}
```

---

## 4. Indexing Pipeline

### 4.1 Event → Index Transformation

```rust
struct IndexingPipeline {
    wal_reader: WalReader,
    delta_index: Arc<RwLock<DeltaIndex>>,
    content_queue: crossbeam::channel::Sender<ContentExtractionJob>,
    tokenizer: Tokenizer,
}

impl IndexingPipeline {
    fn process_event(&mut self, entry: WalEntry) -> Result<()> {
        match entry.event_type {
            EventType::Created | EventType::Modified => {
                self.index_file(entry)?;
            }
            EventType::Deleted => {
                self.delete_file(entry)?;
            }
            EventType::Renamed => {
                // Handled as delete + create pair
            }
            _ => {}
        }
        
        Ok(())
    }
    
    fn index_file(&mut self, entry: WalEntry) -> Result<()> {
        // 1. Extract metadata
        let doc = Document {
            doc_id: entry.doc_id,
            path: entry.path.clone(),
            filename: Path::new(&entry.path).file_name().unwrap().to_str().unwrap(),
            size: entry.size,
            mtime_ns: entry.mtime_ns,
            metadata_indexed_at: entry.timestamp_us,
            content_indexed_at: 0,  // Not yet
        };
        
        // 2. Tokenize filename and path
        let filename_tokens = self.tokenizer.tokenize(doc.filename);
        let path_tokens = self.tokenizer.tokenize(&doc.path);
        
        // 3. Build posting lists
        let mut postings = HashMap::new();
        
        for token in filename_tokens {
            postings.entry(token.term.clone())
                .or_insert_with(|| Posting::new(doc.doc_id))
                .add_occurrence(FIELD_FILENAME, token.position);
        }
        
        for token in path_tokens {
            postings.entry(token.term.clone())
                .or_insert_with(|| Posting::new(doc.doc_id))
                .add_occurrence(FIELD_PATH, token.position);
        }
        
        // 4. Write to delta index
        let mut delta = self.delta_index.write().unwrap();
        delta.insert_document(doc, postings)?;
        
        // 5. Enqueue content extraction (async)
        if should_extract_content(&entry.path) {
            self.content_queue.send(ContentExtractionJob {
                doc_id: entry.doc_id,
                path: entry.path.clone(),
                priority: compute_content_priority(&entry.path),
            })?;
        }
        
        Ok(())
    }
    
    fn delete_file(&mut self, entry: WalEntry) -> Result<()> {
        let mut delta = self.delta_index.write().unwrap();
        delta.delete_document(entry.doc_id)?;
        Ok(())
    }
}
```

### 4.2 Delta Index Design

**Purpose**: Small, in-memory index holding recent changes. Merged with main index at query time.

```rust
struct DeltaIndex {
    // Inverted index (term → posting list)
    terms: HashMap<String, DeltaPostingList>,
    
    // Document metadata
    docs: HashMap<DocId, Document>,
    
    // Deleted docs (tombstones)
    deleted: HashSet<DocId>,
    
    // Size tracking
    size_bytes: usize,
    max_size_bytes: usize,
}

struct DeltaPostingList {
    postings: Vec<Posting>,
}

struct Posting {
    doc_id: DocId,
    term_freq: u32,
    field_mask: u8,
    positions: Vec<u32>,  // Token positions for snippet extraction
}

impl DeltaIndex {
    fn insert_document(&mut self, doc: Document, postings: HashMap<String, Posting>) -> Result<()> {
        // 1. Add document
        self.docs.insert(doc.doc_id, doc);
        
        // 2. Add postings
        for (term, posting) in postings {
            self.terms.entry(term)
                .or_insert_with(DeltaPostingList::new)
                .postings.push(posting);
        }
        
        // 3. Update size
        self.size_bytes += self.estimate_size(&postings);
        
        // 4. Check if flush needed
        if self.size_bytes > self.max_size_bytes {
            // Signal compaction needed
            COMPACTION_TRIGGER.notify_one();
        }
        
        Ok(())
    }
    
    fn delete_document(&mut self, doc_id: DocId) -> Result<()> {
        self.deleted.insert(doc_id);
        self.docs.remove(&doc_id);
        Ok(())
    }
    
    fn lookup(&self, term: &str) -> Option<&DeltaPostingList> {
        self.terms.get(term)
    }
}
```

### 4.3 Compaction Strategy

**Trigger Conditions:**

```rust
enum CompactionTrigger {
    DeltaSizeExceeded,      // Delta > 200MB
    SegmentCountHigh,       // > 10 segments
    ScheduledMaintenance,   // Every 6 hours during idle
    UserIdle,               // User idle > 5 min
}

fn should_compact() -> Option<CompactionTrigger> {
    if delta_index.size_bytes() > 200 * 1024 * 1024 {
        return Some(CompactionTrigger::DeltaSizeExceeded);
    }
    
    if segment_count() > 10 {
        return Some(CompactionTrigger::SegmentCountHigh);
    }
    
    if user_idle_duration() > Duration::from_secs(300) {
        return Some(CompactionTrigger::UserIdle);
    }
    
    None
}
```

**Compaction Types:**

```rust
enum CompactionType {
    Micro,   // Delta → temp segment (fast, 200ms)
    Minor,   // Merge 2-3 small segments (medium, 2-5s)
    Major,   // Merge all segments (slow, 10-60s)
}

fn select_compaction_type(trigger: CompactionTrigger) -> CompactionType {
    match trigger {
        CompactionTrigger::DeltaSizeExceeded => CompactionType::Micro,
        CompactionTrigger::SegmentCountHigh => CompactionType::Minor,
        CompactionTrigger::UserIdle => CompactionType::Major,
        _ => CompactionType::Micro,
    }
}
```

**Micro-Compaction (Critical Path):**

```rust
fn micro_compact() -> Result<()> {
    // 1. Acquire read lock on delta (allows concurrent queries)
    let delta = DELTA_INDEX.read().unwrap();
    
    // 2. Serialize delta to temp segment
    let temp_segment = SegmentBuilder::from_delta(&delta)?;
    
    // 3. Write to disk
    let segment_path = temp_segment.finalize()?;
    
    // 4. Atomically add to segment set
    let mut segments = SEGMENT_SET.write().unwrap();
    segments.push(Segment::open(segment_path)?);
    
    // 5. Clear delta (acquire write lock briefly)
    drop(delta);
    let mut delta_write = DELTA_INDEX.write().unwrap();
    delta_write.clear();
    
    Ok(())
}
```

**Major Compaction:**

```rust
fn major_compact() -> Result<()> {
    // 1. Read all segments
    let segments = SEGMENT_SET.read().unwrap().clone();
    
    // 2. Merge in background (no locks held)
    let merged = merge_segments(&segments)?;
    
    // 3. Atomically swap segment set
    let mut segment_set = SEGMENT_SET.write().unwrap();
    *segment_set = vec![merged];
    
    // 4. Delete old segments (after ensuring no readers)
    for old_segment in segments {
        old_segment.delete_when_unreferenced()?;
    }
    
    Ok(())
}
```

### 4.4 Content Extraction Pipeline

**XPC Service Architecture:**

```
Main Process                    XPC Service (com.localsearch.extractor)
─────────────────────────────────────────────────────────────────────
Query Engine                    Content Extractor Workers
Indexing Pipeline    ←──────→   (sandboxed, read-only file access)
                     XPC IPC
```

**XPC Interface:**

```rust
// Main process side
struct ContentExtractionClient {
    connection: xpc::Connection,
    pending: HashMap<u64, oneshot::Sender<ExtractedContent>>,
}

impl ContentExtractionClient {
    fn extract(&mut self, job: ContentExtractionJob) -> Result<ExtractedContent> {
        let (tx, rx) = oneshot::channel();
        let request_id = self.next_request_id();
        
        self.pending.insert(request_id, tx);
        
        // Send XPC message
        self.connection.send_message(xpc::Message {
            request_id,
            path: job.path,
            doc_id: job.doc_id,
        })?;
        
        // Wait for response (with timeout)
        let content = rx.recv_timeout(Duration::from_secs(30))?;
        
        Ok(content)
    }
    
    fn handle_response(&mut self, response: xpc::Response) {
        if let Some(tx) = self.pending.remove(&response.request_id) {
            let _ = tx.send(response.content);
        }
    }
}
```

**XPC Service Implementation:**

```rust
// Extractor service (separate process)
struct ContentExtractor {
    workers: ThreadPool,
}

impl ContentExtractor {
    fn handle_request(&self, request: xpc::Message) {
        self.workers.execute(move || {
            let content = match extract_content(&request.path) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("Extraction failed for {:?}: {}", request.path, e);
                    ExtractedContent::empty()
                }
            };
            
            send_response(xpc::Response {
                request_id: request.request_id,
                content,
            });
        });
    }
}

fn extract_content(path: &Path) -> Result<ExtractedContent> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    
    match ext {
        "txt" | "md" | "rs" | "py" | "js" | "ts" => {
            extract_plaintext(path)
        }
        "pdf" => {
            extract_pdf(path)
        }
        "docx" | "xlsx" | "pptx" => {
            extract_office(path)
        }
        "jpg" | "jpeg" | "png" => {
            extract_image_text(path)  // OCR
        }
        _ => {
            Ok(ExtractedContent::empty())
        }
    }
}

fn extract_plaintext(path: &Path) -> Result<ExtractedContent> {
    // Read first 64KB only (fast path)
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; 64 * 1024];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    
    let text = String::from_utf8_lossy(&buf).into_owned();
    
    Ok(ExtractedContent {
        text,
        full_content: false,  // Only partial
    })
}

fn extract_pdf(path: &Path) -> Result<ExtractedContent> {
    // Use PDFKit via FFI
    let text = unsafe {
        pdf_extract_text(path.to_str().unwrap())
    }?;
    
    Ok(ExtractedContent {
        text,
        full_content: true,
    })
}
```

**Crash Handling:**

```rust
impl ContentExtractionClient {
    fn on_connection_interrupted(&mut self) {
        log::warn!("XPC connection interrupted, restarting extractor");
        
        // Fail all pending requests
        for (_, tx) in self.pending.drain() {
            let _ = tx.send(ExtractedContent::empty());
        }
        
        // Reconnect
        self.connection = xpc::Connection::new("com.localsearch.extractor");
    }
}
```

### 4.5 Scheduling & Prioritization

**Content Extraction Priority Queue:**

```rust
struct ContentQueue {
    queue: BinaryHeap<PrioritizedJob>,
}

struct PrioritizedJob {
    job: ContentExtractionJob,
    priority: u32,
}

impl Ord for PrioritizedJob {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority.cmp(&other.priority)
    }
}

fn compute_content_priority(path: &Path) -> u32 {
    let mut priority = 100;
    
    // Hot paths
    if is_hot_path(path) {
        priority += 200;
    }
    
    // Recently accessed
    if let Ok(metadata) = fs::metadata(path) {
        let age_days = (SystemTime::now().duration_since(metadata.accessed()?)?.as_secs() / 86400) as u32;
        priority += (30 - age_days.min(30)) * 5;
    }
    
    // Small files (faster to process)
    if let Ok(metadata) = fs::metadata(path) {
        if metadata.len() < 1024 * 1024 {  // < 1MB
            priority += 50;
        }
    }
    
    priority
}
```

**Backpressure Handling:**

```rust
impl ContentQueue {
    fn enqueue(&mut self, job: ContentExtractionJob) -> Result<()> {
        // If queue is full, drop lowest priority job
        if self.queue.len() >= MAX_QUEUE_SIZE {
            if job.priority > self.queue.peek().unwrap().priority {
                self.queue.pop();
                self.queue.push(PrioritizedJob {
                    job,
                    priority: job.priority,
                });
            }
            // Else drop this job
        } else {
            self.queue.push(PrioritizedJob {
                job,
                priority: job.priority,
            });
        }
        
        Ok(())
    }
}
```

---

## 5. Query Engine

### 5.1 Tokenizer

```rust
struct Tokenizer {
    stemmer: Stemmer,
    stop_words: HashSet<String>,
}

#[derive(Debug, Clone)]
struct Token {
    term: String,
    position: u32,
    original: String,
}

impl Tokenizer {
    fn tokenize(&self, text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut position = 0;
        
        // 1. Unicode normalization (NFC)
        let normalized = text.nfc().collect::<String>();
        
        // 2. Split on whitespace and punctuation
        for word in normalized.split(|c: char| c.is_whitespace() || c.is_ascii_punctuation()) {
            if word.is_empty() {
                continue;
            }
            
            // 3. Lowercase
            let lower = word.to_lowercase();
            
            // 4. Skip stop words
            if self.stop_words.contains(&lower) {
                continue;
            }
            
            // 5. Stem
            let stemmed = self.stemmer.stem(&lower);
            
            tokens.push(Token {
                term: stemmed.to_string(),
                position,
                original: word.to_string(),
            });
            
            position += 1;
        }
        
        // 6. CamelCase splitting
        tokens.extend(self.split_camel_case(&tokens));
        
        tokens
    }
    
    fn split_camel_case(&self, tokens: &[Token]) -> Vec<Token> {
        let mut result = Vec::new();
        
        for token in tokens {
            if token.original.chars().any(|c| c.is_uppercase()) {
                // Split on uppercase boundaries
                let parts = split_on_case_change(&token.original);
                for part in parts {
                    result.push(Token {
                        term: part.to_lowercase(),
                        position: token.position,
                        original: part,
                    });
                }
            }
        }
        
        result
    }
}
```

### 5.2 Query Parser

```rust
#[derive(Debug)]
struct Query {
    tokens: Vec<String>,
    scope: Option<PathBuf>,
    filters: Vec<Filter>,
    intent: QueryIntent,
}

#[derive(Debug)]
enum Filter {
    Kind(String),           // kind:pdf
    After(SystemTime),      // after:2024-01-01
    Before(SystemTime),     // before:2024-12-31
    Size(SizeRange),        // size:>1MB
}

#[derive(Debug, Clone, Copy)]
enum QueryIntent {
    Navigational,   // User knows what they want
    Lookup,         // Recent file
    Exploratory,    // Browsing
    Recovery,       // Lost file
}

impl Query {
    fn parse(input: &str) -> Result<Self> {
        let mut tokens = Vec::new();
        let mut scope = None;
        let mut filters = Vec::new();
        
        for part in input.split_whitespace() {
            if let Some(filter) = Self::parse_filter(part)? {
                filters.push(filter);
            } else if part.starts_with("in:") {
                scope = Some(PathBuf::from(&part[3..]));
            } else {
                tokens.push(part.to_lowercase());
            }
        }
        
        let intent = Self::classify_intent(&tokens, &filters);
        
        Ok(Query {
            tokens,
            scope,
            filters,
            intent,
        })
    }
    
    fn classify_intent(tokens: &[String], filters: &[Filter]) -> QueryIntent {
        // Short query, no filters → Lookup
        if tokens.len() <= 2 && filters.is_empty() {
            return QueryIntent::Lookup;
        }
        
        // Time filters → Recovery
        if filters.iter().any(|f| matches!(f, Filter::After(_) | Filter::Before(_))) {
            return QueryIntent::Recovery;
        }
        
        // Long query → Exploratory
        if tokens.len() >= 4 {
            return QueryIntent::Exploratory;
        }
        
        QueryIntent::Navigational
    }
}
```

### 5.3 Query Execution Pipeline

```rust
struct QueryExecutor {
    inverted_index: Arc<InvertedIndex>,
    delta_index: Arc<RwLock<DeltaIndex>>,
    path_trie: Arc<PathTrie>,
    bk_tree: Arc<BkTree>,
    memory_state: Arc<AtomicU8>,
}

impl QueryExecutor {
    fn execute(&self, query: Query) -> Result<Vec<SearchResult>> {
        // 1. Check prefix cache (fast path)
        if query.tokens.len() == 1 && query.filters.is_empty() {
            if let Some(cached) = self.prefix_cache.get(&query.tokens[0]) {
                return Ok(cached.clone());
            }
        }
        
        // 2. Fuzzy expansion (if enabled)
        let expanded_tokens = self.expand_fuzzy(&query.tokens)?;
        
        // 3. Scope resolution
        let scope_mask = match &query.scope {
            Some(path) => self.path_trie.scope_query(path),
            None => RoaringBitmap::full(),  // All docs
        };
        
        // 4. Query main index + delta index (parallel)
        let (main_results, delta_results) = rayon::join(
            || self.query_main_index(&expanded_tokens),
            || self.query_delta_index(&expanded_tokens),
        );
        
        // 5. Merge results
        let mut merged = self.merge_results(main_results?, delta_results?);
        
        // 6. Apply scope filter
        merged.retain(|r| scope_mask.contains(r.doc_id.0 as u32));
        
        // 7. Apply filters
        merged = self.apply_filters(merged, &query.filters)?;
        
        // 8. Score and rank
        let scored = self.score_results(merged, &query)?;
        
        // 9. Top-K selection
        let mut top_k = BinaryHeap::new();
        for result in scored {
            if top_k.len() < 20 {
                top_k.push(result);
            } else if result.score > top_k.peek().unwrap().score {
                top_k.pop();
                top_k.push(result);
            }
        }
        
        // 10. Extract snippets
        let mut results: Vec<_> = top_k.into_sorted_vec();
        for result in &mut results {
            result.snippet = self.extract_snippet(result.doc_id, &query.tokens)?;
        }
        
        Ok(results)
    }
    
    fn expand_fuzzy(&self, tokens: &[String]) -> Result<Vec<Vec<String>>> {
        let state = MemoryState::from(self.memory_state.load(Ordering::Relaxed));
        
        if state == MemoryState::Minimal || state == MemoryState::Critical {
            // No fuzzy expansion in degraded mode
            return Ok(tokens.iter().map(|t| vec![t.clone()]).collect());
        }
        
        let mut expanded = Vec::new();
        
        for token in tokens {
            let mut variants = vec![token.clone()];
            
            // BK-tree fuzzy search (edit distance ≤ 2)
            if let Some(bk_tree) = self.bk_tree.as_ref() {
                let fuzzy_matches = bk_tree.search(token, 2);
                variants.extend(fuzzy_matches);
            }
            
            expanded.push(variants);
        }
        
        Ok(expanded)
    }
    
    fn query_main_index(&self, tokens: &[Vec<String>]) -> Result<Vec<RawResult>> {
        let mut results = Vec::new();
        
        for token_variants in tokens {
            for token in token_variants {
                // Lookup in each segment
                for segment in self.inverted_index.segments.read().unwrap().iter() {
                    if let Some(postings) = segment.lookup_term(token)? {
                        for posting in postings.iter() {
                            results.push(RawResult {
                                doc_id: posting.doc_id,
                                term: token.clone(),
                                term_freq: posting.term_freq,
                                field_mask: posting.field_mask,
                            });
                        }
                    }
                }
            }
        }
        
        Ok(results)
    }
    
    fn query_delta_index(&self, tokens: &[Vec<String>]) -> Result<Vec<RawResult>> {
        let delta = self.delta_index.read().unwrap();
        let mut results = Vec::new();
        
        for token_variants in tokens {
            for token in token_variants {
                if let Some(postings) = delta.lookup(token) {
                    for posting in &postings.postings {
                        // Skip deleted docs
                        if delta.deleted.contains(&posting.doc_id) {
                            continue;
                        }
                        
                        results.push(RawResult {
                            doc_id: posting.doc_id,
                            term: token.clone(),
                            term_freq: posting.term_freq,
                            field_mask: posting.field_mask,
                        });
                    }
                }
            }
        }
        
        Ok(results)
    }
}
```

### 5.4 Ranking System

```rust
struct Ranker {
    signal_db: SignalDb,
    context: QueryContext,
}

struct QueryContext {
    time_of_day: u8,           // 0-23
    day_of_week: u8,           // 0-6
    frontmost_app: Option<String>,
    session_clicked_paths: Vec<PathBuf>,
}

impl Ranker {
    fn score(&self, result: &RawResult, query: &Query) -> f32 {
        let weights = self.get_weights(query.intent);
        
        let bm25 = self.compute_bm25(result, query);
        let recency = self.compute_recency(result);
        let field_boost = self.compute_field_boost(result);
        let click_signal = self.signal_db.get_click_score(result.doc_id, &query.tokens);
        let session_coherence = self.compute_session_coherence(result);
        let context_boost = self.compute_context_boost(result);
        
        weights.bm25 * bm25
            + weights.recency * recency
            + weights.field_boost * field_boost
            + weights.click_signal * click_signal
            + weights.session_coherence * session_coherence
            + weights.context_boost * context_boost
    }
    
    fn compute_bm25(&self, result: &RawResult, query: &Query) -> f32 {
        // BM25 parameters
        let k1 = 1.2;
        let b = 0.75;
        
        let tf = result.term_freq as f32;
        let doc_len = self.get_doc_length(result.doc_id);
        let avg_doc_len = self.inverted_index.avg_doc_length;
        let n = self.inverted_index.total_docs as f32;
        let df = self.inverted_index.get_doc_freq(&result.term) as f32;
        
        // IDF component
        let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
        
        // TF component with length normalization
        let tf_component = (tf * (k1 + 1.0)) / 
            (tf + k1 * (1.0 - b + b * (doc_len / avg_doc_len)));
        
        idf * tf_component
    }
    
    fn compute_recency(&self, result: &RawResult) -> f32 {
        let doc = self.get_doc_metadata(result.doc_id);
        let age_days = (SystemTime::now().duration_since(doc.accessed_at).unwrap().as_secs() / 86400) as f32;
        
        // Log decay
        1.0 / (1.0 + (1.0 + age_days).ln())
    }
    
    fn compute_field_boost(&self, result: &RawResult) -> f32 {
        let mut boost = 0.0;
        
        if result.field_mask & FIELD_FILENAME != 0 {
            boost += 3.0;  // Filename match is strongest signal
        }
        if result.field_mask & FIELD_PATH != 0 {
            boost += 1.0;
        }
        if result.field_mask & FIELD_CONTENT != 0 {
            boost += 0.5;
        }
        
        boost
    }
    
    fn compute_session_coherence(&self, result: &RawResult) -> f32 {
        let doc = self.get_doc_metadata(result.doc_id);
        let doc_path = Path::new(&doc.path);
        
        // Check if doc is in same directory as recently clicked docs
        for clicked_path in &self.context.session_clicked_paths {
            if doc_path.parent() == clicked_path.parent() {
                return 1.5;  // Directory proximity boost
            }
        }
        
        0.0
    }
    
    fn compute_context_boost(&self, result: &RawResult) -> f32 {
        let mut boost = 0.0;
        
        // Time-of-day boost
        if self.context.time_of_day >= 9 && self.context.time_of_day <= 17 {
            // Work hours: boost recently modified files
            let doc = self.get_doc_metadata(result.doc_id);
            let age_hours = SystemTime::now().duration_since(doc.mtime).unwrap().as_secs() / 3600;
            if age_hours < 48 {
                boost += 0.5;
            }
        }
        
        // Frontmost app boost
        if let Some(app) = &self.context.frontmost_app {
            let doc = self.get_doc_metadata(result.doc_id);
            let ext = Path::new(&doc.path).extension().and_then(|e| e.to_str());
            
            match (app.as_str(), ext) {
                ("Xcode", Some("swift")) => boost += 1.0,
                ("Xcode", Some("m")) => boost += 1.0,
                ("VSCode", Some("rs")) => boost += 1.0,
                ("VSCode", Some("ts")) => boost += 1.0,
                ("Pages", Some("docx")) => boost += 1.0,
                ("Pages", Some("pdf")) => boost += 0.5,
                _ => {}
            }
        }
        
        boost
    }
    
    fn get_weights(&self, intent: QueryIntent) -> RankingWeights {
        match intent {
            QueryIntent::Navigational => RankingWeights {
                bm25: 1.0,
                recency: 0.5,
                field_boost: 3.0,  // Exact filename match dominates
                click_signal: 1.0,
                session_coherence: 0.5,
                context_boost: 0.5,
            },
            QueryIntent::Lookup => RankingWeights {
                bm25: 0.5,
                recency: 4.0,      // Recency dominates
                field_boost: 2.0,
                click_signal: 3.0,  // Click history matters
                session_coherence: 1.0,
                context_boost: 1.0,
            },
            QueryIntent::Exploratory => RankingWeights {
                bm25: 2.0,         // Content relevance matters
                recency: 1.0,
                field_boost: 1.0,
                click_signal: 0.5,
                session_coherence: 0.5,
                context_boost: 1.5,
            },
            QueryIntent::Recovery => RankingWeights {
                bm25: 2.0,
                recency: 0.5,      // Old files are OK
                field_boost: 1.5,
                click_signal: 0.5,
                session_coherence: 0.0,
                context_boost: 0.5,
            },
        }
    }
}
```

### 5.5 Snippet Extraction

```rust
fn extract_snippet(doc_id: DocId, query_tokens: &[String]) -> Result<String> {
    let doc = get_doc_metadata(doc_id)?;
    
    // Get token positions from posting lists
    let positions = get_token_positions(doc_id, query_tokens)?;
    
    if positions.is_empty() {
        // No content match, return filename
        return Ok(doc.filename.clone());
    }
    
    // Find best window (most query tokens in smallest span)
    let window = find_best_window(&positions, 50)?;  // 50 token window
    
    // Extract text around window
    let content = read_content_window(doc_id, window)?;
    
    // Highlight query terms
    let highlighted = highlight_terms(&content, query_tokens);
    
    Ok(highlighted)
}

fn find_best_window(positions: &[u32], window_size: u32) -> Result<(u32, u32)> {
    let mut best_start = 0;
    let mut best_count = 0;
    
    for start in positions {
        let end = start + window_size;
        let count = positions.iter().filter(|&&p| p >= *start && p < end).count();
        
        if count > best_count {
            best_count = count;
            best_start = *start;
        }
    }
    
    Ok((best_start, best_start + window_size))
}
```

---

## 6. Concurrency & Threading Model

### 6.1 Thread Pools

```rust
struct ThreadPools {
    query: ThreadPool,        // 2 threads, QOS_USER_INTERACTIVE
    indexing: ThreadPool,     // 1 thread, QOS_UTILITY
    compaction: ThreadPool,   // 1-2 threads, QOS_BACKGROUND
    extraction: XpcClient,    // XPC service, 2-4 threads
}

impl ThreadPools {
    fn new() -> Self {
        Self {
            query: ThreadPoolBuilder::new()
                .num_threads(2)
                .thread_name("query-worker")
                .build()
                .unwrap(),
            
            indexing: ThreadPoolBuilder::new()
                .num_threads(1)
                .thread_name("indexing-worker")
                .build()
                .unwrap(),
            
            compaction: ThreadPoolBuilder::new()
                .num_threads(1)
                .thread_name("compaction-worker")
                .build()
                .unwrap(),
            
            extraction: XpcClient::new("com.localsearch.extractor"),
        }
    }
}
```

### 6.2 Synchronization Primitives

```rust
// Global state with explicit synchronization
struct GlobalState {
    // Delta index: 1 writer (indexing thread), N readers (query threads)
    delta_index: Arc<RwLock<DeltaIndex>>,
    
    // Segment set: N readers, 1 swapper (compaction thread)
    segments: Arc<RwLock<Vec<Segment>>>,
    
    // WAL cursor: 1 writer (FSEvents thread), 1 reader (indexing thread)
    wal_cursor: Arc<Mutex<WalCursor>>,
    
    // Memory state: N readers, 1 writer (memory controller)
    memory_state: Arc<AtomicU8>,
    
    // Path trie: read-only after initial build
    path_trie: Arc<PathTrie>,
    
    // Signal DB: N writers, N readers (SQLite handles this)
    signal_db: Arc<SignalDb>,
}
```

**Lock Ordering (Deadlock Prevention):**

```
Rule: Always acquire locks in this order:
1. segments (if needed)
2. delta_index (if needed)
3. wal_cursor (if needed)

Never acquire in reverse order.
Never hold multiple locks across await points.
```

### 6.3 Lock Contention Analysis

**Delta Index RwLock:**

```
Contention scenario:
- 2 query threads hold read lock (30ms each)
- Indexing thread requests write lock

Probability both query threads hold lock simultaneously:
  P = (write_duration / query_duration)^2
    = (50µs / 30ms)^2
    = 0.0028%

Expected write wait time: ~50µs (negligible)
```

**Segment Set RwLock:**

```
Contention scenario:
- N query threads hold read lock
- Compaction thread requests write lock to swap segments

Write lock hold time: ~100ns (pointer swap only)
Read lock hold time: ~30ms (full query)

Compaction uses write-priority RwLock:
- Once write request queued, new reads block
- In-progress reads complete
- Max wait = longest query = 500ms (P99)

This is acceptable for background compaction.
```

### 6.4 Memory Ownership Model

```rust
// Segments are Arc<Segment> - shared ownership
// Query threads hold Arc clone during query
// Compaction creates new Arc, swaps pointer
// Old Arc dropped when last query completes

struct Segment {
    mmap: Mmap,  // Memory-mapped file
    // ... other fields
}

impl Drop for Segment {
    fn drop(&mut self) {
        // Last reference dropped, can delete file
        if Arc::strong_count(&self) == 1 {
            let _ = fs::remove_file(&self.path);
        }
    }
}
```

**Query Thread Scratch Arena:**

```rust
thread_local! {
    static SCRATCH_ARENA: RefCell<Arena> = RefCell::new(Arena::new(4 * 1024 * 1024));
}

fn execute_query(query: Query) -> Result<Vec<SearchResult>> {
    SCRATCH_ARENA.with(|arena| {
        let mut arena = arena.borrow_mut();
        arena.reset();  // Clear previous allocations
        
        // All query allocations use arena
        let results = arena.alloc_vec();
        // ... query execution
        
        // Arena automatically freed at end of scope
        Ok(results.to_vec())  // Copy out of arena
    })
}
```

### 6.5 Race Condition Analysis

**Race 1: File modified between stat() and read()**

```
Timeline:
T0: FSEvents delivers Modified event
T1: Indexing thread calls stat() → mtime = X
T2: User modifies file → mtime = Y
T3: Content extractor reads file → gets new content
T4: Index stores (mtime=X, content=new)

Result: Inconsistent state (old mtime, new content)

Mitigation:
- Content extractor re-stats after read
- If mtime changed, discard extraction and re-enqueue
- Store content_hash in index for verification
```

**Race 2: File deleted during indexing**

```
Timeline:
T0: FSEvents delivers Created event
T1: Indexing thread resolves DocId
T2: User deletes file
T3: Indexing thread tries to stat() → ENOENT

Mitigation:
- Handle ENOENT gracefully
- Write Delete entry to WAL
- Don't crash or log error (expected case)
```

**Race 3: Concurrent queries see inconsistent delta**

```
Timeline:
T0: Query 1 starts, acquires delta read lock
T1: Indexing thread waits for write lock
T2: Query 2 starts, blocks on read lock (write-priority)
T3: Query 1 completes, releases read lock
T4: Indexing thread acquires write lock, modifies delta
T5: Query 2 acquires read lock, sees new delta

Result: Query 2 sees different index state than Query 1

This is acceptable: queries see a consistent snapshot,
just not the same snapshot. No torn reads.
```

---

## 7. Resource Management

### 7.1 Memory Control Loop

```rust
struct MemoryController {
    state: Arc<AtomicU8>,
    budget: usize,
    check_interval: Duration,
}

impl MemoryController {
    fn run(&self) {
        loop {
            thread::sleep(self.check_interval);
            
            let rss = self.get_rss();
            let pressure_ratio = rss as f32 / self.budget as f32;
            
            let new_state = match pressure_ratio {
                r if r > 0.95 => MemoryState::Critical,
                r if r > 0.80 => MemoryState::Minimal,
                r if r > 0.65 => MemoryState::Reduced,
                _ => MemoryState::Full,
            };
            
            let old_state = MemoryState::from(self.state.load(Ordering::Relaxed));
            
            if new_state != old_state {
                self.transition(old_state, new_state);
            }
            
            // Attempt recovery if pressure eased
            if pressure_ratio < 0.50 && old_state != MemoryState::Full {
                self.attempt_recovery();
            }
        }
    }
    
    fn transition(&self, from: MemoryState, to: MemoryState) {
        log::info!("Memory state transition: {:?} → {:?}", from, to);
        
        match to {
            MemoryState::Reduced => {
                evict_component(Component::HnswIndex);
            }
            MemoryState::Minimal => {
                evict_component(Component::BkTree);
                evict_component(Component::TrigramIndex);
            }
            MemoryState::Critical => {
                flush_delta_to_temp_segment();
                suspend_background_workers();
            }
            MemoryState::Full => {
                // Recovery handled separately
            }
        }
        
        self.state.store(to as u8, Ordering::Relaxed);
        notify_ui_state_change(to);
    }
    
    fn get_rss(&self) -> usize {
        // macOS: task_info(TASK_VM_INFO)
        unsafe {
            let mut info: task_vm_info = mem::zeroed();
            let mut count = TASK_VM_INFO_COUNT;
            
            task_info(
                mach_task_self(),
                TASK_VM_INFO,
                &mut info as *mut _ as *mut i32,
                &mut count,
            );
            
            info.phys_footprint as usize
        }
    }
}
```

### 7.2 CPU Scheduling

```rust
fn set_thread_qos(qos: QoSClass) {
    unsafe {
        pthread_set_qos_class_self_np(qos as u32, 0);
    }
}

// Query threads
set_thread_qos(QoSClass::UserInteractive);

// Indexing thread
set_thread_qos(QoSClass::Utility);

// Compaction threads
set_thread_qos(QoSClass::Background);
```

### 7.3 I/O Throttling

```rust
fn set_io_policy(policy: IoPolicy) {
    unsafe {
        setiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_THREAD, policy as i32);
    }
}

// Query threads
set_io_policy(IoPolicy::Important);

// Compaction threads
set_io_policy(IoPolicy::Throttle);
```

### 7.4 Thermal & Battery Awareness

```rust
struct PowerMonitor {
    thermal_state: Arc<AtomicU8>,
    on_battery: Arc<AtomicBool>,
}

impl PowerMonitor {
    fn monitor(&self) {
        // Subscribe to thermal state notifications
        let center = NSNotificationCenter::defaultCenter();
        center.addObserver(
            self,
            selector("thermalStateChanged:"),
            NSProcessInfoThermalStateDidChangeNotification,
            None,
        );
        
        // Subscribe to power source notifications
        IORegisterForSystemPower(/* ... */);
    }
    
    fn should_compact(&self) -> bool {
        let thermal = ThermalState::from(self.thermal_state.load(Ordering::Relaxed));
        let on_battery = self.on_battery.load(Ordering::Relaxed);
        
        match thermal {
            ThermalState::Critical | ThermalState::Serious => false,
            ThermalState::Fair if on_battery => false,
            _ => true,
        }
    }
}
```

---

## 8. Observability & Reliability

### 8.1 Metrics Collection

```rust
struct Metrics {
    // Index health (collected every 30s)
    index_segment_count: Gauge,
    index_delta_size_bytes: Gauge,
    index_wal_lag_events: Gauge,
    index_doc_count: Gauge,
    
    // Query performance (per query)
    query_latency_ms: Histogram,
    query_result_count: Histogram,
    
    // Storage in SQLite (7-day retention)
    db: SqliteConnection,
}

impl Metrics {
    fn record_query(&self, latency: Duration, result_count: usize) {
        self.db.execute(
            "INSERT INTO query_metrics (ts, latency_ms, result_count) VALUES (?, ?, ?)",
            params![now(), latency.as_millis(), result_count],
        );
    }
}
```

### 8.2 Corruption Detection

```rust
fn integrity_check() -> Result<IntegrityReport> {
    // Sample 1000 random docs
    let sample = sample_random_docs(1000);
    
    let mut phantom_count = 0;
    let mut stale_count = 0;
    
    for doc_id in sample {
        let doc = get_doc_metadata(doc_id)?;
        
        match fs::metadata(&doc.path) {
            Ok(metadata) => {
                if metadata.mtime() != doc.mtime {
                    stale_count += 1;
                }
            }
            Err(_) => {
                phantom_count += 1;
            }
        }
    }
    
    Ok(IntegrityReport {
        phantom_rate: phantom_count as f32 / 1000.0,
        stale_rate: stale_count as f32 / 1000.0,
    })
}
```

---

## 9. Cold Start & Lifecycle

### 9.1 First Launch

```rust
fn first_launch() -> Result<()> {
    // 1. Show onboarding, request TCC permissions
    request_full_disk_access()?;
    
    // 2. Index hot paths synchronously (fast)
    let hot_paths = vec![
        home_dir().join("Desktop"),
        home_dir().join("Documents"),
        home_dir().join("Downloads"),
    ];
    
    for path in hot_paths {
        index_directory_sync(&path)?;
    }
    
    // 3. Start background crawl for remaining paths
    spawn_background_crawler();
    
    // 4. Serve queries immediately (hot paths + Spotlight fallback)
    Ok(())
}
```

### 9.2 Warm Restart

```rust
fn warm_restart() -> Result<()> {
    // 1. Load WAL header
    let wal_header = read_wal_header()?;
    
    // 2. Replay WAL from last checkpoint
    let entries = replay_wal(wal_header.checkpoint_seq)?;
    for entry in entries {
        apply_to_delta_index(entry)?;
    }
    
    // 3. Prefault hot slab
    madvise(hot_slab_addr, hot_slab_size, MADV_WILLNEED);
    
    // 4. Ready to serve queries
    Ok(())
}
```

### 9.3 Crash Recovery

```rust
fn crash_recovery() -> Result<()> {
    // 1. Check for incomplete compaction
    if let Some(temp_segment) = find_temp_segment()? {
        fs::remove_file(temp_segment)?;  // Discard partial segment
    }
    
    // 2. Replay WAL
    let last_seq = recover_wal()?;
    
    // 3. Verify segment integrity
    for segment in list_segments()? {
        if !segment.verify_checksum()? {
            log::error!("Corrupt segment: {:?}", segment.path);
            fs::remove_file(segment.path)?;
        }
    }
    
    // 4. Resume normal operation
    Ok(())
}
```

---

## 10. Failure Modes & Chaos Scenarios

### 10.1 Failure Taxonomy

```rust
enum FailureMode {
    // WAL failures
    WalCorruption { seq: u64 },
    WalDiskFull,
    
    // FSEvents failures
    EventQueueOverflow { path: PathBuf },
    EventStreamRestart,
    
    // Extractor failures
    XpcCrash,
    ExtractionTimeout { path: PathBuf },
    
    // I/O failures
    DiskReadError { path: PathBuf },
    PermissionDenied { path: PathBuf },
    
    // Index failures
    SegmentCorruption { segment_id: u64 },
}
```

### 10.2 Chaos Test Scenarios

**Scenario 1: WAL Corruption**

```rust
#[test]
fn test_wal_corruption_recovery() {
    // 1. Write 1000 entries
    for i in 0..1000 {
        wal.append(create_entry(i));
    }
    
    // 2. Corrupt entry 500
    corrupt_wal_entry(500);
    
    // 3. Restart system
    let recovered_seq = crash_recovery().unwrap();
    
    // 4. Verify: recovered up to entry 499
    assert_eq!(recovered_seq, 499);
    
    // 5. Verify: entries 500-999 re-delivered by FSEvents
    // (simulated by re-enqueueing)
}
```

**Scenario 2: FSEvents Storm**

```rust
#[test]
fn test_fsevent_storm() {
    // Simulate 50 MUST_SCAN_SUBDIRS events
    for i in 0..50 {
        enqueue_reconciliation(&format!("/Users/test/dir{}", i));
    }
    
    // Verify: hot paths reconciled first
    let first_reconciled = wait_for_reconciliation(Duration::from_secs(10));
    assert!(first_reconciled.contains(&PathBuf::from("/Users/test/Documents")));
}
```

---

## 11. Deployment & Evolution

### 11.1 Index Versioning

```rust
const CURRENT_FORMAT_VERSION: u16 = 1;
const MIN_READER_VERSION: u16 = 1;

fn check_compatibility() -> Result<MigrationPlan> {
    let index_version = read_index_version()?;
    
    if index_version > CURRENT_FORMAT_VERSION {
        return Err(Error::IncompatibleIndex);
    }
    
    if index_version < CURRENT_FORMAT_VERSION {
        return Ok(plan_migration(index_version, CURRENT_FORMAT_VERSION)?);
    }
    
    Ok(MigrationPlan::None)
}
```

### 11.2 Migration Strategy

```rust
enum MigrationPlan {
    None,
    Additive,           // No action needed
    ReEncode,           // Re-encode segments (fast)
    FullReindex,        // Full reindex (slow)
}

fn execute_migration(plan: MigrationPlan) -> Result<()> {
    match plan {
        MigrationPlan::None | MigrationPlan::Additive => Ok(()),
        
        MigrationPlan::ReEncode => {
            // Read old segments, write new format
            for old_segment in list_segments()? {
                let new_segment = re_encode_segment(old_segment)?;
                atomic_replace(old_segment, new_segment)?;
            }
            Ok(())
        }
        
        MigrationPlan::FullReindex => {
            // Backup old index
            backup_index()?;
            
            // Start fresh crawl
            clear_index()?;
            start_full_crawl()?;
            
            Ok(())
        }
    }
}
```

---

## 12. Testing Strategy

### 12.1 Test Pyramid

```rust
// Unit tests: ~500 tests, <30s total
#[test]
fn test_bk_tree_edit_distance() {
    let tree = BkTree::new();
    tree.insert("hello");
    tree.insert("hallo");
    
    let results = tree.search("helo", 1);
    assert_eq!(results, vec!["hello"]);
}

// Integration tests: ~200 tests, <10min total
#[test]
fn test_end_to_end_indexing() {
    let temp_dir = TempDir::new()?;
    fs::write(temp_dir.path().join("test.txt"), "content")?;
    
    wait_for_index();
    
    let results = query("test")?;
    assert_eq!(results.len(), 1);
}

// Parity test: after every integration run
#[test]
fn test_index_disk_parity() {
    let disk_files = walk_filesystem(test_corpus)?;
    let index_files = query_all_docs()?;
    
    let phantom = index_files.difference(&disk_files);
    let missing = disk_files.difference(&index_files);
    
    assert_eq!(phantom.len(), 0);
    assert!(missing.len() as f32 / disk_files.len() as f32 < 0.01);
}
```

---

## 13. Rust Implementation Blueprint

### 13.1 Crate Structure

```
localsearch/
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── wal/
│   │   ├── mod.rs
│   │   ├── writer.rs
│   │   ├── reader.rs
│   │   └── entry.rs
│   ├── index/
│   │   ├── mod.rs
│   │   ├── inverted.rs
│   │   ├── delta.rs
│   │   ├── segment.rs
│   │   └── trie.rs
│   ├── query/
│   │   ├── mod.rs
│   │   ├── parser.rs
│   │   ├── executor.rs
│   │   └── ranker.rs
│   ├── fs/
│   │   ├── mod.rs
│   │   ├── events.rs
│   │   ├── identity.rs
│   │   └── reconcile.rs
│   ├── extract/
│   │   ├── mod.rs
│   │   ├── client.rs
│   │   └── extractors.rs
│   ├── resource/
│   │   ├── mod.rs
│   │   ├── memory.rs
│   │   └── power.rs
│   └── metrics/
│       ├── mod.rs
│       └── collector.rs
├── extractor-xpc/
│   ├── Cargo.toml
│   └── src/
│       └── main.rs
└── ui/
    └── (Swift/SwiftUI)
```

### 13.2 Key Traits

```rust
// Core abstraction for index segments
trait IndexSegment: Send + Sync {
    fn lookup_term(&self, term: &str) -> Result<Option<PostingList>>;
    fn get_doc_metadata(&self, doc_id: DocId) -> Result<DocMetadata>;
    fn doc_count(&self) -> u32;
}

// Query execution trait
trait QueryExecutor {
    fn execute(&self, query: Query) -> Result<Vec<SearchResult>>;
}

// Event source abstraction
trait EventSource {
    fn next_event(&mut self) -> Result<Option<FsEvent>>;
}
```

### 13.3 FFI Boundary (Rust ↔ Swift)

```rust
// Rust side
#[repr(C)]
pub struct SearchResult {
    pub doc_id: u64,
    pub path: *const c_char,
    pub score: f32,
    pub snippet: *const c_char,
}

#[no_mangle]
pub extern "C" fn localsearch_query(
    query: *const c_char,
    results: *mut *mut SearchResult,
    count: *mut usize,
) -> i32 {
    // Implementation
}

// Swift side
@_silgen_name("localsearch_query")
func localsearch_query(
    _ query: UnsafePointer<CChar>,
    _ results: UnsafeMutablePointer<UnsafeMutablePointer<SearchResult>?>,
    _ count: UnsafeMutablePointer<Int>
) -> Int32
```

---

## 14. Production Checklist

### 14.1 Pre-Launch Verification

- [ ] All chaos scenarios pass
- [ ] Parity test shows <0.1% phantom rate
- [ ] P95 query latency <150ms on 1M file corpus
- [ ] Memory usage within tier budget for 24hr run
- [ ] No crashes in 72hr stress test
- [ ] TCC permissions handled gracefully
- [ ] Migration from v0 to v1 tested
- [ ] Backup/restore tested
- [ ] Crash recovery tested (kill -9 during compaction)
- [ ] FSEvents overflow handled correctly
- [ ] XPC extractor crash recovery works
- [ ] Disk full handled gracefully
- [ ] Permission denied handled gracefully

### 14.2 Monitoring (Post-Launch)

- [ ] Query latency P50/P95/P99 tracked
- [ ] Zero-result rate tracked
- [ ] Phantom rate tracked (daily integrity check)
- [ ] Memory pressure events logged
- [ ] Compaction frequency logged
- [ ] XPC crash rate logged
- [ ] User-reported "missing file" incidents tracked

---

## 15. Known Limitations & Future Work

### 15.1 V1 Limitations

1. **No semantic search**: HNSW index designed but not implemented
2. **No network volume change notifications**: Polling only
3. **No cross-volume search**: Each volume indexed separately
4. **No encrypted volume support**: APFS encrypted volumes not accessible
5. **No iCloud Drive support**: Requires CloudKit integration

### 15.2 Future Enhancements

1. **Semantic search (v2)**: Add HNSW index for intent-based queries
2. **Collaborative filtering (v3)**: Learn from aggregate user behavior
3. **Smart folders (v3)**: Saved queries with live updates
4. **Duplicate detection (v4)**: Content-based deduplication
5. **Timeline view (v4)**: Temporal navigation of file history

---

## 16. References & Dependencies

### 16.1 Core Dependencies

```toml
[dependencies]
roaring = "0.10"              # Roaring bitmaps
lz4 = "1.24"                  # Compression
zstd = "0.13"                 # Compression
crossbeam = "0.8"             # Channels
rayon = "1.7"                 # Parallel iterators
memmap2 = "0.9"               # Memory mapping
rusqlite = "0.31"             # SQLite
fsevent-sys = "4.1"           # FSEvents bindings
xxhash-rust = "0.8"           # Fast hashing
unicode-normalization = "0.1" # Unicode NFC
rust-stemmers = "1.2"         # Porter stemmer
```

### 16.2 Platform APIs

- FSEvents: File system change notifications
- TCC: Transparency, Consent, and Control (permissions)
- XPC: Inter-process communication
- IOKit: Power/thermal monitoring
- pthread: Thread QoS
- mach: Task info (RSS measurement)

---

**END OF SPECIFICATION**

This document provides a complete, implementable specification for LocalSearch. Every component is defined from lowest-level primitives (WAL binary format, segment layout) through high-level architecture (threading model, resource management). An engineer can begin implementation immediately without guessing critical details.

