//! Prototype and end-to-end simulation of a document-centric
//! "folder-clustered fingerprint index", measured against the real saved
//! index (read-only; nothing in the data directory is modified).
//!
//!     cargo run --release --example proto_fingerprint_index
//!
//! Layout under test:
//!   * documents are numbered in path order, so neighbours share a folder;
//!   * each document stores a fingerprint filter of its terms (binary fuse
//!     filter with 16-bit fingerprints; tiny documents keep raw 32-bit hashes);
//!   * documents are grouped in blocks of BLOCK consecutive ids, each with a
//!     Bloom filter over every term in the block, used to skip whole blocks.
//!
//! The simulation covers: build, lookups across the whole vocabulary,
//! multi-word queries, what ranking loses without term counts, edits, adds
//! and deletes, saving and loading, and behaviour at 10x the corpus.

use localsearch::engine::Engine;
use localsearch::index::delta::{DocId, Posting, FIELD_CONTENT};
use localsearch::index::segment::Segment;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use xorf::{BinaryFuse16, Filter};
use xxhash_rust::xxh3::xxh3_64;

const BLOCK: usize = 64;
/// Below this many terms a fuse filter's fixed overhead costs more than raw hashes
const SMALL_DOC: usize = 24;
const BLOCK_BLOOM_BITS_PER_TERM: usize = 10;
const BLOCK_BLOOM_HASHES: u32 = 7;

#[derive(Serialize, Deserialize)]
enum DocFilter {
    Small(Box<[u32]>),
    Fuse(BinaryFuse16),
    /// Deleted document: matches nothing, costs nothing
    Dead,
}

impl DocFilter {
    fn build(terms: &[u64]) -> Self {
        if terms.len() < SMALL_DOC {
            let mut hashes: Vec<u32> = terms.iter().map(|&h| h as u32).collect();
            hashes.sort_unstable();
            hashes.dedup();
            DocFilter::Small(hashes.into_boxed_slice())
        } else {
            DocFilter::Fuse(BinaryFuse16::try_from(terms).expect("fuse filter builds"))
        }
    }

    fn contains(&self, hash: u64) -> bool {
        match self {
            DocFilter::Small(hashes) => hashes.binary_search(&(hash as u32)).is_ok(),
            DocFilter::Fuse(filter) => filter.contains(&hash),
            DocFilter::Dead => false,
        }
    }

