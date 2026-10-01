# Compressed inverted indexes that keep exact term frequencies

Notes compiled 2026-10-02. Target context: 59,642 files, 535,517 terms, 7,157,948 postings; currently ~172 MB in RAM (about 24 bytes per posting once HashMap/Vec overhead is included); fingerprint prototype 29.6 MB without term frequencies.

Conventions: "bpi" = bits per integer. A posting with a doc id and a term frequency costs (doc bpi + freq bpi). All benchmark collections below are web-scale (24-50 million documents, 5-20 billion postings), far larger than the target corpus; this is flagged again wherever it matters.

## 1. Bits per doc id and per frequency achieved by the main codecs

### Takeaway
On full web-scale indexes, bit-packed and Elias-Fano style codecs cost roughly 5.5-9 bits per doc id and 2-5.6 bits per frequency without doc-id reordering (about 1.3-1.8 bytes per posting in total), while byte-aligned codecs (VByte, StreamVByte, Varint-GB/G8IU) cost 8-12.5 bits per doc id and 8-10 bits per frequency (about 2.1-2.8 bytes per posting). The most compact codecs (Interpolative, ANS, PEF) reach about 1.2 bytes per posting at random ordering and under 1 byte with reordering, at a decode-speed cost of 2-6x for the bit-at-a-time ones.

### Cited Findings

