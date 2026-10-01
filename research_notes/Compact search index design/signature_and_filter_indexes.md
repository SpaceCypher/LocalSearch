# Signature-file and filter-based search indexes (per-document probabilistic fingerprints)

Reader notes on provenance:
- "Verified" below means I read the primary text in this session (BitFunnel SIGIR 2017 paper, Binary Fuse paper, BuRR paper, COBS paper, RAMBO paper, CQF paper intro, Zobel et al. abstract, crate docs).
- Items marked **[background, not verified this session]** come from my prior knowledge of the literature and could not be re-checked against a source in the time available. They are placed under Gaps rather than Cited Findings.
- Almost every large-scale number in this family comes from web search (Bing, TREC Gov2) or genomics (k-mer indexes). Nothing found is a measured result for word-level text on a desktop corpus of about 60k files. This is flagged per section.

Prototype numbers used for comparison (supplied by the team): 59,642 files, 535,517 distinct terms, 7,157,948 term-document pairs ("postings"), 29.6 MB fingerprint index versus about 172 MB in-memory inverted index.

---

## 1. History and verdict on signature files (Zobel, Moffat, Ramamohanarao, TODS 1998)

### Takeaway
The 1998 verdict was unambiguous: inverted files beat signature files on speed, space and functionality. It was partly overturned for one regime only (very large RAM-resident, cluster-sharded, matching-only web indexes) by BitFunnel in 2017; BitFunnel's own measurements still show a compressed inverted index is 2x to 5x smaller per posting.