    fn bytes(&self) -> usize {
        match self {
            DocFilter::Small(hashes) => hashes.len() * 4,
            DocFilter::Fuse(filter) => filter.fingerprints.len() * 2 + 32,
            DocFilter::Dead => 0,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Bloom {
    bits: Box<[u64]>,
}

impl Bloom {
    fn new(terms: usize) -> Self {
        let bits = (terms * BLOCK_BLOOM_BITS_PER_TERM).max(64);
        Self { bits: vec![0u64; bits.div_ceil(64)].into_boxed_slice() }
    }

    fn slot(&self, hash: u64, i: u32) -> usize {
        let h2 = hash.rotate_left(32) | 1;
        (hash.wrapping_add(h2.wrapping_mul(i as u64)) % (self.bits.len() as u64 * 64)) as usize
    }

    fn insert(&mut self, hash: u64) {
        for i in 0..BLOCK_BLOOM_HASHES {
            let p = self.slot(hash, i);
            self.bits[p / 64] |= 1 << (p % 64);
        }
    }

    fn contains(&self, hash: u64) -> bool {
        (0..BLOCK_BLOOM_HASHES).all(|i| {
            let p = self.slot(hash, i);
            self.bits[p / 64] & (1 << (p % 64)) != 0
        })
    }

    fn bytes(&self) -> usize {
        self.bits.len() * 8
    }

    fn fill(&self) -> f64 {
        self.bits.iter().map(|w| w.count_ones() as f64).sum::<f64>() / (self.bits.len() * 64) as f64
    }
}

#[derive(Serialize, Deserialize)]
struct FingerprintIndex {
    filters: Vec<DocFilter>,
    blocks: Vec<Bloom>,
}

impl FingerprintIndex {
    fn build(doc_terms: &[Vec<u64>]) -> Self {
        let filters = doc_terms.iter().map(|terms| DocFilter::build(terms)).collect();
        let blocks = doc_terms
            .chunks(BLOCK)
            .map(|chunk| {
                let distinct: HashSet<u64> = chunk.iter().flatten().copied().collect();
                let mut bloom = Bloom::new(distinct.len());
                distinct.into_iter().for_each(|hash| bloom.insert(hash));
                bloom
            })
            .collect();
        Self { filters, blocks }
    }

    /// Documents whose fingerprint contains the term, and how many blocks were opened
    fn lookup(&self, hash: u64) -> (Vec<usize>, usize) {
        let mut found = Vec::new();
        let mut opened = 0;
        for (block_index, block) in self.blocks.iter().enumerate() {
            if !block.contains(hash) {
                continue;
            }
            opened += 1;
            let start = block_index * BLOCK;
            let end = (start + BLOCK).min(self.filters.len());
            for (offset, filter) in self.filters[start..end].iter().enumerate() {
                if filter.contains(hash) {
                    found.push(start + offset);
                }
            }
        }
        (found, opened)
    }

    /// Every term must be present. Blocks are skipped unless they may hold all of them.
    fn lookup_all(&self, hashes: &[u64]) -> Vec<usize> {
        let mut found = Vec::new();
        for (block_index, block) in self.blocks.iter().enumerate() {
            if !hashes.iter().all(|&h| block.contains(h)) {
                continue;
            }
            let start = block_index * BLOCK;
            let end = (start + BLOCK).min(self.filters.len());
            for (offset, filter) in self.filters[start..end].iter().enumerate() {
                if hashes.iter().all(|&h| filter.contains(h)) {
                    found.push(start + offset);
                }
            }
        }
        found
    }

    /// A file changed: rebuild its fingerprint, and teach its block the new
    /// terms. The block cannot forget the old ones (a Bloom filter has no delete).
    fn update(&mut self, doc: usize, terms: &[u64]) {
        self.filters[doc] = DocFilter::build(terms);
        let block = &mut self.blocks[doc / BLOCK];
        terms.iter().for_each(|&hash| block.insert(hash));
    }

    fn bytes(&self) -> usize {
        self.filters.iter().map(DocFilter::bytes).sum::<usize>()
            + self.blocks.iter().map(Bloom::bytes).sum::<usize>()
            + self.filters.len() * std::mem::size_of::<DocFilter>()
    }
}

fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[samples.len() / 2]
}

fn percentile(samples: &mut [f64], p: usize) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[(samples.len() * p / 100).min(samples.len() - 1)]
}

fn time_ms<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let out = f();
    (out, t.elapsed().as_secs_f64() * 1000.0)
}

fn mb(bytes: usize) -> f64 {
    bytes as f64 / 1e6
}

/// BM25 as the engine computes it (k1 = 1.2, b = 0.75)
fn bm25(tf: f32, doc_len: f32, avg_len: f32, doc_freq: usize, total_docs: usize) -> f32 {
    let idf = ((total_docs.saturating_sub(doc_freq) as f32 + 0.5) / (doc_freq as f32 + 0.5) + 1.0).ln();
    idf * (tf * 2.2) / (tf + 1.2 * (0.25 + 0.75 * doc_len / avg_len))
}

fn top_k(mut scored: Vec<(DocId, f32)>, k: usize) -> Vec<DocId> {
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
    scored.into_iter().take(k).map(|(id, _)| id).collect()
}