**Full index, docs and freqs, three doc-id orderings (Mallia, Siedlaczek, Suel, ECIR 2019; PISA; Intel i7-4770 Haswell; blocks of 128; ALL terms included: Gov2 = 24.6M docs, 35.6M terms, 5.74B postings; ClueWeb09 = 50.1M docs, 92.1M terms, 15.86B postings).** Values are doc bpi / freq bpi. — [Mallia et al., "An Experimental Study of Index Compression and DAAT Query Processing Methods"](https://research.engineering.nyu.edu/~suel/papers/daat-ecir19.pdf)

| Codec | Gov2 Random | ClueWeb09 Random | Gov2 URL-order | ClueWeb09 URL-order | Gov2 BP-order | ClueWeb09 BP-order |
|---|---|---|---|---|---|---|
| Packed+ANS2 | 7.71 / 2.54 | 7.65 / 2.14 | 3.96 / 1.85 | 5.36 / 1.94 | 3.25 / 1.72 | 4.80 / 1.87 |
| Interpolative (BIC) | 7.58 / 2.62 | 7.52 / 2.12 | 3.80 / 2.14 | 5.15 / 1.87 | 3.11 / 2.06 | 4.65 / 1.81 |
| PEF | 7.60 / 3.05 | 7.53 / 2.39 | 4.11 / 2.37 | 5.85 / 2.20 | 3.30 / 2.23 | 5.29 / 2.11 |
| OptPFD | 8.13 / 3.14 | 8.07 / 2.76 | 4.48 / 2.38 | 6.18 / 2.41 | 3.74 / 2.23 | 5.56 / 2.30 |
| Simple16 | 9.43 / 3.85 | 9.40 / 3.35 | 5.34 / 2.90 | 6.92 / 2.79 | 4.62 / 2.73 | 6.34 / 2.65 |
| Simple8b | 9.24 / 4.63 | 9.18 / 4.14 | 5.53 / 3.27 | 7.36 / 3.47 | 4.77 / 3.03 | 6.87 / 3.27 |
| QMX | 9.16 / 5.07 | 9.14 / 4.53 | 5.98 / 3.36 | 7.98 / 3.75 | 5.19 / 3.06 | 7.43 / 3.49 |
| SIMD-BP128 | 8.82 / 5.60 | 8.76 / 4.89 | 6.35 / 3.41 | 8.68 / 3.97 | 5.42 / 2.98 | 8.00 / 3.61 |
| Varint-G8IU | 11.38 / 8.83 | 11.60 / 8.84 | 10.35 / 8.81 | 10.75 / 8.83 | 10.09 / 8.81 | 10.51 / 8.83 |
| VarintGB | 12.01 / 9.77 | 12.18 / 9.81 | 11.15 / 9.77 | 11.43 / 9.80 | 10.94 / 9.77 | 11.27 / 9.80 |
| StreamVByte | 12.30 / 10.04 | 12.44 / 10.03 | 11.37 / 10.04 | 11.65 / 10.03 | 11.21 / 10.04 | 11.49 / 10.03 |

- Total index sizes from the same table, Gov2 Random ordering: Packed+ANS2 7.36 GB, Interpolative 7.32 GB, PEF 7.65 GB, OptPFD 8.09 GB, SIMD-BP128 10.35 GB, StreamVByte 16.03 GB (5.74B postings, so about 1.3, 1.3, 1.3, 1.4, 1.8 and 2.8 bytes per posting). — [Mallia et al. ECIR 2019](https://research.engineering.nyu.edu/~suel/papers/daat-ecir19.pdf)
- "Variable byte methods benefit little from URL or BP reordering ... no improvement is seen for frequency encodings. On the other hand, packing methods are highly sensitive to ordering"; with Packed+ANS2, URL ordering shrinks Gov2 by 43% and ClueWeb09 by 27% versus Random. — [Mallia et al. ECIR 2019](https://research.engineering.nyu.edu/~suel/papers/daat-ecir19.pdf)
- Query speed in the same study (Gov2, Random order, TREC05, k=10, MaxScore, ms/query): SIMD-BP128 6.27, Varint-G8IU 5.86, StreamVByte 6.44, OptPFD 7.43, QMX 7.50, PEF 11.77, Packed+ANS2 13.91, Interpolative 16.53. With VBMW: SIMD-BP128 5.75, PEF 6.23, Packed+ANS2 13.73, Interpolative 18.65. — [Mallia et al. ECIR 2019](https://research.engineering.nyu.edu/~suel/papers/daat-ecir19.pdf)

**Doc-id-only survey table (Pibiri & Venturini, ACM Computing Surveys 53(6), 2020/2021; Intel i9-9900K; URL-ordered collections). IMPORTANT: only lists longer than 4,096 postings were kept (39,177 lists for Gov2, 96,722 for ClueWeb09, 76,474 for CCNews, covering 93%/94%/98% of postings), so these figures exclude the rare-term tail entirely.** bits/int and ns per decoded integer: — [Pibiri & Venturini, "Techniques for Inverted Index Compression"](https://arxiv.org/abs/1908.10598)

| Method | Gov2 bpi | Gov2 ns/int | ClueWeb09 bpi | ClueWeb09 ns/int | CCNews bpi | CCNews ns/int |
|---|---|---|---|---|---|---|
| VByte (SIMD, blocks of 128) | 8.81 | 0.96 | 9.20 | 1.09 | 9.29 | 1.03 |
| Opt-VByte | 3.89 | 0.73 | 5.72 | 0.92 | 6.42 | 0.72 |
| BIC | 2.94 | 5.06 | 4.43 | 6.31 | 5.24 | 6.97 |
| Elias delta | 3.74 | 3.56 | 5.17 | 3.72 | 6.36 | 3.85 |
| Rice | 4.08 | 2.92 | 5.31 | 3.25 | 5.82 | 3.32 |
| PEF | 3.12 | 0.76 | 4.99 | 1.10 | 5.45 | 1.31 |
| DINT | 3.53 | 1.13 | 5.35 | 1.56 | 6.44 | 1.65 |
| Opt-PFor | 3.63 | 1.38 | 5.46 | 1.79 | 6.07 | 1.53 |
| Simple16 | 4.19 | 1.53 | 5.85 | 1.87 | 6.41 | 1.89 |
| QMX | 5.12 | 0.80 | 7.29 | 0.87 | 7.40 | 0.84 |
| Roaring | 6.63 | 0.50 | 9.78 | 0.71 | 9.49 | 0.61 |
| Slicing | 4.31 | 0.53 | 7.06 | 0.68 | 7.78 | 0.69 |

- Entropy of the gaps in those filtered collections: 3.02 (Gov2), 4.46 (ClueWeb09), 5.44 (CCNews) bits. — [Pibiri & Venturini survey](https://arxiv.org/abs/1908.10598)
- Prefix-summing gaps costs about 0.5 ns per integer and "sometimes dominates that of decoding the gaps". — [Pibiri & Venturini survey](https://arxiv.org/abs/1908.10598)
- Boolean AND, average ms/query on ClueWeb09: Roaring 2.9, Slicing 4.5, QMX 11.8, VByte 12.5, Opt-VByte 13.3, PEF 13.5, DINT 15.0, Opt-PFor 16.3, Simple16 16.6, Rice 26.6, delta 29.3, BIC 45.3. OR queries: Roaring 10.3 vs 155-288 ms for the gap-coded methods. — [Pibiri & Venturini survey](https://arxiv.org/abs/1908.10598)
- The survey explicitly considers doc ids only ("we ignore additional information about each term"); Roaring and Slicing have no native frequency representation in that comparison. — [Pibiri & Venturini survey](https://arxiv.org/abs/1908.10598)

**Docs and freqs, URL-ordered, full collections (Pibiri & Venturini, "On Optimally Partitioning Variable-Byte Codes", i7-4790K).** doc bpi / freq bpi: — [Pibiri & Venturini, arXiv 1804.10949](https://arxiv.org/abs/1804.10949)

| Method | Gov2 | ClueWeb09 | CCNews |
|---|---|---|---|
| VByte / Masked VByte (same format) | 9.53 / 8.02 | 9.90 / 8.01 | 9.42 / 8.00 |
| VByte, uniform partitions | 5.41 / 3.31 | 7.37 / 2.69 | 7.27 / 2.55 |
| VByte, optimal partitions | 4.87 / 3.04 | 6.54 / 2.48 | 6.85 / 2.39 |
| PEF epsilon-optimal | 4.10 / 2.38 | 5.85 / 2.20 | 5.84 / 2.18 |
| BIC | 3.80 / 2.14 | 5.15 / 1.87 | 5.37 / 1.98 |

- Partitioned VByte gives a "2x improvement with respect to the original VByte format", cutting its gap to bit-aligned methods from about 172% to about 20%. AND-query times: Gov2 0.89 ms (opt-VByte) vs 0.98 (PEF) vs 0.88 (QMX); ClueWeb09 5.70 / 5.87 / 5.30. — [arXiv 1804.10949](https://arxiv.org/abs/1804.10949)
- Index build time, minutes (Gov2 / ClueWeb09 / CCNews): optimal VByte 10.5 / 28.5 / 35.5; PEF epsilon-opt 41.3 / 125.5 / 85.2; BIC 7.0 / 20.5 / 28.3. — [arXiv 1804.10949](https://arxiv.org/abs/1804.10949)

**Raw decode speed (OLDER results, 2012-2017 hardware).**
- Lemire & Boytsov (Core i7-2600, 2012): ClueWeb09 — SIMD-BP128 1,600 million ints/s at 9.5 bpi (2,300 mis at 11 bpi for the variant with vectorised differential coding), SIMD-FastPFOR 1,200 mis at 8.1 bpi, varint-G8IU 1,400 mis at 12 bpi, plain Variable Byte 540 mis at 9.6 bpi. Gov2 — SIMD-BP128 1,700 mis at 6.3 bpi, Variable Byte 680 mis at 8.7 bpi. — [Lemire & Boytsov, "Decoding billions of integers per second through vectorization"](https://arxiv.org/abs/1209.2137)
- Stream VByte (i7-4770 Haswell, 2017), ClueWeb09: 4.0 billion ints/s on highly compressible lists and 1.1 on poorly compressible, vs varint-G8IU 2.7 / 1.1, Masked VByte 2.6 / 0.5, varint-GB 2.6 / 0.5, scalar VByte 1.1 / 0.3. Stream VByte costs about 0.5-2 bits per integer more than VByte; Masked VByte uses the unchanged VByte format. — [Lemire, Kurz, Rupp, "Stream VByte"](https://arxiv.org/abs/1709.08990)
- ANS: Moffat & Petri's WSDM 2018 method uses two-dimensional conditioning contexts per block and a byte-friendly ANS bucket mapping, evaluated on the 426 GiB Gov2 and a news collection; the "Packed+ANS2" row in the table above is the PISA integration of that reference code (max:med contexts). — [Moffat & Petri, WSDM 2018](https://ir.webis.de/anthology/2018.wsdm_conference-2018.52/); [Mallia et al. ECIR 2019](https://research.engineering.nyu.edu/~suel/papers/daat-ecir19.pdf)
- Partitioned Elias-Fano original paper (Ottaviano & Venturini, SIGIR 2014): two-level structure, list split into chunks, each chunk and the chunk endpoints Elias-Fano coded to exploit local clustering. — [Ottaviano & Venturini, SIGIR 2014](https://dl.acm.org/doi/10.1145/2600428.2609615)

### Inferences
- The "Random ordering" columns are the right reference for a desktop corpus unless doc ids are assigned in path order. Assigning ids in sorted-path order is the desktop analogue of URL ordering and should move a bit-packed codec some of the way from the Random to the URL column; it will do nothing for byte-aligned codecs or for frequencies under VByte.
- For frequencies, the choice between byte-aligned and bit-packed matters more than for doc ids: 8-10 bits versus 2-5.6 bits. A full byte per frequency is the single biggest waste in VByte-family layouts because most term frequencies are 1-3.
- Roaring is the fastest for boolean set operations but stores no frequencies and is among the largest at web scale; it would need a parallel frequency array, so it is a poor fit when BM25 with exact tf is the goal.
- BIC and ANS give the smallest indexes but 2-3x slower top-k queries than SIMD-BP128; at 7M postings decode speed is irrelevant in absolute terms (7M postings at even 5 ns each is 35 ms for a full scan of the whole index), so the densest codecs are viable here if implementation complexity is acceptable.

### Gaps
- Could not retrieve the bits-per-posting tables from the original PEF (SIGIR 2014) and Moffat & Petri (CIKM 2017 / WSDM 2018) papers directly; PEF and ANS numbers above come from later third-party reproductions (PISA and Pibiri & Venturini).
- No measured Roaring-plus-frequencies figure was found.
- The RISE Rust library paper (Savino & Venturini, arXiv 2606.07187, June 2026) claims "speedups of up to 2x over the current state of the art", but only the abstract was read; codecs, sizes and baselines were not verified. — [arXiv 2606.07187](https://arxiv.org/abs/2606.07187)
- No benchmark newer than the 2019-2021 studies with a comparable codec-by-codec table was found; treat the tables above as still the standard reference rather than as 2026 measurements.

## 2. Small corpora, long tails of rare terms, and very short posting lists

### Takeaway
Published codec benchmarks say almost nothing about short lists (the main survey drops every list under 4,096 postings), and both Lucene and Tantivy abandon bit-packing for any list or tail under 128 postings in favour of variable-length integers; Lucene additionally inlines singleton doc ids into the term dictionary. For a 60k-document corpus the practical floor is set by fixed-width arithmetic rather than by codec choice.

### Cited Findings
- The Pibiri & Venturini survey retains only lists longer than 4,096 postings; those lists are 39,177 of Gov2's terms yet cover 93% of its postings (94% ClueWeb09, 98% CCNews). — [Pibiri & Venturini survey](https://arxiv.org/abs/1908.10598)
- Gov2 has 35,636,425 terms for 5,742,630,292 postings (average list length about 161); ClueWeb09 92,094,694 terms for 15,857,983,641 postings (about 172). — [Mallia et al. ECIR 2019](https://research.engineering.nyu.edu/~suel/papers/daat-ecir19.pdf)
- Lucene: packed blocks of 128 with uniform bit width; the remainder goes into a VInt block ("the first 256 document ids are encoded as two packed blocks, while the remaining 3 are encoded as one VInt block"). — [Lucene912PostingsFormat](https://lucene.apache.org/core/10_0_0/core/org/apache/lucene/codecs/lucene912/Lucene912PostingsFormat.html)
- Lucene singleton inlining: for a term in exactly one document, "instead of writing a file pointer to the .doc file (DocFPDelta), and then a VIntBlock at that location, the single document ID is written to the term dictionary". — [Lucene912PostingsFormat](https://lucene.apache.org/core/10_0_0/core/org/apache/lucene/codecs/lucene912/Lucene912PostingsFormat.html)
- Lucene VInt tail trick for frequencies: the doc delta is shifted left one bit; "When DocDelta is odd, the frequency is one. When DocDelta is even, the frequency is read as another VInt." So tf=1 postings cost zero extra bytes in tail blocks. — [Lucene912PostingsFormat](https://lucene.apache.org/core/10_0_0/core/org/apache/lucene/codecs/lucene912/Lucene912PostingsFormat.html)
- Lucene's stated block-size trade-off: "Smaller block size result in smaller variance among width of integers hence smaller indexes. Larger block size result in more efficient bulk i/o". — [Lucene912PostingsFormat](https://lucene.apache.org/core/10_0_0/core/org/apache/lucene/codecs/lucene912/Lucene912PostingsFormat.html)
- Tantivy: "the last block may contain an arbitrary number of docs between 1 and 127 documents. We then use variable int encoding instead of bitpacking." — [Tantivy ARCHITECTURE.md](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md)
- Tantivy writes skip data only when doc_freq >= COMPRESSION_BLOCK_SIZE (128), so short lists carry no skip or block-max overhead. — [tantivy src/postings/serializer.rs](https://github.com/quickwit-oss/tantivy/blob/main/src/postings/serializer.rs)

### Inferences
- Target corpus arithmetic (not measured): 7,157,948 postings / 535,517 terms = 13.4 postings per term on average, versus about 161-172 in Gov2/ClueWeb09. Nearly every list is shorter than one 128-block, so in a Lucene/Tantivy-style layout almost the whole index would be encoded by the VInt tail path, and the bit-packing figures in section 1 mostly would not apply.
- Fixed-width bound: 59,642 documents fit in 16 bits. A completely uncompressed layout of u16 doc id + u8 tf + u8 field mask is 4 bytes per posting = 28.6 MB, already matching the 29.6 MB fingerprint prototype while keeping exact tf. Dropping to u16 + u8 (field mask folded elsewhere) is 3 bytes = 21.5 MB.
- With gap coding the per-list cost depends on list length n: average gap is about 59,642/n, so a singleton needs about 16 bits and a 10-document term about 13 bits per doc id; only terms present in several hundred or more documents get down to the 6-9 bit range. VByte gaps therefore cost 1-2 bytes per doc id, and with the Lucene odd/even tf trick most postings need no separate tf byte. A plausible planning range for postings is 1.5-2.5 bytes per posting, i.e. about 11-18 MB, with the field mask adding 0-1 byte per posting depending on how it is packed (e.g. 2-3 bits alongside tf). This is an estimate; the real distribution must be measured on the corpus.
- At about 1 million files, doc ids need 20 bits and web-scale figures become more representative for common terms, but the rare-term tail still dominates the term count.
- With 535k terms, per-term overhead rivals postings: every extra byte per term is 0.5 MB. Inlining singletons (and plausibly any list that fits in the bytes a file pointer would have used) removes both the pointer and the list-header overhead for the majority of terms.
- Skip lists and block-max metadata are pointless for lists under 128 postings; only the few thousand most common terms need them.

### Gaps
- No published study was found that reports bits per posting for a desktop-scale corpus (tens of thousands of documents) or that breaks codec performance down by list length; the fraction of terms with df < 10 in the target corpus is not given and should be measured.
- No source was found documenting Tantivy inlining singleton doc ids into the term dictionary; its TermInfo holds doc_freq plus postings and positions byte ranges, which suggests it does not, but this was not verified beyond the serializer source.
- No recent Elastic, Quickwit, ParadeDB or Meilisearch post with bytes-per-posting measurements was found in the searches run.

## 3. Term dictionary compression: FST, front coding, minimal perfect hashing, tries

### Takeaway
An FST for roughly 500k natural-language terms should cost on the order of 1.5-5 MB (about 3-10 bytes per key by the fst crate author's own measurements), supports prefix, range, regex and fuzzy lookups, and can be memory-mapped; a minimal perfect hash is far smaller (about 2 bits per key) but stores no keys and supports only exact lookup.

### Cited Findings
- fst crate measurements (2015 blog post, OLDER but the format is unchanged): English dictionary, 119,095 keys, 1.1 MB raw -> 324 KB FST (29.4%; gzip 302 KB, xz 232 KB), built in 0.12 s. Gutenberg word list, 3,539,670 keys, 41 MB -> 22 MB (53.7%), 2.04 s. Wikipedia titles, 15,777,626 keys, 384 MB -> 157 MB (40.9%), 18.31 s. DOI URLs, 49,118,091 keys, 2,800 MB -> 113 MB (4.0%). Common Crawl URLs, 1,649,195,774 keys, 134 GB -> 27 GB (20.1%). — [BurntSushi, "Index 1,600,000,000 Keys with Automata and Rust"](https://burntsushi.net/transducers/)
- Same post: prefix/range query on Wikipedia titles in 0.023 s, Levenshtein fuzzy search in 0.094 s (whole CLI invocation). — [BurntSushi blog](https://burntsushi.net/transducers/)
- The fst crate works over any `AsRef<[u8]>`: "one can store a set/map created by this crate on disk and search it without actually reading the entire set/map into memory". Keys must be inserted in lexicographic order; map values are u64; supports range, regex and Levenshtein automata (the latter described as proof-of-concept quality with significant memory overhead). — [fst crate docs](https://docs.rs/fst/latest/fst/)
- Tantivy's dictionary: "Term -> TermOrdinal is addressed by a finite state transducer, implemented by the fst crate", then a TermInfo store maps ordinal -> metadata. — [Tantivy ARCHITECTURE.md](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md)
- Tantivy TermInfoStore: blocks of 256 terms (`BLOCK_LEN = 256`); each block has a 35-byte header (u64 offset + one full reference TermInfo + three bit-width bytes) and the other 255 terms bit-pack doc_freq, postings offset and positions offset at the block's minimal widths. — [tantivy term_info_store.rs](https://github.com/quickwit-oss/tantivy/blob/main/src/termdict/fst_termdict/term_info_store.rs)
- Tantivy also ships an SSTable dictionary (tantivy-sstable) used by Quickwit as an alternative to the FST, chosen for locality: one block fetch per lookup once the block index is loaded, rather than needing the whole dictionary. — [tantivy-sstable crate](https://crates.io/crates/tantivy-sstable)
- Lucene block tree dictionary: terms stored in blocks of 25-48 entries in .tim with shared prefixes and suffix lengths; the .tip FST "maps a term prefix to the on-disk block that holds all terms starting with that prefix"; per term it stores DocFreq and TotalTermFreq (as a delta from DocFreq) plus postings metadata; .tmd holds field-level stats. — [Lucene90BlockTreeTermsWriter](https://lucene.apache.org/core/10_0_0/core/org/apache/lucene/codecs/lucene90/blocktree/Lucene90BlockTreeTermsWriter.html)
- Minimal perfect hashing: PtrHash achieves 2.0 bits per key with default parameters and 8-12 ns per query on 1 billion integer keys, at least 2.1x faster to query than other MPHFs (SEA 2025). — [Groot Koerkamp, "PtrHash"](https://arxiv.org/abs/2502.15539)
- Front coding: strings sorted, each stored as shared-prefix length plus remaining suffix, grouped into buckets (e.g. 16 strings in SAP HANA) for random access; the best compressed string dictionaries in Martinez-Prieto et al. "may use as little as 5% of the original dictionary size" with microsecond lookups (2016, OLDER; that figure is for highly redundant dictionaries such as URLs, not word lists). — [Martinez-Prieto et al., "Practical Compressed String Dictionaries"](https://users.dcc.uchile.cl/~gnavarro/abstracts/is15.2.html)

### Inferences
- Scaling the fst author's natural-language measurements (2.7 bytes per key for a 119k-word dictionary, 6.2 for 3.5M Gutenberg words, 10 for Wikipedia titles) to 535,517 terms gives about 1.5-5.3 MB. File-system corpora contain many code identifiers, numbers and hashes that share fewer suffixes, so expect the upper half of that range.
- Per-term metadata in a Tantivy-style store (doc_freq + postings offset bit-packed, no positions) should be roughly 3-5 bytes per term including the 35/256 byte block header, about 1.6-2.7 MB for 535k terms. This is derived from the layout, not measured.
- MPH at 2 bits per key is about 134 KB for 535k terms, but an MPH returns an arbitrary slot for unknown terms, so a per-term fingerprint (e.g. 16-32 bits, 1-2 MB) is needed to reject misses, and prefix/fuzzy/typo queries become impossible without a second structure. For an interactive desktop search box with prefix matching, FST or front-coded blocks are the better fit.
- A Lucene-style hybrid (small FST or sparse index over front-coded blocks of 25-48 terms) keeps the randomly-accessed part tiny and makes each lookup touch one or two pages, which suits memory-mapping.

### Gaps
- No measured bytes-per-key figure was obtained for front coding or for a succinct trie (e.g. LOUDS/Marisa-style) on a natural-language vocabulary; the only front-coding number found is the best-case 5% for redundant dictionaries.
- No measured size for a full Tantivy or Lucene term dictionary at exactly this scale was found; the estimates above are extrapolations.

## 4. How Lucene and Tantivy lay out a segment, what is memory-mapped, what stays on the heap

### Takeaway
Both engines store each segment as a handful of immutable files (term dictionary plus index, postings with interleaved skip data, positions, norms, stored fields) designed to be read directly from a memory map, with heap use limited to small per-segment metadata; Tantivy states its readers "require very little anonymous memory".

### Cited Findings
- Tantivy segment files are named `segment-id.ext`; components: .term (term dictionary), .idx (postings), .pos (positions), .fieldnorm, .fast (columnar fast fields), .store (document store), .del (alive bitset). — [Tantivy ARCHITECTURE.md](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md)
- Tantivy postings: "The posting list is organized in block of 128 documents. One block of doc ids is followed by one block of term frequencies. The doc ids are delta encoded and bitpacked. The term frequencies are bitpacked." — [Tantivy ARCHITECTURE.md](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md)
- Tantivy readers are "designed to require very little anonymous memory. The data is read straight from an mmapped file." — [Tantivy ARCHITECTURE.md](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md)
- Tantivy TermInfo holds doc_freq, the postings byte range and the positions byte range; skip data (length-prefixed with a VInt) precedes the blocks and is only present for doc_freq >= 128. — [tantivy serializer.rs](https://github.com/quickwit-oss/tantivy/blob/main/src/postings/serializer.rs)
- Tantivy's bit-packing is a pure-Rust reimplementation of simdcomp's SSE3 delta + bit-packing on blocks of 128. — [Paul Masurel, "Of bitpacking with or without SSE3"](https://fulmicoton.com/posts/bitpacking/)
- Lucene 9.12/10.0 postings files: .doc (doc ids, frequencies and skip data), .pos (positions), .pay (payloads and offsets), with .tim/.tip for the term dictionary and its index. Skip data is two-level and inline: "Level 0 skip data is interleaved between every packed block. Level 1 skip data is interleaved between every 32 packed blocks" (4,096 docs). — [Lucene912PostingsFormat](https://lucene.apache.org/core/10_0_0/core/org/apache/lucene/codecs/lucene912/Lucene912PostingsFormat.html)
- Lucene term dictionary files: .tim (term blocks), .tip (FST prefix index), .tmd (field metadata). — [Lucene90BlockTreeTermsWriter](https://lucene.apache.org/core/10_0_0/core/org/apache/lucene/codecs/lucene90/blocktree/Lucene90BlockTreeTermsWriter.html)
- Since Lucene 8 (2019) the terms index FST can be read off-heap. FSTLoadMode AUTO decides "if FSTs are read from disk depending if the segment read from an MMAPDirectory", with an exception for ID fields in an IndexWriter context, which stay on heap; the cost is slightly slower term lookups, mostly noticeable for primary-key lookups. — [BlockTreeTermsReader.FSTLoadMode](https://lucene.apache.org/core/8_1_0/core/org/apache/lucene/codecs/blocktree/BlockTreeTermsReader.FSTLoadMode.html); [Elastic, "What's new in Lucene 8"](https://www.elastic.co/blog/whats-new-in-lucene-8)
- Lucene "tries to delegate as much of this memory management to the operating system as it can by loading index data using memory mapping", letting hot data stay in the page cache. — [Elastic, "What's new in Lucene 8"](https://www.elastic.co/blog/whats-new-in-lucene-8)
- PISA and the ds2i-derived research engines follow the same model: "Indexes are saved to disk after construction, and memory-mapped to be queried, so that there are no hidden space costs due to loading of additional data structures in memory." — [Mallia et al. ECIR 2019](https://research.engineering.nyu.edu/~suel/papers/daat-ecir19.pdf)
- fst crate: when streaming all 1.6 billion keys of a memory-mapped 27 GB FST the author observed resident memory growing only as pages were touched, and construction of the sorted-input FST peaked at about 56 MB resident. — [BurntSushi blog](https://burntsushi.net/transducers/)

### Inferences
- The common pattern to copy: (1) one postings file of concatenated per-term byte ranges, each self-describing (optional skip header, then 128-blocks, then a VInt tail); (2) a term dictionary that maps term -> small fixed record {doc_freq, postings offset} via FST or front-coded blocks; (3) a one-byte-per-document norm/length array; (4) everything immutable and little-endian so the file can be mmapped and decoded in place; (5) updates handled by writing new small segments and merging.
- For a 15-30 MB index, resident memory equals whatever pages queries touch; the dictionary's upper levels and the norms array (60 KB at 1 byte per doc for 59,642 docs) will stay hot, and postings pages are faulted in on demand. Heap use can be limited to a file handle, the mmap, and per-query decode buffers (a 128-entry u32 block is 512 bytes).
- Memory-mapped pages are file-backed and evictable, so on macOS they do not count against the app's dirty/anonymous footprint the way the current 172 MB HashMap does.

### Gaps
- No source giving a measured resident-memory figure (MB of heap or RSS) for querying a memory-mapped Lucene or Tantivy index of a given size was found; the statements above are qualitative design claims from the projects themselves.
- A WebFetch summary of the Elastic Lucene 8 post claimed posting lists "remain on-heap by default"; this contradicts the rest of that post and Lucene's mmap design and looks like a summarisation error, so it is not reported as a finding.
- Lucene 10.x newer postings formats (after Lucene912) were not checked for layout changes.

## 5. What block-max WAND / MaxScore need per block, and what it adds

### Takeaway
Block-max pruning needs, per block of postings, the last doc id (already there for skipping) plus an upper bound on the block's score: Tantivy stores it as 2 bytes (fieldnorm id + saturating tf) inside an 8-byte per-128-postings skip entry, i.e. about 0.5 bits per posting in total and only for lists of at least 128 postings; Lucene stores a small set of competitive (freq, norm) pairs per block in its skip data.

### Cited Findings
- Tantivy skip entry per 128-doc block, with frequencies: last doc (4 bytes) + doc bit width (1) + tf bit width (1) + fieldnorm id (1) + block-wand term freq (1) = 8 bytes; 12 bytes when positions are indexed (adds a 4-byte tf sum); 5 bytes for doc-only. The block-wand tf saturates at 255. — [tantivy src/postings/skip.rs](https://github.com/quickwit-oss/tantivy/blob/main/src/postings/skip.rs)
- Tantivy picks, per block, the (fieldnorm_id, term_freq) pair that maximises the BM25 tf factor and stores that pair, so the bound is recomputed with current collection statistics at query time. — [tantivy serializer.rs](https://github.com/quickwit-oss/tantivy/blob/main/src/postings/serializer.rs)
- Lucene stores "pairs of term frequency and document length" per block rather than scores, so impacts work with any scoring function; document length is the one-byte norm. — [Elastic, "Faster retrieval of top hits in Elasticsearch with block-max WAND"](https://www.elastic.co/blog/faster-retrieval-of-top-hits-in-elasticsearch-with-block-max-wand); [Elastic, "What's new in Lucene 8"](https://www.elastic.co/blog/whats-new-in-lucene-8)
- Lucene writes impacts ("CompetitiveFreqDelta and CompetitiveNormDelta pairs") into both skip levels, only when frequencies are indexed. — [Lucene912PostingsFormat](https://lucene.apache.org/core/10_0_0/core/org/apache/lucene/codecs/lucene912/Lucene912PostingsFormat.html)
- Elastic's measured speedups from block-max WAND: term queries 3x-7x, conjunctions 3% to 7x, disjunctions from 8% slower to 15x faster; costs: scores may not be negative and total hit counts are no longer exact unless requested. — [Elastic BMW blog](https://www.elastic.co/blog/faster-retrieval-of-top-hits-in-elasticsearch-with-block-max-wand)
- PISA's BMW and BMM "store maximum impact scores for blocks of size 128, while VBMW uses blocks of average length 40". — [Mallia et al. ECIR 2019](https://research.engineering.nyu.edu/~suel/papers/daat-ecir19.pdf)
- Variable-sized-block BMW (Mallia, Ottaviano, Porciani, Tonellotto, Venturini, SIGIR 2017) is roughly 2x faster than fixed-block BMW; a compressed form of the block data cuts its space by roughly 50% for at most 10% slowdown. — [Mallia et al., "Faster BlockMax WAND with Variable-sized Blocks"](https://arpi.unipi.it/handle/11568/887216)
- Measured on web-scale data, plain MaxScore (which needs only one max score per term, no block data) is competitive with or faster than block-max methods for many codecs at k=10: Gov2 Random TREC05 with SIMD-BP128: MaxScore 6.27 ms, WAND 10.68, BMM 7.62, BMW 8.81, VBMW 5.75. — [Mallia et al. ECIR 2019](https://research.engineering.nyu.edu/~suel/papers/daat-ecir19.pdf)

### Inferences
- Overhead arithmetic for the Tantivy layout: 8 bytes per 128 postings = 0.5 bits per posting (0.75 with positions). Because lists under 128 postings get no skip data, on the target corpus this applies only to the postings in common terms; an upper bound if every posting were in a full block is 7,157,948 / 128 x 8 bytes = about 447 KB.
- MaxScore needs only a per-term maximum score (4 bytes per term, or computable from a stored max-tf/min-norm pair), so it is nearly free. With 60k documents an exhaustive BM25 evaluation of a multi-term query touches at most a few hundred thousand postings, so dynamic pruning is an optimisation to add later, not a prerequisite; reserving the 2 bytes per block in the format costs almost nothing.
- Storing (norm, tf) pairs instead of precomputed scores is the right choice for an index that is updated incrementally, because average document length and idf change as files are added.

### Gaps
- No source quantifies the index-size increase from Lucene impacts or from PISA's block-max files as a percentage; the Elastic posts give none.
- No block-max measurements exist for small corpora; all speedups quoted are for multi-million-document collections.

## 6. Rust crates implementing these codecs (state as of 2026-10-02)

### Takeaway
The Tantivy ecosystem crates (bitpacking, tantivy-bitpacker, tantivy-sstable, tantivy itself) plus roaring, sucds and memmap2 are all actively released in 2026 under MIT or MIT/Apache-2.0; the original fst crate (2021) and stream-vbyte (2023) are stable but not recently released.

### Cited Findings
Latest version, release date and licence as returned by the crates.io API on 2026-10-02:

| Crate | Latest | Released | Licence | Role | Source |
|---|---|---|---|---|---|
| bitpacking | 0.9.3 | 2026-01-08 | MIT | SIMD block bit-packing (BitPacker1x/4x/8x; 4x is the SIMD-BP128 layout) with delta coding | [crates.io](https://crates.io/crates/bitpacking) |
| tantivy-bitpacker | 0.10.0 | 2026-03-31 | MIT | Tantivy's scalar bit-packer / block codec | [crates.io](https://crates.io/crates/tantivy-bitpacker) |
| tantivy | 0.26.2 | 2026-09-08 | MIT | Full Lucene-like engine | [crates.io](https://crates.io/crates/tantivy) |
| tantivy-fst | 0.5.0 | 2023-11-21 | Unlicense/MIT | Tantivy's fork of fst | [crates.io](https://crates.io/crates/tantivy-fst) |
| tantivy-sstable | 0.7.0 | 2026-03-31 | MIT | Front-coded block dictionary alternative to FST | [crates.io](https://crates.io/crates/tantivy-sstable) |
| fst | 0.4.7 | 2021-06-06 | Unlicense/MIT | FST sets/maps, mmap-friendly | [crates.io](https://crates.io/crates/fst) |
| stream-vbyte | 0.4.1 | 2023-05-23 | "non-standard" licence field on crates.io; repo on Bitbucket | Stream VByte with SIMD decode | [crates.io](https://crates.io/crates/stream-vbyte) |
| sucds | 0.9.1 | 2026-08-29 | MIT OR Apache-2.0 | Succinct structures incl. Elias-Fano, rank/select | [crates.io](https://crates.io/crates/sucds) |
| vers-vecs | 1.10.2 | 2026-08-10 | MIT OR Apache-2.0 | Rank/select bit vectors, Elias-Fano | [crates.io](https://crates.io/crates/vers-vecs) |
| roaring | 0.11.5 | 2026-08-12 | MIT OR Apache-2.0 | Pure-Rust Roaring bitmaps | [crates.io](https://crates.io/crates/roaring) |
| croaring | 2.8.0 | 2026-09-20 | Apache-2.0 | Bindings to C CRoaring | [crates.io](https://crates.io/crates/croaring) |
| memmap2 | 0.9.11 | 2026-06-22 | MIT OR Apache-2.0 | Memory-mapped files | [crates.io](https://crates.io/crates/memmap2) |
| ptr_hash | 2.1.2 | 2026-09-27 | MIT | PtrHash minimal perfect hashing | [crates.io](https://crates.io/crates/ptr_hash) |
| ph | 0.11.0 | 2026-02-12 | MIT OR Apache-2.0 | MPHFs (bsuccinct) | [crates.io](https://crates.io/crates/ph) |
| boomphf | 0.6.0 | 2023-07-26 | MIT | BBHash-style MPHF | [crates.io](https://crates.io/crates/boomphf) |

- Usage signal (recent downloads per crates.io at the same time): roaring about 12.4M, fst 6.9M, bitpacking 5.8M, tantivy-fst 4.6M, tantivy 4.2M, sucds 0.27M, stream-vbyte about 21k. — [crates.io](https://crates.io/crates/roaring)
- The underlying C libraries are also maintained: streamvbyte 3.0.0 packaged June 2026 (Apache-2.0); CRoaring 4.6.1 packaged March 2026 (Apache-2.0 or MIT). — [Debian NEW queue: libstreamvbyte](https://dfsg-new-queue.debian.org/reviews/libstreamvbyte); [Debian NEW queue: croaring](https://dfsg-new-queue.debian.org/reviews/croaring/4.6.1+ds-0.1/94a3ef52)
- RISE, a new Rust inverted-index library from the PISA/Pisa-university group, was announced on arXiv in June 2026. — [Savino & Venturini, arXiv 2606.07187](https://arxiv.org/abs/2606.07187)

### Inferences
- Lowest-risk building blocks for a custom mmap index: `fst` (or `tantivy-fst`) for the dictionary, `bitpacking` for full 128-blocks, hand-written VInt for tails, `memmap2` for mapping. The fst crate's 2021 release date reflects a finished format rather than abandonment, given about 6.9M recent downloads, but it should be treated as feature-frozen.
- Adopting Tantivy wholesale gives all of sections 4-5 for free under MIT, at the cost of a larger dependency and its segment/merge model; it is the fastest route to a measured answer for this corpus (index the 59,642 files and read the .term/.idx file sizes).
- `stream-vbyte` is the weakest choice: stale since 2023, non-standard licence metadata, and byte-aligned codecs are the least compact for frequencies.
- No maintained Rust crate for Partitioned Elias-Fano, Binary Interpolative Coding, QMX or ANS posting codecs was identified; sucds/vers-vecs provide plain Elias-Fano only.

### Gaps
- Licence text for `stream-vbyte` was not inspected (crates.io reports only "non-standard"); verify before use.
- Whether RISE is published on crates.io, its licence and its codec list were not determined.
- Repository activity (open issues, last commit) was not checked beyond release dates; the crate name `quantized-pulp` queried during the search does not exist.
