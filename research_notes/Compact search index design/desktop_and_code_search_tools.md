# Index architectures of desktop file-search and local code-search tools

Notes compiled 2026-10-02. Every figure is attributed to the page it came from. Where a page was read through a fetch-and-summarise tool, the quoted strings are as returned by that tool; a few are flagged where I could not confirm the surrounding context. Items I know only from memory and could not re-source in this session are listed under Gaps, not under Cited Findings.

Reference point for the reader (from the assignment, not researched): the app under redesign holds about 172 MB of inverted index in RAM for about 60,000 files (roughly 2.9 KB per file), loads in 1.4 s, and answers in 2-13 ms.

## 1. Apple Spotlight: on-disk store, process split, importers, update path, index size

### Takeaway
Spotlight keeps its index on disk per volume in `.Spotlight-V100/Store-V2/<UUID>/` as roughly a hundred files (a proprietary `store.db` metadata store plus separate dictionary, index and posting files), with a small "live" index for recent changes that is merged into static tables later. The format is undocumented by Apple and known only from reverse engineering; I found no official or measured figure for index size beyond one anecdote (2.8 GB, 6.2% of storage).

### Cited Findings
- Volume indexes live in a hidden `.Spotlight-V100` folder at the root of each indexed volume; it contains `VolumeConfiguration.plist` (exclusions, options, a Stores dictionary keyed by UUID) and `Store-V2/<UUID>/`, which holds "around 99 files and folders". — [Eclectic Light, "A deeper dive into Spotlight indexes", 30 Jul 2025](https://eclecticlight.co/2025/07/30/a-deeper-dive-into-spotlight-indexes/)
- `store.db` and `.store.db` are not SQLite: "store.db uses a proprietary format that has been reversed by Yogesh Khatri". The dictionaries, indexes and postings are spread over multiple other files rather than held in `store.db`. — [Eclectic Light, 30 Jul 2025](https://eclecticlight.co/2025/07/30/a-deeper-dive-into-spotlight-indexes/)
- Update design: recently changed content goes into transient posting tables (a "live index" tuned for frequent updates), which are periodically folded into more static tables. Deleted files are not removed immediately; removal happens "when the store next undergoes housekeeping". — [Eclectic Light, 30 Jul 2025](https://eclecticlight.co/2025/07/30/a-deeper-dive-into-spotlight-indexes/)
- Database locations per the reverse-engineering parser: `/.Spotlight-V100/Store-V2/<UUID>/` with `store` and `.store` database files on macOS; since macOS 10.13 also per-user databases under `~/Library/Metadata/CoreSpotlight/index.spotlightV3/`; iOS under `/private/var/mobile/Library/Spotlight/CoreSpotlight/.../index.spotlightV2`. The parser depends on `lz4` and `pyliblzfse`, i.e. store blocks are compressed with LZ4 and LZFSE. — [ydkhatri/spotlight_parser README](https://github.com/ydkhatri/spotlight_parser)
- Apple does not document the format; the parser README points to a peer-reviewed paper, "Investigating spotlight internals to extract metadata", as the reverse-engineering reference. — [ydkhatri/spotlight_parser README](https://github.com/ydkhatri/spotlight_parser)
- Process split: `mds` coordinates and serves the indexes, `mds_stores` manages the database (and compresses extracted text), `mdworker` processes do the per-file extraction using the file type's `mdimporter` plugin. — [Eclectic Light, "A deeper dive into Spotlight indexing and local search", 4 Aug 2025](https://eclecticlight.co/2025/08/04/a-deeper-dive-into-spotlight-indexing-and-local-search/) (read via search-result summary, not full fetch)
- What an `mdworker` extracts: file attributes (datestamps), extended attributes, structured metadata defined by the importer (for example EXIF), and text content exported by the importer. — [Eclectic Light, 4 Aug 2025](https://eclecticlight.co/2025/08/04/a-deeper-dive-into-spotlight-indexing-and-local-search/)
- Update trigger: a write recorded in the FSEvents database triggers an XPC call to process the file if it is in Spotlight's scope. — [Eclectic Light, 4 Aug 2025](https://eclecticlight.co/2025/08/04/a-deeper-dive-into-spotlight-indexing-and-local-search/)
- Measured incremental latency (log-timed, one author's Mac): first `mdworker` spawned within about 1 s of the file change; workers active about 0.2 s before their new posting lists are ready to add to the volume index; `mds_stores` finishes compressing content about 6.7 s after the files were created; "full indexing cycle: approximately 7 seconds". — [Eclectic Light, 4 Aug 2025](https://eclecticlight.co/2025/08/04/a-deeper-dive-into-spotlight-indexing-and-local-search/); [Eclectic Light, "Spotlight and Core Spotlight are different", 4 Jul 2026](https://eclecticlight.co/2026/07/04/spotlight-and-core-spotlight-are-different/)
- Core Spotlight is a separate system: per-user, per-app indexes under `~/Library/Metadata/CoreSpotlight`, written by `corespotlightd` when apps call `indexSearchableItems()`; `mdutil` does not manage it. Core Spotlight has its own journal directory. — [Eclectic Light, 4 Jul 2026](https://eclecticlight.co/2026/07/04/spotlight-and-core-spotlight-are-different/); [Eclectic Light, 30 Jul 2025](https://eclecticlight.co/2025/07/30/a-deeper-dive-into-spotlight-indexes/)
- Only size data point found: a blog commenter reported a 2.8 GB Spotlight index, 6.2% of total storage. This is an anecdote from one machine, not a measurement by the author. — [Eclectic Light, 30 Jul 2025 (comments)](https://eclecticlight.co/2025/07/30/a-deeper-dive-into-spotlight-indexes/)

### Inferences
- Spotlight is an LSM-like design: small mutable live index plus large immutable-ish static posting files, with lazy deletes cleaned at "housekeeping". This is the same shape as Lucene/Tantivy segments and FTS5 levels (sections 5), reached independently by the system this app competes with.
- Keeping metadata (`store.db`) in a different structure from content postings mirrors the split the app could make between a small always-resident filename/metadata index and an on-disk content index.
- Text is stored compressed by `mds_stores` (LZ4/LZFSE dependencies in the parser support this), so Spotlight pays disk rather than RAM for the content copy.

### Gaps
- No Apple documentation of the store format, posting encoding, or memory mapping behaviour was found. Whether `mds_stores` memory-maps the index files is not stated in any source I read.
- No published resident-memory figure for `mds`/`mds_stores` and no query latency figure was found.
- No reliable index-size-to-data ratio: only the single 6.2% anecdote. Treat "typical Spotlight index size" as unknown.
- The first Eclectic Light URL I tried for the 4 Aug 2025 article returned 404; its content above comes from the search engine's summary of the correct URL, not a full read. Worth re-reading before quoting timings.
- Apple's archived Spotlight Importer Programming Guide was not fetched in this session.

## 2. voidtools Everything (Windows): filename index, memory per file, content indexing

### Takeaway
Everything is the clearest example of a pure in-RAM index: about 100 bytes of RAM and 45 bytes of disk per file for names, kept current via the NTFS USN journal. Its content indexing (1.5 alpha) simply stores all extracted text in RAM, costing roughly the size of the text itself, which is the model the app under redesign should move away from.

### Cited Findings
- "about 35 MB of ram and less than 14 MB of disk space" for 250,000 files (fresh Windows 11); "about 100 MB of ram and 45 MB of disk space" for 1,000,000 files. That is about 100-140 bytes RAM and 45-56 bytes disk per file. — [voidtools FAQ](https://www.voidtools.com/faq/)
- Initial index build: "about 5 seconds" for 250,000 files, "about 1 minute" for 1,000,000 files. — [voidtools FAQ](https://www.voidtools.com/faq/)
- Updates: "Everything will automatically keep your NTFS indexes up to date with the NTFS USN Journal. Changes will not be missed when Everything is not running as the system maintains the NTFS USN Journal." — [voidtools FAQ](https://www.voidtools.com/faq/)
- Storage model: "Everything stores the database in memory when running"; the database (`%LOCALAPPDATA%\Everything\Everything.db`) "is only saved to disk when you exit Everything". Name and path are always indexed; size, dates and attributes are optional and cost extra memory (shown per option in a tooltip, no figures on the page). — [voidtools Indexes documentation](https://www.voidtools.com/support/everything/indexes/)
- Stable 1.4: "File content is not indexed, searching content is slow" (content search reads files at query time). — [voidtools FAQ](https://www.voidtools.com/faq/)
- Everything 1.5 (alpha) content indexing: content is stored in the database and the whole database is loaded in RAM; the developer's rule of thumb is that the total size of the files selected for content indexing is roughly the extra RAM needed. Baseline restated as roughly 100 MB per 1 million items for name, path, size and date modified. — [voidtools forum, "Everything 1.5.0.1391a consuming a lot of memory"](https://voidtools.com/forum/viewtopic.php?p=74571); [voidtools forum, "Using 1,3 GByte RAM for Everything 1.5, is this normal?"](https://www.voidtools.com/forum/viewtopic.php?p=75831)

### Inferences
- At Everything's density, a filename-only index for the app's 60,000 files would be about 6-8 MB of RAM. The app's 172 MB is therefore almost entirely content postings, not names; a redesign can keep names fully in RAM at negligible cost.
- Everything 1.5's content option is not an inverted index at all (it keeps raw text and scans it), so it offers no lesson on compact postings, only confirmation that RAM-resident content scales with corpus size.

### Gaps
- The FAQ fetch did not return the MFT-reading description; that Everything builds its initial index by reading the NTFS Master File Table directly is widely stated but not re-sourced here.
- No official statement of the in-memory data structure (sorted name arrays, etc.) was found.
- No query latency figures are published by voidtools.

## 3. Recoll, Tracker/TinySPARQL, Baloo, DocFetcher, Windows Search

### Takeaway
The Linux and Windows desktop indexers all keep the index on disk in a general-purpose store (Xapian B-trees, SQLite, LMDB, ESE/SQLite) and their well-known problems are indexing-time memory spikes and index bloat, not query-time RAM. Documented size ratios cluster around "index is smaller than the text": Windows Search about 10% of content, Xapian "somewhat smaller than the source", Lucene 20-30% of text.

### Cited Findings
Recoll (Xapian)
- Index size only becomes a concern above about 10 GB ("10 GBytes would be around 4000 bibles"); "in 2025, we heard of a 550 GB, 11+ million documents index". — [Recoll manual, index storage for big indexes](https://www.recoll.org/usermanual/webhelp/docs/RCL.INDEXING.STORAGE.BIG.html)
- "the text content of PDFs is typically less than 5% of the file size". — [Recoll manual](https://www.recoll.org/usermanual/webhelp/docs/RCL.INDEXING.STORAGE.BIG.html)
- "The amount of writing performed by Xapian during index creation is not linear with the index size (it is somewhere between linear and quadratic)"; Xapian 1.4.10 or newer reduces writes significantly. — [Recoll manual](https://www.recoll.org/usermanual/webhelp/docs/RCL.INDEXING.STORAGE.BIG.html)
- Indexer memory is bounded by `idxflushmb`, the megabytes of new text accumulated before flushing to disk; default 100 MB, can be set as low as 10 to save memory, 200 or more recommended for very large indexes. Recoll flushes by data volume because memory use depends on document size, not count. — [Recoll manual, performance parameters](https://www.recoll.org/usermanual/webhelp/docs/RCL.INSTALL.CONFIG.RECOLLCONF.PERFS.html)

Xapian
- "Typically the Xapian database will be somewhat smaller than the source data" (glass backend, 1.4.x); without positional information the database "will probably be about 2/3 the size"; `xapian-compact` shrinks a database by packing blocks and dropping unused ones. — [Xapian FAQ: DatabaseSize](https://trac.xapian.org/wiki/FAQ/DatabaseSize)

Baloo (KDE, LMDB)
- Two overlapping databases: PostingDB (term to list of doc ids) and PositionDB (term to list of doc id plus positions); the PostingDB data is fully redundant with PositionDB. — [KDE Baloo merge request 91, "Use MDB_DUPSORT for PostingDB"](https://invent.kde.org/frameworks/baloo/-/merge_requests/91)
- Each posting list is one LMDB value, so adding a document means fetching, modifying and rewriting the whole entry for every term, "leading to huge IO usage" for frequent terms. — [KDE Baloo MR 91](https://invent.kde.org/frameworks/baloo/-/merge_requests/91)
- Baloo indexes in batches of 40 files held on the heap, then does the read-modify-write; a batch of 40 large (about 10 MB) text files "can easily lead to gigabytes" of memory; a reported case used 9 GiB RAM indexing a folder of about 650 files. — [KDE Phabricator T11859](https://phabricator.kde.org/T11859)
- LMDB never shrinks the file; freed pages go to a freelist, giving roughly a 30% gap between actual and expected index size after indexing. — [KDE Phabricator D16876](https://phabricator.kde.org/D16876)
- A crash was reported whenever the index exceeded 5 GB. — [KDE bug 364475](https://bugs.kde.org/show_bug.cgi?id=364475)

Tracker / TinySPARQL (GNOME LocalSearch)
- TinySPARQL is "a light-weight RDF triple store implementation with a SPARQL 1.1 interface" and is the storage backend of the Tracker Miner FS file indexer. — [GNOME Tracker API overview](https://tracker.api.gnome.org/overview.html)

Windows Search
- Database is `Windows.edb` (Windows 10, ESE) or `Windows.db` (Windows 11) in `C:\ProgramData\Microsoft\Search\Data\Applications\Windows`. — [Microsoft Learn, "Troubleshoot Windows Search performance", updated Feb 2026](https://learn.microsoft.com/en-us/troubleshoot/windows-client/shell-experience/windows-search-performance-issues)
- "The index is generally 10 percent of the size of the content that is being indexed." — [Microsoft Learn](https://learn.microsoft.com/en-us/troubleshoot/windows-client/shell-experience/windows-search-performance-issues)
- Scale guidance: typical user fewer than 30,000 items; power user up to 300,000; performance issues may begin above 400,000; hard design limit about 1 million items, beyond which the Indexer "may fail or cause resource problems". Compression becomes less effective as the database grows. A full rebuild can take up to 24 hours. — [Microsoft Learn](https://learn.microsoft.com/en-us/troubleshoot/windows-client/shell-experience/windows-search-performance-issues)
- Reclaiming space requires an offline defragmentation (`EsentUtl.exe /d`) with the service stopped. — [Microsoft Learn](https://learn.microsoft.com/en-us/troubleshoot/windows-client/shell-experience/windows-search-performance-issues)

DocFetcher (Lucene)
- Lucene's own stated figures: "index size roughly 20-30% the size of text indexed"; "small RAM requirements -- only 1MB heap"; "incremental indexing as fast as batch indexing". — [Apache Lucene features page](https://lucene.apache.org/core/features.html)

### Inferences
- The Baloo failure mode is a direct warning for a redesign: storing one mutable posting list per term in a key-value store makes every document update rewrite large values. Segment-based designs (Lucene, Tantivy, FTS5, Spotlight) avoid this by never mutating posting lists in place.
- The app's 60,000 files sit at the low end of what Microsoft calls a typical-to-power-user index, so any of these on-disk architectures is well within its comfort zone.
- Ratios are measured against extracted text, not file bytes. For a PDF-heavy corpus (text under 5% of file size per Recoll) the index will look tiny relative to the files on disk.

### Gaps
- No documented resident-memory figures at query time for Recoll, Baloo, Tracker or Windows Search were found; the published complaints concern indexing.
- Tracker/LocalSearch: I did not find a sourced statement in this session of its full-text mechanism or index size. From memory it uses SQLite with the FTS5 extension; treat as unverified.
- DocFetcher itself publishes no index size or memory figures that I found; only Lucene's generic claims are cited. The "1MB heap" claim is a long-standing marketing line and refers to the minimum indexing buffer, not a realistic process footprint.
- Whether Windows 11's `Windows.db` is SQLite-based is asserted by third-party sites; the Microsoft page names the file but not the engine.

## 4. Code search engines: trigram, positional trigram, suffix array, sparse n-grams, no index

### Takeaway
Code search tools trade index size for substring/regex capability: a document-level trigram index costs about 20% of the corpus (Russ Cox), a positional trigram index about 2x the corpus plus a copy of the content (Zoekt, 3-3.5x total on disk) but can be memory-mapped so that resident memory is a few percent of that, and a suffix array costs more RAM still. Zoekt's 2021 optimisation shows the resident part can be squeezed to about 310 KB per repository by replacing hash maps with sorted arrays.

### Cited Findings
Google Code Search / Russ Cox `codesearch`
- Index is a list of files plus trigram posting lists (trigram to the documents containing it); queries are converted to AND/OR trigram queries and candidates are verified by running the regex. — [Russ Cox, "Regular Expression Matching with a Trigram Index", Jan 2012](https://swtch.com/~rsc/regexp/regexp4.html)
- Size: Linux 3.1.3 kernel source, 420 MB, produces "a 77 MB index" (about 18%); generally "this index tends to be around 20% of the size of the files being indexed". — [Russ Cox](https://swtch.com/~rsc/regexp/regexp4.html)
- Access: "csearch uses mmap to map the index into memory and in doing so read directly from the operating system's file cache." — [Russ Cox](https://swtch.com/~rsc/regexp/regexp4.html)
- Selectivity: a "hello world" search over the kernel is narrowed from 36,972 files to 25 candidates, "about 100x" faster than brute force. — [Russ Cox](https://swtch.com/~rsc/regexp/regexp4.html)
- Updates: `cindex` adds to an existing index and can rescan previously indexed directories; it writes a new index and merges. — [Russ Cox](https://swtch.com/~rsc/regexp/regexp4.html)

Zoekt (Google, now Sourcegraph)
- Index is "an index of ngrams (n=3), where we store the offset of each ngram's occurrence within a file", so a substring query checks that trigrams occur at the right distance apart. — [sourcegraph/zoekt doc/design.md](https://raw.githubusercontent.com/sourcegraph/zoekt/main/doc/design.md)
- Shards are laid out so they "can be mmap'd efficiently" and hold file contents, file names, posting lists, branch masks and metadata; uint32 offsets cap a shard at 4 GB and its content at 1 GB. — [zoekt design.md](https://raw.githubusercontent.com/sourcegraph/zoekt/main/doc/design.md)
- Size: index is "about 3x the corpus size, composed of 2x (offsets), and 1x (original content)"; in practice "the shard size is about 3.5x the corpus size". — [zoekt design.md](https://raw.githubusercontent.com/sourcegraph/zoekt/main/doc/design.md)
- The design doc contrasts this with suffix arrays and gives "only 1.2x corpus size of RAM" for the positional trigram approach (as extracted; this is the original design-time figure, before the mmap and 2021 work below). — [zoekt design.md](https://raw.githubusercontent.com/sourcegraph/zoekt/main/doc/design.md)
- Ranking signals: match count, proximity, word-boundary matches, update time, filename length, symbol/token context; optional BM25 scoring. — [zoekt design.md](https://raw.githubusercontent.com/sourcegraph/zoekt/main/doc/design.md)
- Updates are per repository: changed repositories are reindexed into new shards; there is no in-place document update. — [zoekt design.md](https://raw.githubusercontent.com/sourcegraph/zoekt/main/doc/design.md)
- Sourcegraph's measured resident memory (19 Aug 2021): a server holding 19,000 repositories, 2.6 billion lines, 166 GB on disk went from 22 GB to 4 GB of live heap, i.e. from 1,400 KB to 310 KB per repository. — [Sourcegraph blog, "A 5x reduction in RAM usage with Zoekt memory optimizations"](https://sourcegraph.com/blog/zoekt-memory-optimizations-for-sourcegraph-cloud/)
- Where the RAM went: `readNgrams`, which builds the in-memory map from trigram to posting-list location on disk, was 67% of memory. Go maps cost "roughly 40 bytes each" per entry. — [Sourcegraph blog](https://sourcegraph.com/blog/zoekt-memory-optimizations-for-sourcegraph-cloud/)
- Step-by-step savings: map replaced by sorted arrays with binary search 15 GB to 5 GB; splitting the 64-bit n-gram keys 5 GB to 3.5 GB; separate ASCII and Unicode trigram tables 3.5 GB to 2.3 GB; filename posting compression 3.3 GB to 1.1 GB; byte-offset table compression 2.3 GB to 0.2 GB; trimming slice capacity saved 500 MB. — [Sourcegraph blog](https://sourcegraph.com/blog/zoekt-memory-optimizations-for-sourcegraph-cloud/)

livegrep
- Index is a set of suffix arrays (not one) over the concatenated corpus, plus a sorted "file content map" of (start, end, file) ranges; input is compressed by deduplicating identical lines; built with libdivsufsort. No size or RAM numbers are given. — [Nelson Elhage, "Regular Expression Search with Suffix Arrays", Feb 2015](https://blog.nelhage.com/2015/02/regular-expression-search-with-suffix-arrays/)
- Weak spot: queries that filter heavily by path but make poor use of the suffix array are the slowest. — [Nelson Elhage](https://blog.nelhage.com/2015/02/regular-expression-search-with-suffix-arrays/)

GitHub Blackbird
- Scale: 115 TB of code, 15.5 billion documents, 45 million repositories; after content-addressed deduplication about 28 TB unique; the whole index is "just 25 TB, which includes not only all the indices (including the ngrams), but also a compressed copy of all unique content", i.e. under 1x of unique content and about a quarter of raw. — [GitHub blog, "The technology behind GitHub's new code search", Feb 2023](https://github.blog/engineering/architecture-optimization/the-technology-behind-githubs-new-code-search/)
- Latency and throughput: shard p99 "on the order of 100 ms"; about 640 queries per second on a 64-core host; ingest about 120,000 documents per second; full reindex about 18 hours thanks to delta indexing. — [GitHub blog](https://github.blog/engineering/architecture-optimization/the-technology-behind-githubs-new-code-search/)
- Brute-force baseline quoted there: ripgrep over a 13 GB cached file takes "2.769 seconds, or about 0.6 GB/sec/core". — [GitHub blog](https://github.blog/engineering/architecture-optimization/the-technology-behind-githubs-new-code-search/)

### Inferences
- ripgrep's quoted 0.6 GB/s/core gives a useful floor: if the app's 60,000 files contain a few hundred MB of extracted text, a brute-force scan of a compressed on-disk text copy would take on the order of a second single-threaded, which is too slow for interactive use but fine as a verification step over a few dozen candidates. That is exactly how trigram indexes are used (filter, then verify).
- For a desktop corpus the document-level trigram index (about 20% of text, mmap'd) is the cheapest way to get substring search; the positional variant's 2x cost buys speed on huge corpora that this app does not have.
- The Zoekt optimisation list is directly transferable to a Rust in-RAM dictionary: sorted arrays instead of hash maps, separate compact tables for ASCII keys, and delta-compressed offset tables removed more than 80% of resident memory without touching the on-disk posting format.
- Zoekt after optimisation keeps about 4 GB resident for 166 GB on disk, roughly 2.4%. If the 166 GB is index size (see Gaps), a dictionary-in-RAM, postings-on-disk split costs a few percent of index size in RAM.

### Gaps
- Sourcegraph's "166 GB" was returned as "disk size"; I could not confirm from the extract whether it is corpus size or shard size on disk. The per-repository figures (1,400 KB to 310 KB) are unambiguous.
- Zoekt's "1.2x corpus size of RAM" was extracted without its surrounding sentence; verify wording before quoting. It predates memory-mapped shards being the norm.
- The GitHub article describes "sparse grams" (variable-length n-grams chosen by a weight function so fewer, more selective grams are looked up); the fetch did not return those details, so the mechanism is stated here from memory of the same article and should be re-read before use.
- livegrep publishes no index-to-corpus ratio in the post read. A search snippet attributed "3x the corpus size" to livegrep, but that text is Zoekt's design doc describing Zoekt; I found no livegrep-specific number.
- ripgrep: no index by design; I did not fetch a primary source for this beyond GitHub's benchmark line.

## 5. Embedded full-text libraries: Tantivy, SQLite FTS5, Xapian, Lucene, Meilisearch, Sonic

### Takeaway
Every one of these keeps the index on disk and reads it through the OS page cache (mmap or a B-tree pager), uses immutable segments or levels with background merges, and handles deletes with tombstones. Documented index sizes run from about 8% of the text (FTS5 `detail=none`) through 20-30% (Lucene) to about 45% (FTS5 with full positions), so a 172 MB in-RAM index for this corpus could become tens of MB on disk with single-digit MB resident.

### Cited Findings
SQLite FTS5
- The index is "a series of b-trees" inside the SQLite database file. A level-0 b-tree holds one transaction's terms; b-trees at the same level are merged into the next level. `automerge` default 4 (merge starts when 4 b-trees share a level, work spread over later writes); `crisismerge` default 16 (merge done immediately); `usermerge` default 4; index pages are blobs of `pgsz` default 4050 bytes. — [SQLite FTS5 documentation](https://www.sqlite.org/fts5.html)
- `detail` option, measured by the SQLite authors on an email corpus of 1636 MiB: `detail=full` (rowid, column, offset) index 743 MiB (45% of text); `detail=column` (rowid, column) 340 MiB (21%); `detail=none` (rowid only) 134 MiB (8%). — [SQLite FTS5 documentation](https://www.sqlite.org/fts5.html)
- Cost of dropping detail: `detail=column` loses phrase and NEAR queries; `detail=none` also loses column filters. — [SQLite FTS5 documentation](https://www.sqlite.org/fts5.html)
- Contentless tables (`content=''`) store only the index, not the text; originally no UPDATE or DELETE. Since SQLite 3.43.0, `contentless_delete=1` allows DELETE and INSERT OR REPLACE (UPDATE must supply all columns). Deletes attach a tombstone with the rowid to the b-tree; tombstoned rows are dropped during merges; `deletemerge` default 10 (percent of tombstoned rows that makes a b-tree eligible for merging). — [SQLite FTS5 documentation](https://www.sqlite.org/fts5.html)
- External-content tables (`content='table'`) index text held in another table and must be kept in sync by the application; `rebuild` regenerates the index. `columnsize=0` drops per-row token counts (saves space, but those counts are what BM25 uses). `secure-delete` (3.42.0+) removes entries immediately at a speed cost. — [SQLite FTS5 documentation](https://www.sqlite.org/fts5.html)
- A built-in `trigram` tokenizer gives substring, LIKE and GLOB matching; queries shorter than 3 characters cannot use it. — [SQLite FTS5 documentation](https://www.sqlite.org/fts5.html)
- A SQLite forum report of a real database: FTS data just under 32% of the total database with defaults, about 7.6% with `detail=none`. — [SQLite forum thread](https://sqlite.org/forum/info/3baccecae55769ff) (seen as a search snippet; which of the three listed forum threads carries the figure was not confirmed)

Tantivy
- An index is "a collection of smaller independent immutable segments" listed in `meta.json`; on commit, one segment per indexing thread is written and `meta.json` is replaced atomically. — [Tantivy ARCHITECTURE.md](https://raw.githubusercontent.com/quickwit-oss/tantivy/main/ARCHITECTURE.md)
- Storage goes through a `Directory` trait with `MmapDirectory` and `RamDirectory`; "loading an index is as fast as mmapping its files". — [Tantivy ARCHITECTURE.md](https://raw.githubusercontent.com/quickwit-oss/tantivy/main/ARCHITECTURE.md)
- Deletes are by term; on commit the matching documents are cleared in a per-segment alive bitset file (`segment_id.commit_opstamp.del`); background merges eventually drop tombstoned documents. Hundreds of segments measurably slow search. — [Tantivy ARCHITECTURE.md](https://raw.githubusercontent.com/quickwit-oss/tantivy/main/ARCHITECTURE.md)
- Structures: FST term dictionary mapping term to ordinal to term info; postings in blocks of 128 documents, "doc ids delta encoded and bitpacked", term frequencies bitpacked, last partial block variable-length encoded; positions in a separate file; field norms one byte per document per field; bit-packed columnar fast fields; LZ4-compressed doc store. — [Tantivy ARCHITECTURE.md](https://raw.githubusercontent.com/quickwit-oss/tantivy/main/ARCHITECTURE.md)

Lucene
- "index size roughly 20-30% the size of text indexed"; "small RAM requirements -- only 1MB heap"; "incremental indexing as fast as batch indexing". — [Apache Lucene features page](https://lucene.apache.org/core/features.html)

Xapian
- Database "somewhat smaller than the source data" with positions, about 2/3 of that without; compaction needed to reclaim block slack. — [Xapian FAQ: DatabaseSize](https://trac.xapian.org/wiki/FAQ/DatabaseSize)

Meilisearch (LMDB)
- LMDB stores data in a memory-mapped file; reads are returned straight from the map with no allocation or copy. Deleting documents does not shrink the file: LMDB marks pages free for reuse and never returns them to the OS. — [Meilisearch docs, storage internals](https://www.meilisearch.com/docs/resources/internals/storage)
- Vendor guidance: a RAM-to-disk ratio around 1/3 does not materially hurt performance and about 1/10 works for many workloads; it does not crash when the index exceeds RAM. — [Meilisearch docs, storage internals](https://www.meilisearch.com/docs/resources/internals/storage)

Sonic
- Inverted index in RocksDB (LSM tree); a per-bucket FST for word suggestion/autocomplete that is stored on disk and memory-mapped. It is an identifier index: it returns ids, stores no documents and does no relevance scoring comparable to BM25. — [valeriansaliou/sonic README](https://github.com/valeriansaliou/sonic)
- Author's figures: about 30 MB RAM under load with microsecond-range queries; a production instance with half a billion objects has a 20 GB compressed index and about 200 MB RAM worst case, 20 MB cold-started; a single-thread benchmark indexing 1M records under 1k rps search stayed under 28 MB. These are self-reported by the author, not independently measured. — [valeriansaliou/sonic README](https://github.com/valeriansaliou/sonic)

### Inferences
- The FTS5 table is the most useful single dataset for the redesign: positions cost about 55% of the index (743 vs 340 MiB) and column tags a further 25% (340 vs 134 MiB). If the app's 172 MB index stores positions, dropping or sidelining them is the largest available saving, at the price of phrase queries (which could be verified against stored text instead).
- All of these get low resident memory the same way: only a term dictionary (FST or B-tree interior pages) is hot, postings are paged in on demand. None documents a "resident memory for a small index" number, but the structure implies single-digit to low-tens of MB for a 60,000-document corpus, consistent with Sonic's 20-30 MB and Zoekt's 2.4%.
- Cheap incremental update is solved identically everywhere: append a small new segment, tombstone the old document, merge later. FTS5's `automerge` (amortised) vs `crisismerge` (blocking) split is a ready-made policy template.
- LMDB-style stores (Meilisearch, Baloo) trade away file shrinkage and can suffer from large-value rewrites; segment files (Tantivy, Lucene) reclaim space on merge.
- FTS5 contentless-delete plus `detail=column` or `none` is the lowest-effort route in Rust (via rusqlite) to an on-disk index at 8-21% of text, with BM25 built in; Tantivy is the route if phrase queries, per-field norms and custom scoring must be kept.

### Gaps
- Tantivy publishes no index-size-to-text ratio or resident-memory figure in ARCHITECTURE.md; I found none elsewhere in this session.
- No independent, like-for-like benchmark of Tantivy vs FTS5 vs Xapian on index size and RSS was found. A search snippet claimed FTS5 is "5-140x faster" than a Tantivy-backed FTS inside Turso, but the Turso post I fetched ([turso.tech/blog/beyond-fts5](https://turso.tech/blog/beyond-fts5)) contains no numbers, so that claim is unverified and should not be used.
- FTS5's in-memory pending-terms hash (`hashsize`) default was not returned by the fetch; not cited.
- Meilisearch publishes no index-size-to-dataset ratio on the page read; from memory its indexes are several times the raw data, but that is unsourced here.

## 6. Published measurements on roughly 50k-1M documents on a laptop

### Takeaway
I found almost nothing measured at this scale on laptop hardware. The usable anchors are vendor or author figures: Everything (250k and 1M files), SQLite's FTS5 detail table (1.6 GiB of email), Russ Cox's kernel index (420 MB), Sonic's 1M-record run, and Microsoft's item-count guidance.

### Cited Findings
- Everything, 250,000 files: about 35 MB RAM, under 14 MB disk, about 5 s to index; 1,000,000 files: about 100 MB RAM, 45 MB disk, about 1 minute. Filenames only. — [voidtools FAQ](https://www.voidtools.com/faq/)
- FTS5 on 1636 MiB of email: index 743 / 340 / 134 MiB for detail full / column / none. Document count and hardware not stated. — [SQLite FTS5 documentation](https://www.sqlite.org/fts5.html)
- Trigram index of the Linux 3.1.3 kernel (420 MB, 36,972 files matched by a full scan): 77 MB index. — [Russ Cox](https://swtch.com/~rsc/regexp/regexp4.html)
- Sonic: 1M records indexed while serving 1k rps, under 28 MB RAM (author's benchmark). — [valeriansaliou/sonic README](https://github.com/valeriansaliou/sonic)
- Windows Search: under 30,000 items typical, 300,000 power user, degradation above 400,000, index about 10% of content. — [Microsoft Learn](https://learn.microsoft.com/en-us/troubleshoot/windows-client/shell-experience/windows-search-performance-issues)
- Spotlight incremental update: about 7 s from file write to searchable, on one Mac. — [Eclectic Light, 4 Jul 2026](https://eclecticlight.co/2026/07/04/spotlight-and-core-spotlight-are-different/)

### Inferences
- Scaling the cited ratios to the app (unknown text volume T): on-disk index of roughly 0.08T-0.45T with FTS5, 0.2T-0.3T with Lucene-class postings, 0.2T for a document-level trigram index. The app's current 172 MB in RAM can be compared against these once T is measured; that measurement is the missing input for the redesign.
- The app's 2-13 ms query time is already better than anything these tools publish for themselves (most publish nothing), so the redesign's latency budget has room: an mmap'd index that adds a few milliseconds of page faults on cold queries would still be competitive.
- Because no neutral benchmark exists at this scale, the team should plan to measure candidates (FTS5 contentless, Tantivy mmap, custom mmap'd postings) on their own 60,000-file corpus rather than rely on published numbers.

### Gaps
- No independent laptop-scale benchmark comparing embedded libraries on index size, RSS and latency together was found.
- No published resident-memory numbers for Spotlight, Recoll, Baloo, Tracker, DocFetcher or Windows Search at query time.
- Query latency is rarely published for desktop tools at all; only Blackbird (server, about 100 ms p99 per shard) and Sonic (sub-millisecond, self-reported) give figures.