fn main() -> anyhow::Result<()> {
    let data_dir = Engine::default_data_dir();
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(data_dir.join("manifest.json"))?)?;
    let segment_name = manifest["segment"].as_str().expect("an index has been built");
    let segment_path = data_dir.join(segment_name);
    let (parts, current_load_ms) = time_ms(|| Segment::open(&segment_path).map(Segment::into_parts));
    let (term_dict, documents) = parts?;
    let mut rng = StdRng::seed_from_u64(7);

    // ═══ 1. Build ════════════════════════════════════════════════════════════
    println!("## 1. Build\n");
    let postings: usize = term_dict.values().map(Vec::len).sum();
    let term_bytes: usize = term_dict.keys().map(String::len).sum();
    // 16 B posting; per term: String (24) + Vec (24) + hash-map slot (~16) + two heap allocations (~32)
    let current = postings * 16 + term_bytes + term_dict.len() * 96;
    println!("corpus: {} documents, {} distinct terms, {} postings", documents.len(), term_dict.len(), postings);
    println!("current inverted index (estimated): {:.1} MB", mb(current));

    let mut by_path: Vec<_> = documents.values().collect();
    by_path.sort_by(|a, b| a.path.cmp(&b.path));
    let dense: HashMap<DocId, usize> = by_path.iter().enumerate().map(|(i, doc)| (doc.doc_id, i)).collect();
    let mut doc_terms: Vec<Vec<u64>> = vec![Vec::new(); by_path.len()];
    for (term, list) in &term_dict {
        let hash = xxh3_64(term.as_bytes());
        for posting in list {
            if let Some(&index) = dense.get(&posting.doc_id) {
                doc_terms[index].push(hash);
            }
        }
    }
    for terms in &mut doc_terms {
        terms.sort_unstable();
        terms.dedup();
    }
    let (mut index, build_ms) = time_ms(|| FingerprintIndex::build(&doc_terms));
    println!("fingerprint index: {:.1} MB, built in {:.0} ms ({:.1}x smaller)", mb(index.bytes()), build_ms, current as f64 / index.bytes() as f64);

    // Is the folder ordering doing anything? Same structure, documents shuffled.
    let mut shuffled = doc_terms.clone();
    shuffled.shuffle(&mut rng);
    let shuffled_index = FingerprintIndex::build(&shuffled);
    let block_mb = |i: &FingerprintIndex| mb(i.blocks.iter().map(Bloom::bytes).sum());
    println!(
        "control, same documents in random order: {:.1} MB (block summaries {:.1} MB vs {:.1} MB in folder order)\n",
        mb(shuffled_index.bytes()), block_mb(&shuffled_index), block_mb(&index)
    );

    // ═══ 2. Lookups across the whole vocabulary ══════════════════════════════
    println!("## 2. Single-word lookups, 3,000 terms sampled from the real vocabulary\n");
    let mut vocabulary: Vec<(&String, &Vec<Posting>)> = term_dict.iter().collect();
    vocabulary.sort_by(|a, b| a.0.cmp(b.0));
    vocabulary.shuffle(&mut rng);
    let exact_set = |list: &Vec<Posting>| -> HashSet<usize> { list.iter().filter_map(|p| dense.get(&p.doc_id).copied()).collect() };

    println!("{:<22} {:>6} {:>10} {:>9} {:>12} {:>14} {:>10} {:>10}", "term frequency", "terms", "true hits", "missed", "false hits", "blocks opened", "median ms", "p99 ms");
    let bands: [(&str, usize, usize); 4] = [("rare (1-9 files)", 1, 9), ("uncommon (10-99)", 10, 99), ("common (100-999)", 100, 999), ("very common (1000+)", 1000, usize::MAX)];
    let (mut all_missed, mut all_false, mut all_checks) = (0usize, 0usize, 0usize);
    for (label, low, high) in bands {
        let sample: Vec<_> = vocabulary.iter().filter(|(_, l)| (low..=high).contains(&l.len())).take(750).collect();
        let (mut hits, mut missed, mut false_hits, mut opened_total) = (0usize, 0usize, 0usize, 0usize);
        let mut times = Vec::new();
        for (term, list) in &sample {
            let hash = xxh3_64(term.as_bytes());
            let ((found, opened), ms) = time_ms(|| index.lookup(hash));
            times.push(ms);
            let exact = exact_set(list);
            let found: HashSet<usize> = found.into_iter().collect();
            hits += exact.len();
            missed += exact.difference(&found).count();
            false_hits += found.difference(&exact).count();
            opened_total += opened;
            all_checks += opened * BLOCK;
        }
        all_missed += missed;
        all_false += false_hits;
        println!(
            "{:<22} {:>6} {:>10} {:>9} {:>12} {:>9.1}/{:<4} {:>10.3} {:>10.3}",
            label, sample.len(), hits, missed, false_hits,
            opened_total as f64 / sample.len().max(1) as f64, index.blocks.len(),
            median(&mut times.clone()), percentile(&mut times, 99)
        );
    }
    // Words that are in no file at all
    let mut absent_false = 0usize;
    let mut absent_times = Vec::new();
    for i in 0..2000u64 {
        let hash = xxh3_64(format!("absent-term-{i}-{}", rng.gen::<u64>()).as_bytes());
        let ((found, opened), ms) = time_ms(|| index.lookup(hash));
        absent_false += found.len();
        all_checks += opened * BLOCK;
        absent_times.push(ms);
    }
    println!("{:<22} {:>6} {:>10} {:>9} {:>12} {:>14} {:>10.3} {:>10.3}", "absent (in no file)", 2000, 0, "-", absent_false, "-", median(&mut absent_times.clone()), percentile(&mut absent_times, 99));
    println!("\nmissed matches overall: {all_missed}   false matches overall: {}   (about 1 per {} fingerprint checks)\n",
        all_false + absent_false, all_checks / (all_false + absent_false).max(1));

    // ═══ 3. Multi-word queries ═══════════════════════════════════════════════
    println!("## 3. Two-word queries (both words required), 500 random pairs of common words\n");
    let common: Vec<_> = vocabulary.iter().filter(|(_, l)| l.len() >= 200).take(1000).collect();
    let (mut pair_hits, mut pair_missed, mut pair_false) = (0usize, 0usize, 0usize);
    let mut pair_times = Vec::new();
    for pair in common.chunks(2).filter(|c| c.len() == 2).take(500) {
        let hashes = [xxh3_64(pair[0].0.as_bytes()), xxh3_64(pair[1].0.as_bytes())];
        let (found, ms) = time_ms(|| index.lookup_all(&hashes));
        pair_times.push(ms);
        let exact: HashSet<usize> = exact_set(pair[0].1).intersection(&exact_set(pair[1].1)).copied().collect();
        let found: HashSet<usize> = found.into_iter().collect();
        pair_hits += exact.len();
        pair_missed += exact.difference(&found).count();
        pair_false += found.difference(&exact).count();
    }
    println!("true hits {pair_hits}, missed {pair_missed}, false {pair_false}; median {:.3} ms, p99 {:.3} ms\n", median(&mut pair_times.clone()), percentile(&mut pair_times, 99));

    // ═══ 4. What ranking loses without term counts ═══════════════════════════
    println!("## 4. Ranking without term counts (content matches only, 400 terms found in 30-5000 files)\n");
    let total_docs = documents.len();
    let avg_len = documents.values().map(|d| d.doc_len as f32).sum::<f32>() / total_docs as f32;
    let (mut overlap_presence, mut overlap_bucket, mut top1_presence, mut top1_bucket, mut ranked_terms) = (0.0f64, 0.0f64, 0usize, 0usize, 0usize);
    for (_, list) in vocabulary.iter().filter(|(_, l)| (30..=5000).contains(&l.len())).take(400) {
        let content: Vec<&Posting> = list.iter().filter(|p| p.field_mask == FIELD_CONTENT).collect();
        if content.len() < 30 {
            continue;
        }
        let score = |tf_of: &dyn Fn(u32) -> f32| -> Vec<DocId> {
            top_k(
                content
                    .iter()
                    .filter_map(|p| documents.get(&p.doc_id).map(|d| (p.doc_id, bm25(tf_of(p.term_freq), d.doc_len as f32, avg_len, content.len(), total_docs))))
                    .collect(),
                20,
            )
        };
        let full = score(&|tf| tf as f32);
        // Fingerprints as built: a word is present or it is not
        let presence = score(&|_| 1.0);
        // Variant: two spare bits per word keep a rough count (1, 2-3, 4-7, 8+)
        let bucket = score(&|tf| match tf { 1 => 1.0, 2..=3 => 2.5, 4..=7 => 5.5, _ => 12.0 });
        let full_set: HashSet<_> = full.iter().collect();
        overlap_presence += presence.iter().filter(|d| full_set.contains(d)).count() as f64 / 20.0;
        overlap_bucket += bucket.iter().filter(|d| full_set.contains(d)).count() as f64 / 20.0;
        top1_presence += (presence[0] == full[0]) as usize;
        top1_bucket += (bucket[0] == full[0]) as usize;
        ranked_terms += 1;
    }
    let n = ranked_terms.max(1) as f64;
    println!("compared with ranking by exact term counts, over {ranked_terms} terms:");
    println!("  present/absent only : {:.0}% of the top 20 are the same files; same #1 result {:.0}% of the time", overlap_presence / n * 100.0, top1_presence as f64 / n * 100.0);
    println!("  with a 2-bit count  : {:.0}% of the top 20 are the same files; same #1 result {:.0}% of the time\n", overlap_bucket / n * 100.0, top1_bucket as f64 / n * 100.0);

    // ═══ 5. Life after the build: edits, new files, deletes ══════════════════
    println!("## 5. Edits, additions and deletions\n");
    let probe_terms: Vec<u64> = vocabulary.iter().filter(|(_, l)| (10..=99).contains(&l.len())).take(300).map(|(t, _)| xxh3_64(t.as_bytes())).collect();
    let blocks_opened = |i: &FingerprintIndex| probe_terms.iter().map(|&h| i.lookup(h).1).sum::<usize>() as f64 / probe_terms.len() as f64;
    let fill = |i: &FingerprintIndex| i.blocks.iter().map(Bloom::fill).sum::<f64>() / i.blocks.len() as f64;
    println!("before: {:.1} blocks opened per uncommon-word lookup; block summaries {:.0}% full", blocks_opened(&index), fill(&index) * 100.0);

    // Edit files with real content: each time, 30% of its words are replaced by new ones
    let editable: Vec<usize> = (0..doc_terms.len()).filter(|&d| doc_terms[d].len() >= SMALL_DOC).collect();
    let mut truth = doc_terms.clone();
    let mut update_times = Vec::new();
    for round in 1..=3 {
        let edits = editable.len() / 5; // a fifth of all content files per round
        for _ in 0..edits {
            let doc = *editable.choose(&mut rng).unwrap();
            let terms = &mut truth[doc];
            let replace = terms.len() * 3 / 10;
            terms.shuffle(&mut rng);
            terms.truncate(terms.len() - replace);
            terms.extend((0..replace).map(|_| rng.gen::<u64>()));
            terms.sort_unstable();
            terms.dedup();
            let ((), ms) = time_ms(|| index.update(doc, terms));
            update_times.push(ms);
        }
        // Correctness after the edits: every word a file now has must still be found
        let mut missed = 0usize;
        for &doc in editable.iter().step_by(37) {
            for &hash in truth[doc].iter().step_by(11) {
                if !index.blocks[doc / BLOCK].contains(hash) || !index.filters[doc].contains(hash) {
                    missed += 1;
                }
            }
        }
        println!(
            "after round {round} ({edits} edits): missed {missed}; {:.1} blocks opened per lookup; summaries {:.0}% full",
            blocks_opened(&index), fill(&index) * 100.0
        );
    }
    println!("cost of one edit: median {:.3} ms, p99 {:.3} ms (the current engine rescans every posting list per batch of edits)", median(&mut update_times.clone()), percentile(&mut update_times, 99));

    // Rebuilding the summaries needs each file's words again. Fingerprints cannot
    // give them back, so in practice this means re-reading the block's files.
    let (rebuilt, rebuild_ms) = time_ms(|| FingerprintIndex::build(&truth));
    println!("full rebuild from word lists: {:.0} ms -> {:.1} blocks opened per lookup, summaries {:.0}% full", rebuild_ms, blocks_opened(&rebuilt), fill(&rebuilt) * 100.0);
    index = rebuilt;

    // New files arrive at the end, out of folder order (copies of random existing files)
    let before_add = index.bytes();
    let added: Vec<Vec<u64>> = (0..5000).map(|_| truth[*editable.choose(&mut rng).unwrap()].clone()).collect();
    let mut with_added = truth.clone();
    with_added.extend(added);
    let appended = FingerprintIndex::build(&with_added);
    println!(
        "5,000 new files appended out of folder order: +{:.1} MB ({:.0} bytes per file; folder-ordered average is {:.0})",
        mb(appended.bytes() - before_add), (appended.bytes() - before_add) as f64 / 5000.0, before_add as f64 / truth.len() as f64
    );

    // Deletes: drop the fingerprint
    let mut deleted = 0usize;
    for doc in (0..index.filters.len()).step_by(10) {
        deleted += index.filters[doc].bytes();
        index.filters[doc] = DocFilter::Dead;
    }
    println!("deleting every 10th file frees {:.1} MB immediately; none of them can match again\n", mb(deleted));
    let index = FingerprintIndex::build(&truth);

    // ═══ 6. Saving and loading ═══════════════════════════════════════════════
    println!("## 6. Saving and loading\n");
    let path = std::env::temp_dir().join(format!("ls-fingerprint-{}.bin", std::process::id()));
    let (bytes, serialize_ms) = time_ms(|| bincode::serialize(&index).unwrap());
    std::fs::write(&path, &bytes)?;
    let (loaded, load_ms) = time_ms(|| bincode::deserialize::<FingerprintIndex>(&std::fs::read(&path).unwrap()).unwrap());
    std::fs::remove_file(&path)?;
    let same = probe_terms.iter().all(|&h| loaded.lookup(h).0 == index.lookup(h).0);
    println!("file on disk: {:.1} MB uncompressed (current segment: {:.1} MB compressed)", mb(bytes.len()), mb(std::fs::metadata(&segment_path)?.len() as usize));
    println!("write: {:.0} ms   read back: {:.0} ms   identical answers after reload: {same}", serialize_ms, load_ms);
    println!("current engine: {:.0} ms just to read and decode its segment, before rebuilding its lookup structures\n", current_load_ms);

    // ═══ 7. Ten times the files ══════════════════════════════════════════════
    println!("## 7. At 10x the corpus ({} files), by scanning the structure ten times per lookup\n", documents.len() * 10);
    let mut scaled = Vec::new();
    for (term, _) in vocabulary.iter().filter(|(_, l)| l.len() >= 1000).take(200) {
        let hash = xxh3_64(term.as_bytes());
        let (_, ms) = time_ms(|| (0..10).map(|_| index.lookup(hash).0.len()).sum::<usize>());
        scaled.push(ms);
    }
    println!("very common word: median {:.2} ms, p99 {:.2} ms", median(&mut scaled.clone()), percentile(&mut scaled, 99));
    println!("index size: {:.0} MB (current design, same estimate: {:.0} MB)", mb(index.bytes()) * 10.0, mb(current) * 10.0);
    Ok(())
}
