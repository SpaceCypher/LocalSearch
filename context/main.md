versions are just increment , everything that has been talked about will be implemented in a single version. There is no separate version to be created, the different versions like v2 , v3 merely exist to fix the issues the previous might have had so we get a final fixed single version that is issue free



---

# LocalSearch: A Production-Grade macOS File Search System — Design Document

---

## 1. Problem Reframing

Finder search is not simply "slow." It is architecturally broken across four dimensions simultaneously: **consistency** (the index diverges from disk state), **recall** (indexed documents are missing from result sets), **ranking** (results are not ordered by relevance to user intent), and **query expressiveness** (the parser cannot handle fuzzy, phonetic, or semantic queries).

Framed precisely: this is a **near-real-time search system** over a corpus of ~1–10M documents (files), where:
- The corpus mutates continuously (writes, deletes, renames, permission changes)
- Documents have heterogeneous structure (binary, plaintext, structured metadata)
- Queries arrive with low latency requirements (<100ms P99)
- Users issue short, ambiguous, error-prone queries
- The index must maintain **strong consistency guarantees** — a file that exists on disk must be findable within seconds of creation

Spotlight fails on all four. It is an **eventually consistent, opaque, centralized daemon** with no SLA, no freshness guarantees, degraded recall under I/O pressure, and a ranking model that has no signal from user behavior.

---

## 2. Full Failure Analysis

### 2.1 Indexing Architecture

Spotlight's `mds` daemon uses `FSEvents` for change notification but processes them **asynchronously with unbounded latency**. During heavy I/O (large file copies, Xcode builds, package installs), the queue depth grows faster than it drains. The daemon applies **backpressure by dropping events** — silently. There is no replay log, no durability guarantee, no way for the consumer to detect that events were missed. This is a classic **at-most-once delivery** semantics on a system that requires **exactly-once** (or at minimum, **at-least-once with deduplication**).

Additionally, Spotlight excludes paths listed in `~/.metadata_never_index` and system-managed exclusion lists, but these lists are **not user-visible** and are applied inconsistently. External volumes are indexed lazily and only when mounted; the index for a volume is stored per-volume, and mount/unmount events don't always trigger re-reconciliation.

### 2.2 Query Parsing

Spotlight's query parser is a thin wrapper over `NSPredicate` with no tokenization pipeline. There is:
- No stemming (searching "running" does not match "run")
- No stop-word removal (searching "the file" includes "the" as a token)
- No phonetic expansion (searching "Mayer" does not match "Meyer")
- No n-gram fallback for partial matches
- No edit-distance tolerance — a single typo returns zero results
- Scope resolution is passed to the query as a URL predicate, evaluated **after** the full index scan, not as a pruning condition

### 2.3 Ranking

Spotlight applies a **static, metadata-driven ranking** model: file type weight × recency boost × exact-match bonus. There is no:
- BM25 or TF-IDF over file content
- Click-through feedback loop
- Dwell time signal
- Query-document relevance model
- Learning-to-rank layer

The result is that a rarely-accessed file with an exact filename match outranks a heavily-used file with a partial match — which is precisely backwards for most user intent.

### 2.4 Storage Layer

Spotlight stores its index in `/private/var/folders/.../com.apple.metadata:_kMDItemUserTags` and the `mds` store directory — a **proprietary binary format** with no documented schema. This means:
- No external tooling can inspect index health
- Corruption is silent and unrecoverable without a full reindex
- There is no WAL (write-ahead log), so a crash mid-write can leave the index in an inconsistent state
- The index is not memory-mapped for reads; it goes through the kernel page cache with no warm-up strategy

### 2.5 Freshness Guarantees

The system provides **no freshness SLA**. In practice:
- New files appear in results anywhere from 2 seconds to 30+ minutes after creation
- Files deleted from disk continue to appear in results until the next GC sweep
- Files renamed are indexed under the old name until a re-crawl
- Content changes (editing a document) may not propagate to the content index for hours

### 2.6 Scope Resolution

When a user searches "in this folder," Spotlight applies the folder predicate as a **post-filter on the full result set**, not as an index partition. The query planner has no concept of a spatial index over filesystem paths — it cannot use the directory hierarchy as a B-tree key prefix for pruning. Every scoped query pays the full index scan cost.

### 2.7 Content Indexing

File content is indexed by `mdimport` plugins, which run as **separate processes per file type**, with no scheduling guarantees and no content pipeline. For large files (PDFs, archives), content extraction can take minutes and blocks other indexing work. There is no partial content indexing (index the first 64KB of every file, then go deeper on demand). Content index updates are not atomic with metadata index updates — you can have a state where filename is indexed but content is not.

---

## 3. Proposed Redesign

### 3.1 Core Architecture Principles

- **Event-driven, not poll-driven.** The index is a **materialized view** of the filesystem, kept current via a durable event log.
- **Tiered consistency.** Metadata (filename, path, size, mtime) achieves **strong consistency** (<2s latency). Content indexing is **eventual** but with explicit staleness tracking.
- **Query-time index merging.** The main index is immutable segments (LSM-inspired). Recent changes live in a small, fully-scanned **delta index** that merges with segment results at query time.
- **Explicit scope as a first-class index dimension.** Paths are stored as trie keys so scoped queries prune the search space structurally.

### 3.2 Indexing Strategy

**Tier 1 — Filesystem Event Watcher (latency: <100ms)**
Use `FSEvents` with `kFSEventStreamCreateFlagFileEvents` for fine-grained file-level events. Unlike Spotlight, maintain a **durable write-ahead log (WAL)** of all received events with sequence numbers. If the daemon crashes, it replays from the last committed sequence number. Apply **exactly-once semantics** via an idempotency key (inode + sequence number).

**Tier 2 — Delta Index (latency: 2–5s to searchability)**
Events from the WAL are processed by an indexing pipeline that writes to a **delta index** — a small, always-in-memory inverted index holding the last ~10 minutes of changes. Queries always merge delta index results with the main index, with delta results taking precedence (higher recency score).

**Tier 3 — Main Index (background compaction)**
Periodically, the delta index is merged into the main index via an **LSM-style compaction** process. Segments are immutable after being written. Reads can happen concurrently with compaction because old segments remain valid until the merge is committed atomically. This gives us **non-blocking reads** during compaction — a property Spotlight completely lacks.

**Tier 4 — Content Index (eventual, prioritized)**
Content extraction runs in a separate process with an explicit priority queue. Files accessed in the last 7 days are prioritized. For each file, extract the first 64KB immediately (fast), then enqueue full extraction. Content index entries carry a `content_indexed_at` timestamp; queries can optionally show "content may be incomplete" for recently modified files.

### 3.3 Data Structures

**Inverted Index** — The core structure. Term → posting list of `(doc_id, term_frequency, field_mask)`. Field mask encodes which fields the term appeared in (filename=1, path=2, content=4, tags=8), allowing field-boosted scoring without separate per-field indexes.

**Trie for path prefix queries** — All filesystem paths are inserted into a **compressed radix trie** keyed by path component. Scoped searches (`search in /Users/alice/Projects`) translate to a trie prefix lookup that returns a set of `doc_ids` as a **bitset**. This bitset is ANDed with query results at scoring time — O(1) scope filtering per document.

**Roaring Bitmap for doc_id sets** — All posting lists and scope filters use **Roaring Bitmaps** for set intersection/union. This gives 10–100x better compression and faster boolean operations than sorted integer arrays, especially for high-cardinality sets.

