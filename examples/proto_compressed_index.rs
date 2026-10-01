//! End-to-end simulation of a compressed, memory-mapped inverted index as a
//! replacement for the in-memory one, measured against the real saved index
//! (read-only; nothing in the data directory is modified).
//!
//!     cargo run --release --example proto_compressed_index
//!
//! Format under test (the Lucene/Tantivy recipe, sized for a desktop corpus):
//!   * documents numbered in path order;
//!   * term dictionary: a finite-state transducer (`fst`) mapping term -> u64;
//!   * a term found in exactly one place is stored inside that u64, no list;
//!   * other terms point into one postings file: a count, then per posting a
//!     variable-length integer holding (doc-id gap, 2-bit field, "tf is 1"
//!     flag), followed by the term frequency only when it is above 1;
//!   * both files are memory-mapped; nothing is decoded until a query needs it.
//!
//! The same data is also indexed with Tantivy for comparison.

use fst::{Map, MapBuilder};
use localsearch::engine::Engine;
use localsearch::index::delta::{DocId, FIELD_CONTENT, FIELD_FILENAME, FIELD_PATH};
use localsearch::index::segment::Segment;
use memmap2::Mmap;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// One occurrence list entry: (document, field code, term frequency)
type Entry = (u32, u8, u32);

// ─── Encoding ─────────────────────────────────────────────────────────────────

fn put_vint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn get_vint(bytes: &[u8], pos: &mut usize) -> u64 {
    let (mut value, mut shift) = (0u64, 0);
    loop {
        let byte = bytes[*pos];
        *pos += 1;
        value |= ((byte & 0x7F) as u64) << shift;
        if byte < 0x80 {
            return value;
        }
        shift += 7;
    }
}

/// content = 0, filename = 1, path = 2, filename + path = 3
fn field_code(mask: u8) -> u8 {
    if mask & FIELD_CONTENT != 0 {
        0
    } else {
        ((mask & FIELD_FILENAME != 0) as u8) | (((mask & FIELD_PATH != 0) as u8) << 1)
    }
}

fn field_boost(code: u8) -> f32 {
    match code {
        0 => 1.0,
        2 => 1.5,
        _ => 3.0,
    }
}

const INLINE: u64 = 1;

/// Build the dictionary and postings bytes from per-term entry lists (sorted by term).
fn build(terms: &[(&str, Vec<Entry>)]) -> (Vec<u8>, Vec<u8>) {
    let mut postings = Vec::new();
    let mut dictionary = MapBuilder::memory();
    for (term, entries) in terms {
        let value = if let [(doc, field, tf)] = entries[..] {
            // A single occurrence lives in the dictionary value itself
            INLINE | ((doc as u64) << 1) | ((field as u64) << 25) | ((tf as u64) << 27)
        } else {
            let offset = postings.len() as u64;
            put_vint(&mut postings, entries.len() as u64);
            let mut previous = 0u32;
            for &(doc, field, tf) in entries {
                let head = (((doc - previous) as u64) << 3) | ((field as u64) << 1) | (tf == 1) as u64;
                put_vint(&mut postings, head);
                if tf != 1 {
                    put_vint(&mut postings, (tf - 2) as u64);
                }
                previous = doc;
            }
            offset << 1
        };
        dictionary.insert(term, value).expect("terms are sorted and unique");
    }
    (dictionary.into_inner().expect("fst builds"), postings)
}

/// Decode one term's entries straight from the mapped bytes.
fn lookup<D: AsRef<[u8]>>(dictionary: &Map<D>, postings: &[u8], term: &str, out: &mut Vec<Entry>) {
    out.clear();
    let Some(value) = dictionary.get(term) else { return };
    if value & INLINE != 0 {
        out.push((((value >> 1) & 0xFF_FFFF) as u32, ((value >> 25) & 3) as u8, (value >> 27) as u32));
        return;
    }
    let mut pos = (value >> 1) as usize;
    let count = get_vint(postings, &mut pos);
    let mut doc = 0u32;
    for _ in 0..count {
        let head = get_vint(postings, &mut pos);
        doc += (head >> 3) as u32;
        let tf = if head & 1 == 1 { 1 } else { get_vint(postings, &mut pos) as u32 + 2 };
        out.push((doc, ((head >> 1) & 3) as u8, tf));
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn time_ms<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let out = f();
    (out, t.elapsed().as_secs_f64() * 1000.0)
}

fn median(samples: &[f64]) -> f64 {
    let mut s = samples.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[s.len() / 2]
}

fn p99(samples: &[f64]) -> f64 {
    let mut s = samples.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[(s.len() * 99 / 100).min(s.len() - 1)]
}

fn mb(bytes: usize) -> f64 {
    bytes as f64 / 1e6
}

fn rss_mb() -> f64 {
    Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

fn bm25(tf: f32, doc_len: f32, avg_len: f32, doc_freq: usize, total_docs: usize) -> f32 {
    let idf = ((total_docs.saturating_sub(doc_freq) as f32 + 0.5) / (doc_freq as f32 + 0.5) + 1.0).ln();
    idf * (tf * 2.2) / (tf + 1.2 * (0.25 + 0.75 * doc_len / avg_len))
}

fn top20(entries: &[Entry], doc_len: &[u32], avg_len: f32) -> Vec<(u32, f32)> {
    let mut scored: Vec<(u32, f32)> = entries
        .iter()
        .map(|&(doc, field, tf)| (doc, bm25(tf as f32, doc_len[doc as usize] as f32, avg_len, entries.len(), doc_len.len()) * field_boost(field)))
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
    scored.truncate(20);
    scored
}

fn dir_sizes(dir: &Path) -> (usize, Vec<(String, usize)>) {
    let mut by_ext: HashMap<String, usize> = HashMap::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let ext = entry.path().extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        *by_ext.entry(ext).or_default() += entry.metadata().map_or(0, |m| m.len() as usize);
    }
    let mut list: Vec<_> = by_ext.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1));
    (list.iter().map(|(_, size)| size).sum(), list)
}