### Cited Findings
- Abstract, verbatim: "we ... demonstrate that inverted files are distinctly superior to signature files. Not only can inverted files be used to evaluate typical queries in less time than can signature files, but inverted files require less space and provide greater functionality." — [Moffat's publication page for the TODS 1998 paper](https://people.eng.unimelb.edu.au/ammoffat/abstracts/zmr98acmtods.html)
- The comparison used both experiments and "a refined approach to modelling of signature files", and a synthetic text database. — [same source](https://people.eng.unimelb.edu.au/ammoffat/abstracts/zmr98acmtods.html)
- A later secondary summary of the paper: signature files "are slower, offer less functionality, and require larger indexes", and bit-sliced signature files "under-perform on almost all counts". — [TopSig paper, arXiv 1204.5373](https://arxiv.org/pdf/1204.5373)
- BitFunnel's authors open with: "Since the mid-90s there has been a widely-held belief that signature files are inferior to inverted files for text indexing", then report Bing replaced a production inverted index with bit-sliced signatures. — [BitFunnel SIGIR 2017 paper](https://danluu.com/bitfunnel-sigir.pdf)
- BitFunnel lists the classic problems that the 1998 verdict rested on: (1) a single-term match must examine one signature per document, costing "considerably more CPU and memory cycles than the equivalent operation on an inverted index"; (2) term frequency is Zipfian, so signatures must be long to get an acceptable false-positive rate for the rarest terms; (3) document sizes vary widely, so signatures must be sized for the longest documents; (4) configuration is complex. — [BitFunnel paper](https://danluu.com/bitfunnel-sigir.pdf)
- BitFunnel's reason sharding by length was previously rejected: on hard disks it "would multiply the number of disk seeks by the number of shards"; that objection disappears when the index is in RAM or on SSD and already sharded across machines. — [BitFunnel paper](https://danluu.com/bitfunnel-sigir.pdf)
- Even in 2017 the space verdict held: on five TREC Gov2 shards BitFunnel used 11.69 to 38.43 bits per posting versus 6.15 to 7.64 for Partitioned Elias-Fano (PEF). BitFunnel was faster in all five, but PEF had the better space-time product (DQ) on the two short-document shards, by factors of 3.4 and 1.6. — [BitFunnel paper, Table 3](https://danluu.com/bitfunnel-sigir.pdf)

### Inferences
- The 1998 conclusion still holds for **space** and **functionality** (ranking data, positions, exact answers). What changed is the cost model: with the index in RAM, bitwise row scans became cheap enough that signatures can win on **throughput** for long documents and conjunctive queries.
- The prototype's 29.6 MB over 7,157,948 postings is about 33 bits per posting (my arithmetic). That sits inside BitFunnel's reported range (11.7 to 38.4) and is roughly 4x to 5x above what a compressed inverted index achieved per posting on Gov2 (6 to 8 bits). The 172 MB baseline is about 192 bits per posting, which indicates an uncompressed in-memory structure. The measured 5.8x saving is therefore largely a comparison against an uncompressed baseline, not against the state of the art for inverted indexes.
- Because 59,642 < 65,536, every document ID fits in 16 bits; a plain uncompressed `u16` posting array would be about 14.3 MB for postings (7,157,948 x 2 bytes, my arithmetic) plus the term dictionary. This is an exact, rankable, deletable structure of similar size to the filter index, before any compression. Caveat: Gov2 bits-per-posting figures do not transfer directly to a 60k-file corpus, and the dictionary for 535,517 terms is a separate cost the filter design avoids entirely.

### Gaps
- I did not read the full 1998 paper body, only the abstract, so the specific model parameters and the exact reasons given (e.g. handling of long documents, multi-term queries, update cost) are reported second-hand via BitFunnel's summary and TopSig.
- **[background, not verified this session]** The 1998 paper is generally understood to have also argued that signature files cannot support ranked queries well because they carry no within-document frequencies. I could not quote this from the primary source.

---

## 2. BitFunnel (Goodwin et al., SIGIR 2017, Bing)

### Takeaway
BitFunnel is the closest production prior art to the prototype: per-document Bloom signatures stored bit-sliced, with three fixes (higher-rank rows, frequency-conscious signatures, sharding by document length). It is explicitly a matching-only filter; ranking was done by a separate forward index holding term frequencies. That is the same split the prototype would need.

### Cited Findings
**How it works**
- Each document's term set is a Bloom-filter signature; storage is bit-sliced (one row per bit position across all documents), so a query intersects a few rows with word-wide AND operations. — [BitFunnel paper](https://danluu.com/bitfunnel-sigir.pdf); [Wikipedia: BitFunnel](https://en.wikipedia.org/wiki/BitFunnel)
- **Higher rank rows**: "BitFunnel generalizes the idea of blocking so that each term simultaneously hashes to multiple bit-sliced signatures with different blocking factors." A bit at rank r is the logical OR of 2^r rank-0 bits, i.e. one bit summarises a block of 2^r documents; higher-rank rows are shorter and scanned faster. The paper notes higher-rank rows "magnify the bit density" and introduce "correlated noise" that intersection does not remove. — [BitFunnel paper, sections 4.1 and 5.1](https://danluu.com/bitfunnel-sigir.pdf)
- **Frequency-conscious signatures**: the number of hash functions/rows is chosen per term by frequency. "rare terms require more hashes to ensure a given signal-to-noise level." Example from Gov2: a term with frequency 0.1 ("picture") needs far fewer hashes than one with frequency 0.0001 ("rotisserie") for a signal-to-noise ratio of 10. With classical Bloom filters "one must configure for the rarest term in the lexicon". — [BitFunnel paper, section 4.2](https://danluu.com/bitfunnel-sigir.pdf)
- **Sharding by document length**: "we partition the corpus according to the number of unique terms in each document such that each instance of BitFunnel manages a shard in which documents have similar sizes." Test shards used unique-term ranges 64-127, 128-255, 256-511, 1,024-2,047, 2,048-4,095. — [BitFunnel paper, section 4.3 and Table 1](https://danluu.com/bitfunnel-sigir.pdf)
- Configuration is a constrained optimisation maximising DQ (documents per unit storage times queries per unit compute) subject to a minimum signal-to-noise ratio (10 in the experiments). — [BitFunnel paper, section 5](https://danluu.com/bitfunnel-sigir.pdf)

**Reported numbers (TREC Gov2, 4-core i7-6700, 32 GB RAM; web-scale text, not desktop)**
- Ablation on Corpus D at bit density 0.15: classic bit-sliced signatures 46.7 bits/posting, 9.1 kQPS; plus frequency-conscious signatures 14.7 bits/posting, 24.0 kQPS; plus higher-rank rows 13.7 bits/posting, 57.0 kQPS. The paper states frequency consciousness plus higher-rank rows gives a "21x improvement" in DQ over classic bit-sliced signatures. — [BitFunnel paper, Table 2](https://danluu.com/bitfunnel-sigir.pdf)
- Versus PEF / MG4J / Lucene (conjunctive Boolean matching only):

  | Shard (unique terms/doc) | Docs (M) | BitFunnel QPS | PEF QPS | BitFunnel false positives | BitFunnel bits/posting | PEF bits/posting |
  |---|---|---|---|---|---|---|
  | A (64-127) | 5.870 | 21,427 | 14,675 | 1.62% | 38.43 | 7.64 |
  | B (128-255) | 7.545 | 8,674 | 5,049 | 4.32% | 20.72 | 7.33 |
  | C (256-511) | 3.726 | 12,722 | 3,959 | 3.88% | 16.91 | 6.63 |
  | D (1,024-2,047) | 0.494 | 57,014 | 8,268 | 2.43% | 13.69 | 6.25 |
  | E (2,048-4,095) | 0.157 | 105,782 | 13,151 | 2.64% | 11.69 | 6.15 |

  — [BitFunnel paper, Tables 1 and 3](https://danluu.com/bitfunnel-sigir.pdf)
- "BitFunnel's overall performance relative to PEF improves as document lengths increase"; the authors attribute this to row length being proportional to the number of documents. They also caution that some of the gain may come from compiling each query to x64 machine code. — [BitFunnel paper, section 6.3](https://danluu.com/bitfunnel-sigir.pdf)
- Production claim: running in Bing "for the last four years on thousands of servers", and versus the inverted-list engine it replaced it "improved server query capacity by a factor of 10." No production memory figure is given. — [BitFunnel paper, introduction](https://danluu.com/bitfunnel-sigir.pdf)

**Ranking**
- BitFunnel is positioned as a cheap filter in front of a "ranking oracle": "we insert inexpensive filters upstream of the oracle to discard documents that the oracle would score low." "BitFunnel wins when its time savings in the Boolean matching phase is greater than the time the oracle spends scoring false positives." For web search "the cost of filtering out false positives is negligible"; for exact-match (database-style) use "the cost of filtering out the false positives can be prohibitive". — [BitFunnel paper, section 3](https://danluu.com/bitfunnel-sigir.pdf)
- "The version of BitFunnel used by Bing includes a forward index with term frequencies used for BM25F ranking. Because this ranking code was not available to us at the time we designed our experiment, we limited our comparison to conjunctive boolean matching." — [BitFunnel paper, section 6.3](https://danluu.com/bitfunnel-sigir.pdf)

**Afterwards**
- Open-sourced on GitHub in September 2016 under the MIT licence; components are BitFunnel, WorkBench and NativeJIT; won the SIGIR 2017 Best Paper Award. — [Wikipedia: BitFunnel](https://en.wikipedia.org/wiki/BitFunnel); [SIGIR 2017 awards](https://sigir.org/sigir2017/program/awards)
- Follow-up academic work treated BitFunnel as a speed-for-space trade and hybridised it with inverted lists: "A Hybrid BitFunnel and Partitioned Elias-Fano Inverted Index" (WWW 2019) stores each part of the index in either BitFunnel or PEF form to meet a space or time budget, reporting time reductions of up to 47% against a hybrid baseline and intersections 16% to 76% faster than three encoding methods "without significantly increasing the index size". — [ACM DL entry](https://dl.acm.org/doi/pdf/10.1145/3308558.3313553) (numbers from search-result abstract; the page itself returned HTTP 403)
- A paper titled "Index Compression for BitFunnel Query Processing" (2018) also exists. — [ResearchGate listing](https://www.researchgate.net/publication/326134053_Index_Compression_for_BitFunnel_Query_Processing)

### Inferences
- The prototype's "block of 64 files with a Bloom filter over all terms in the block" is functionally a single rank-6 row layer in BitFunnel's terminology (2^6 = 64 documents per bit). BitFunnel's contribution beyond that is using several ranks at once, chosen per term by frequency. This is established prior art, not a new idea.
- The prototype's two measured weaknesses map directly onto BitFunnel's design: (1) no frequencies in the signature, solved in Bing by a separate forward index, not by enriching the signature; (2) OR-summarised rows accumulate density. BitFunnel's own data show false-positive rates of 1.6% to 4.3% were considered acceptable because a ranker re-checks candidates.
- BitFunnel is weakest exactly where a desktop corpus sits: many short documents (shard A: 38 bits/posting, DQ 3.4x worse than PEF). The prototype averages about 120 distinct terms per file (7,157,948 / 59,642, my arithmetic), which is BitFunnel's shard A/B regime.
- The prototype differs from BitFunnel in using an exact-size static filter (binary fuse) per document rather than a fixed-length Bloom row set, which removes the need for length sharding. I found no publication that combines per-document xor/fuse filters with block-level summaries for text search (see section 8).

### Gaps
- No public statement was found on whether Bing still runs BitFunnel in 2026. Search results describe the open-source repository as incomplete ("only a small portion of the algorithm was brought over") and apparently inactive, but I could not confirm the last commit date or archived status (GitHub API unauthenticated in this environment). — partial source: [bitfunnel.org](https://bitfunnel.org/)
- Dan Luu's write-up and QCon 2016 slides exist ([slides PDF](https://qconsf.com/sf2016/system/files/presentation-slides/danluu_bitfunnel_qcon_talk_draft_1.pdf)) but could not be parsed here; nothing from them is cited.
- How Bing handled document deletion and update inside BitFunnel rows is not described in the passages I read.

---

## 3. Hierarchical and bit-sliced filter indexes (Bloofi, SBT family, COBS, Mantis, RAMBO)

### Takeaway
There is a large, mostly genomics, literature on "many Bloom filters, find which contain X". The consistent findings are that tree-of-OR'ed-filters designs suffer from saturated upper levels, that flat bit-sliced layouts (COBS) with filter sizes adapted per block of similar-sized documents win in practice, and that systems tolerate very high single-probe false-positive rates (around 0.3) because queries contain many terms.

### Cited Findings
**Bloofi (Crainiceanu and Lemire, 2015)**
- A B+-tree-like hierarchy where leaves are the indexed Bloom filters and each inner node is the bitwise OR of its children; negative lookups can stop at the root, positive ones take logarithmic steps. It "works well for Bloom filters that are not densely populated" and inserts require updating multiple inner nodes. A flat bit-sliced variant, "Flat-Bloofi", is also given. — [Bloofi paper, arXiv 1501.01941](https://arxiv.org/pdf/1501.01941); [Apache Commons Collections note on multidimensional Bloom filters](https://commons.apache.org/proper/commons-collections/bloomFilters/multidimensional.html)

**Sequence Bloom Trees and successors (genomics)**
- Original SBT: querying 214,293 transcripts against 2,652 human RNA-seq experiments took "just under 4 days", using under 239 MB of RAM and one CPU. — [Solomon and Kingsford, PMC4804353](https://pmc.ncbi.nlm.nih.gov/articles/PMC4804353)
- AllSome SBT: construction time reduced 52.7%, query time reduced 39 to 85%, at up to 3x memory. — [AllSome SBT, bioRxiv](https://www.biorxiv.org/content/10.1101/090464.full.pdf)
- Mantis (exact, counting-quotient-filter based, maps k-mer to a "colour class" of experiments): index construction 6x faster and index 20% smaller than SSBT; queries 6 to 108x faster; no false positives; 200,400 transcripts over 2,652 experiments in 82 minutes versus close to 4 days for SSBT. — [Mantis paper](https://www.biorxiv.org/content/10.1101/217372v1.full-text)
- Index sizes on 1,000 microbial documents (COBS paper's comparison): HowDe-SBT 1,911 MiB; COBS 3,022 MiB; SSBT 3,254 MiB; SeqOthello 4,410 MiB; classic bit-sliced (ClaBS) 16,236 MiB; Mantis 16,486 MiB. — [COBS paper, arXiv 1905.09624](https://ar5iv.labs.arxiv.org/html/1905.09624)
- SBT variants were skipped beyond 10,000 documents "because their construction time was growing super-linearly". — [COBS paper](https://ar5iv.labs.arxiv.org/html/1905.09624)

**COBS (Bingmann, Bradley, Gauger, Iqbal, 2019)**
- Described as "a cross-over between an inverted index and Bloom filters"; targets k-mers of DNA "or q-grams from text documents"; false positives decrease exponentially with query length. — [COBS abstract](https://arxiv.org/abs/1905.09624)
- Scale: 100,000 microbial samples, 336,846 million 31-mers, 3.984 TiB input. — [COBS paper](https://ar5iv.labs.arxiv.org/html/1905.09624)
- **Handling differing document sizes**: the classic layout sizes every filter for the largest document; the compact layout "adapt[s] the size of each Bloom filter bit array to the document it indexes and aim[s] to keep the false positive rate constant", with parameters held constant for blocks of consecutive documents (documents are ordered by size; block size 1,024 documents in the 100,000-document run, 8,192 in an illustration). On 1,000 documents this cut the index from 16,236 MiB (classic) to 3,022 MiB (compact). — [COBS paper](https://ar5iv.labs.arxiv.org/html/1905.09624)
- Parameters: "the minimum k=1 and a high false positive rate around 0.3 are desirable for our q-gram index application". Measured single 31-mer false-positive rate was 22.7%, but zero false positives were returned for all queries of length 100 or more. Worked example: 70 distinct 31-grams, p = 0.3, threshold 0.5 gives a query false-positive probability of about 0.000143. — [COBS paper](https://ar5iv.labs.arxiv.org/html/1905.09624)
- COBS does not need the whole index in RAM (designed for external memory). — [COBS abstract](https://arxiv.org/abs/1905.09624)

**RAMBO (Gupta et al., SIGMOD 2021)**
- Solves "multiple set membership testing": documents are randomly partitioned into groups, each group's terms are merged into one Bloom filter, and the partitioning is repeated independently several times; the candidate set is the intersection across repetitions. Query time O(sqrt(K) log K) for K sets versus O(K) for an array of Bloom filters, with an O(log K) worst-case memory factor. — [RAMBO, arXiv 1910.02611](https://arxiv.org/pdf/1910.02611)
- 170 TB, 460,500 genome files; full index 1.8 TB with folding; target false-positive rate 0.01 (measured 0.0095 to 0.01). Per-k-mer query at 2,000 files: RAMBO 0.191 ms, COBS 2.72 ms, SSBT 161.58 ms. Index at 2,000 files: RAMBO 140 GB, COBS 28 GB, SSBT 72 GB, HowDe-SBT exceeded 192 GB RAM. Supports "cheap updates for streaming inputs" (insertion only). — [RAMBO genomics paper, arXiv 1910.04358](https://ar5iv.labs.arxiv.org/html/1910.04358)
- The RAMBO paper lists BitFunnel among the document-indexing baselines it compares against. — [RAMBO, arXiv 1910.02611](https://arxiv.org/pdf/1910.02611)

### Inferences
- The prototype's two-level design (per-file filter plus a merged filter per group of 64 consecutive files) is a two-level Bloofi/SBT, or equivalently a one-repetition RAMBO with deterministic rather than random grouping. The idea of merging documents into group filters to skip work is established prior art.
- RAMBO's insight is directly relevant to weakness (2): with a single grouping, one polluted block must always be opened; with R independent random groupings, a document is a candidate only if all R of its groups match, so stale bits in one grouping are masked by the others. This trades memory (RAMBO was 5x larger than COBS in its own comparison) for robustness.
- The literature's answer to "documents of very different sizes" is consistently: size each filter (or each block of similar-sized documents) to its content at a fixed false-positive rate (COBS compact, BitFunnel length shards). A per-file binary fuse filter is the limiting case of this (exact sizing per document), so the prototype already has this property at the file level; the fixed-size block Bloom filters do not.
- These systems accept 1% to 30% per-probe false positives because queries have tens of k-mers. A desktop search with 1 to 3 word queries cannot rely on that amplification, which is why the prototype's 16-bit fingerprints (about 1 in 65,536 by design) are a different operating point from anything in this literature.

### Gaps
- All sizes and timings in this section are for genomic k-mer sets (documents with 10^6 to 10^8 keys), not word-level text with about 120 keys per document. I found no evaluation of COBS, Bloofi or RAMBO on a desktop-scale text corpus, although COBS says it supports text q-grams.
- I did not retrieve concrete numbers (filter counts, speed-ups) from the Bloofi paper itself, nor primary numbers for HowDe-SBT and SSBT beyond those re-reported by COBS and RAMBO.
- Results conflict across papers on which is smallest: COBS reports HowDe-SBT as smaller than COBS at 1,000 documents; RAMBO reports HowDe-SBT exceeding RAM at 2,000 files. These are different datasets and metrics (index size versus build memory), so they are not directly contradictory, but neither should be treated as a universal ranking.

---

## 4. Modern static and dynamic filters: bits per key and updatability

### Takeaway
Binary fuse filters are near the practical optimum for immutable sets (about 9 bits/key at 0.4% and 18 bits/key at about 0.0015% false positives for large sets), but they are immutable and their published efficiency is for large sets; per-document sets of about 100 keys are outside the regime the paper measures. Filters that support deletion (cuckoo, quotient) cost roughly 2 to 3 extra bits per key plus empty-slot overhead.

### Cited Findings
- Lower bound: log2(1/eps) bits per key. Bloom filters need 1.44 log2(1/eps), i.e. 44% over the bound, and about seven memory accesses at 1% false positives. — [Graf and Lemire, Binary Fuse Filters, arXiv 2201.01174](https://arxiv.org/pdf/2201.01174)
- Xor filters: 23% over the bound; 9.84 bits/key with 8-bit fingerprints; array size about 1.23n + 32 words. Xor+ about 14 to 15% over. — [Binary Fuse Filters paper](https://arxiv.org/pdf/2201.01174); [BuRR paper](https://arxiv.org/pdf/2109.01892)
- Binary fuse, measured "for large sets, with at least a million keys": 3-wise reaches 9 bits/key (12.5% over the bound); 4-wise about 8.6 bits/key (7.5% over), for 8-bit fingerprints with false-positive probability 1/2^8 (about 0.4%). "The number of bits per entry for the 16-bit filters would be double." — [Binary Fuse Filters paper](https://arxiv.org/pdf/2201.01174)
- Small-set caveat: "For smaller sets, the benefits of the binary fuse filters are less significant. For tiny sets (fewer than 20,000 keys), xor filters might even be slightly smaller due to a smaller overhead." and "More expensive constructions could be advantageous for small sets." — [Binary Fuse Filters paper](https://arxiv.org/pdf/2201.01174)
- Immutability: "Xor and binary fuse filters require access to the full set of keys at construction time. In this sense, they are immutable." Construction is "more than twice as fast" as xor in some cases; 3-wise query speed matches xor, 4-wise is slightly slower. — [Binary Fuse Filters paper](https://arxiv.org/pdf/2201.01174)
- Cuckoo filters "may use only 30% to 40% more memory than the optimal bound in the 12-bit and 16-bit cases"; benchmarked at maximum load 0.94. At very low false-positive rates a cuckoo filter becomes slightly smaller than fuse: 3-wise and 4-wise fuse need 65 and 62 bits to match a 64-bit cuckoo filter. — [Binary Fuse Filters paper](https://arxiv.org/pdf/2201.01174)
- Blocked Bloom filters are the fastest to query but cost about 30% more space than a standard Bloom filter. — [Binary Fuse Filters paper](https://arxiv.org/pdf/2201.01174)
- Functionality summary from the BuRR authors: "(Blocked) Bloom allows dynamic insertion, Cuckoo, Morton and Quotient additionally allow deletion and counting"; xor, coupled, LMSS and all ribbon variants are static. — [BuRR paper](https://arxiv.org/pdf/2109.01892)
- Quotient filters "incur an overhead of a few bits per key (2-3 depending on the implementation) plus a multiplicative overhead due to empty entries" and "support not only insertions but also deletions and counting"; they "cannot compete with alternatives (e.g., blocked Bloom filters and cuckoo filters) for dynamic AMQs without deletions". Cuckoo and Morton filters "are the most space efficient dynamic AMQ for small" false-positive rates. — [BuRR paper, related work](https://arxiv.org/pdf/2109.01892)
- Ribbon filters: standard ribbon about 10% overhead; homogeneous ribbon avoids construction failure; for a 1% false-positive rate about 6.65 bits/key versus about 10 for Bloom (33% reduction, the RocksDB use case). — [BuRR paper](https://arxiv.org/pdf/2109.01892); [blog write-up of ribbon filters in RocksDB](https://mohakchugh.is-a.dev/blog/ribbon-filters-space-optimal-static-filters-rocksdb) (secondary source for the 6.65 figure)
- BuRR: overhead "well below 1%", "about 0.1% already for r = 8 and n = 2^24", against "around 10%" for the best previous competitor, at a moderate speed penalty. — [BuRR paper](https://arxiv.org/pdf/2109.01892)
- The counting quotient filter "supports deletions, counting (even on skewed data sets), resizing, merging, and highly concurrent access" and is reported as smaller than the Bloom, counting Bloom and spectral Bloom filters in its regime. — [Pandey et al., CQF paper (SIGMOD 2017)](https://users.cs.utah.edu/~pandey/courses/cs6968/spring23/papers/cqf.pdf)

### Inferences
- Theoretical floor for the prototype's per-file filters: 7,157,948 keys x 16 bits x 1.125 is about 16.1 MB (my arithmetic). The measured 29.6 MB total therefore includes roughly 13 MB of block Bloom filters, per-filter headers and small-set padding. With about 120 keys per file the fixed per-filter overhead (seed, segment layout, the xor filter's "+32" slots or the fuse equivalent) is a significant fraction and is the first place to look for savings.
- Because per-file sets are tiny, concatenating many files into one larger retrieval structure, or using a ribbon-style structure whose overhead does not depend on set size in the same way, may recover part of that gap. This is not something the papers above evaluate at 100-key scale.
- The measured false-match rate of about 1 per 110,000 probes is somewhat better than the 1/65,536 expected from 16-bit fingerprints; that is plausible only if some probes are rejected earlier (for example by block skipping or empty slots), and is worth checking so the reported figure is understood.

### Gaps
- No paper found reports binary fuse or xor filter space for sets of about 100 keys; the published curves start at larger sizes. The real per-key cost at that scale must be measured (the prototype has effectively done this).
- **[background, not verified this session]** Cuckoo filter (Fan et al., CoNEXT 2014) space is commonly quoted as (log2(1/eps) + 3) / 0.955 bits per item for 4-way buckets (about 2 with semi-sorting), and the counting quotient filter as about (2.125 + log2(1/eps)) / load-factor bits per item. I could not extract these formulas from the PDFs in this session.

---

## 5. Storing a small value with each key (term frequency alongside membership)

### Takeaway
This is a solved problem with a name: a **static function / retrieval data structure** (the Bloomier filter family). A fuse or xor filter already is one: it stores an r-bit value per key that happens to be a fingerprint. Storing a 2-bit frequency bucket costs either 2 of the existing 16 bits (false-positive rate goes from 2^-16 to 2^-14 at zero extra space) or about 2 x 1.125 = 2.25 extra bits per key. Membership rejection and value retrieval can share one structure.

### Cited Findings
- Definition: a retrieval data structure ("sometimes called static function") represents f: S -> {0,1}^r; "a query for x in S must return f(x), but a query for x not in S may return any value". The lower bound is n*r bits, far below a dictionary, because "retrieval data structures, surprisingly, need not store the keys". — [Dillinger, Hübschle-Schneider, Sanders, Walzer, "Fast Succinct Retrieval and Approximate Membership using Ribbon" (BuRR), arXiv 2109.01892](https://arxiv.org/pdf/2109.01892)
- Filters are built from retrieval structures by storing a fingerprint as the value: the paper treats "retrieval-based AMQs" as a class and notes cuckoo filters "store a random fingerprint for each key (similar to retrieval-based AMQs)"; the false-positive rate is 2^-r for r retrieved bits. — [BuRR paper](https://arxiv.org/pdf/2109.01892)
- Xor filters are described as "a recent implementation of peeling-based retrieval" with 23% overhead (14% for Xor+); fuse filters are "a variant of Xor". So overhead for storing r bits per key with these is r x 1.23 (xor) or r x 1.125 / 1.075 (3-wise / 4-wise fuse). — [BuRR paper](https://arxiv.org/pdf/2109.01892); [Binary Fuse Filters paper](https://arxiv.org/pdf/2201.01174)
- BuRR: "the first practical succinct retrieval data structure"; overhead well below 1% (about 0.1% for r = 8, n = 2^24); construction O(n w), query O(1 + r w / log n); query cost grows with r (about r*w/2 bits processed per query). — [BuRR paper](https://arxiv.org/pdf/2109.01892)
- GOV (the Genuzio-Ottaviano-Vigna construction, as in Sux4J) "is several times slower than BuRR and exhibits an unfavorable time-overhead tradeoff" in the r = 1 benchmark. — [BuRR paper](https://arxiv.org/pdf/2109.01892)
- FiRe (filtered retrieval) "supports updates to function values as well as a limited form of insertions" but needs "a constant number of metadata bits per key (around 4)", and its only known implementation is closed source (SAP). — [BuRR paper](https://arxiv.org/pdf/2109.01892)
- Minimal perfect hashing plus a value array is listed as the classic way to get "linear construction time and O(1) worst-case query time" retrieval. — [BuRR paper](https://arxiv.org/pdf/2109.01892)
- **Rust: `csf` crate** (part of BSuccinct; version 0.1.14, 2024-02-24): `ls::Map` and `fp::Map` are static functions to b-bit unsigned integers taking "somewhat more than nb bits"; `ls::CMap`, `fp::CMap`, `fp::GOCMap` are *compressed* static functions using space "slightly larger than |X|H, where H is the entropy of the distribution" of values. Explicitly: "None of the static functions ... explicitly store keys. Therefore, these functions are usually unable to detect whether an item belongs to the set of keys". — [csf crate docs](https://docs.rs/crate/csf/latest); [bsuccinct-rs repository](https://github.com/beling/bsuccinct-rs)
- **Rust: `compressed_map`** (frayed ribbon filters, built for CRLite-style revocation maps): `CompressedMap` uses "asymptotically between 0.1% more and 11% more" space than the Shannon entropy of the values; `ApproxSet` takes "about 30% less space" than a Bloom filter; construction is Õ(n^1.5) time; README says "This implementation is still research-grade. Don't deploy it in production." — [compressed_map repository](https://github.com/bitwiseshiftleft/compressed_map)
- General background on the concept. — [Wikipedia: Retrieval data structure](https://en.wikipedia.org/wiki/Retrieval_Data_Structure)

### Inferences
- **Cheapest option for the prototype**: keep the 16-bit slots and define the stored value as (14-bit fingerprint, 2-bit frequency bucket). A member key returns its exact bucket; a non-member matches the fingerprint part with probability 2^-14 (1 in 16,384, versus 1 in 65,536 now). No extra memory. A 12 + 4 split gives a 4-bit bucket at 1 in 4,096. This follows directly from the retrieval definition above; it is the Bloomier-filter construction and is not novel.
- **If the false-positive rate must stay at 2^-16**: widen slots to 18 bits (or add a parallel 2-bit fuse array over the same key set). Cost about 2.25 bits per posting, i.e. about 2.0 MB on 7,157,948 postings (my arithmetic), roughly a 7% increase on 29.6 MB.
- **Entropy-coded option**: term-frequency buckets are highly skewed (most postings have tf = 1), so a compressed static function (`csf::CMap`-style, cost near n x H) could store the bucket in well under 2 bits per posting, possibly under 1. The cost is a variable-length decode per lookup and no membership rejection, so it would sit beside the fuse filter, not replace it.
- The prototype measured 61% to 86% top-20 overlap when simulating a 2-bit bucket. The literature gives the storage mechanism but I found no source quantifying ranking quality as a function of quantised term-frequency bits inside a signature structure; the closest prior art is impact-quantised inverted indexes, which is outside this brief.
- BitFunnel's production answer was different: keep matching and ranking data separate (forward index with term frequencies). A per-document forward record of (term fingerprint -> tf) is exactly what a per-file retrieval structure with value bits is, so the prototype's per-file fuse filter with value bits effectively merges BitFunnel's signature and forward index into one object.

### Gaps
- Original Bloomier filter paper (Chazelle, Kilian, Rubinfeld, Tal, SODA 2004) and Othello hashing were not retrieved; no numbers from them are cited. **[background, not verified this session]** Othello hashing is generally described as using about 2.33 bits per key for a 1-bit value and supporting value updates and key insertion; SeqOthello (cited in section 3 with a 4,410 MiB index) is its genomics application.
- **[background, not verified this session]** Minimal perfect hash functions cost roughly 2 to 3 bits per key in practical implementations (the Rust `ph` crate's fingerprint-based functions and `boomphf` are in the 2 to 4 bits/key range), so MPHF + packed value array costs about r + 2 to 3 bits per key and gives no membership rejection unless a fingerprint is also stored. I did not verify the `ph`, `boomphf`, `sucds`, `xorf`, `cuckoofilter` or `qfilter` crate documentation in this session.
- I found no Rust crate that offers BuRR. Whether the `xorf` crate exposes fuse filters in a way that allows storing arbitrary values rather than hash-derived fingerprints was not checked; if not, the change is small (the construction solves for an arbitrary r-bit target per key).

---

## 6. Updatable alternatives to plain Bloom filters for the block summaries

### Takeaway
The "Bloom filters cannot forget" problem is standard. Published options are counting Bloom filters (several times the space), cuckoo/quotient filters (deletable, about 2 to 3 bits per key over a fingerprint plus slack), or periodic rebuild. For 64-file blocks, rebuilding one block summary from its 64 per-file term lists is cheap and is what immutable-segment designs do.

### Cited Findings
- "Bloom filters don't support deletion"; "The counting Bloom filter can count and delete items, but does not support skewed" inputs well; cuckoo and quotient filters support deletion by storing fingerprints. — [CQF paper](https://users.cs.utah.edu/~pandey/courses/cs6968/spring23/papers/cqf.pdf)
- Cuckoo, Morton and quotient filters "allow deletion and counting"; quotient filters cost "2-3" bits per key over the fingerprint plus empty-slot overhead; cuckoo/Morton are "the most space efficient dynamic AMQ" at low false-positive rates. — [BuRR paper](https://arxiv.org/pdf/2109.01892)
- Cuckoo filters use 30% to 40% more than the lower bound at 12 to 16 bit fingerprints, at load 0.94. — [Binary Fuse Filters paper](https://arxiv.org/pdf/2201.01174)
- Cuckoo filters "outperform previous data structures that extend Bloom filters to support deletions substantially in both time and space". — [Fan et al., Cuckoo Filter, CoNEXT 2014](https://sigcommconfs.hosting2.acm.org/co-next/2014/CoNEXT_papers/p75.pdf) (quoted via search-result summary)
- Tree-of-filters indexes share the problem: in Bloofi "inserting often requires updates to multiple inner nodes", and it works best when filters have "low saturation". — [Apache Commons note on Bloofi](https://commons.apache.org/proper/commons-collections/bloomFilters/multidimensional.html)
- RAMBO supports only insertion-style streaming updates. — [RAMBO, arXiv 1910.04358](https://ar5iv.labs.arxiv.org/html/1910.04358)
- xor/fuse/ribbon filters are immutable and must be rebuilt from the full key set. — [Binary Fuse Filters paper](https://arxiv.org/pdf/2201.01174)

### Inferences
- A block summary is the union of 64 per-file term sets. A term may occur in several files of the block, so a deletable block filter must count occurrences (counting Bloom, counting quotient filter, or a cuckoo filter tolerating duplicate fingerprints) or be recomputed from the member files. Recomputing means enumerating terms of the 63 unchanged files, which a fuse filter cannot do (it does not store keys). So a "rebuild this block only" strategy requires the per-file term lists to be recoverable from elsewhere (re-tokenising the files, or a stored term-ID list). This is a structural consequence of using key-free filters and is the main design constraint to surface.
- Options ranked by memory cost, from the cited overheads: (a) keep plain Bloom and rebuild a block's summary lazily when its stale-bit count crosses a threshold: zero extra space, needs term enumeration; (b) cuckoo filter per block with short fingerprints: about 1.3 to 1.4x the lower bound and deletable, but duplicates across files need handling; (c) 4-bit counting Bloom: roughly 4x the Bloom size.
- The measured rise from 21 to 135 blocks opened per lookup is the "correlated noise / saturation" effect BitFunnel and Bloofi describe for OR-summarised rows; it is expected behaviour, not an implementation defect.
- An alternative that avoids deletion entirely: do not reuse slots. Treat blocks as immutable segments, append edited files to a fresh tail block, mark old file slots dead in a bitmap, and compact in the background (the LSM/Lucene segment approach). Block summaries then only over-approximate by dead files' terms until compaction, which is the same staleness the prototype measured, so the gain comes only from making compaction incremental per block rather than a full rebuild.

### Gaps
- **[background, not verified this session]** Counting Bloom filters are conventionally built with 4-bit counters (about 4x a plain Bloom filter); "stable" or ageing Bloom filters (Deng and Rafiei, SIGMOD 2006) evict randomly and therefore introduce false negatives, which would break the prototype's "0 missed matches" property. No source for either was retrieved in this session.
- No published measurement was found of how block-summary selectivity degrades under edits for a text corpus; the prototype's 21 to 135 figure appears to be new data.

---

## 7. Does ordering or clustering documents help signature-based indexes?

### Takeaway
Document reordering (by URL or content similarity) is well established for **inverted index compression**, where it shrinks d-gaps. For signature and filter indexes the published benefit of ordering is about grouping by **document size**, not by topic. The prototype's finding (about 6% from folder order) is consistent with that; I found no source claiming large gains from topical ordering in a filter index.

### Cited Findings
- Document ID reassignment places textually similar documents at consecutive IDs so that inverted lists become clustered and compress better; it "can significantly improve index compression compared to random document ordering". — [Silvestri et al., "Assigning identifiers to documents to enhance the clustering property of fulltext indexes", SIGIR 2004](https://www.dais.unive.it/~orlando/PAPERS/sigir04assignment.pdf); [Yan, Ding, Suel, "Inverted index compression and query processing with optimized document ordering", WWW 2009](https://www.doi.org/10.1145/1526709.1526764)
- For signature indexes, the ordering that matters in published systems is by size: BitFunnel partitions "according to the number of unique terms in each document"; COBS groups consecutive documents into blocks with a common filter size chosen from the largest document in the block, shrinking the index from 16,236 MiB to 3,022 MiB on 1,000 documents. — [BitFunnel paper](https://danluu.com/bitfunnel-sigir.pdf); [COBS paper](https://ar5iv.labs.arxiv.org/html/1905.09624)
- TopSig's review of signature files states that "the presumed advantages of efficient bit-wise processing and potential for index compression are not generally achievable in practice". — [TopSig, arXiv 1204.5373](https://arxiv.org/pdf/1204.5373)
- Tree-of-filter indexes do cluster by similarity: SBT-family construction orders/clusters leaves so that sibling filters share bits, and HowDe-SBT was the smallest index in COBS's 1,000-document comparison (1,911 MiB versus 3,022 MiB for COBS). — [COBS paper](https://ar5iv.labs.arxiv.org/html/1905.09624)

### Inferences
- In the prototype, per-file filter size is fixed by the file's term count regardless of order, so ordering can only help the block layer: a block summary is smaller or sparser when its 64 files share vocabulary. With block summaries being a minority of the 29.6 MB, a ceiling of a few percent is what one would expect, matching the measured 6%.
- Where ordering would matter more is **selectivity** (number of blocks opened per query), not memory: if files containing a term are concentrated in few blocks, fewer blocks are opened. The prototype's 21 blocks per lookup on a clean index should be compared against random order to see whether folder order is buying query time even though it buys little memory. I found no published measurement of this for filter indexes.
- If an inverted or hybrid layout were adopted instead, the reordering literature suggests folder/path order would give a larger benefit there than it does for the filter layout.

### Gaps
- I did not extract quantitative reordering gains (percent reduction in bits per posting) from the Silvestri or Yan/Ding/Suel papers; only their qualitative conclusion is cited.
- No source was found that measures URL- or path-ordering specifically on a Bloom/signature index for text. This appears to be unreported; the prototype's 6% figure may be the only data point.

---

## 8. Published desktop or code-search systems using per-document filters

### Takeaway
Mainstream code and desktop search engines use inverted indexes (positional trigrams or n-grams), not per-file filters. Per-file Bloom filters do appear, but as a secondary pre-filter stage, or in hobby/"serverless" engines. I found no published system that uses per-document xor/binary fuse filters with block summaries as the primary text index.

### Cited Findings
- Zoekt (Sourcegraph's indexed search) "uses positional trigram indexing, which involves storing the offsets of 3-grams within files". — [Zoekt indexing overview](https://shoulder.dev/github.com/sourcegraph/zoekt/learn/codebase/indexing); [Sourcegraph architecture docs](https://sourcegraph.com/docs/admin/architecture.md)
- GitHub's code search engine (Blackbird) was written from scratch in Rust because general text-search products gave a poor experience, were slow to index and expensive to host; it is an n-gram inverted index design. — [GitHub engineering blog, "The technology behind GitHub's new code search"](https://github.blog/engineering/architecture-optimization/the-technology-behind-githubs-new-code-search/)
- `ix` (crate `moeix`), a Rust code search tool: a byte-level trigram inverted index with ZSTD-compressed posting blocks narrows candidates, then "candidates are filtered through per-file bloom filters (256 B, 0.7% false-positive rate)" with 5 hashes, then a streaming regex matcher verifies. The per-file filters were 1.2% of index size (18 KB for 70 files) in its README example. — [moeix README on docs.rs](https://docs.rs/crate/moeix/0.12.9/source/README.md)
- A Bloom-filter-per-document web search engine built on Go and AWS Lambda, explicitly modelled on BitFunnel, was presented at ServerlessDays ANZ 2024. — [Ben Boyter, "Abusing Go, AWS Lambda and bloom filters to make a true Australian serverless search engine"](https://boyter.org/static/serverlessdaysanz2024/)
- A Go library `go-bloomindex` implements a Bloom-filter document index in the BitFunnel style. — [go-bloomindex on pkg.go.dev](https://beta.pkg.go.dev/github.com/dgryski/go-bloomindex)
- A Cassandra secondary index based on multidimensional Bloom filters (Bloofi-style) has been described in a vendor blog. — [Instaclustr blog](https://www.instaclustr.com/blog/multidimensional-bloom-filter-secondary-index-the-what-why-and-how)

### Inferences
- **Established prior art** (the prototype is re-deriving these): per-document Bloom signatures for text (1980s-1990s signature files); bit-slicing; block/"higher-rank" summaries over groups of documents (BitFunnel, Bloofi, SBT); sizing filters to document size (COBS compact, BitFunnel shards); merging groups of documents into shared filters (RAMBO); storing a value with a key in a key-free structure (Bloomier / retrieval / static functions); matching-only filter plus separate frequency store for ranking (BitFunnel + forward index).
- **Apparently unexplored or at least unpublished** (nothing found in this search; absence of evidence, not proof of novelty):
  1. Using xor/binary fuse filters, rather than Bloom filters, as the per-document signature in a text search index. The filter papers target key-value stores (RocksDB/LSM) and the signature-index papers all use Bloom rows.
  2. Packing a quantised term-frequency bucket into the fingerprint slot of a per-document retrieval structure to recover BM25-like ranking. The mechanism is textbook; an evaluation of ranking quality versus bits (the prototype's 61% to 86% top-20 overlap) was not found in the literature.
  3. Measurements at desktop scale (tens of thousands of short documents, 1 to 3 term queries, 2^-16 false-positive target). Published systems are either web scale with a ranker absorbing 2% to 4% false positives, or genomics with long multi-key queries absorbing 20% to 30% per-probe false positives.
  4. Quantified degradation of block summaries under edits (21 to 135 blocks opened).
- The practical lesson from the systems that do exist is that per-file filters are used as a *complement* to a compact inverted index, not a replacement (`ix`), or as the matching stage ahead of an exact scorer (BitFunnel). A hybrid (inverted lists for frequent terms where they are cheapest, filters for the long tail) is what the 2019 BitFunnel-PEF hybrid paper proposes.

### Gaps
- **[background, not verified this session]** Sourcegraph experimented with adding Bloom filters to Zoekt shards (to skip shards that cannot match) and I recall this being later removed; I could not confirm details or numbers. Elasticsearch/Lucene have a Bloom-filter postings format used for primary-key lookups, not for full-text matching. Neither was verified.
- No information was found on whether macOS Spotlight, Windows Search, Recoll, Everything or similar desktop engines use per-document filters; all are, to my knowledge, inverted-index or filename-table designs, but I retrieved no source for this.
- No academic evaluation of per-file trigram Bloom filters for code search was found; `ix` reports design parameters, not comparative benchmarks.