**Suffix array for substring search** — For "contains" queries (where the user doesn't know the start of the filename), maintain a **suffix array** over filenames. This supports O(log n) substring lookups without a full scan. The suffix array is rebuilt during compaction, not maintained incrementally (too expensive).

**Embedding index for semantic search** — Optional, Tier 2 feature. Run a small quantized embedding model (e.g. a 4-bit quantized MiniLM, ~50MB) locally. Embed filenames + first 512 chars of content. Store embeddings in an **HNSW approximate nearest neighbor index** (using `hnswlib`). This enables queries like "that report about Q3 revenue" to find "q3_sales_summary.pdf" even with zero keyword overlap. **Tradeoff:** 2–4GB RAM overhead, 200–400ms latency for semantic layer (run in parallel with keyword search, merge results).

### 3.4 Ranking System

Use a **multi-signal linear combination** at query time, not a static heuristic:

```
score(d, q) =
  α × BM25(d, q)             // term frequency / inverse doc frequency
  + β × recency_score(d)     // log-decay over last-access timestamp
  + γ × path_depth_penalty(d) // penalize deeply nested files
  + δ × field_boost(d, q)    // filename match >> path match >> content match
  + ε × user_signal(d, q)    // click-through history for (query, doc) pairs
```

**BM25** over the inverted index handles term relevance. Parameters k1=1.2, b=0.75 (standard; tune on a held-out query log after launch).

**Recency decay**: `1 / (1 + log(1 + days_since_access))` — a file accessed today scores 1.0, a file not accessed in a year scores ~0.33.

**User signals** are stored in a local SQLite table: `(query_hash, doc_id, click_count, last_clicked_at)`. At query time, a doc with prior click-through gets a multiplicative boost. This is a crude but effective **implicit feedback loop** — it personalizes results without any server-side ML.

**Re-ranking layer**: Top-50 BM25 results are passed to a lightweight **LambdaMART-style reranker** (trained locally on click data, retrained weekly). This is optional and activates only after enough click data accumulates (>500 queries).

### 3.5 Fuzzy Matching

Three-layer fuzzy pipeline, applied in order:

**Layer 1 — Edit distance (Damerau-Levenshtein)**: For queries ≤3 typos, generate candidate terms within edit distance 2 using a **BK-tree** (a metric tree over the edit distance metric). BK-tree lookup for edit distance 2 is O(log n) amortized, not O(n). These expanded terms are ORed into the query with a `fuzzy_penalty` score discount (0.8× per edit).

**Layer 2 — N-gram index**: Decompose every indexed term into character trigrams. `"finder"` → `{fin, ind, nde, der}`. Store a separate trigram inverted index. Fuzzy queries that miss the BK-tree (e.g. short queries, heavy misspellings) fall back to trigram Jaccard similarity. A query trigram set with >0.5 Jaccard overlap with a term is treated as a match.

**Layer 3 — Phonetic normalization**: Apply **Double Metaphone** to both query tokens and indexed tokens at index time. Store phonetic keys alongside terms. Queries that return <3 results trigger a phonetic expansion pass. This handles "Meyer / Mayer", "Smith / Smyth", name variations.

**Prefix acceleration**: For each node in the query trie, store the top-100 most relevant documents as a **precomputed priority queue** (refreshed during compaction). Prefix queries (`find.*`) hit this cache directly — zero index scan.

### 3.6 Content Indexing

**Extraction pipeline** (runs as an XPC service, sandboxed):
- **Plain text, Markdown, source code**: direct UTF-8 read, tokenize, index
- **PDF**: use PDFKit for text layer; fallback to Vision OCR for scanned PDFs
- **Office documents (docx, xlsx)**: unzip + parse XML
- **Images**: run Vision framework for text detection (OCR), index detected text with a `ocr_content` field flag
- **Archives (zip, tar)**: index filenames within archive only; don't extract content (too expensive)
- **Binary/unknown**: index MIME type and skip content

Each document gets a `content_hash` (SHA256 of first 64KB). On re-index, skip if hash unchanged. This makes the content pipeline **idempotent** and avoids redundant work on large unchanged files.

Content tokens are stored with **position offsets** to enable phrase queries ("quarterly report") and snippet extraction (highlight the matched passage in the UI).

### 3.7 External Drives and Network Volumes

**External drives** are straightforward: when a volume mounts (`DADiskRegisterDiskAppearedCallback`), trigger an incremental reconciliation against a stored **volume snapshot** (a Merkle tree of inode + mtime pairs stored per volume UUID). Only changed paths are re-indexed. Full scan on first mount.

**Network volumes (SMB, NFS, AFP)** are the hard problem. `FSEvents` does **not work** on network volumes — you get no change notifications. Options:

- **Polling at configurable interval** (default: 5 min for network, 30s for local NAS). Expensive but correct.
- **SMBv3 change notifications** via `IOCTL_QUERY_CHANGES` if the server supports it — use this when available, fall back to polling.
- **User-triggered reindex**: offer a "Reindex this volume" action. For most users this is sufficient.

Network volume content indexing is **opt-in only** — the latency and bandwidth cost is too high to be on by default.

---

## 4. Explicit Tradeoffs

| Dimension | Choice | Cost | Benefit |
|---|---|---|---|
| Delta index in RAM | ~50–200MB for 10min of changes | Memory pressure | <2s freshness on metadata |
| Roaring bitmaps | ~2–5% index size overhead vs raw arrays | Slight write overhead | 10-100x faster set ops |
| Suffix array | ~200MB for 1M filenames | Rebuild cost on compaction | True substring search |
| Embedding index | 2–4GB RAM, 200ms latency | Large footprint | Semantic/intent queries |
| BK-tree fuzzy | ~50MB in memory | RAM | O(log n) fuzzy expansion |
| Content indexing | 500ms–5s per file, high CPU | Background CPU | Full-text search |
| LSM compaction | Background CPU bursts | CPU spikes | Non-blocking reads |
| Per-query reranker | +5–20ms latency | Latency | Personalized ranking |

**The fundamental memory/latency tradeoff**: keeping the delta index and BK-tree in RAM costs ~300MB but buys sub-100ms query latency. If you evict these to disk, latency goes to 500ms–2s — still better than Spotlight, but noticeably slower for interactive use. The right call is to keep them in RAM and implement memory pressure handling (evict the embedding index first, then the suffix array, never evict the core inverted index or BK-tree).

---

## 5. System Architecture---

## 6. Index Update Pipeline

```
FSEvents → WAL write (durable) → event deduplication (inode key)
  → metadata extractor (stat(), xattr())
    → delta index write (in-memory)
      → content extraction queue (prioritized by recency)
        → content tokenizer → posting list write
          → LSM compaction trigger (when delta > threshold)
            → immutable segment flush
              → BK-tree rebuild (background)
              → suffix array rebuild (background)
              → old segment GC
```

**Critical path** (metadata only, <2s): `FSEvents → WAL → delta index`. Everything else is background.

---

## 7. Query Execution Pipeline

```
raw query string
  → tokenizer (unicode-aware, camelCase split, path parse)
    → stop word filter
      → stemmer (Porter or Snowball, language-detected)
        → scope parse ("in:~/Projects", "kind:pdf", "after:2024-01-01")
          → scope resolver → Roaring Bitmap scope mask

          → fuzzy expander (BK-tree, edit dist ≤ 2)
            → trigram fallback if BK-tree returns < 3 candidates
              → phonetic expansion if result count still < 3

              → query plan builder:
                  mode = KEYWORD if token count > 0
                  mode = SEMANTIC if token count == 0 OR query is long phrase
                  mode = HYBRID always (merge both)

              → [parallel execution]:
                  (A) inverted index lookup → posting list merge → BM25 score
                  (B) HNSW approximate nearest neighbor (semantic)
                  (C) delta index scan (always, full scan, fast due to small size)

              → result merge: interleave A+B+C, deduplicate by doc_id
                → AND with scope bitmap
                  → scorer: BM25 × recency × field_boost × user_signal
                    → top-50 candidates → reranker (if trained)
                      → snippet extraction (position offsets → context window)
                        → return top-20 with metadata + snippets
```

**Latency budget**: Tokenization 1ms, BK-tree 3ms, inverted index 15ms, scope AND 1ms, scoring 5ms, HNSW 200ms (parallel, doesn't block keyword path), total keyword path **~25ms P50, ~80ms P99**.

---

## 8. What Would Actually Fail in Production

### Hard Failures

**FSEvents queue overflow under heavy I/O.** When Xcode builds, Webpack watches, or Time Machine backs up, `FSEvents` can receive 50K+ events/second. The kernel coalesces events, but your consumer can still fall behind. At that point, macOS emits a `kFSEventStreamEventFlagMustScanSubDirs` flag — meaning "you missed events, re-scan this directory." You must handle this correctly or you get permanent index divergence. Most implementations, including Spotlight, handle this poorly.

**LSM compaction write amplification.** With a heavy write workload (developer machine with active Xcode project), compaction can amplify writes by 10–30×. On a laptop SSD, this eats write endurance and causes thermal throttling. You need aggressive rate-limiting on compaction and a minimum compaction interval (no more than 1 compaction/5 min during battery-on-AC transition).

**Content index on large files causes latency spikes.** A 2GB PDF or video triggers a content extraction job that can run for 60+ seconds. If not properly isolated in a separate XPC process with `DISPATCH_QUEUE_PRIORITY_BACKGROUND` and strict CPU usage caps, this will visibly degrade battery life and UI responsiveness. Apple got this wrong with `mdimport` and it remains a complaint today.

**Network volume disconnection mid-index.** If a network volume disconnects during an active index crawl, you need to handle `ECONNRESET`, `ETIMEDOUT`, and `EIO` at every file operation and roll back any partial index state for that volume. Failure to do this leaves phantom documents pointing to a dead volume in the index — they show up in results but can't be opened.

### Soft Failures (Correctness Degradation)

**BK-tree edit distance on non-ASCII.** Damerau-Levenshtein on raw bytes breaks for multibyte UTF-8 characters. You must operate on Unicode code points, not bytes. Filenames with CJK characters, emoji, or diacritics will cause incorrect edit distance calculations and spurious fuzzy matches if this isn't handled at the tokenizer level.

**Stale click signals poisoning rankings.** If a user searches "budget" 50 times and always clicks `budget_2023.xlsx`, then renames that file to `budget_2024.xlsx`, the old `doc_id` accumulates click signal for a document that no longer exists. Worse, if a new file gets the same inode (inode reuse after deletion), it inherits the click history. You need to invalidate signal entries on rename/delete events from the WAL.

**Suffix array consistency during compaction.** The suffix array is rebuilt during compaction, but during the rebuild window, suffix queries fall back to a full scan of the inverted index (which doesn't support arbitrary substring queries natively). This causes a temporary regression in "contains" search quality. Users who type in the middle of a filename will notice. Solution: maintain two suffix arrays (current + being-rebuilt) and swap atomically.

**Sandboxing vs full filesystem access.** Distributing on the Mac App Store requires full-disk access entitlement, which Apple grants but reviews carefully. The XPC content extraction service needs a separate entitlement. If Apple revokes or restricts the entitlement in a future macOS version, the entire content index goes dark. Distributing outside the App Store avoids this — but loses Gatekeeper trust by default. This is a **business and distribution risk**, not just an engineering one, and it's the reason no Mac App Store app has successfully built a full Finder replacement.

---

## 9. Engineering Constraints Summary

The system is buildable. The hard constraints are not algorithmic — they are **systems-level and platform-level**:

- `FSEvents` at scale requires careful back-pressure handling that most implementations skip
- Apple's sandboxing model actively resists full filesystem access at the level needed
- Network volumes have no native change notification — polling is the only reliable answer
- LSM compaction on a laptop SSD must be throttled to avoid thermal/battery impact
- The content extraction XPC service must be treated as an untrusted, crash-tolerant worker — it will crash on malformed PDFs and corrupt Office files regularly

The query path is solvable with well-understood algorithms. The **index freshness and consistency guarantees** are where this system lives or dies, and they require the same care you'd give to a distributed database's replication log — on a single machine, but with a far less predictable I/O environment.


Fair. Every one of those is a legitimate hit. Let me address each one with the same rigor the original doc demanded.

---

# LocalSearch v2 — Production Hardening Document

---

## Counter 1: Memory Pressure — Tiered Architecture with Eviction Policy

The original allocation was aspirational, not operational. Here's how you actually handle a constrained memory environment.

**The core principle**: the system must degrade *gracefully and predictably* — never crash, never get OOM-killed, never silently return stale results without telling the user.

### Memory Budget by Machine Class

| Tier | RAM | Resident target | Hard ceiling |
|---|---|---|---|
| Constrained | 8GB | 180MB | 280MB |
| Standard | 16GB | 400MB | 600MB |
| Comfortable | 32GB+ | 800MB | 1.2GB |

Detected at launch via `host_statistics64(HOST_VM_INFO64)`. Budget is set once and respected throughout the process lifetime.

### Eviction Priority Stack

Components are assigned eviction priority from most to least expendable:

```
EVICT FIRST  ──→  Embedding index (HNSW)         2–4GB  → evict entirely on 8GB tier
                  Suffix array                    ~200MB → evict to mmap'd file
                  BK-tree warm cache              ~50MB  → evict precomputed prefix queues
                  Trigram index                   ~80MB  → evict to disk, query time +30ms
                  Delta index overflow            beyond watermark → flush to temp segment
EVICT LAST   ──→  Core inverted index hot slab    ~60MB  → never evict
                  Path trie                       ~30MB  → never evict
                  WAL cursor + sequence state     ~2MB   → never evict
```

### Memory Pressure Response

Subscribe to `NSProcessInfo.thermalState` notifications *and* `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE`. React to three levels:

**Level 1 — Warning** (`DISPATCH_MEMORYPRESSURE_WARN`): Suspend the embedding worker. Stop prefetching. Compact the delta index immediately regardless of schedule.

**Level 2 — Critical** (`DISPATCH_MEMORYPRESSURE_CRITICAL`): Evict the HNSW index from RAM (it persists on disk; queries fall back to keyword-only with a UI note: "Semantic search paused — low memory"). Evict suffix array from RAM; mmap it from disk. Mark `degraded_mode = true`.

**Level 3 — OOM imminent** (application-level watermark, checked every 500ms): Write a checkpoint to the WAL, suspend all background workers, release the trigram index. Accept query quality degradation to survive.

### Degraded Mode — What the User Sees

Critically: *never* return silently degraded results. The UI must show a persistent (but non-intrusive) indicator:

- **Semantic search paused** — queries still work, just no intent matching
- **Index refreshing** — results may be seconds old (shown when delta is flushing)
- **Search scope limited** — network volumes offline
- **Full-text search paused** — keyword search on filenames still works

This turns a silent failure mode (Spotlight's core sin) into a *visible, recoverable* state. Users trust the system more when they understand why it's behaving differently, not less.

### Suffix Array — mmap Strategy

On constrained tiers, the suffix array lives in a memory-mapped file. You pay page fault cost (~0.5–2ms per cold access) instead of RAM. For substring queries (a rare query type), this is acceptable. The key: build the suffix array as a **flat binary file** (sorted array of `uint32` offsets into a filename blob), not a heap-allocated data structure. mmap reads are then sequential and cache-friendly — the OS prefetcher handles warm-up after the first few queries.

---

## Counter 2: LSM Compaction — Adaptive Scheduling

Compaction cannot run on a fixed timer. It needs to be a first-class citizen of the macOS power model.

### Signal Inputs for Compaction Scheduler

```
NSProcessInfo.thermalState          → {nominal, fair, serious, critical}
NSProcessInfo.isLowPowerModeEnabled → bool
IOPMCopyBatteryInfo()               → {charging, level, timeRemaining}
host_processor_info()               → per-core utilization
delta_index.size                    → current delta pressure
last_user_interaction               → timestamp of last query
```

### Scheduling Decision Tree

```
thermalState == .critical  → SUSPEND all compaction, flush WAL only
thermalState == .serious   → PAUSE, retry in 10min
isLowPowerModeEnabled      → throttle to 1 segment/20min, CPU cap 15%
battery < 20% AND !charging → defer all compaction
userIdle > 5min            → FULL compaction window, CPU cap 80%
userIdle < 30s             → NO compaction (user is active)
userIdle 30s–5min          → MICRO-COMPACTION only (merge delta, no full segment merge)
```

**Micro-compaction** is the key concept the original doc missed. Instead of one large compaction job, define a micro-compaction as: merge the delta index into a single small immutable segment, nothing else. This takes ~200ms, produces minimal I/O, and keeps the delta index small (preserving query quality) without triggering the expensive full segment merge that causes thermal impact.

Full segment merges — the expensive, write-amplifying operations — run *only* during the `userIdle > 5min` window, and only when plugged in OR battery > 50%.

### QoS Integration

All compaction work runs under `QOS_CLASS_BACKGROUND` with an I/O priority of `IOPOL_THROTTLE`. This puts the process at the bottom of the I/O scheduler — Xcode builds, Chrome page loads, and Time Machine all preempt it. The tradeoff: compaction may take 10× longer under load. That's the correct tradeoff. A background indexer that competes with user tasks is a product killer.

Set `proc_setpcontrol(PROC_SETPC_THROTTLEMEM, ...)` to opt into memory throttling as well — the OS will preferentially page out your memory before foreground apps.

### Write Amplification Mitigation

Level-tiered compaction (LevelDB-style) compounds write amplification for update-heavy workloads. For this use case — where the "corpus" (the filesystem) changes incrementally, not wholesale — use **size-tiered compaction** instead. Merge segments of similar size together. This produces fewer, larger merges and halves write amplification at the cost of slightly worse read performance (more segments to scan). On a laptop where write endurance is finite and user-visible, this is the right tradeoff.

---

## Counter 3: FSEvents `MUST_SCAN_SUBDIRS` — Incremental Reconciliation

The original doc waved at this. Here is the actual strategy.

### The Real Problem

When `kFSEventStreamEventFlagMustScanSubDirs` fires for a path, you have potentially missed an unbounded number of events in that subtree. A naive implementation rescans the entire subtree from scratch — on `~/Library` this can be millions of inodes. This is how you burn battery for 20 minutes and anger the user.

### Snapshot-Diff Reconciliation

At the time you process each `MUST_SCAN_SUBDIRS` event, you have two sources of truth:

1. **Index state**: what your index believes is in the subtree (doc_ids, inodes, mtimes)
2. **Disk state**: what is actually on disk right now

The reconciliation job does not rescan the entire tree blindly. It runs a **stat-only pass** — no content reads, no xattr reads, just `lstat()` on each path. This is ~100× cheaper than a full reindex because it skips content extraction entirely. For each inode:

- `mtime_disk == mtime_index` → skip (unchanged)
- `mtime_disk > mtime_index` → enqueue metadata update only
- `inode_disk` not in index → new file, enqueue full index
- `inode_index` not on disk → deleted, remove from index

This produces a minimal **change set** rather than a full reindex. For a subtree of 100K files where 50 actually changed, you process 50 updates instead of 100K.

### Priority-Based Reindexing Queue

Not all paths are equally important. The reconciliation queue uses a **weighted priority** system:

```
priority = recency_weight × path_depth_bonus × user_access_frequency

recency_weight:
  modified in last 1hr  → 100
  modified in last 24hr → 50
  modified in last week → 20
  older               → 1

path_depth_bonus:
  ~/Desktop, ~/Documents, ~/Downloads → ×3
  ~/Projects, ~/code               → ×2
  deeper than 6 levels             → ×0.5

user_access_frequency:
  queried via LocalSearch in last 7 days → ×5
```

The hot paths (`~/Desktop`, `~/Documents`) get reconciled within 5–10 seconds of a `MUST_SCAN_SUBDIRS` event. `~/Library/Application Support/SomeApp/Cache` gets reconciled whenever the system is idle. This is the correct tradeoff: prioritize the paths users actually search.

### Partial Correctness Guarantee

During reconciliation, the system must not serve stale results from the affected subtree without disclosure. Mark affected subtrees as `reconciling` in the index header. Queries hitting a `reconciling` subtree include a caveat tag in results: "Results in ~/Projects may be incomplete." This is removed when reconciliation completes.

This is not a UX weakness — it is the only honest behavior. The alternative (silently serving stale results) is exactly what Spotlight does.

---

## Counter 4: Query Latency — Honest Estimates

The 25ms P50 estimate assumed warm caches, no page faults, and no contention. That's a benchmark, not a production number.

### Realistic Latency Model

```
Component              Warm (hot cache)   Cold (page fault)   Degraded (I/O contention)
─────────────────────────────────────────────────────────────────────────────────────
Tokenization + parse        1ms               1ms                  1ms
BK-tree fuzzy expand        3ms               8ms (page faults)   20ms
Inverted index lookup       8ms              30ms (mmap faults)   80ms
Scope bitmap AND            1ms               1ms                  2ms
Delta index scan            2ms               2ms                  5ms
Scoring (top-50)            4ms               4ms                 10ms
Snippet extraction          5ms              15ms                 30ms
─────────────────────────────────────────────────────────────────────────────────────
TOTAL (keyword only)       24ms              61ms                148ms
HNSW semantic (parallel)  180ms             400ms               600ms+
```

**Honest published SLA**:
- Keyword search: 50ms P50, 150ms P95, 400ms P99
- Keyword + semantic: 200ms P50, 500ms P95 (semantic runs in parallel, doesn't block keyword results)
- First-keystroke (prefix cache): 15ms P50 (precomputed top-100 per prefix node)

The prefix cache is the real latency weapon for the common case. When a user types "re", you hit a trie node that already has the top 100 most-relevant files starting with "re" precomputed. That path is a memory lookup, not an index query — latency is 5–15ms regardless of index size. The full inverted index query fires only when the user pauses typing (>150ms between keystrokes) or the prefix cache misses.

### Warming Strategy

The inverted index hot slab (the 20% of postings that serve 80% of queries, by Zipf's law) is pre-faulted into RAM at startup using `madvise(MADV_WILLNEED)` on the mmap region. This converts the cold-start penalty from query-time spikes to a predictable 2–4 second background warm-up at launch.

---

## Counter 5: Semantic Search — Deferred to v2, Designed for v1

The critique is correct: shipping HNSW in v1 is over-engineering. But the *design* must account for it from the start, or you paint yourself into a corner.

### v1 Ships Without Embeddings

v1 keyword stack:
- Inverted index with BM25
- BK-tree fuzzy
- Trigram fallback
- Phonetic expansion
- Prefix cache

This is the entire query surface for v1. The embedding worker, HNSW index, and semantic merge path are **not built**.

### v1 Must Be Architected for v2 Semantics

The places where you must design for future semantic search *now*, even if you don't implement it:

**Doc ID space**: Assign globally unique 64-bit `doc_id`s from the start. If you use inode numbers as doc IDs (tempting shortcut), you cannot add semantic embeddings later because embedding vectors need stable IDs that survive renames. Inodes are reused. Use a dedicated ID allocator with a `(volume_uuid, inode, generation)` tuple stored in a lookup table.

**Result merge interface**: The query executor must accept results as `Vec<(doc_id, score)>` from *multiple* providers, not a single index. If you hardcode the inverted index as the only result source in v1, adding a semantic provider in v2 requires rewriting the executor. Define the `ResultProvider` interface now; implement it once.

**Score normalization**: BM25 scores are not on the same scale as cosine similarity scores (HNSW). If you ever want to merge them, you need a normalization layer. Design the scoring pipeline to accept a `normalized_score: f32` in [0, 1] per provider. BM25 scores get min-max normalized against the current result set. This is a 10-line addition now; it's a rewrite later.

**The actual v2 scope**: When semantic search ships, the use case is narrow and specific: *long, descriptive queries that produce zero keyword results*. "that presentation I made about the sales funnel last year" — zero BM25 signal, but HNSW finds `Q3_sales_deck_final_v2.pptx` because the embedding of the query overlaps with the embedding of the filename + first paragraph. The trigger: if keyword search returns <3 results, automatically fire the semantic path. Not as a parallel search — as a fallback. This eliminates the latency cost for the 95% of queries where keywords work fine.

---

## Counter 6: Observability — The System That Watches the System

This is the most important addition. A search system without observability is a black box that you cannot debug, cannot tune, and cannot trust.

### Metrics Layer

All metrics are written to a local **SQLite ring buffer** (fixed 7-day retention, 50MB max). No external telemetry. No network calls. Privacy is non-negotiable for a filesystem indexer.

**Index health metrics** (collected every 30s):

```
index.segment_count              → number of LSM segments (should stay < 10)
index.delta_size_bytes           → delta index RAM usage
index.wal_lag_events             → events in WAL not yet indexed (freshness proxy)
index.wal_lag_seconds            → age of oldest unprocessed WAL event
index.doc_count                  → total indexed documents
index.content_indexed_fraction   → % of docs with content indexed
index.last_compaction_at         → timestamp
index.reconciling_paths          → count of subtrees currently reconciling
```

**Query metrics** (per query, sampled at 100%):

```
query.latency_ms                 → total query time
query.keyword_result_count       → results before reranking
query.cache_hit                  → bool (prefix cache used)
query.fuzzy_expanded             → bool (BK-tree triggered)
query.scope_filter_applied       → bool
query.degraded_mode              → bool (any component evicted)
```

**Recall proxy** — the hardest metric to collect locally. You cannot compute true recall without ground truth labels. Instead, track:

```
query.result_count == 0          → "zero result rate" (high = bad recall or bad indexing)
query.user_opened_rank           → rank of the doc the user actually opened (click position)
query.reformulation_rate         → user typed a second query within 5s of the first (frustration signal)
```

A rising `result_count == 0` rate is your first signal that the index has diverged from disk. A rising `reformulation_rate` means your ranking is wrong. Both are observable without any server-side infrastructure.

### Corruption Detection

Every 6 hours (during idle), run a **lightweight integrity check**:

```
1. Sample 1000 random doc_ids from the index
2. For each: verify the file exists at the stored path (lstat())
3. For each: verify the stored mtime matches disk mtime
4. Compute: phantom_rate = docs_not_on_disk / sample_size
5. Compute: stale_rate   = docs_with_wrong_mtime / sample_size
```

If `phantom_rate > 5%` → trigger full reconciliation of affected paths.
If `stale_rate > 10%` → log alert, surface in developer debug panel.
If `phantom_rate > 25%` → the index is severely corrupted → trigger full reindex with user notification.

This is the equivalent of a database's `DBCC CHECKDB` — a regular, lightweight sanity check that catches silent corruption before users notice it.

### Index Health Dashboard (Debug Panel)

Accessible via a keyboard shortcut in the app (opt+cmd+I or similar). Not visible to normal users. Shows:

```
Index status:         HEALTHY / DEGRADED / RECONCILING
Total documents:      1,247,832
Content indexed:      94.3%
WAL lag:             0 events (0ms)
Last compaction:      4 min ago  (23 segments → 4 segments)
Memory usage:         214MB / 280MB budget  (Standard tier)
Components evicted:   none
Thermal state:        Nominal
Last integrity check: 2h ago  ✓ (phantom 0.3%, stale 1.1%)

Recent query P50:     48ms
Recent query P99:     187ms
Zero-result rate:     2.1%  ↑ (baseline 0.8%)  ⚠️
Reformulation rate:   8.4%
```

The `zero-result rate ↑` warning is the single most valuable thing in this panel. It tells you, before any user files a bug report, that something is wrong with index freshness.

### WAL as the Source of Truth for Debugging

Every index operation is traceable back to a WAL entry. When a user reports "I can't find file X," the debug flow is:

```
1. Does X exist on disk?           → if no, user error
2. Is X's path excluded?           → check exclusion list
3. Is X in the index?              → query by inode
4. Is X's WAL event present?       → check WAL for inode
5. What is the WAL lag for X?      → check sequence gap
6. Is X's path currently reconciling? → check reconciliation queue
```

This gives you a complete causal chain from disk event to index state to query result. Without the WAL, step 4 and 5 are impossible — you're debugging blind, exactly like debugging Spotlight today.

---

## What This System Actually Looks Like Now

The revised design is less glamorous and more honest than the original:

- **v1 ships no embeddings**. The semantic layer is *designed for* but not built.
- **Memory is a first-class constraint**, not an afterthought. The system knows what tier it's on and behaves accordingly.
- **Compaction is power-aware**, not timer-driven. It is a background citizen, not a performance peer.
- **FSEvents failures are handled with a concrete reconciliation algorithm**, not a wave of the hand.
- **Latency SLAs are honest**. 50ms P50 is achievable and defensible. 25ms was a benchmark fiction.
- **Observability is load-bearing infrastructure**, not a v3 nice-to-have. The system watches itself, detects its own corruption, and surfaces degradation before users do.

The difference between a prototype and a production system is almost never the core algorithm. It is almost always **these six things**: memory pressure handling, power-aware scheduling, graceful degradation under partial failure, honest performance contracts, incremental correctness recovery, and the ability to observe your own health. None of them are exciting. All of them are the job.

Good. All four are legitimate. Let's close each one properly.

---

# LocalSearch v3 — Hardening Document

---

## Counter 2: Memory — Continuous Adaptation, Not Static Tiers

The previous version set a budget at startup and held it. That's not how macOS memory works. The OS can compress your pages, reclaim them without warning, and do it at any time — not just when the machine is first booting. The budget needs to be a **live control loop**, not a one-time configuration.

### The Memory Pressure Loop

Subscribe to two distinct signals and treat them differently:

`DISPATCH_SOURCE_TYPE_MEMORYPRESSURE` gives you three levels. But it fires *after* pressure has built — it's a lagging indicator. The leading indicator is your own resident set size (RSS), which you can poll cheaply via `task_info(mach_task_self(), TASK_VM_INFO, ...)` every 10 seconds on a background timer.

The control loop:

```
every 10s:
  rss_current = task_vm_info.phys_footprint
  rss_budget   = compute_budget()           // based on total RAM, current tier
  pressure_ratio = rss_current / rss_budget

  if pressure_ratio > 0.95:
    trigger_eviction(level: CRITICAL)
  elif pressure_ratio > 0.80:
    trigger_eviction(level: WARNING)
  elif pressure_ratio < 0.50 AND evictions_active:
    attempt_recovery()                      // re-promote evicted components

on MEMORYPRESSURE_WARN:
  trigger_eviction(level: WARNING)         // OS signal overrides timer

on MEMORYPRESSURE_CRITICAL:
  trigger_eviction(level: CRITICAL)
  suspend_all_background_workers()
```

The critical addition over the previous design is `attempt_recovery()`. When pressure eases — say, the user quits Chrome — the system should **re-promote previously evicted components** back into RAM. Without this, a machine that spikes to 95% memory and then drops back to 40% leaves your system permanently degraded for no reason. Recovery is gated on:

- Pressure has been below 60% for at least 60 continuous seconds (avoid thrashing)
- The component's re-promotion cost (load time) is less than 5× the query latency savings
- Thermal state is nominal

### macOS Memory Compression Interaction

This is the part the previous doc ignored. macOS's memory compressor (Jetsam) operates at a different layer than what `task_vm_info` reports. `phys_footprint` includes compressed pages at their *compressed* size. When the compressor is working hard, you can have 400MB of virtual memory that physically occupies 120MB — and then suddenly gets decompressed on access, causing latency spikes of 5–50ms on what you thought were cheap memory reads.

The implication: **mmap'd components are a liability under memory pressure**, not just a neutral fallback. When the OS compresses your mmap'd suffix array region, subsequent page faults decompress individual pages on-demand, causing unpredictable latency spikes mid-query. You cannot predict when this happens from userspace.

The correct response is a **two-phase eviction policy** that distinguishes between components that can live on disk safely (suffix array, HNSW index) and components that must stay in RAM or be fully absent — no mmap middle ground:

```
Component            Memory model       Under pressure
───────────────────────────────────────────────────────
Core inverted index  mlock()'d slab     Never evict — pin in RAM
Path trie            mlock()'d slab     Never evict — pin in RAM
BK-tree              heap               Evict entirely; queries fall back to trigram
Delta index          heap               Flush to temp segment, clear RAM
Trigram index        heap               Evict; queries accept no fuzzy fallback
Suffix array         file-backed        Evict file mapping entirely; substring = unavailable
HNSW index           file-backed        Evict entirely; semantic = unavailable
```

`mlock()` the hot slab (core inverted index + trie, ~90MB combined) so the OS cannot page or compress it. This is the one component where latency unpredictability is unacceptable — query response must be deterministic even under extreme pressure. Everything else is expendable and transitions cleanly to "feature unavailable" rather than "feature slow and unpredictable."

### Degraded Mode State Machine

The memory controller owns a state machine with explicit transitions and user-visible states:

```
FULL → REDUCED (BK-tree evicted, trigram active)
     → MINIMAL (trigram evicted, exact-only queries)
     → CRITICAL (delta flushed, keyword-only on main index)

REDUCED → FULL (on recovery, after 60s below threshold)
MINIMAL → REDUCED (on partial recovery)
```

Each state transition fires a notification to the UI layer. The UI shows a small, non-intrusive indicator — not a modal, not a toast, just a persistent status dot in the search field with a tooltip: "Fuzzy matching paused — low memory." Users who never look at it are unaffected. Users who wonder why "Meyer" isn't matching "Mayer" have an explanation.

---

## Counter 3: Cold Start — Progressive Warmup

The previous doc mentioned `madvise(MADV_WILLNEED)` as a warmup strategy. That's correct but incomplete. It addresses the *warm restart* case (app relaunched, index exists). The actual hard problem is **first launch**, where there is no index at all, and the user has already opened the search window expecting results.

### Three Distinct Cold Start Scenarios

These need separate handling:

**Scenario A — First ever launch** (no index exists): User opens the app, types a query, and the index is empty. This is the worst case. The only honest answer is: show results immediately from `NSMetadataQuery` (Spotlight's API), clearly labeled as "Spotlight results — building local index." The user gets *something* instantly. Local index results replace Spotlight results progressively as they become available, file by file, without a refresh flash.

**Scenario B — Restart after clean shutdown** (index exists, not warm): Index is on disk but not in RAM. `madvise(MADV_WILLNEED)` starts prefaulting pages in the background. Queries during this window go to a fast path: trie lookup (small, loads fast) + linear scan of the delta WAL (always small) + Spotlight fallback for anything not yet warm. The Spotlight fallback is transparent to the user.

**Scenario C — Crash recovery** (index may be partially written): WAL cursor is the source of truth. Replay from last committed sequence number. Do not serve queries until WAL replay is complete for the hot paths (Documents, Desktop, Downloads). This takes 1–5 seconds. Show a progress indicator.

### Priority-Ordered Warmup for Scenario B

Don't prefault the entire index uniformly. Use a **heat map** of historical query patterns stored in the signal database:

```
Warmup order:
  1. Path trie root + top-3 levels       (always — enables scope resolution)
  2. Posting lists for paths queried     (from signal DB: top-50 path prefixes)
     in last 7 days
  3. Inverted index slabs for top-500    (from signal DB: most-clicked terms)
     query terms
  4. BK-tree                            (enables fuzzy — medium priority)
  5. Remainder of inverted index        (background, lowest priority)
```

On a machine with a cold index and a warm SSD, steps 1–3 complete in under 2 seconds. Step 4 in under 5. The user's first query — which is almost certainly a term they've searched before — hits a warm cache within 3 seconds of launch.

### First-Launch Bootstrap Strategy

The first launch problem is fundamentally a **data availability problem**, not a performance problem. You have no index and no signal history. The solution is a two-phase bootstrap:

**Phase 1 — Instant results (0–2s)**: Index only the four hot paths synchronously at launch: `~/Desktop`, `~/Documents`, `~/Downloads`, `~/Applications`. These are small (typically <50K files combined), index in 1–2 seconds, and cover >80% of what users search for. Serve queries from these paths immediately.

**Phase 2 — Background crawl (2s → hours)**: Expand outward from hot paths in a BFS traversal ordered by path recency (most recently modified directories first). The UI shows "Indexing your files — results expanding" with a subtle progress ring. Results improve incrementally as more paths are covered.

The key constraint: Phase 1 must complete before the user finishes typing their first query. On a modern Mac, indexing `~/Desktop` takes ~200ms. That's the budget for getting *something* useful into the index before the first keystroke completes.

---

## Counter 4: Ranking — Intent, Session, and Context

The previous ranking model was a linear combination of static signals. That's a reasonable baseline but it leaves the biggest wins on the table. Finder fails hardest at ranking not because it uses bad signals, but because it has *no model of user intent*. Two users who both type "report" mean completely different things, and the ranking should reflect that.

### Query Intent Classification

Before scoring, classify the query into one of four intent types. This classification is cheap (rule-based + simple heuristics, <1ms) and dramatically changes which signals dominate the ranking:

```
NAVIGATIONAL  — user knows what they want, knows roughly where it is
                signals: "invoice", "resume", specific filenames
                boosting: exact filename match ×3, recent access ×2

LOOKUP        — user wants a specific file they've opened recently
                signals: short queries, high click history on specific doc
                boosting: recency ×4, click signal ×3

EXPLORATORY   — user is browsing, not sure what exists
                signals: broad terms, generic nouns ("notes", "photos")
                boosting: diversity penalty (avoid showing 10 files from same dir)

RECOVERY      — user lost something, hasn't accessed it recently
                signals: long queries, time qualifiers ("from last year")
                boosting: temporal filter, path breadth over recency
```

Intent is detected from:
- Query length (1–2 tokens → navigational/lookup; 4+ tokens → recovery/exploratory)
- Query specificity (proper nouns, file extensions → navigational)
- Session history (user has searched 3 times in 5 minutes → recovery mode)
- Time of day signal (morning → lookup of recent work; afternoon → navigational; evening → exploratory)

### Session-Based Ranking

The previous model treated every query as independent. In reality, queries within a session are heavily correlated — the user is looking for something, and their successive queries are refinements of the same intent. A session is defined as: queries within a 5-minute sliding window.

Session signals that modify ranking:

**Query reformulation**: If the current query shares >50% tokens with the previous query, the user is refining. Boost documents that appeared in the previous result set but weren't clicked — they're likely relevant but ranked wrongly. This is a lightweight **within-session learning** signal.

**Negative feedback**: If the user typed a new query within 3 seconds of seeing results without clicking anything, the top-ranked result was probably wrong. Apply a **transient demotion** to that document for the remainder of the session (×0.5 score multiplier, not persisted to signal DB).

**Session path coherence**: If the user has clicked documents in `~/Projects/AlphaApp/` in this session, boost documents from that path for subsequent queries in the same session. The user is clearly working in that context.

### Context Signals

Four contextual signals that require no user interaction to collect:

**Temporal context**: The most powerful and most ignored signal. Search behavior has strong diurnal patterns:
- 9–11am on weekdays: high probability of searching for current-project files
- Evening/weekends: exploratory, personal files
- Query for "meeting" before 10am → strongly prefer files modified in last 48h
- Query for "meeting" on Sunday → probably searching history

Apply a time-of-week × time-of-day prior over the recency signal. This is a lookup table, not a model — precompute weights for each hour-of-week bucket from the click history.

**Active application context**: What app is in the foreground when the search is invoked? Use `NSWorkspace.shared.frontmostApplication` at query time. If the user is in Xcode and searches "AppDelegate", strongly prefer `.swift` files. If they're in Pages and search "contract", prefer `.docx` and `.pdf`. This is a 5-line implementation that has a large recall impact for power users.

**Recent file activity**: `NSWorkspace.shared.noteFileSystemChanged` and the recent documents APIs give you a list of files the OS considers recently active across all apps. A file that appeared in an app's recent documents list in the last 2 hours gets a `recently_active` boost (×1.5) regardless of its global recency score. This is different from the index's `last_accessed_at` — it's the OS's notion of recency across all apps.

**Directory co-occurrence**: Files in the same directory as recently-clicked results get a proximity boost. If the user searched "budget" and opened `budget_2024.xlsx`, then searches "forecast", files in the same directory as `budget_2024.xlsx` get a directory-proximity boost. This is how power users actually navigate — they know the neighborhood even if they don't know the exact filename.

### The Ranking Function (Revised)

```
score(d, q, context) =
  intent_weight(intent_class) × (
      α × BM25(d, q)
    + β × recency_decay(d, context.time)
    + γ × click_signal(d, q)
    + δ × field_boost(d, q)
    + ε × session_coherence(d, session)
    + ζ × context_boost(d, context)
    + η × directory_proximity(d, session)
  ) × session_demotion(d, session)

where context = {
  time_of_week_bucket,
  frontmost_app,
  recently_active_files,
  session_clicked_paths
}
```

Each weight is tuned per intent class. Navigational queries weight `field_boost` (exact filename match) heavily. Recovery queries weight `BM25` over content heavily. The intent classifier selects the weight vector; the scoring function is identical.

---

## Counter 5: Threading Model — Explicit, Designed, Auditable

"Workers and queues" is not a threading model. Here is the actual design.

### Thread Pool Architecture

The system runs four distinct thread pools with explicit sizing, priority, and ownership. These are not GCD queues wrapped in abstraction — they are named, observable, and their contention behavior is predictable.

**Pool 1 — Query threads** (`QOS_CLASS_USER_INTERACTIVE`, 2 threads fixed)

Exactly two threads. Not a pool — a fixed pair. Why two and not four? Because the inverted index hot slab is read-only and lock-free (immutable segments), but the delta index requires a reader-writer lock. Two threads means at most one waiting on the delta lock while the other serves a query. More threads increase delta lock contention without improving throughput — query latency is dominated by memory bandwidth, not CPU.

Each query thread owns its own **scratch arena allocator** (a pre-allocated 4MB buffer per thread, reset between queries). All per-query heap allocations (posting list merge buffers, score arrays, snippet buffers) come from this arena. Zero `malloc` calls on the hot query path. This eliminates malloc contention and fragmentation on the query path entirely.

**Pool 2 — Indexing thread** (`QOS_CLASS_UTILITY`, 1 thread fixed)

Exactly one thread. The delta index is written by exactly one writer, eliminating all write-side synchronization on the delta index. `FSEvents` events flow through the WAL into a `mpsc_queue` (multiple-producer, single-consumer). The single indexing thread drains this queue, processes events, and writes to the delta index under no contention.

Single-writer semantics simplify the delta index data structure: it can use a simple append-only in-memory B-tree with no internal locking. Readers acquire a `RwLock` (readers share, writer excludes). Since the writer is a single thread doing sequential appends, write lock hold time is microseconds — reader starvation is not a practical concern.

**Pool 3 — Compaction pool** (`QOS_CLASS_BACKGROUND`, 1–2 threads, adaptive)

One thread normally, two during the `userIdle > 5min` window. Compaction is the only operation that produces new immutable segments. When compaction completes a segment, it atomically swaps the segment reference (a pointer update behind a `Mutex<Arc<SegmentSet>>`). Query threads acquire a read lock on `SegmentSet` for the duration of a query, then release. Compaction acquires a write lock only to swap the pointer — hold time is nanoseconds. This is the **epoch-based reclamation** pattern: old segments are freed when no query thread holds a reference to them.

**Pool 4 — Content extraction pool** (`QOS_CLASS_BACKGROUND`, 2–4 threads, adaptive)

Runs in a **separate XPC process** (`com.localsearch.extractor`), not in the main process. This is the most important isolation decision in the threading model. Content extraction calls PDFKit, Vision, and third-party format parsers — any of these can crash, hang, or leak memory on malformed input. An XPC process crash is contained; a main process crash takes the index and all queries with it.

The main process communicates with the extractor via a bounded channel (capacity 32). If the extractor falls behind, back-pressure propagates to the indexing thread, which simply stops enqueuing content work until the channel drains. No data is lost — the WAL records that content indexing is pending for those files.

### Synchronization Map

Every shared data structure has an explicitly named synchronization primitive:

```
Structure              Access pattern       Primitive
──────────────────────────────────────────────────────────────────
Delta index            1 writer, N readers  RwLock<DeltaIndex>
SegmentSet (main idx)  N readers, 1 swapper RwLock<Arc<SegmentSet>>
WAL cursor             1 writer, 1 reader   Mutex<WalCursor>  (rarely contested)
Path trie              Read-only post-build  None (immutable)
Signal DB              N writers, N readers  SQLite WAL mode   (handles this)
Memory state           N readers, 1 writer  Atomic<MemoryState> (enum as u8)
Prefix cache           N readers, 1 writer  RwLock<PrefixCache>
Query scratch arenas   Thread-local         None (no sharing)
```

The `MemoryState` enum (FULL, REDUCED, MINIMAL, CRITICAL) is an `AtomicU8`. Every component checks this on the hot path with a single `load(Relaxed)` — no lock, no syscall, ~1ns. This is how the query thread knows, mid-query, whether fuzzy expansion is available without acquiring any lock.

### Contention Analysis

The only lock with meaningful contention potential is `RwLock<DeltaIndex>`. Under 2 query threads + 1 indexing thread:

- Read contention: 2 concurrent readers → no contention (RwLock allows concurrent reads)
- Write contention: indexing thread needs write lock, 0–2 query threads may hold read locks

The indexing thread's write lock requests occur at most once per processed event, for a hold duration of ~50µs (a single delta index append). With 2 query threads each executing queries of ~30ms duration, the probability that both hold a read lock when the indexing thread requests a write is: (50µs / 30ms)² ≈ 0.003%. Write starvation of the indexing thread is not a real concern.

The dangerous case is a **slow query** (e.g., cold cache, large result set, 500ms duration) holding the read lock while the indexing thread queues up. To prevent this, the delta RwLock has a **write-priority flag**: once a write request is queued, new read requests are blocked (but in-progress reads complete). Maximum write wait time = max query duration = 500ms in the worst case. This is acceptable — WAL events are durable and can wait 500ms without loss.

### Thread Lifecycle and Observability

Every thread registers itself in a `ThreadRegistry` at startup:

```rust
struct ThreadInfo {
  name:           &'static str,
  pool:           Pool,
  current_op:     AtomicPtr<OpDescriptor>,   // what it's doing right now
  op_start_us:    AtomicU64,                  // when current op started
  total_ops:      AtomicU64,
  total_wait_us:  AtomicU64,                  // time spent waiting on locks
}
```

The health dashboard (from the observability doc) reads this registry to show:
- Which threads are active
- What each is doing
- Whether any thread has been blocked on a lock for >100ms (lock contention alert)
- Whether the content extraction XPC process is alive

A thread that has held a lock for >500ms triggers a `SIGABRT` via a watchdog timer. This converts a deadlock — which would silently corrupt results — into a crash with a full stack trace. Crashes in the content extractor XPC are expected and handled; a crash in the main process is a bug and should produce a report.

---

## What the System Looks Like Now

The four counters were all hitting the same underlying problem: the previous design had **no feedback loops**. Memory was set at startup and held. Warmup was a one-time `madvise` call. Ranking used fixed signals. Threading was described but not specified.

The revised design closes every loop:

- Memory is a **continuous control system** with a 10-second measurement cycle, explicit state transitions, and recovery behavior when pressure eases
- Cold start is **stratified by scenario** — first launch, warm restart, and crash recovery each have a concrete strategy, with Spotlight as an instant fallback during bootstrap
- Ranking is **intent-aware and session-aware** — the same query scores differently depending on who is asking, when, from what application, and what they did 3 queries ago
- Threading is **fully specified** — four named pools, explicit sizing rationale, every shared structure paired with its synchronization primitive, contention analysis for the one lock that matters

Now let's diagram the threading model, since that's the one that benefits most from a visual.The threading model has one property worth emphasizing that the diagram shows clearly: **every pool has a fixed upper bound on thread count with an explicit reason for that number**. Two query threads because delta lock contention scales quadratically with reader count. One indexing thread because single-writer semantics eliminate all write-side synchronization. One-to-two compaction threads because compaction is I/O-bound, not CPU-bound, and a second thread only earns its cost during the idle window. Two-to-four extractor threads in XPC because content extraction is embarrassingly parallel and the XPC boundary contains crashes.

The system never spawns unbounded workers. Every concurrency decision has a specific contention or failure mode it's designed to prevent — not a general "more threads = more throughput" assumption, which is how you get lock convoys and priority inversions on a laptop under load.


All five are legitimate gaps. Let me close each one with the same precision as the rest of the doc.

---

# LocalSearch v4 — Resilience, I/O, Security, Migration, Testing

---

## Counter 1: Failure Injection and Chaos Testing

The previous design handled *anticipated* failures: FSEvents overflow, network volume disconnection, OOM pressure. These are failures you can reason about in advance. The harder problem is the class of failures you didn't model — the ones that only appear when two unlikely conditions coincide, or when the system has been running for six weeks and state has drifted in a way that no unit test exercises.

The standard answer is chaos engineering. On a local desktop system, the tooling looks different from Netflix's `Chaos Monkey` but the principle is identical: **deliberately inject failures in a controlled way, verify that the system's response matches its documented degraded-mode behavior, and discover cases where it doesn't before users do**.

### Fault Taxonomy

Before you can inject failures, you need to enumerate the failure space. Desktop search has five distinct fault classes:

```
Class               Fault examples
─────────────────────────────────────────────────────
WAL faults          Corrupt entry, truncated write, duplicate seq number,
                    seq number gap, checksum mismatch, partial flush

FSEvents faults     Dropped events (silent), coalesced events (loss of
                    granularity), out-of-order delivery, MUST_SCAN_SUBDIRS
                    storm (100 subtrees simultaneously), stream restart

Extractor faults    XPC process crash mid-extraction, timeout on large PDF,
                    infinite loop in third-party parser, memory exhaust
                    in extractor process, malformed output written to channel

I/O faults          EIO on segment read, ENOSPC on WAL write, partial write
                    on segment flush, permission denied on previously readable
                    path, disk full mid-compaction

Index faults        Segment header corruption, posting list length mismatch,
                    doc_id collision, trie node pointing to freed memory,
                    BK-tree returning incorrect edit distances after partial
                    rebuild
```

### Fault Injection Infrastructure

Build a `FaultInjector` singleton that is compiled into the binary in debug/test builds and stripped in release builds via conditional compilation. It intercepts calls at defined **injection points** — locations in the code where the injector can simulate a fault instead of executing the real operation:

```
Injection point          What it can inject
────────────────────────────────────────────────────────
wal::write(entry)        return Err(IoError::Corrupt) with probability p
wal::read(seq)           return a bit-flipped copy of the entry
fsevents::next()         skip the next N events, simulating a drop
extractor::extract(path) crash the XPC process after N bytes of output
segment::read(offset)    return EIO with probability p
segment::write(buf)      return ENOSPC after writing first N bytes
compaction::flush()      interrupt mid-flush, leave partial segment
```

Each injection point consults a `FaultSpec` — a struct specifying fault type, probability, and trigger condition (e.g., "only inject after the 1000th call to this point"). The `FaultSpec` is loaded from a JSON file at startup in test mode, so you can script arbitrary fault sequences without recompiling.

### Chaos Test Scenarios

Five scenarios that must pass before any release:

**Scenario 1 — WAL corruption recovery**: Inject a corrupt WAL entry at position N. Verify: the system detects the checksum mismatch, truncates the WAL to position N-1, replays correctly from position N-1, and indexes all events after N on the next FSEvents delivery. Verify: no documents from events before N are missing from the index. Verify: the health dashboard shows a WAL corruption event in the log.

**Scenario 2 — FSEvents storm + MUST_SCAN_SUBDIRS flood**: Simulate 50 simultaneous `MUST_SCAN_SUBDIRS` events across different subtrees. Verify: the reconciliation queue prioritizes hot paths (Documents, Desktop) over cold paths. Verify: queries during reconciliation show the "results may be incomplete" marker for affected paths. Verify: total reconciliation completes within 10 minutes on a corpus of 500K files. Verify: no duplicate documents in the index after reconciliation completes.

**Scenario 3 — Extractor crash storm**: Configure the fault injector to crash the extractor XPC process every 10th extraction. Verify: the main process detects each crash via the XPC connection interruption handler. Verify: the content channel drains correctly and enqueued work is re-submitted after process restart. Verify: files whose extraction was interrupted are marked `content_pending` in the index, not `content_indexed`. Verify: query results for those files still appear (metadata search) with a "content not indexed" annotation.

**Scenario 4 — Mid-compaction disk full**: Inject `ENOSPC` after the compaction process has written 50% of a new segment. Verify: the partial segment is detected and discarded on next startup (segment header checksum fails). Verify: the old segments remain valid and queryable. Verify: compaction retries after clearing space, and the retry succeeds without duplicate data.

**Scenario 5 — Permission revocation mid-crawl**: Start a reconciliation crawl of `~/Documents`. Midway through, revoke read permission on a subdirectory (`chmod 000`). Verify: the crawler handles `EACCES` gracefully, logs the permission failure, marks the path as `permission_denied` in the index, and continues with the rest of the reconciliation. Verify: documents previously indexed from that directory remain in the index with a `stale_permissions` flag. Verify: the user sees a clear indicator that those files may be inaccessible.

### Invariant Checking

Beyond scenario tests, define five **system invariants** that must hold at all times and write a continuous invariant checker that runs as a background thread in test builds:

```
Invariant 1 — WAL monotonicity: WAL sequence numbers are strictly increasing.
              No gaps, no duplicates.

Invariant 2 — Segment immutability: A segment file's content hash must not
              change after it is finalized. Check on every read in test mode.

Invariant 3 — Doc ID uniqueness: No two index entries share a doc_id.
              Verified after every compaction.

Invariant 4 — Trie-index consistency: Every path reachable via trie traversal
              corresponds to an existing doc_id in the inverted index.
              Verified on a 1% sample every 60s in test mode.

Invariant 5 — Delta-main disjointness: No doc_id appears in both the delta
              index and the current SegmentSet with different content.
              Verified after every micro-compaction.
```

Any invariant violation in test mode triggers `SIGABRT` with a structured fault report. In production, invariant violations are logged and trigger a targeted reindex of the affected subtree — not a crash, not silent continuation.

---

## Counter 2: Disk I/O Scheduling

The previous design handled CPU and memory explicitly but treated disk as implicit. On a search system that mmap's its index, runs background compaction, and does content extraction in parallel, uncoordinated disk access is how you get 400ms query latency spikes on a machine where "everything should be warm."

### I/O Priority Classes

macOS exposes I/O priority through two mechanisms: `setiopolicy_np()` for per-thread I/O policy, and `F_SETFL` with `O_SYNC`/`O_DSYNC` for per-file descriptor durability guarantees. The system uses four I/O priority levels, one per thread pool:

```
Thread pool         I/O policy                  Rationale
────────────────────────────────────────────────────────────────
Query threads       IOPOL_TYPE_DISK IMPORTANT   User is waiting — preempt background I/O
Indexing thread     IOPOL_TYPE_DISK DEFAULT      Normal priority — event processing is latency-sensitive
Compaction pool     IOPOL_TYPE_DISK THROTTLE     Never compete with user I/O
Extractor (XPC)     IOPOL_TYPE_DISK THROTTLE     Content reads are best-effort
```

`IOPOL_THROTTLE` is the key setting. Under `IOPOL_THROTTLE`, the kernel's I/O scheduler deprioritizes requests from this thread to the point of near-starvation when any non-throttled thread is issuing I/O. In practice, compaction and extraction stop competing with query page faults — the scheduler guarantees forward progress for query threads even during heavy background I/O.

### Read Path: mmap vs `pread` Decision

The choice between mmap and explicit `pread` calls for index reads is not aesthetic — it has concrete I/O behavior implications.

mmap is appropriate for the **inverted index hot slab** (the mlock'd portion): the OS manages page residency, prefetching happens automatically for sequential scans, and there is no syscall overhead per read. The hot slab is accessed continuously, so pages stay resident and the compressor leaves them alone.

mmap is *not* appropriate for **cold segments and the suffix array**. For these, use `pread()` with explicit read-ahead. The difference: mmap page faults are handled by the VM subsystem and block the faulting thread synchronously. `pread()` on a background thread can be issued asynchronously, allowing the query thread to continue with other work while the read completes. Concretely:

```
Cold segment access pattern:
  mmap approach:   query thread page-faults → stalls → 2–50ms latency spike
  pread approach:  background thread issues pread → query thread checks result
                   channel → if not ready, serves results from warm segments only,
                   marks cold-segment results as pending → delivers partial results
                   first, appends cold-segment results when ready
```

Partial result delivery — show what you have from warm data, then append cold-segment results as they load — is strictly better UX than stalling for 50ms waiting for a cold page fault. Users see results appearing progressively rather than a blank delay.

### Write Path: Batching and Durability

Three classes of write, each with different durability requirements and batching strategies:

**WAL writes** — highest durability, no batching. Every WAL entry is written with `O_DSYNC` (data sync — flushes data to disk without waiting for metadata). `fsync()` is not called on every write (too slow) but is called when the WAL cursor advances past a checkpoint boundary (every 256 events). This gives you a guaranteed recovery point every 256 events with minimal fsync overhead.

**Segment writes during compaction** — medium durability, aggressive batching. Write the entire segment to a temp file using `writev()` to coalesce all posting lists into a single syscall per segment. Only `fsync()` the temp file once it is fully written, then `rename()` it to its final path. `rename()` is atomic on HFS+/APFS — the segment is either fully present or absent, never partially visible. No intermediate states.

**Signal DB writes (click history)** — lowest durability, maximum batching. SQLite in WAL mode batches writes automatically. Set `PRAGMA synchronous = NORMAL` (not FULL) — a single OS crash may lose the last few click records, which is acceptable (ranking degrades slightly, not catastrophically). Set a 500ms WAL checkpoint timer. On a write-heavy session, this reduces the signal DB to roughly one `fsync()` per 500ms regardless of click volume.

### SSD vs HDD Detection and Adaptation

Detect the storage medium at startup via `IOServiceGetMatchingService` + `IORegistryEntryCreateCFProperties`, checking for `Solid State: Yes` in the drive properties. This affects two decisions:

On SSD: compaction can use larger write batches (4MB segments instead of 1MB) because sequential write performance is high and the overhead per I/O operation is low. Mmap is safe for warm segments. Aggressive prefetching (`madvise(MADV_SEQUENTIAL)`) is appropriate.

On HDD (rare in 2025 but nonzero for external drives): compaction must minimize random I/O. Segment reads are issued as sequential scans, never random-access by offset. The suffix array must live in a single contiguous file (not fragmented). The BK-tree should be serialized to disk in BFS order so tree traversal maps to sequential file reads. mmap is avoided — HDD random I/O latency (5–15ms per page fault) makes mmap page faults catastrophic on the query path.

### I/O Contention Detection

The thread registry (from the threading model) tracks `total_wait_us` per thread. Add a second counter: `io_wait_us` — time spent blocked on I/O specifically, measured via `clock_gettime()` brackets around every `pread()` and `fsync()` call on the query path. If `io_wait_us / total_query_time > 30%` for more than 10 consecutive queries, the system is I/O bound on the query path. This triggers two responses: promote the query thread's I/O policy from `IMPORTANT` to `CRITICAL` (the highest level), and emit an I/O contention alert to the health dashboard.

---

## Counter 3: Security and Permissions Model

The previous design assumed full filesystem access was granted and held indefinitely. On macOS this is wrong on both counts: access is user-granted, can be revoked at any time, and applies non-uniformly across the filesystem.

### Permission State Machine per Path

Every indexed path has an associated permission state, stored as an enum in the path trie node:

```
ACCESSIBLE      — lstat() succeeds, content readable
METADATA_ONLY   — lstat() succeeds, open() fails (restricted content)
INACCESSIBLE    — lstat() fails with EACCES or EPERM
SANDBOX_EXCLUDED — path is in a TCC-protected location, never attempt
REVOKED          — was ACCESSIBLE, now INACCESSIBLE (transition matters)
```

Transition from `ACCESSIBLE` to `REVOKED` is the dangerous case. It means documents that were indexed and appear in results can no longer be opened by the user — clicking a result produces a permission error. The system must handle this gracefully: on any `open()` failure in the result-open handler, re-check the permission state, update the trie node, and show the user a clear message ("This file is no longer accessible").

### TCC (Transparency, Consent, and Control) Integration

TCC controls access to protected locations: `~/Desktop`, `~/Documents`, `~/Downloads`, `~/Photos Library`, and the full-disk-access entitlement for everything else. The system must request TCC access correctly and handle denial without crashing.

At first launch, request Full Disk Access via `AXIsProcessTrusted()` equivalents for file access. If denied, fall back to requesting per-location access for the five hot paths. If that is also denied, operate with the default sandbox entitlement — you can index `~/Library` app containers and the app's own data but nothing else. Show the user a clear onboarding screen explaining what access is needed and what degrades without it. Never silently operate in a degraded access state.

TCC access can be revoked at any time in System Settings → Privacy & Security. Subscribe to `kTCCServiceSystemPolicyAllFiles` revocation via a `DistributedNotificationCenter` observer. On revocation: immediately suspend all crawling, mark all paths outside the remaining access scope as `SANDBOX_EXCLUDED`, and stop returning results for those paths. Re-prompt the user for access with a non-intrusive banner.

### Restricted Paths

Certain paths must never be indexed regardless of permissions held:

```
NEVER_INDEX = {
  "/private/var/",           # system internals
  "/System/",                 # sealed system volume
  "*/Keychain/",              # credentials
  "*/Cookies/",               # browser state
  "~/.ssh/",                  # private keys
  "~/.gnupg/",                # GPG keys
  "*/1Password*/",            # password managers
  "*/Bitwarden*/",            # password managers
  Any path with .pem, .key, .p12, .pfx extension in content
}
```

This list is hardcoded in the binary, not user-configurable. The reason: a user can accidentally grant access to sensitive paths and not realize what the indexer has ingested. Conservative defaults protect users from themselves. The list is checked before any path is enqueued for crawling — it is not a post-hoc filter.

### Privilege Separation

The system runs as three distinct processes with minimal privileges each:

**Main process** — holds TCC entitlements, manages index, handles queries. Does not execute arbitrary code from user files.

**Extractor XPC** — reads files, extracts text, returns structured data. Has read-only file access. Does not write to the index directly — it only returns extracted content over the XPC channel. If a malicious PDF exploits a PDFKit vulnerability, the attacker has read-only file access in a sandboxed process, not arbitrary code execution in the main process.

**Compaction helper XPC** — reads and writes index segments only. Has no filesystem access to user files. Cannot be exploited via user file content.

This privilege separation means that the most likely attack vector (malicious content in an indexed file) can at most compromise the extractor process, which has minimal capabilities. The main process and index are isolated from extractor compromises by the XPC boundary.

### Permission Failure Audit Log

Every permission failure is written to a structured audit log (append-only SQLite table, 30-day retention):

```sql
CREATE TABLE permission_audit (
  ts          INTEGER,   -- unix timestamp
  path        TEXT,
  failure     TEXT,      -- EACCES, EPERM, TCC_DENIED, etc.
  transition  TEXT       -- ACCESSIBLE→REVOKED, NEW→INACCESSIBLE, etc.
);
```

This log serves two purposes: it enables the health dashboard to show "47 paths became inaccessible in the last 7 days," which is a signal that the user changed permissions or macOS updated a TCC policy. And it provides a forensic trail if a user reports "my file disappeared from search results" — you can show them exactly when and why access was lost.

---

## Counter 4: Upgrade and Migration Strategy

Index formats change. Data structures evolve. Ranking weights get retuned. Without a migration strategy, every release that touches the index format forces a full reindex — which takes 20 minutes and destroys the user experience.

### Index Versioning Schema

Every index artifact (segment files, WAL, signal DB, trie serialization) carries a version header:

```
Artifact          Header fields
────────────────────────────────────────────────────────
Segment file      magic: u32, format_version: u16,
                  min_reader_version: u16, created_at: u64,
                  checksum: u64

WAL file          magic: u32, format_version: u16,
                  entry_format_version: u16

Signal DB         schema version tracked in SQLite user_version pragma

Trie file         magic: u32, format_version: u16, node_count: u64
```

`format_version` is the version of the data format. `min_reader_version` is the minimum application version that can read this artifact. When a newer version of the app opens an index with `min_reader_version > app_version`, it refuses to use it and downgrades gracefully (explained below).

### Three-Tier Migration Classification

Not all format changes are equal. Classify every schema change into one of three tiers before shipping:

**Tier 1 — Additive change**: New field added to segment metadata, new posting list field, new column in signal DB. Migration: old reader ignores unknown fields (forward-compatible by design — all struct parsing uses "skip unknown fields" semantics). No migration required. `format_version` increments, `min_reader_version` stays the same. Zero user impact.

**Tier 2 — Compatible change**: Field semantics change (e.g., recency score formula changes), new index structure added alongside existing one (e.g., trigram index added in v1.4). Migration: build new structure lazily as queries arrive, or build it in background during the first idle window after upgrade. The old structure remains valid and serves queries until the new one is ready. `min_reader_version` does not increase. User sees no interruption.

**Tier 3 — Breaking change**: Core segment format changes, doc_id space changes, inverted index layout changes. Migration: requires a full reindex. This must be treated as a product-level event, not an engineering detail. Mitigation strategies in order of preference:

1. Design the new format to be readable by the migration tool without a full reindex (e.g., re-encode segment format by reading old segments and re-writing to new format without re-crawling the filesystem). This is a "re-encode" migration — fast (minutes, not hours) because it reads from the existing index, not from disk.
2. If re-encode is not possible, run the new index in parallel with the old one, building in background. Serve queries from the old index until the new one is ready, then atomically swap. User sees no interruption. Requires 2× disk space temporarily.
3. Full reindex as last resort. Show a progress indicator and estimated completion time. Schedule for the next idle+plugged-in window.

### Migration Execution

The migration manager runs at application startup, before any queries are served:

```
startup:
  current_version = read_binary_version()
  index_version   = read_index_format_version()

  if index_version == current_version:
    proceed normally

  if index_version > current_version:
    // downgrade scenario (user reverted to older app version)
    if index.min_reader_version <= current_version:
      log warning, proceed (forward-compatible index, older reader)
    else:
      show "Index incompatible — rebuilding" banner
      rename existing index to index.backup/
      start fresh crawl
      serve Spotlight results during rebuild

  if index_version < current_version:
    migration = lookup_migration(index_version, current_version)
    if migration.tier <= 2:
      apply_migration_in_background()
      proceed with old index until migration completes
    elif migration.tier == 3:
      if re_encode_migration_available:
        run_re_encode_migration()   // minutes, background, no user interruption
      else:
        schedule_full_reindex_for_idle_window()
        serve Spotlight results during reindex
```

### Rollback

Every breaking change (Tier 3) must support rollback for 30 days post-release. Before a Tier 3 migration, the existing index is copied to a versioned backup directory (`~/Library/Application Support/LocalSearch/index.v{N}.backup/`). If the user downgrades the app within 30 days, the backup index is restored without a full reindex. Backups older than 30 days are deleted by the housekeeping job.

### Signal Database Migration

The signal DB (click history, per-query statistics) requires special handling because it accumulates valuable personalization data. A schema migration that wipes this DB destroys the user's ranking personalization — a silent regression that manifests as "search felt better before the update."

All signal DB migrations must be additive for the first six months of a new schema. Old columns are never dropped; new columns are added with default values. The ranking model is updated to use new columns when available, falling back to old columns when not. This gives a clean migration path even if the migration itself is never explicitly run.

---

## Counter 5: Testing Strategy

The system's correctness cannot be verified by reading the code. It requires a test suite that is treated as a first-class engineering artifact, not an afterthought.

### Test Pyramid

Four layers with explicit scope boundaries:

**Layer 1 — Unit tests** (fast, isolated, no I/O): Each data structure tested in isolation. BK-tree: verify edit distance ≤ 2 for a fixed vocabulary. Inverted index: verify BM25 scores are monotonically decreasing for decreasing term frequency. WAL: verify that a sequence of writes followed by a simulated crash and replay produces the correct final state. Roaring bitmap: verify set intersection produces the correct doc_id set. Target: 500+ unit tests, each <10ms. Total suite runtime <30s.

**Layer 2 — Integration tests** (medium speed, controlled filesystem): Use a **hermetic test filesystem** (a directory in `/tmp` populated by the test harness) with a known, fixed corpus of files. Verify end-to-end: create files → wait for FSEvents → issue query → verify results match expected set. The hermetic corpus has five properties that matter for correctness:

```
Property                Test case
─────────────────────────────────────────────────────────────────
Exact filename match    query "quarterly_report" → returns quarterly_report.pdf
Fuzzy match             query "quartely_report"  → returns quarterly_report.pdf
Scope filtering         query in ~/test/A/ doesn't return files in ~/test/B/
Content match           query "synergy" → returns doc containing "synergy"
Deletion propagation    delete a file → query no longer returns it within 5s
Rename propagation      rename a.pdf to b.pdf → query "b" returns it, "a" doesn't
```

Target: 200+ integration tests, each <2s. Total suite runtime <10min.

**Layer 3 — Index-disk parity tests** (correctness): After every integration test suite run, execute a **parity check** that independently walks the test filesystem and compares it against the index state:

```
parity_check(corpus_root):
  disk_files = walk(corpus_root)          // ground truth
  index_files = query_all_docs()          // what the index believes

  phantom = index_files - disk_files      // indexed but doesn't exist
  missing = disk_files - index_files      // exists but not indexed

  assert phantom.count == 0              // no ghost documents
  assert missing.count / disk_files.count < 0.01  // <1% miss rate
```

This test has caught more real bugs than any other test in search systems — it directly measures the gap between what the system believes and what is true. Run it after every integration test and on every CI build.

**Layer 4 — Performance benchmarks** (regression detection, not correctness): A fixed benchmark corpus (50K files, realistic distribution of file types and directory depth) with a fixed query set (200 queries sampled from realistic search patterns). Benchmarks measure:

```
Metric                    Alert threshold
──────────────────────────────────────────────────────
Query P50 latency         > 80ms   → regression alert
Query P99 latency         > 350ms  → regression alert
Full index build time     > 20min  → regression alert
Memory at steady state    > budget for tier → regression alert
Recall@10 on test set     < 0.85   → regression alert
Zero-result rate          > 5%     → regression alert
```

Benchmarks run on every merge to main. Results are stored in a time-series table. A regression alert fires when any metric crosses its threshold *and* the change is statistically significant (Mann-Whitney U test, p < 0.05) compared to the last 10 runs. This prevents noisy CI from alerting on random variance while catching real regressions reliably.

### Recall@10 Without Ground Truth Labels

The hardest testing challenge: measuring recall on a local filesystem where you have no labeled query-document pairs. Two approaches:

**Synthetic recall**: Generate query-document pairs synthetically. Create a file with known content ("This document discusses synergy in Q3 revenue"), generate the query "Q3 revenue synergy", assert the file appears in the top 10 results. This tests that your pipeline works end-to-end but doesn't measure quality on *real* user queries.

**Behavioral recall proxy**: Use the click-through data from the signal DB. A query that resulted in a click at rank ≤ 3 was likely a high-quality result. A query that resulted in a reformulation within 5 seconds was likely a failure. Track:

```
success_rate = queries_with_click_at_rank_1_to_3 / total_queries_with_clicks
failure_rate = queries_with_reformulation_within_5s / total_queries
```

These are not true recall metrics — they are behavioral proxies. But they are the *only* recall signals available on a local system without user labeling. They are good enough to detect gross regressions (a ranking change that increases failure rate by 5%) and to validate that improvements are real (a new signal that increases success rate by 3%).

### Regression Detection for Ranking Changes

Ranking changes are the most dangerous category because their effect is subtle and delayed. A change that improves average ranking for 80% of queries while worsening it for 20% will show up in aggregate metrics as an improvement — but the 20% of users who are harmed will notice immediately.

Before shipping any ranking change, run a **shadow ranking evaluation**:

1. On the current release, log the ranked result list for every query (query hash → ranked doc_id list) for 24 hours. This is the baseline.
2. Apply the proposed ranking change to a copy of the index.
3. Re-run the logged queries against the modified ranking.
4. Compute rank-biased overlap (RBO) between baseline and modified result sets for each query.
5. Identify the 10th percentile of RBO scores — the queries where the ranking changed most dramatically.
6. Manually review those queries and their new result sets.

If the 10th percentile queries look worse, the ranking change is rejected regardless of average metrics. Protecting the worst-served users is more important than improving the average. This is the lesson that every search team learns the hard way.

---

## What This System Looks Like Now

The system has crossed from "well-designed" to "production-honest":

**Failure injection** turns known-failure handling into verified behavior. The five chaos scenarios aren't aspirational — they are CI gates. A release that fails Scenario 4 (mid-compaction disk full) does not ship.

**Disk I/O scheduling** makes the implicit explicit. `IOPOL_THROTTLE` on compaction and extraction, `IMPORTANT` on query threads, `pread` over mmap for cold data, and the SSD/HDD detection fork are each a concrete, testable decision with a measurable effect on query latency under load.

**The permissions model** stops assuming access exists and starts tracking it as state. The `REVOKED` transition, the TCC revocation handler, and the restricted path list turn security from a deployment footnote into a runtime-managed property.

**Migration strategy** turns every format change from a potential support disaster into a classified, plannable event. Tier 1 ships invisibly. Tier 3 has a checklist: re-encode migration available? Backup created? Rollback tested? No more "the v2 release bricked existing indexes."

**The test pyramid** — unit, integration, parity, benchmark — gives you four independent ways to detect that something broke. The parity test in particular is the one that catches what everything else misses: the silent divergence between what the index believes and what is on disk. That is the core failure mode of every search system ever built, and the only way to catch it reliably is to measure it directly.