// ─── Child-process modes: memory measured in a process holding nothing else ───

/// `--mapped <dir>`: map the two files, run the sample lookups, report memory.
fn mapped_child(dir: &Path) -> anyhow::Result<()> {
    let rss_start = rss_mb();
    let t = Instant::now();
    let dictionary = Map::new(unsafe { Mmap::map(&File::open(dir.join("terms.fst"))?)? })?;
    let postings = unsafe { Mmap::map(&File::open(dir.join("postings.bin"))?)? };
    let doc_len: Vec<u32> = std::fs::read(dir.join("doclen.bin"))?
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;
    let rss_open = rss_mb();

    let avg_len = doc_len.iter().sum::<u32>() as f32 / doc_len.len() as f32;
    let sample = std::fs::read_to_string(dir.join("sample.txt"))?;
    let mut entries = Vec::new();
    let mut times = Vec::new();
    let mut checksum = 0usize;
    for term in sample.lines() {
        let t = Instant::now();
        lookup(&dictionary, &postings, term, &mut entries);
        checksum += top20(&entries, &doc_len, avg_len).len();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "{open_ms:.2} {rss_start:.1} {rss_open:.1} {:.1} {:.4} {:.4} {} {checksum}",
        rss_mb(), median(&times), p99(&times), times.len()
    );
    Ok(())
}

/// `--tantivy <dir>`: open the Tantivy index, run the sample term queries, report memory.
fn tantivy_child(dir: &Path) -> anyhow::Result<()> {
    use tantivy::collector::TopDocs;
    use tantivy::query::TermQuery;
    use tantivy::schema::IndexRecordOption;
    use tantivy::{Index, Term};

    let rss_start = rss_mb();
    let t = Instant::now();
    let index = Index::open_in_dir(dir.join("tantivy"))?;
    register_tokenizer(&index);
    let body = index.schema().get_field("body")?;
    let reader = index.reader()?;
    let searcher = reader.searcher();
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;
    let rss_open = rss_mb();

    let sample = std::fs::read_to_string(dir.join("sample.txt"))?;
    let mut times = Vec::new();
    let mut checksum = 0usize;
    for term in sample.lines() {
        let query = TermQuery::new(Term::from_field_text(body, term), IndexRecordOption::WithFreqs);
        let t = Instant::now();
        checksum += searcher.search(&query, &TopDocs::with_limit(20))?.len();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "{open_ms:.2} {rss_start:.1} {rss_open:.1} {:.1} {:.4} {:.4} {} {checksum}",
        rss_mb(), median(&times), p99(&times), times.len()
    );
    Ok(())
}

/// `--names <dir>`: build the structures the engine keeps for names today and
/// report process memory after each, in a process holding nothing else.
fn names_child(dir: &Path) -> anyhow::Result<()> {
    use localsearch::index::bktree::BkTree;
    use localsearch::index::trie::PathTrie;
    use localsearch::index::trigram::TrigramIndex;
    use localsearch::query::parser::Tokenizer;
    use localsearch::query::phonetic::double_metaphone;

    let paths: Vec<String> = std::fs::read_to_string(dir.join("paths.txt"))?.lines().map(String::from).collect();
    let home = std::env::var("HOME").unwrap_or_default();
    let tokenizer = Tokenizer::new();
    let mut readings = vec![rss_mb()];

    // Documents table and path -> id map (two copies of every path, as today)
    let documents: HashMap<u64, String> = paths.iter().enumerate().map(|(i, p)| (i as u64, p.clone())).collect();
    let path_ids: HashMap<String, u64> = paths.iter().enumerate().map(|(i, p)| (p.clone(), i as u64)).collect();
    readings.push(rss_mb());

    let mut trie = PathTrie::new();
    for (i, path) in paths.iter().enumerate() {
        trie.insert(path, DocId(i as u64));
    }
    trie.rebuild_prefix_cache(20);
    readings.push(rss_mb());

    let mut trigrams = TrigramIndex::new();
    for (i, path) in paths.iter().enumerate() {
        trigrams.insert(path.rsplit('/').next().unwrap_or(path), DocId(i as u64));
    }
    readings.push(rss_mb());

    // Typo and sound-alike structures over the words of names and folders below the home directory
    let mut bk_tree = BkTree::new();
    let mut phonetic: HashMap<String, Vec<String>> = HashMap::new();
    let mut name_terms = 0usize;
    for path in &paths {
        for token in tokenizer.tokenize(path.strip_prefix(&home).unwrap_or(path)) {
            bk_tree.insert(&token.term);
            let entry = phonetic.entry(double_metaphone(&token.term).primary).or_default();
            if !entry.contains(&token.term) {
                entry.push(token.term);
                name_terms += 1;
            }
        }
    }
    readings.push(rss_mb());
    std::hint::black_box((&documents, &path_ids, &trie, &trigrams, &bk_tree, &phonetic));
    println!("{} {name_terms}", readings.iter().map(|r| format!("{r:.1}")).collect::<Vec<_>>().join(" "));
    Ok(())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut previous = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let current = row[j];
            row[j] = (previous + (a[i - 1] != b[j - 1]) as usize).min(row[j] + 1).min(row[j - 1] + 1);
            previous = current;
        }
    }
    row[b.len()]
}

