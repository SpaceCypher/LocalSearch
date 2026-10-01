# Index update architectures and document reordering (state as of October 2026)

Reader note on evidence quality: several primary papers (Mackenzie et al. ECIR 2019, Yan/Ding/Suel WWW 2009, Ding/Attenberg/Suel WWW 2010, Moffat & Mackenzie 2022 full text) are PDFs that could not be parsed in this session; only their abstracts, search-result summaries, or HTML mirrors were read. Where a number comes from an automated page summary rather than from a table I read directly, it is flagged. Anything I recall but did not verify is in "Gaps", not "Cited Findings".

## 1. How do Lucene and Tantivy handle updates (segments, buffer, deletes, merge policies, NRT), and what does it cost?

### Takeaway
Both use the same design: write-once segments, an in-RAM indexing buffer that is flushed to a new small segment, deletes recorded as a per-segment bitset and filtered at query time, and background merges that physically drop deleted documents. The costs are write amplification from merging, wasted space and some query slowdown from not-yet-purged deletes, and RAM for the buffer.

### Cited Findings
- Lucene: a delete or an update (= delete + add) only marks a bit in a per-segment bitset; searches skip those documents; space is reclaimed only when segments merge. This is done because updating write-once structures and aggregate statistics per deletion would be far too costly. — [Elastic blog, McCandless, "Lucene's Handling of Deleted Documents"](https://www.elastic.co/blog/lucenes-handling-of-deleted-documents)
- Costs of unpurged deletes listed there: they tie up disk; per-document in-memory structures (norms, field data) still consume RAM for them; search throughput is lower because every candidate hit is checked against the bitset. In the blog's test with 50% deleted documents the slowdown was "typically quite a bit lower than the percentage deletes" because deleted docs are filtered before the expensive matchers/scorers. Forcing reclamation (expungeDeletes) is described as "very costly". — [same](https://www.elastic.co/blog/lucenes-handling-of-deleted-documents)
- Lucene TieredMergePolicy defaults (Lucene 10.1 javadoc): segmentsPerTier 10; maxMergeAtOnce 10; maxMergedSegmentMB 5 GB; floorSegmentMB 2 MB; deletesPctAllowed 20% (allowed range 5–50%); forceMergeDeletesPctAllowed 10%. It merges by byte size and pro-rates by percentage of deletes. Lowering deletesPctAllowed makes merges more frequent and raises the write amplification factor (number of times each document is written), with more CPU and I/O. — [Lucene TieredMergePolicy javadoc](https://lucene.apache.org/core/10_1_0/core/org/apache/lucene/index/TieredMergePolicy.html)
- Lucene near-real-time (NRT) search, added in 2.9 (2009): asking the IndexWriter for a reader flushes buffered documents to a segment but does not commit (no fsync, no new segments file); the reader sees committed segments plus the flushed-but-uncommitted ones. The wiki calls NRT "a workaround for the latency of fsync". Deletes are held in RAM and not written until commit. Stated cost: large merges can evict segments from the OS page cache and make previously fast queries slow. No reopen-latency figures are given ("hopefully within milliseconds"). — [Lucene wiki, NearRealtimeSearch](https://cwiki.apache.org/confluence/display/lucene/NearRealtimeSearch)
- Tantivy: documents are queued and indexed by threads that each build their own segment in RAM within a user-set memory budget (about 1/3 hash table, 2/3 bump-allocated arena that is wiped after the segment is finalised). Segments are write-once. Nothing is searchable before `commit()`. — [Paul Masurel, "Of tantivy's indexing"](http://fulmicoton.com/posts/behold-tantivy-part2/)
- Tantivy: on commit one segment per indexing thread is written, then `meta.json` (the list of segments and schema) is updated atomically. Deletes are by term; at commit Tantivy finds matching documents in existing segments and writes a tombstone file holding a bitset of deleted docs, named `segment_id.commit_opstamp.del`. An opstamp is an incrementing id for every operation. — [Tantivy ARCHITECTURE.md (mirror)](https://github.com/opencollector/tantivy/blob/main/ARCHITECTURE.md)
- Tantivy's default merge policy is LogMergePolicy, with settable knobs: minimum number of segments to merge, maximum docs in a segment for it to be merge-eligible, minimum layer size, ratio between levels, and tolerated ratio of deleted docs in a segment. — [docs.rs LogMergePolicy](https://docs.rs/tantivy/latest/tantivy/indexer/struct.LogMergePolicy.html)
- Tantivy indexing throughput (author's benchmark, older blog post): 5 million Wikipedia documents (8 GB) in 94 s with 4 threads; 3–4 minutes including merging. The same post notes multiple segments have little query impact when the index fits in RAM. — [fulmicoton](http://fulmicoton.com/posts/behold-tantivy-part2/)

### Inferences
- With tiered/log merging and a fan-out of about 10, each posting is rewritten roughly once per tier, so write amplification is about log10(index size / flush size) plus delete-driven merges. For a 50 MB index and flushes of tens of KB that is 3–4 rewrites per posting over its life — far less I/O than rewriting 50 MB on every save.
- The Tantivy "delete by term at commit" model means a path field (or stable file id) must be an indexed term so a file can be deleted without scanning; this is the standard answer to the app's current full-pass delete.
- Tantivy's "nothing visible until commit" plus one segment per thread per commit means frequent small commits create many tiny segments; for a trickle workload a single indexing thread and a commit cadence of seconds, not per event, is the sensible configuration.

### Gaps
- Tantivy LogMergePolicy default values and the minimum per-thread memory budget were not retrieved (docs page lists setters only). My recollection, unverified: min_num_segments 8, max_docs_before_merge 10,000,000, min_layer_size 10,000, level_log_size 0.75, del_docs_ratio 1.0, and a minimum of roughly 15 MB per indexing thread.
- Lucene's default indexing buffer (I recall 16 MB, unverified) and LogByteSizeMergePolicy details were not fetched.
- No measured merge-pause or write-amplification figures for small (tens of MB) indexes were found; published numbers are for GB-scale server indexes.
- Quickwit, Meilisearch and Elastic posts on update performance were not reached.

## 2. How does SQLite FTS5 handle incremental updates and deletes?

### Takeaway
FTS5 is a small log-structured merge tree stored inside SQLite tables: each transaction writes a new level-0 segment b-tree, segments of equal level are merged incrementally, and deletes are either delete-keys in the new segment or (contentless-delete tables) rowid tombstones purged at merge. Durability comes for free from SQLite's transaction machinery.

### Cited Findings
- Each write transaction creates a new level-0 b-tree. When `automerge` (default 4, max 16, 0 disables) b-trees exist on one level, FTS5 begins merging them into one b-tree at the next level, with the work spread over subsequent INSERT/UPDATE/DELETE statements. — [SQLite FTS5 docs](https://www.sqlite.org/fts5.html)
- `crisismerge` (default 16): once that many b-trees are on one level they are all merged immediately, so the triggering write "may take a long time". — [same](https://www.sqlite.org/fts5.html)
- `'merge', N` does about N pages of merge work on demand (positive N honours `usermerge`, default 4, range 2–16; negative N first assigns all b-trees to one level). `'optimize'` merges everything into one b-tree — smallest and fastest to query, but "can take a long time". An incremental optimise is `merge -N` once then `merge N` repeatedly until `sqlite3_total_changes()` changes by less than 2. Default page size `pgsz` is 4050 bytes. — [same](https://www.sqlite.org/fts5.html)
- Normal deletes/updates: "instead of removing entries from the full-text index, delete-keys are added to the new b-tree created by the transaction". With `secure-delete=1` entries are physically removed, which is slower. — [same](https://www.sqlite.org/fts5.html)
- External-content and contentless tables: to delete, FTS5 must be given the old column values (it re-tokenises them to know which tokens to remove), via reading the content table or the `'delete'` command; if the supplied values differ from what was indexed "the results may be unpredictable". An UPDATE is a DELETE followed by an INSERT, and the FTS table must be updated before the content row is changed. — [same](https://www.sqlite.org/fts5.html)
- Contentless-delete tables (`content='', contentless_delete=1`, SQLite 3.43.0+, 2023): a delete attaches a tombstone with the rowid to the b-tree holding the row's entries; queries omit tombstoned rows; rows and tombstones are discarded when that b-tree is merged. `deletemerge` (default 10%) makes a b-tree merge-eligible once that share of its rows is tombstoned. — [same](https://www.sqlite.org/fts5.html)

### Inferences
- FTS5's contentless-delete mode is exactly the "doc-id tombstone, purge on merge" pattern and removes the need to keep or re-read old file text in order to delete — relevant to a file indexer where the old content is gone by the time the change event arrives.
- The FTS5 defaults (fan-in 4, force at 16, purge at 10% tombstones) are a tested parameter set for an embedded, single-user, tens-of-MB index and are a reasonable starting point for a custom design.

### Gaps
- No official FTS5 measurements of update cost or index size were found in the documentation.

## 3. What does the classic literature (Lester/Moffat/Zobel, Büttcher & Clarke, logarithmic merging) recommend for small, frequently updated collections?

### Takeaway
The literature converges on merge-based maintenance with a bounded number of geometrically sized partitions, optionally with in-place handling of only the longest lists; it was written for disk-bound, multi-GB collections, and for an index that fits in RAM and rewrites in under a second its trade-offs mostly collapse to "rebuild/re-merge is fine".

### Cited Findings
- Lester, Williams, Zobel (ACSC 2004) compared in-place update, re-merge and full re-build on large web data: re-merge is fastest for large numbers of updates; in-place is suitable when the update rate is low or buffer size is limited. — [CRPIT abstract](https://crpit.scem.westernsydney.edu.au/abstracts/CRPITV26Lester.html)
- Lester, Moffat, Zobel (CIKM 2005; extended ACM TODS 33(3), 2008): geometric partitioning keeps a set of partitions of geometrically increasing size, giving "a range of tradeoffs between costs" that "can be adapted to different balances of insertion and querying operations", with "substantial savings in online indexing costs". — [TODS abstract](https://people.eng.unimelb.edu.au/ammoffat/abstracts/lmz08acmtods.html); [CIKM abstract](https://people.eng.unimelb.edu.au/ammoffat/abstracts/lmz05cikm.html)
- Logarithmic merging: sub-indexes of exponentially increasing size, merged when two reach the same size; total indexing cost O(N log N) and O(log N) active sub-indexes. Büttcher and Clarke characterise the strategies and show the best choice depends on the query-to-update ratio. — [survey, arXiv 2605.01260 (2026), as summarised in search results](https://arxiv.org/pdf/2605.01260)
- Büttcher, Clarke, Lushman (SIGIR 2006), hybrid index maintenance: short posting lists are merge-maintained; lists longer than a threshold T are updated in place by appending. One variant with non-contiguous long lists is reported to reach asymptotically optimal O(N) disk complexity under a Zipf distribution. — [SIGIR 2006 entry](https://ir.webis.de/anthology/2006.sigirconf_conference-2006.47); [review summary](https://liner.com/review/hybrid-index-maintenance-for-growing-text-collections)
- Moffat & Mackenzie (2022), immediate-access dynamic index held in memory as fixed-size blocks: about 2.0–2.1 bytes per posting versus 1.16–1.80 for static PISA indexes; ingestion over 1 GiB/minute excluding tokenisation; Robust04 conjunctive queries about 550 microseconds and top-10 ranked about 5 ms. The paper does not address deletion. (Figures from an automated summary of the HTML mirror.) — [arXiv 2211.06030](https://arxiv.org/abs/2211.06030)

### Inferences
- The price of keeping postings directly appendable is modest: roughly 15–80% more space than a static compressed index by the Moffat & Mackenzie figures. So the mutable delta need not be compressed carefully as long as it stays small.
- All of this literature optimises for append-heavy growth and disk seeks; none of it centres on deletes. For a desktop index, where most events are modifications (delete + re-add), the tombstone and purge policy matters more than the choice between geometric and logarithmic merging.
- With 7.2 million postings, a two-level structure (delta + base) is already within the "few partitions" regime these papers recommend; adding more levels buys little.

### Gaps
- Exact numbers from the Lester et al. and Büttcher et al. papers (build times, query slowdown per partition count, recommended radix) were not retrieved; only abstracts and secondary summaries were read.
- No paper was found that studies collections as small as 60k–1M documents.

## 4. Is "small mutable in-memory delta + one immutable base, rebuilt periodically" a recognised and adequate design for a tens-of-MB index? Failure modes?

### Takeaway
Yes: it is the two-level special case of the segment/LSM designs above (re-merge in the Lester et al. taxonomy; Zoekt's shard-plus-tombstone model is a production example), and at 50 MB the full re-merge is cheap enough that multi-tier merging is optional. Its failure modes are bursts that swell the delta, tombstone accumulation, and loss of the unflushed delta on crash.

### Cited Findings
- Re-merge (merge the in-memory buffer with the whole on-disk index) is a recognised strategy and is the fastest of the three for large numbers of updates. — [Lester, Williams, Zobel 2004](https://crpit.scem.westernsydney.edu.au/abstracts/CRPITV26Lester.html)
- Zoekt (Sourcegraph code search): when a repository is updated it is re-indexed and its old data in a compound shard is tombstoned; tombstones are later vacuumed, and compound shards that shrink too far are dismantled. — [Sourcegraph docs, search details](https://5.1.sourcegraph.com/code_search/explanations/search_details); [Zoekt shard management overview (third-party wiki)](https://deepwiki.com/sourcegraph/zoekt/4.7-shard-management)
- Zoekt shard merging: merging many small shards into compound shards was expected to cut memory by about 50% at a 2 GiB target size; 75% of shards were under 2.1 MiB; the index is about 2–3 times the input size; trigram data was over 70% of the web server heap. The stated trade-off is possibly higher search latency. — [Sourcegraph blog, shard merging](https://sourcegraph.com/blog/tackling-the-long-tail-of-tiny-repos-with-shard-merging)
- Unpurged deletes cost disk, RAM and query time until a merge removes them; Lucene tolerates 20% by default and FTS5 triggers at 10%. — [Elastic/McCandless](https://www.elastic.co/blog/lucenes-handling-of-deleted-documents); [Lucene javadoc](https://lucene.apache.org/core/10_1_0/core/org/apache/lucene/index/TieredMergePolicy.html); [SQLite FTS5](https://www.sqlite.org/fts5.html)

### Inferences
- Failure modes to design for: (1) a burst such as a git checkout touching thousands of files makes the delta large; cap it by size and flush it as an intermediate immutable segment or trigger a base rebuild, rather than letting RAM grow; (2) tombstones in the base inflate posting lists and document-frequency statistics until rebuild, so trigger a rebuild at a deleted fraction of 10–20%; (3) rebuild needs temporarily about twice the disk space and must swap files atomically while readers hold the old mapping; (4) the delta is lost on crash unless logged; (5) every query must consult both structures and merge results, and ranking statistics must be combined; (6) a document modified twice must be masked in both the base and the older delta entry, so tombstones need to apply by document id and generation, not by segment alone.
- At the app's measured 0.4–1.0 s for a 50 MB rewrite, rebuilding the base every few minutes of activity is affordable; the design starts to hurt around the point where rebuild time exceeds the acceptable background-work budget (hundreds of MB to GB), at which point a third tier is the standard fix.

### Gaps
- I found no source that states a size threshold below which two levels are sufficient; the adequacy claim rests on inference from the app's own rebuild timings.

## 5. Durability and crash safety; modified versus renamed documents

### Takeaway
The standard pattern is: immutable segment files written first, then a small commit-point file switched atomically; anything newer than the last commit is either replayed from a log or re-derived. Modification is universally delete + re-add; none of the sources describe a special cheap rename.

### Cited Findings
- Lucene: commit means fsyncing files and writing a new segments file; NRT readers deliberately skip this to avoid fsync latency, so NRT-visible changes are not durable until commit. — [Lucene wiki, NearRealtimeSearch](https://cwiki.apache.org/confluence/display/lucene/NearRealtimeSearch)
- Tantivy: segments are written, then `meta.json` is replaced atomically at commit; uncommitted documents are not visible. — [Tantivy ARCHITECTURE.md (mirror)](https://github.com/opencollector/tantivy/blob/main/ARCHITECTURE.md); [fulmicoton](http://fulmicoton.com/posts/behold-tantivy-part2/)
- Lucene: an update is delete + add. — [Elastic/McCandless](https://www.elastic.co/blog/lucenes-handling-of-deleted-documents)
- FTS5: UPDATE is implemented as DELETE followed by INSERT. — [SQLite FTS5](https://www.sqlite.org/fts5.html)
- Zoekt-style delta indexing is described as recording renames and removals as tombstones in a metadata sidecar. (Source is release notes of a third-party project built on Zoekt, low authority.) — [seek v0.8.0 release notes](https://newreleases.io/project/github/dualeai/seek/release/v0.8.0)

### Inferences
- For a file indexer the file system itself is the source of truth, so a write-ahead log is optional: persist the base's commit point together with the FSEvents event id it reflects, and on start-up replay FSEvents from that id (or rescan by modification time) to rebuild the delta. This gives crash safety without fsync on every change.
- Atomic commit on macOS: write the new file, `fsync` (or `F_FULLFSYNC` if power-loss durability is wanted), then `rename` over the manifest. An index is re-creatable, so plain `fsync` plus a checksum and "rebuild if corrupt" is a defensible choice.
- Rename can be made cheap if postings refer to an internal document id and the path lives in a separate, mutable document table: a rename then touches only the table (plus path/filename postings if the path is itself indexed), with no content re-tokenisation. This is a design inference, not something the cited engines document.

### Gaps
- No source was fetched on Elasticsearch's translog or on Lucene's two-phase commit details.
- No authoritative Zoekt source describing rename handling was read.

## 6. How do engines avoid a full scan to delete a document's postings?

### Takeaway
None of them touch the postings at delete time: they record the document id as dead (bitset or tombstone list), filter at query time, and drop the postings when the containing segment is next merged.

### Cited Findings
- Lucene: per-segment deleted bitset checked for every potential hit; bytes reclaimed at merge. — [Elastic/McCandless](https://www.elastic.co/blog/lucenes-handling-of-deleted-documents)
- Tantivy: per-segment tombstone bitset file written at commit. — [Tantivy ARCHITECTURE.md (mirror)](https://github.com/opencollector/tantivy/blob/main/ARCHITECTURE.md)
- FTS5 contentless-delete: rowid tombstones on the affected b-tree, discarded together with the rows at merge; merge forced at 10% tombstones by default. — [SQLite FTS5](https://www.sqlite.org/fts5.html)
- Zoekt: old index data tombstoned on re-index, vacuumed later. — [Sourcegraph docs](https://5.1.sourcegraph.com/code_search/explanations/search_details)

### Inferences
- For 60k–1M documents a live-docs bitset is 7.5–125 KB; the query-time check is one bit test per candidate. This replaces the app's current pass over every posting list with an O(1) operation per changed file.
- The alternative of a forward index (per-document term list) to delete postings precisely costs roughly as much space as the inverted index itself and is unnecessary when tombstones are used.

### Gaps
- None material.

## 7. What compression and speed gains are reported for document identifier reassignment?

### Takeaway
On web collections, going from a random or crawl order to URL order is the big, nearly free win (reported up to 40%), and recursive graph bisection (BP) adds a further roughly 13–23% in doc-id bits over the natural order; query speed-ups in Lucene's own benchmark were large for conjunctions and phrases and slightly negative for some disjunctions. These are web-scale results and should be discounted for a 60k-file desktop corpus.

### Cited Findings
- Silvestri (ECIR 2007, best paper): assigning ids by lexicographic URL order improved compression ratio by up to 40% on about 6 million web documents, computed in about 90 seconds with 100 MB of memory. — [ECIR 2007 entry](https://ir.webis.de/anthology/2007.ecir_conference-2007.12); [CNR record](https://iris.cnr.it/handle/20.500.14243/43612)
- Dhulipala et al. (KDD 2016), BP versus the "Natural" order, bits per posting (doc ids), as extracted from the paper's Table 3 by an automated summary: Gov2 (25M pages) 3.12 → 2.44 with Partitioned Elias-Fano and 2.52 → 1.95 with Binary Interpolative Coding; ClueWeb09-B (50M pages) 4.99 → 4.34 (PEF) and 4.05 → 3.50 (BIC); a Facebook posts index 10.19 → 4.18 (PEF) and 9.95 → 3.61 (BIC). Gains are described as almost identical for PEF and BIC. BP run time: 29 min (Gov2), 129 min (ClueWeb09), 163 min (FB-Posts-1B). — [arXiv 1602.08820](https://arxiv.org/abs/1602.08820)
- Mackenzie, Mallia, Petri, Culpepper, Suel (ECIR 2019): a clean-room reimplementation reproduced the core results and showed BP generalises to other collections and frameworks; across gap-based compressors random order was uniformly worst and BP always gave the smallest index, with URL order in between. — [paper](https://research.engineering.nyu.edu/~suel/papers/bp-ecir19.pdf); [NSF record](https://par.nsf.gov/biblio/10171634)
- PISA documentation calls BP "currently the state-of-the-art for minimizing the compressed space used by an inverted index" and supports random, URL/name-sorted and BP orderings; it gives no numbers. — [PISA docs](https://pisa.readthedocs.io/en/stable/document_reordering.html)
- Mackenzie, Petri, Moffat (IEEE TKDE 2023), "Tradeoff Options for Bipartite Graph Partitioning": algorithmic and heuristic refinements that compute BP orderings faster. — [University of Melbourne record](https://findanexpert.unimelb.edu.au/scholarlywork/1703404-tradeoff-options-for-bipartite-graph-partitioning)
- Lucene's BP implementation (PR 12489, 2023), English Wikipedia benchmarks. wikibigall: postings 2779 → 2260 MB (−18.8%), positions 11356 → 10522 MB (−7.4%), stored fields −9.2%, total index 15734 → 14360 MB (−8.7%). Query throughput: conjunction AndHighLow +64.5%, LowPhrase +82.7%, MedPhrase +72.4%, Fuzzy1 +32.8%; OrHighLow −3.5% and primary-key lookup −4.3%. The disjunction slowdown is attributed to clustering interfering with dynamic pruning. Reordering 10M documents took 5.6 minutes on 24 threads. — [apache/lucene PR 12489](https://github.com/apache/lucene/pull/12489)
- Same PR, wikimedium10m (10M short docs): postings 1706 → 1685 MB, stored fields 255 → 364 MB (+42.7%), total 5664 → 5747 MB (+1.5%), i.e. almost no postings gain and worse stored-field compression. (The automated summary labelled the postings change "+1.5%", which contradicts its own figures; treat with care.) — [apache/lucene PR 12489](https://github.com/apache/lucene/pull/12489)
- Lucene's BPIndexReorderer javadoc: BP "has been observed to also speed up queries significantly by clustering documents that have similar sets of terms together"; it is a slow operation using O(maxDoc + numTerms × numThreads) memory; defaults consider only terms with document frequency at least 4,096 and stop recursing at partitions of 32 documents. — [Lucene 9.9 javadoc](https://lucene.apache.org/core/9_9_1/misc/org/apache/lucene/misc/index/BPIndexReorderer.html)
- Yan, Ding, Suel (WWW 2009): with URL-sorted ids on Gov2 (25.2M pages) report "very significant improvements in index size and query processing speed", and propose frequency-compression techniques that exploit the ordering. — [WWW 2009 entry](https://ir.webis.de/anthology/2009.wwwconf_conference-2009.41/)
- Ding, Attenberg, Suel (WWW 2010): a TSP computation over a sparse graph built with locality-sensitive hashing improves compression further and scales to tens of millions of documents. — [WWW 2010 entry](https://ir.webis.de/anthology/2010.wwwconf_conference-2010.32)

### Inferences
- Computed from the Dhulipala figures: BP over Natural is −22% (Gov2 PEF), −23% (Gov2 BIC), −13% (ClueWeb09 PEF), −14% (ClueWeb09 BIC), −59% (Facebook posts, where the natural order has no locality).
- Expected transfer to a desktop corpus: a directory-path order is the analogue of URL order and should capture most of the available locality for source trees and project folders. But (a) with 60k documents doc-id gaps are already at most 16 bits and long lists are already dense, so absolute savings per posting are smaller than at 25–50M documents; (b) Lucene's BP ignores terms with fewer than 4,096 documents, which at 60k documents means only very common terms would drive the ordering; (c) the wikimedium10m result shows near-zero gain on a small-document corpus with an uninformative baseline. A team measuring 6% from path ordering in a filter-based structure is not in conflict with the literature: filters do not encode gaps, so they cannot benefit the way delta-coded lists do. For a delta-coded or Elias-Fano-style index I would expect a low-double-digit percentage reduction in doc-id bits from path order relative to arbitrary order, and little extra from BP — this is an estimate, not a measured result.
- Query-speed gains come from clustering (longer skips in conjunctions, fewer blocks decoded), so they should transfer in kind to AND-style desktop queries, though absolute query times at this scale are already small.

### Gaps
- The per-codec, per-collection tables of the ECIR 2019 reproducibility study (random vs URL vs BP bits per posting; query timings) could not be read because the PDF did not parse. They are the best single source and should be consulted directly.
- Numbers for Blandford & Blelloch (2002) and Silvestri, Orlando, Perego (SIGIR 2004) clustering approaches were not retrieved; the SIGIR 2004 paper is at https://www.dais.unive.it/~orlando/PAPERS/sigir04assignment.pdf.
- Numeric results from Yan/Ding/Suel 2009 and Ding/Attenberg/Suel 2010 were not retrieved (403 and unparsed PDF).
- The Dhulipala summary also claimed BP was "faster than MinHash (14, 42, 70 minutes)", which is inconsistent with BP's 29/129/163 minutes; I read those as MinHash's times, with BP slower. Also unverified: what "Natural" means per collection in that table (I believe URL order for the web collections).
- The speed-up achieved by the 2023 TKDE faster-BP work was not retrieved, nor any 2024–2026 follow-ups.

## 8. Does ordering help term-frequency compression as well as doc-id compression?

### Takeaway
Yes, but less directly: frequencies are not delta-coded, so the gain comes from similar documents sitting together and producing runs of similar small values; Yan, Ding and Suel designed frequency codecs specifically to exploit this, and Lucene's BP benchmark shows smaller gains for positions than for doc ids.

### Cited Findings
- Yan, Ding, Suel (WWW 2009) "propose and evaluate techniques for compressing frequency values" under an optimised (URL-sorted) document ordering, in addition to doc-id techniques. — [WWW 2009 entry](https://ir.webis.de/anthology/2009.wwwconf_conference-2009.41/)
- Lucene BP on wikibigall: postings (doc ids and frequencies together in Lucene's file) −18.8%, positions −7.4%. — [apache/lucene PR 12489](https://github.com/apache/lucene/pull/12489)

### Inferences
- Expect the frequency stream to gain noticeably less than the doc-id stream from path ordering; if the desktop index stores no frequencies or positions, this question is moot.

### Gaps
- No separate doc-id versus frequency bits-per-posting breakdown under reordering was retrieved (the ECIR 2019 and WWW 2009 tables contain one).

## 9. Is there evidence for file systems or source-code repositories (directory-path ordering)? Do Lucene or Tantivy index sorting document gains?

### Takeaway
I found no published measurement of directory-path ordering for a desktop or source-code inverted index; the nearest evidence is URL ordering on web crawls and Zoekt's observation that grouping repositories shares vocabulary. Engine documentation on index sorting describes the mechanism qualitatively without numbers.

### Cited Findings
- Elasticsearch documentation: with index sorting, documents with similar structure and values are compressed together, which "should improve the compression ratio" (stored fields and doc values); no figures given. — [Elastic docs, tune for disk usage](https://www.elastic.co/docs/deploy-manage/production-guidance/optimize-performance/disk-usage)
- Zoekt: because of trigram overlap, "the larger a compound is, the cheaper it is, in terms of memory, to merge it with another shard"; about 50% memory reduction expected from compound shards. This is a vocabulary-sharing effect across repositories, not a doc-id-gap effect. — [Sourcegraph blog](https://sourcegraph.com/blog/tackling-the-long-tail-of-tiny-repos-with-shard-merging)
- Lucene sorted segments also allow early termination when the query sort matches index order. — [Lucene SortingMergePolicy javadoc](https://lucene.apache.org/core/5_3_0/misc/org/apache/lucene/index/SortingMergePolicy.html)

### Inferences
- A file path is structurally a URL (host/path hierarchy), so Silvestri's argument applies, but his 40% is an upper bound from web pages with heavy per-site boilerplate. Source repositories have comparable per-directory vocabulary sharing; a home folder of mixed documents has much less.
- The team's own prototype (6% in a filter structure) is currently the only direct evidence for this corpus type; a quick test with delta-coded postings under path order versus arbitrary order on the real 60k-file corpus would settle it at low cost.

### Gaps
- No Tantivy documentation of compression gains from index sorting was found; I also recall, unverified, that Tantivy deprecated or removed its index-sorting feature in recent releases — check before relying on it.
- No academic or engineering source measuring path-ordered ids on file-system or code corpora was found.

## 10. How is a good ordering maintained under updates? Do engines re-order at merge time?

### Takeaway
Engines do not keep the order globally under updates: newly flushed data is in arrival order and the order is restored per segment when segments are merged or rebuilt.

### Cited Findings
- Lucene's SortingMergePolicy reorders documents by the configured sort before merging; "all segments resulting from a merge will be sorted while segments resulting from a flush will be in the order in which documents have been added"; it scatters doc ids and requires an idempotent sort. — [Lucene 5.3 javadoc (older API, 2015)](https://lucene.apache.org/core/5_3_0/misc/org/apache/lucene/index/SortingMergePolicy.html)
- Lucene's BP reorderer is an offline, slow operation applied to an existing index/reader. — [Lucene 9.9 javadoc](https://lucene.apache.org/core/9_9_1/misc/org/apache/lucene/misc/index/BPIndexReorderer.html)

### Inferences
- In a delta + base design this falls out naturally: the delta is small and unordered; each base rebuild re-assigns ids in path order (a sort of 60k–1M paths takes milliseconds to tens of milliseconds). Document ids are therefore not stable across rebuilds, so anything outside the index must reference files by a stable key (path or file id), not by doc id.
- Leaving id gaps for future insertions is unnecessary at this scale given cheap rebuilds.

### Gaps
- I recall that later Lucene versions sort at flush as well when an index sort is configured, and that Lucene 9.8+ includes a merge policy that applies BP at merge time (BPReorderingMergePolicy); a search did not confirm the latter, so both are unverified.
- No source was found on incremental maintenance of BP orderings.