fn register_tokenizer(index: &tantivy::Index) {
    use tantivy::tokenizer::{TextAnalyzer, WhitespaceTokenizer};
    index.tokenizers().register("ws", TextAnalyzer::builder(WhitespaceTokenizer::default()).build());
}

fn run_child(mode: &str, dir: &Path) -> Vec<f64> {
    let exe = std::env::current_exe().expect("own path");
    let out = Command::new(exe).arg(mode).arg(dir).output().expect("child runs");
    String::from_utf8_lossy(&out.stdout).split_whitespace().filter_map(|v| v.parse().ok()).collect()
}

// ─── Main ─────────────────────────────────────────────────────────────────────

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--mapped" {
        return mapped_child(Path::new(&args[2]));
    }
    if args.len() == 3 && args[1] == "--tantivy" {
        return tantivy_child(Path::new(&args[2]));
    }
    if args.len() == 3 && args[1] == "--names" {
        return names_child(Path::new(&args[2]));
    }

    let data_dir = Engine::default_data_dir();
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(data_dir.join("manifest.json"))?)?;
    let segment_path = data_dir.join(manifest["segment"].as_str().expect("an index has been built"));
    let (term_dict, documents) = Segment::open(&segment_path)?.into_parts();
    let mut rng = StdRng::seed_from_u64(7);
    let work: PathBuf = std::env::temp_dir().join(format!("ls-compressed-{}", std::process::id()));
    std::fs::create_dir_all(&work)?;

    let postings_total: usize = term_dict.values().map(Vec::len).sum();
    let current = postings_total * 16 + term_dict.keys().map(String::len).sum::<usize>() + term_dict.len() * 96;
    println!("## 1. Build\n");
    println!("corpus: {} documents, {} distinct terms, {} postings", documents.len(), term_dict.len(), postings_total);
    println!("current in-memory inverted index (estimated): {:.1} MB", mb(current));

    // Document numbering: path order, and a shuffled control
    let mut by_path: Vec<_> = documents.values().collect();
    by_path.sort_by(|a, b| a.path.cmp(&b.path));
    let path_order: HashMap<DocId, u32> = by_path.iter().enumerate().map(|(i, d)| (d.doc_id, i as u32)).collect();
    let doc_len: Vec<u32> = by_path.iter().map(|d| d.doc_len.max(1)).collect();
    let mut shuffled_ids: Vec<u32> = (0..by_path.len() as u32).collect();
    shuffled_ids.shuffle(&mut rng);

    let mut sorted_terms: Vec<&String> = term_dict.keys().collect();
    sorted_terms.sort();
    let entries_for = |renumber: &dyn Fn(u32) -> u32| -> Vec<(&str, Vec<Entry>)> {
        sorted_terms
            .iter()
            .map(|term| {
                let mut entries: Vec<Entry> = term_dict[*term]
                    .iter()
                    .filter_map(|p| path_order.get(&p.doc_id).map(|&d| (renumber(d), field_code(p.field_mask), p.term_freq.max(1))))
                    .collect();
                entries.sort_unstable();
                (term.as_str(), entries)
            })
            .collect()
    };

    let source = entries_for(&|d| d);
    let ((dictionary_bytes, postings_bytes), build_ms) = time_ms(|| build(&source));
    let singletons = source.iter().filter(|(_, e)| e.len() == 1).count();
    let total = dictionary_bytes.len() + postings_bytes.len() + doc_len.len() * 4;
    println!("\ncompressed index, documents in folder order (built in {:.0} ms):", build_ms);
    println!("  term dictionary (fst):  {:>6.2} MB  ({:.1} bytes per term)", mb(dictionary_bytes.len()), dictionary_bytes.len() as f64 / source.len() as f64);
    println!("  postings:               {:>6.2} MB  ({:.2} bytes per posting; {} terms stored inline)", mb(postings_bytes.len()), postings_bytes.len() as f64 / (postings_total - singletons) as f64, singletons);
    println!("  document lengths:       {:>6.2} MB", mb(doc_len.len() * 4));
    println!("  total:                  {:>6.2} MB  ({:.1}x smaller than current; fingerprint prototype was 29.6 MB)", mb(total), current as f64 / total as f64);

    let shuffled = entries_for(&|d| shuffled_ids[d as usize]);
    let (_, shuffled_postings) = build(&shuffled);
    println!(
        "\ncontrol, same documents numbered at random: postings {:.2} MB -> folder order saves {:.0}% of postings",
        mb(shuffled_postings.len()),
        (1.0 - postings_bytes.len() as f64 / shuffled_postings.len() as f64) * 100.0
    );
    drop(shuffled);

    // ═══ 2. Exactness ════════════════════════════════════════════════════════
    println!("\n## 2. Is it exact?\n");
    let dictionary = Map::new(dictionary_bytes.clone())?;
    let mut decoded = Vec::new();
    let (mut wrong_terms, mut checked) = (0usize, 0usize);
    let (_, verify_ms) = time_ms(|| {
        for (term, entries) in &source {
            lookup(&dictionary, &postings_bytes, term, &mut decoded);
            wrong_terms += (decoded != *entries) as usize;
            checked += entries.len();
        }
    });
    let mut absent = 0usize;
    for i in 0..2000u64 {
        lookup(&dictionary, &postings_bytes, &format!("absent-term-{i}-{}", rng.gen::<u64>()), &mut decoded);
        absent += decoded.len();
    }
    println!("every term decoded and compared with the source: {} terms, {} postings, {} terms differ", source.len(), checked, wrong_terms);
    println!("2,000 words that are in no file returned {absent} matches");
    println!("(decoding the entire index took {:.0} ms)", verify_ms);

    // ═══ 3. Speed, including exact BM25 top-20 ═══════════════════════════════
    println!("\n## 3. Lookup and ranking speed (in this process)\n");
    let avg_len = doc_len.iter().sum::<u32>() as f32 / doc_len.len() as f32;
    let mut order: Vec<usize> = (0..source.len()).collect();
    order.shuffle(&mut rng);
    println!("{:<22} {:>6} {:>14} {:>12} {:>12}", "term frequency", "terms", "decode ms", "+ top-20 ms", "p99 ms");
    let mut sample_terms: Vec<&str> = Vec::new();
    for (label, low, high) in [("rare (1-9 files)", 1, 9), ("uncommon (10-99)", 10, 99), ("common (100-999)", 100, 999), ("very common (1000+)", 1000, usize::MAX)] {
        let band: Vec<&str> = order.iter().map(|&i| &source[i]).filter(|(_, e)| (low..=high).contains(&e.len())).take(750).map(|(t, _)| *t).collect();
        let (mut decode, mut ranked) = (Vec::new(), Vec::new());
        for term in &band {
            let (_, d) = time_ms(|| lookup(&dictionary, &postings_bytes, term, &mut decoded));
            let (_, r) = time_ms(|| std::hint::black_box(top20(&decoded, &doc_len, avg_len)));
            decode.push(d);
            ranked.push(d + r);
        }
        println!("{:<22} {:>6} {:>14.4} {:>12.4} {:>12.4}", label, band.len(), median(&decode), median(&ranked), p99(&ranked));
        sample_terms.extend(band);
    }

    // ═══ 4. Memory when mapped from disk, in a clean process ═════════════════
    println!("\n## 4. Memory when the index is mapped from disk (separate process holding nothing else)\n");
    std::fs::write(work.join("terms.fst"), &dictionary_bytes)?;
    std::fs::write(work.join("postings.bin"), &postings_bytes)?;
    std::fs::write(work.join("doclen.bin"), doc_len.iter().flat_map(|l| l.to_le_bytes()).collect::<Vec<u8>>())?;
    std::fs::write(work.join("sample.txt"), sample_terms.join("\n"))?;
    let m = run_child("--mapped", &work);
    if m.len() >= 7 {
        println!("open: {:.2} ms (current engine: about 1,400 ms to load)", m[0]);
        println!("process memory: {:.1} MB before opening, {:.1} MB after opening, {:.1} MB after {} ranked lookups", m[1], m[2], m[3], m[6] as usize);
        println!("lookup + exact top-20 from the mapped files: median {:.4} ms, p99 {:.4} ms", m[4], m[5]);
    }

    // ═══ 5. Updates: tombstones + a small in-memory delta ════════════════════
    println!("\n## 5. Edits, without touching the files on disk\n");
    // Per-document term lists (what an edit replaces)
    let mut doc_terms: Vec<Vec<(u32, u8, u32)>> = vec![Vec::new(); doc_len.len()];
    for (term_id, (_, entries)) in source.iter().enumerate() {
        for &(doc, field, tf) in entries {
            doc_terms[doc as usize].push((term_id as u32, field, tf));
        }
    }
    let editable: Vec<u32> = (0..doc_len.len() as u32).filter(|&d| doc_terms[d as usize].len() >= 24).collect();
    let probes: Vec<usize> = order.iter().copied().filter(|&i| (10..=999).contains(&source[i].1.len())).take(300).collect();

    let mut tombstone = vec![false; doc_len.len()];
    let mut delta: HashMap<u32, Vec<Entry>> = HashMap::new(); // term id -> entries of edited documents
    let mut delta_postings = 0usize;
    let query = |term_id: usize, tombstone: &[bool], delta: &HashMap<u32, Vec<Entry>>, out: &mut Vec<Entry>, scratch: &mut Vec<Entry>| {
        lookup(&dictionary, &postings_bytes, source[term_id].0, scratch);
        out.clear();
        out.extend(scratch.iter().filter(|e| !tombstone[e.0 as usize]));
        if let Some(extra) = delta.get(&(term_id as u32)) {
            out.extend_from_slice(extra);
        }
        out.sort_unstable();
    };

    let (mut out, mut scratch) = (Vec::new(), Vec::new());
    let baseline: Vec<f64> = probes.iter().map(|&t| time_ms(|| query(t, &tombstone, &delta, &mut out, &mut scratch)).1).collect();
    println!("before any edit: median lookup {:.4} ms", median(&baseline));

    let mut edit_times = Vec::new();
    for round in 1..=3 {
        let edits = editable.len() / 5;
        for _ in 0..edits {
            let doc = *editable.choose(&mut rng).unwrap();
            let (_, ms) = time_ms(|| {
                // Delete: one bit. (If already edited, drop its delta entries first.)
                if tombstone[doc as usize] {
                    for &(term_id, _, _) in &doc_terms[doc as usize] {
                        if let Some(list) = delta.get_mut(&term_id) {
                            let before = list.len();
                            list.retain(|e| e.0 != doc);
                            delta_postings -= before - list.len();
                        }
                    }
                }
                tombstone[doc as usize] = true;
                // New version: 30% of its words change
                let terms = &mut doc_terms[doc as usize];
                let replace = terms.len() * 3 / 10;
                terms.shuffle(&mut rng);
                terms.truncate(terms.len() - replace);
                for _ in 0..replace {
                    terms.push((rng.gen_range(0..source.len() as u32), 0, 1));
                }
                terms.sort_unstable();
                terms.dedup_by_key(|t| (t.0, t.1));
                for &(term_id, field, tf) in terms.iter() {
                    delta.entry(term_id).or_default().push((doc, field, tf));
                    delta_postings += 1;
                }
            });
            edit_times.push(ms);
        }
        // Check against the truth, found the slow way: scan every document's current word list
        let mut wrong = 0usize;
        let mut times = Vec::new();
        for &term_id in &probes {
            let mut truth: Vec<Entry> = Vec::new();
            for (doc, terms) in doc_terms.iter().enumerate() {
                for &(t, field, tf) in terms {
                    if t as usize == term_id {
                        truth.push((doc as u32, field, tf));
                    }
                }
            }
            truth.sort_unstable();
            let (_, ms) = time_ms(|| query(term_id, &tombstone, &delta, &mut out, &mut scratch));
            times.push(ms);
            wrong += (out != truth) as usize;
        }
        println!(
            "after round {round} ({edits} files edited, {:.0}% of the base now superseded): {wrong} of {} lookups wrong; median lookup {:.4} ms; delta holds {} postings (~{:.1} MB in memory)",
            tombstone.iter().filter(|&&t| t).count() as f64 / doc_len.len() as f64 * 100.0,
            probes.len(), median(&times), delta_postings, mb(delta_postings * 12 + delta.len() * 56)
        );
    }
    println!("cost of one edit: median {:.4} ms, p99 {:.4} ms", median(&edit_times), p99(&edit_times));

    // Merge: write a fresh base from the current word lists, clear tombstones and delta
    let ((merged_dictionary, merged_postings), merge_ms) = time_ms(|| {
        let mut lists: Vec<Vec<Entry>> = vec![Vec::new(); source.len()];
        for (doc, terms) in doc_terms.iter().enumerate() {
            for &(term_id, field, tf) in terms {
                lists[term_id as usize].push((doc as u32, field, tf));
            }
        }
        let merged: Vec<(&str, Vec<Entry>)> = source.iter().zip(lists).filter(|(_, l)| !l.is_empty()).map(|((t, _), l)| (*t, l)).collect();
        build(&merged)
    });
    println!("merge (rewrite the whole base): {:.0} ms -> {:.1} MB, delta and tombstones emptied", merge_ms, mb(merged_dictionary.len() + merged_postings.len()));

    // ═══ 6. The same data in Tantivy ═════════════════════════════════════════
    println!("\n## 6. The same data indexed with Tantivy 0.22\n");
    {
        use tantivy::schema::{IndexRecordOption, Schema, TextFieldIndexing, TextOptions};
        use tantivy::{doc, Index, IndexWriter};

        let dir = work.join("tantivy");
        std::fs::create_dir_all(&dir)?;
        let mut schema = Schema::builder();
        let body = schema.add_text_field(
            "body",
            TextOptions::default().set_indexing_options(TextFieldIndexing::default().set_tokenizer("ws").set_index_option(IndexRecordOption::WithFreqs)),
        );
        let index = Index::create_in_dir(&dir, schema.build())?;
        register_tokenizer(&index);
        let mut writer: IndexWriter = index.writer(150_000_000)?;

        // Rebuild each document as text: every term repeated as often as it occurs.
        // (Fields are folded into one, and frequencies capped at 64 to bound the text size.)
        let mut per_doc: Vec<Vec<(u32, u32)>> = vec![Vec::new(); doc_len.len()];
        for (term_id, (_, entries)) in source.iter().enumerate() {
            for &(doc, _, tf) in entries {
                per_doc[doc as usize].push((term_id as u32, tf.min(64)));
            }
        }
        let (_, index_ms) = time_ms(|| -> anyhow::Result<()> {
            let mut text = String::new();
            for terms in &per_doc {
                text.clear();
                for &(term_id, tf) in terms {
                    for _ in 0..tf {
                        text.push_str(source[term_id as usize].0);
                        text.push(' ');
                    }
                }
                writer.add_document(doc!(body => text.as_str()))?;
            }
            writer.commit()?;
            let segments = index.searchable_segment_ids()?;
            if segments.len() > 1 {
                writer.merge(&segments).wait()?;
            }
            writer.garbage_collect_files().wait()?;
            Ok(())
        });
        writer.wait_merging_threads()?;
        let segment_count = index.searchable_segment_ids()?.len();
        let files = std::fs::read_dir(&dir)?.count();
        println!("segments after merge: {segment_count}; files in the index directory: {files}");
        let live: usize = index
            .searchable_segment_metas()?
            .iter()
            .flat_map(|meta| meta.list_files())
            .filter_map(|file| std::fs::metadata(dir.join(file)).ok())
            .map(|m| m.len() as usize)
            .sum();
        println!("size of the files the live segment actually uses: {:.2} MB", mb(live));
        let (total, by_ext) = dir_sizes(&dir);
        println!("indexed in {:.1} s", index_ms / 1000.0);
        // The directory also holds files from before merging; they are not part of the index
        println!("whole directory, including stale pre-merge files: {:.2} MB", mb(total));
        for (ext, size) in by_ext.iter().take(6) {
            println!("  .{:<10} {:>7.2} MB", ext, mb(*size));
        }
        let t = run_child("--tantivy", &work);
        if t.len() >= 7 {
            println!("open: {:.2} ms; process memory {:.1} MB before, {:.1} MB after opening, {:.1} MB after {} top-20 queries", t[0], t[1], t[2], t[3], t[6] as usize);
            println!("term query with BM25 top-20: median {:.4} ms, p99 {:.4} ms", t[4], t[5]);
        }
    }

    // ═══ 7. Multi-word queries ═══════════════════════════════════════════════
    println!("\n## 7. Multi-word queries on the compressed lists\n");
    {
        let common: Vec<usize> = order.iter().copied().filter(|&i| source[i].1.len() >= 200).take(1500).collect();
        let score_of = |list: &[Entry]| -> HashMap<u32, f32> {
            let mut scores: HashMap<u32, f32> = HashMap::new();
            for &(doc, field, tf) in list {
                *scores.entry(doc).or_default() += bm25(tf as f32, doc_len[doc as usize] as f32, avg_len, list.len(), doc_len.len()) * field_boost(field);
            }
            scores
        };
        let rank = |scores: HashMap<u32, f32>| -> Vec<u32> {
            let mut v: Vec<(u32, f32)> = scores.into_iter().collect();
            v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
            v.into_iter().take(20).map(|(d, _)| d).collect()
        };
        for (label, words) in [("two words", 2usize), ("three words", 3)] {
            let (mut all_times, mut any_times) = (Vec::new(), Vec::new());
            let (mut wrong_all, mut wrong_any, mut queries) = (0usize, 0usize, 0usize);
            for group in common.chunks(words).filter(|g| g.len() == words).take(400) {
                // From the compressed index
                let ((top_all, top_any), ms_both) = time_ms(|| {
                    let lists: Vec<Vec<Entry>> = group
                        .iter()
                        .map(|&t| {
                            let mut list = Vec::new();
                            lookup(&dictionary, &postings_bytes, source[t].0, &mut list);
                            list
                        })
                        .collect();
                    let mut any: HashMap<u32, f32> = HashMap::new();
                    let mut hits: HashMap<u32, usize> = HashMap::new();
                    for list in &lists {
                        let mut seen_in_list: Vec<u32> = Vec::new();
                        for (doc, score) in score_of(list) {
                            *any.entry(doc).or_default() += score;
                            seen_in_list.push(doc);
                        }
                        for doc in seen_in_list {
                            *hits.entry(doc).or_default() += 1;
                        }
                    }
                    let all: HashMap<u32, f32> = any.iter().filter(|(d, _)| hits[*d] == words).map(|(d, s)| (*d, *s)).collect();
                    (rank(all), rank(any))
                });
                all_times.push(ms_both);
                any_times.push(ms_both);
                // The same from the uncompressed source lists
                let mut any: HashMap<u32, f32> = HashMap::new();
                let mut hits: HashMap<u32, usize> = HashMap::new();
                for &t in group {
                    for (doc, score) in score_of(&source[t].1) {
                        *any.entry(doc).or_default() += score;
                        *hits.entry(doc).or_default() += 1;
                    }
                }
                let all: HashMap<u32, f32> = any.iter().filter(|(d, _)| hits[*d] == words).map(|(d, s)| (*d, *s)).collect();
                wrong_all += (rank(all) != top_all) as usize;
                wrong_any += (rank(any) != top_any) as usize;
                queries += 1;
            }
            println!(
                "{label}: {queries} queries of common words; top-20 differs from exact in {wrong_all} (all words required) and {wrong_any} (any word); median {:.3} ms, p99 {:.3} ms for both rankings together",
                median(&all_times), p99(&any_times)
            );
        }
    }

    // ═══ 8. Typos and prefixes straight from the dictionary ══════════════════
    println!("\n## 8. Typo and prefix matching using the dictionary itself\n");
    {
        use fst::automaton::{Automaton, Levenshtein, Str};
        use fst::{IntoStreamer, Streamer};

        for (typed, intended) in [("reprot", "report"), ("screnshot", "screenshot"), ("licnese", "licens"), ("functoin", "function"), ("confg", "config")] {
            for distance in [1u32, 2] {
                let automaton = Levenshtein::new(typed, distance)?;
                let (found, ms) = time_ms(|| {
                    let mut stream = dictionary.search(&automaton).into_stream();
                    let mut found = Vec::new();
                    while let Some((term, _)) = stream.next() {
                        found.push(String::from_utf8_lossy(term).into_owned());
                    }
                    found
                });
                // Brute force over every term, to check the automaton misses nothing
                let (brute, brute_ms) = time_ms(|| source.iter().filter(|(t, _)| levenshtein(typed, t) <= distance as usize).count());
                println!(
                    "{typed:<10} within {distance} edit(s): {:>4} words in {:>7.3} ms (brute force: {brute} words, {:.0} ms); finds '{intended}': {}",
                    found.len(), ms, brute_ms, found.iter().any(|t| t == intended)
                );
            }
        }
        let mut prefix_times = Vec::new();
        let mut prefix_counts = Vec::new();
        for prefix in ["re", "doc", "conf", "scr", "inv", "a", "th"] {
            let ((), ms) = time_ms(|| {
                let mut stream = dictionary.search(Str::new(prefix).starts_with()).into_stream();
                let mut count = 0usize;
                while stream.next().is_some() {
                    count += 1;
                }
                prefix_counts.push(count);
            });
            prefix_times.push(ms);
        }
        println!("prefix search over the whole vocabulary: median {:.3} ms ({:?} words for re/doc/conf/scr/inv/a/th)", median(&prefix_times), prefix_counts);
        println!("note: this counts a swapped pair of letters as two edits; the engine today counts it as one");
    }

    // ═══ 9. What stays in memory today for names ═════════════════════════════
    println!("\n## 9. Memory of the name structures the engine keeps today (separate process)\n");
    std::fs::write(work.join("paths.txt"), by_path.iter().map(|d| d.path.as_str()).collect::<Vec<_>>().join("\n"))?;
    let n = run_child("--names", &work);
    if n.len() >= 6 {
        println!("paths, stored twice (document table + path lookup): {:>6.1} MB", n[1] - n[0]);
        println!("path trie with prefix cache:                        {:>6.1} MB", n[2] - n[1]);
        println!("trigram index over file names:                      {:>6.1} MB", n[3] - n[2]);
        println!("typo tree + sound-alike index ({} name words):   {:>6.1} MB", n[5] as usize, n[4] - n[3]);
        println!("total:                                              {:>6.1} MB", n[4] - n[0]);
    }

    // ═══ 10. A realistic stream of edits with a merge threshold ══════════════
    println!("\n## 10. A working day of edits, merging whenever the side index passes 4 MB\n");
    {
        // Start again from the saved state
        let mut doc_terms: Vec<Vec<(u32, u8, u32)>> = vec![Vec::new(); doc_len.len()];
        for (term_id, (_, entries)) in source.iter().enumerate() {
            for &(doc, field, tf) in entries {
                doc_terms[doc as usize].push((term_id as u32, field, tf));
            }
        }
        let editable: Vec<u32> = (0..doc_len.len() as u32).filter(|&d| doc_terms[d as usize].len() >= 24).collect();
        let mut tombstone = vec![false; doc_len.len()];
        let mut delta: HashMap<u32, Vec<Entry>> = HashMap::new();
        let delta_bytes = |delta: &HashMap<u32, Vec<Entry>>| delta.values().map(|l| l.capacity() * 12).sum::<usize>() + delta.capacity() * 40;
        const THRESHOLD: usize = 4_000_000;
        let (mut merges, mut merge_ms_total, mut peak, mut files_edited) = (0usize, 0.0f64, 0usize, 0usize);
        let mut base_dictionary = dictionary_bytes.clone();
        let mut base_postings = postings_bytes.clone();

        // 400 single saves (one file, 5% of its words change to words from a neighbouring
        // file), plus three bursts of 2,000 files, as after switching a git branch
        let mut events: Vec<usize> = vec![1; 400];
        for at in [100, 220, 350] {
            events.insert(at, 2000);
        }
        for burst in events {
            let start = if burst == 1 { *editable.choose(&mut rng).unwrap() as usize } else { rng.gen_range(0..doc_len.len().saturating_sub(burst)) };
            for doc in start..(start + burst).min(doc_len.len()) {
                if doc_terms[doc].len() < 24 {
                    continue;
                }
                files_edited += 1;
                if tombstone[doc] {
                    for &(term_id, _, _) in &doc_terms[doc] {
                        if let Some(list) = delta.get_mut(&term_id) {
                            list.retain(|e| e.0 as usize != doc);
                        }
                    }
                }
                tombstone[doc] = true;
                let neighbour: Vec<(u32, u8, u32)> = doc_terms[(doc + 1) % doc_terms.len()].clone();
                let terms = &mut doc_terms[doc];
                let replace = (terms.len() / 20).max(1);
                terms.shuffle(&mut rng);
                terms.truncate(terms.len() - replace);
                for _ in 0..replace {
                    if let Some(&borrowed) = neighbour.choose(&mut rng) {
                        terms.push((borrowed.0, 0, 1));
                    }
                }
                terms.sort_unstable();
                terms.dedup_by_key(|t| (t.0, t.1));
                for &(term_id, field, tf) in terms.iter() {
                    delta.entry(term_id).or_default().push((doc as u32, field, tf));
                }
            }
            peak = peak.max(delta_bytes(&delta));
            if delta_bytes(&delta) > THRESHOLD {
                let ((d, p), ms) = time_ms(|| {
                    let mut lists: Vec<Vec<Entry>> = vec![Vec::new(); source.len()];
                    for (doc, terms) in doc_terms.iter().enumerate() {
                        for &(term_id, field, tf) in terms {
                            lists[term_id as usize].push((doc as u32, field, tf));
                        }
                    }
                    let merged: Vec<(&str, Vec<Entry>)> = source.iter().zip(lists).filter(|(_, l)| !l.is_empty()).map(|((t, _), l)| (*t, l)).collect();
                    build(&merged)
                });
                base_dictionary = d;
                base_postings = p;
                merges += 1;
                merge_ms_total += ms;
                delta = HashMap::new();
                tombstone.iter_mut().for_each(|t| *t = false);
            }
        }
        // Final correctness: base + tombstones + delta against a scan of every file's current words
        let base = Map::new(base_dictionary)?;
        let (mut wrong, mut scratch) = (0usize, Vec::new());
        for &term_id in probes.iter().take(200) {
            let mut truth: Vec<Entry> = Vec::new();
            for (doc, terms) in doc_terms.iter().enumerate() {
                truth.extend(terms.iter().filter(|t| t.0 as usize == term_id).map(|&(_, field, tf)| (doc as u32, field, tf)));
            }
            truth.sort_unstable();
            lookup(&base, &base_postings, source[term_id].0, &mut scratch);
            let mut got: Vec<Entry> = scratch.iter().copied().filter(|e| !tombstone[e.0 as usize]).collect();
            got.extend(delta.get(&(term_id as u32)).into_iter().flatten());
            got.sort_unstable();
            wrong += (got != truth) as usize;
        }
        println!("{files_edited} file edits in 403 events (400 single saves, 3 bursts of up to 2,000 files)");
        println!("merges triggered: {merges}, taking {:.0} ms in total ({:.0} ms each)", merge_ms_total, merge_ms_total / merges.max(1) as f64);
        println!("largest the side index ever got: {:.1} MB; at the end: {:.1} MB", mb(peak), mb(delta_bytes(&delta)));
        println!("lookups wrong at the end: {wrong} of 200");
    }

    // ═══ 11. Tantivy with separate name and content fields ═══════════════════
    println!("\n## 11. Tantivy with separate name and content fields, exact word counts\n");
    {
        use tantivy::schema::{IndexRecordOption, Schema, TextFieldIndexing, TextOptions};
        use tantivy::{doc, Index, IndexWriter};

        let dir = work.join("tantivy-fields");
        std::fs::create_dir_all(&dir)?;
        let options = || TextOptions::default().set_indexing_options(TextFieldIndexing::default().set_tokenizer("ws").set_index_option(IndexRecordOption::WithFreqs));
        let mut schema = Schema::builder();
        let name = schema.add_text_field("name", options());
        let folder = schema.add_text_field("folder", options());
        let content = schema.add_text_field("content", options());
        let index = Index::create_in_dir(&dir, schema.build())?;
        register_tokenizer(&index);
        let mut writer: IndexWriter = index.writer(150_000_000)?;

        let mut per_doc: Vec<Vec<(u32, u8, u32)>> = vec![Vec::new(); doc_len.len()];
        for (term_id, (_, entries)) in source.iter().enumerate() {
            for &(doc, field, tf) in entries {
                per_doc[doc as usize].push((term_id as u32, field, tf));
            }
        }
        let mut largest_tf = 0u32;
        let (_, index_ms) = time_ms(|| -> anyhow::Result<()> {
            let mut text = [String::new(), String::new(), String::new()];
            for terms in &per_doc {
                text.iter_mut().for_each(String::clear);
                for &(term_id, field, tf) in terms {
                    largest_tf = largest_tf.max(tf);
                    // filename (1), folder (2), both (3), content (0)
                    let targets: &[usize] = match field { 0 => &[2], 1 => &[0], 2 => &[1], _ => &[0, 1] };
                    for &target in targets {
                        for _ in 0..tf {
                            text[target].push_str(source[term_id as usize].0);
                            text[target].push(' ');
                        }
                    }
                }
                writer.add_document(doc!(name => text[0].as_str(), folder => text[1].as_str(), content => text[2].as_str()))?;
            }
            writer.commit()?;
            Ok(())
        });
        writer.wait_merging_threads()?;
        let live: usize = index
            .searchable_segment_metas()?
            .iter()
            .flat_map(|meta| meta.list_files())
            .filter_map(|file| std::fs::metadata(dir.join(file)).ok())
            .map(|m| m.len() as usize)
            .sum();
        println!("indexed in {:.1} s; largest word count in one file: {largest_tf}", index_ms / 1000.0);
        println!("live index files: {:.2} MB across {} segment(s)", mb(live), index.searchable_segment_ids()?.len());
    }

    let _ = std::fs::remove_dir_all(&work);
    let mut out = std::io::stdout();
    out.flush()?;
    Ok(())
}
