//! The on-disk index: an immutable, memory-mapped inverted index.
//!
//! One generation of the base is three files in the data directory:
//!
//! ```text
//! base_NNNNNN.terms   finite-state transducer: term -> u64
//! base_NNNNNN.post    postings, addressed by offsets held in the transducer
//! base_NNNNNN.docs    the document table, in ordinal order (LZ4 + bincode)
//! ```
//!
//! Documents are numbered by *ordinal*: their rank in path order when the base
//! was written. Neighbouring ordinals therefore share a folder, which keeps
//! the gaps in posting lists small. Ordinals are private to one generation;
//! everything outside this module refers to documents by `DocId`.
//!
//! The u64 a term maps to is either
//!   * `1 | ordinal << 1 | field << 25 | tf << 27` when the term occurs in
//!     exactly one place (two thirds of all terms on real data), or
//!   * `offset << 1` into the postings file, where a list is stored as a
//!     count followed by one variable-length integer per posting holding
//!     `gap << 3 | field << 1 | (tf == 1)`, then `tf - 2` when tf is above 1.
//!
//! The terms and postings files are memory-mapped and decoded only when a
//! query asks for a term, so opening the index costs nothing and its pages
//! are file-backed: the OS can evict them under memory pressure.
//!
//! The base never changes. Edits go to the in-memory delta index and mark the
//! superseded documents dead (see `Bitset`); `merge` writes the next
//! generation from the old base, the dead set and the delta.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use fst::{Map, MapBuilder, Streamer};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

use crate::index::delta::{DocId, Document, Posting, FIELD_CONTENT, FIELD_FILENAME, FIELD_PATH};

/// One decoded posting: (ordinal, field mask, term frequency)
pub type Entry = (u32, u8, u32);

const INLINE: u64 = 1;
/// Ordinals must fit the 24 bits an inline value gives them
pub const MAX_ORDINALS: usize = 1 << 24;
const DOCS_MAGIC: &[u8] = b"LSDOC001";

// ─── Bitset ───────────────────────────────────────────────────────────────────

/// Which ordinals of the base have been superseded or removed since it was written.
#[derive(Debug, Default, Clone)]
pub struct Bitset {
    words: Vec<u64>,
    ones: usize,
}

impl Bitset {
    pub fn new(len: usize) -> Self {
        Self { words: vec![0; len.div_ceil(64)], ones: 0 }
    }

    pub fn set(&mut self, index: u32) {
        let (word, bit) = (index as usize / 64, index % 64);
        if let Some(slot) = self.words.get_mut(word) {
            if *slot & (1 << bit) == 0 {
                *slot |= 1 << bit;
                self.ones += 1;
            }
        }
    }

    pub fn get(&self, index: u32) -> bool {
        self.words
            .get(index as usize / 64)
            .is_some_and(|word| word & (1 << (index % 64)) != 0)
    }

    pub fn count(&self) -> usize {
        self.ones
    }
}

// ─── Encoding ─────────────────────────────────────────────────────────────────

fn put_vint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// None if the bytes end mid-integer or the integer overflows: a damaged file
/// must produce "no results", never a panic.
fn get_vint(bytes: &[u8], pos: &mut usize) -> Option<u64> {
    let (mut value, mut shift) = (0u64, 0u32);
    loop {
        let byte = *bytes.get(*pos)?;
        *pos += 1;
        if shift >= 64 {
            return None;
        }
        value |= ((byte & 0x7F) as u64) << shift;
        if byte < 0x80 {
            return Some(value);
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

fn field_mask(code: u8) -> u8 {
    match code & 3 {
        0 => FIELD_CONTENT,
        1 => FIELD_FILENAME,
        2 => FIELD_PATH,
        _ => FIELD_FILENAME | FIELD_PATH,
    }
}

// ─── Writing ──────────────────────────────────────────────────────────────────

/// Sizes and checksums of one written generation, recorded in the manifest
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseFiles {
    pub generation: u64,
    pub terms_checksum: u64,
    pub postings_checksum: u64,
    pub docs_checksum: u64,
    pub bytes: u64,
}

pub fn file_path(dir: &Path, generation: u64, extension: &str) -> PathBuf {
    dir.join(format!("base_{generation:06}.{extension}"))
}

/// Remove every base file in `dir` except those of `keep`.
pub fn remove_other_generations(dir: &Path, keep: Option<u64>) {
    let keep_prefix = keep.map(|generation| format!("base_{generation:06}."));
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_base = name.starts_with("base_")
            && [".terms", ".post", ".docs"].iter().any(|ext| name.ends_with(ext));
        if is_base && !keep_prefix.as_ref().is_some_and(|prefix| name.starts_with(prefix)) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Streams terms (in sorted order) into the three files of a generation.
struct BaseWriter {
    dir: PathBuf,
    generation: u64,
    dictionary: MapBuilder<BufWriter<File>>,
    postings: Vec<u8>,
}

impl BaseWriter {
    fn create(dir: &Path, generation: u64) -> io::Result<Self> {
        let terms = File::create(file_path(dir, generation, "terms.tmp"))?;
        let dictionary = MapBuilder::new(BufWriter::new(terms)).map_err(io::Error::other)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            generation,
            dictionary,
            // Offset 0 is never a list, and an empty file cannot be mapped
            postings: vec![0],
        })
    }

    /// `entries` must be sorted by (ordinal, field) and non-empty.
    fn add(&mut self, term: &[u8], entries: &[Entry]) -> io::Result<()> {
        let value = if let [(ordinal, mask, tf)] = entries[..] {
            INLINE | ((ordinal as u64) << 1) | ((field_code(mask) as u64) << 25) | ((tf as u64) << 27)
        } else {
            let offset = self.postings.len() as u64;
            put_vint(&mut self.postings, entries.len() as u64);
            let mut previous = 0u32;
            for &(ordinal, mask, tf) in entries {
                let tf = tf.max(1);
                let head = (((ordinal - previous) as u64) << 3) | ((field_code(mask) as u64) << 1) | (tf == 1) as u64;
                put_vint(&mut self.postings, head);
                if tf != 1 {
                    put_vint(&mut self.postings, (tf - 2) as u64);
                }
                previous = ordinal;
            }
            offset << 1
        };
        self.dictionary.insert(term, value).map_err(io::Error::other)
    }

    /// Write, sync and rename all three files into place.
    fn finish(self, documents: &[&Document]) -> io::Result<BaseFiles> {
        let Self { dir, generation, dictionary, postings } = self;

        let terms_file = dictionary.into_inner().map_err(io::Error::other)?.into_inner().map_err(|e| e.into_error())?;
        terms_file.sync_all()?;
        drop(terms_file);

        let mut docs = DOCS_MAGIC.to_vec();
        let encoded = bincode::serialize(documents).map_err(io::Error::other)?;
        docs.extend_from_slice(&lz4_flex::compress_prepend_size(&encoded));

        let write_synced = |extension: &str, bytes: &[u8]| -> io::Result<()> {
            let mut file = File::create(file_path(&dir, generation, &format!("{extension}.tmp")))?;
            file.write_all(bytes)?;
            file.sync_all()
        };
        write_synced("post", &postings)?;
        write_synced("docs", &docs)?;

        let terms_bytes = fs::read(file_path(&dir, generation, "terms.tmp"))?;
        let files = BaseFiles {
            generation,
            terms_checksum: xxh3_64(&terms_bytes),
            postings_checksum: xxh3_64(&postings),
            docs_checksum: xxh3_64(&docs),
            bytes: (terms_bytes.len() + postings.len() + docs.len()) as u64,
        };
        // The generation only counts once the manifest names it, so the order
        // of these renames does not matter for crash safety.
        for extension in ["terms", "post", "docs"] {
            fs::rename(
                file_path(&dir, generation, &format!("{extension}.tmp")),
                file_path(&dir, generation, extension),
            )?;
        }
        Ok(files)
    }
}

/// Write the next generation: the live part of `old`, plus the delta.
///
/// `live` is the full document table as it should be after the merge. A
/// posting survives if its document is in `live` and, for postings from the
/// old base, its ordinal is not in `dead`.
pub fn merge(
    dir: &Path,
    generation: u64,
    old: Option<(&BaseIndex, &Bitset)>,
    delta: &HashMap<String, Vec<Posting>>,
    live: &HashMap<DocId, Document>,
) -> io::Result<BaseFiles> {
    if live.len() > MAX_ORDINALS {
        return Err(io::Error::other(format!("{} documents exceed the index format's limit", live.len())));
    }

    // New ordinals: path order
    let mut documents: Vec<&Document> = live.values().collect();
    documents.sort_by(|a, b| a.path.cmp(&b.path).then(a.doc_id.cmp(&b.doc_id)));
    let ordinal_of: HashMap<DocId, u32> = documents.iter().enumerate().map(|(i, doc)| (doc.doc_id, i as u32)).collect();

    // Old ordinal -> new ordinal, for documents that carry over
    let carried: Vec<Option<u32>> = match old {
        Some((base, dead)) => (0..base.doc_ids.len() as u32)
            .map(|old_ordinal| {
                if dead.get(old_ordinal) {
                    None
                } else {
                    ordinal_of.get(&base.doc_ids[old_ordinal as usize]).copied()
                }
            })
            .collect(),
        None => Vec::new(),
    };

    let mut delta_terms: Vec<(&String, &Vec<Posting>)> = delta.iter().collect();
    delta_terms.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut delta_terms = delta_terms.into_iter().peekable();

    let mut writer = BaseWriter::create(dir, generation)?;
    let mut entries: Vec<Entry> = Vec::new();
    let mut decoded: Vec<Entry> = Vec::new();

    let mut emit = |writer: &mut BaseWriter, term: &[u8], old_value: Option<u64>, new: Option<&Vec<Posting>>| -> io::Result<()> {
        entries.clear();
        if let (Some(value), Some((base, _))) = (old_value, old) {
            base.decode(value, &mut decoded);
            entries.extend(decoded.iter().filter_map(|&(ordinal, mask, tf)| {
                carried.get(ordinal as usize).copied().flatten().map(|new_ordinal| (new_ordinal, mask, tf))
            }));
        }
        if let Some(postings) = new {
            entries.extend(postings.iter().filter_map(|posting| {
                ordinal_of.get(&posting.doc_id).map(|&ordinal| (ordinal, field_mask(field_code(posting.field_mask)), posting.term_freq))
            }));
        }
        if entries.is_empty() {
            return Ok(());
        }
        entries.sort_unstable();
        writer.add(term, &entries)
    };

    // Merge-join the old dictionary (already sorted) with the sorted delta terms
    match old {
        Some((base, _)) => {
            let mut stream = base.dictionary.stream();
            while let Some((term, value)) = stream.next() {
                while let Some((delta_term, postings)) = delta_terms.next_if(|(t, _)| t.as_bytes() < term) {
                    emit(&mut writer, delta_term.as_bytes(), None, Some(postings))?;
                }
                let same = delta_terms.next_if(|(t, _)| t.as_bytes() == term);
                emit(&mut writer, term, Some(value), same.map(|(_, postings)| postings))?;
            }
        }
        None => {}
    }
    for (delta_term, postings) in delta_terms {
        emit(&mut writer, delta_term.as_bytes(), None, Some(postings))?;
    }

    writer.finish(&documents)
}

// ─── Reading ──────────────────────────────────────────────────────────────────

pub struct BaseIndex {
    dictionary: Map<Mmap>,
    postings: Mmap,
    /// ordinal -> DocId
    doc_ids: Vec<DocId>,
    ordinals: HashMap<DocId, u32>,
    files: BaseFiles,
}

impl BaseIndex {
    /// Map a generation's files and read its document table.
    ///
    /// With `verify`, every file is checked against the checksums in `files`
    /// first. That reads the whole index, so it is done after an unclean
    /// shutdown rather than on every launch.
    pub fn open(dir: &Path, files: &BaseFiles, verify: bool) -> io::Result<(Self, Vec<Document>)> {
        let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());

        let docs_bytes = fs::read(file_path(dir, files.generation, "docs"))?;
        // The document table is small and fully read anyway: always verify it
        if xxh3_64(&docs_bytes) != files.docs_checksum || !docs_bytes.starts_with(DOCS_MAGIC) {
            return Err(invalid("document table is damaged or from another format"));
        }
        let decoded = lz4_flex::decompress_size_prepended(&docs_bytes[DOCS_MAGIC.len()..]).map_err(|_| invalid("document table does not decompress"))?;
        let documents: Vec<Document> = bincode::deserialize(&decoded).map_err(|_| invalid("document table does not decode"))?;

        let terms = unsafe { Mmap::map(&File::open(file_path(dir, files.generation, "terms"))?)? };
        let postings = unsafe { Mmap::map(&File::open(file_path(dir, files.generation, "post"))?)? };
        if verify && (xxh3_64(&terms) != files.terms_checksum || xxh3_64(&postings) != files.postings_checksum) {
            return Err(invalid("index files do not match their checksums"));
        }
        let dictionary = Map::new(terms).map_err(|_| invalid("term dictionary is not readable"))?;

        let doc_ids: Vec<DocId> = documents.iter().map(|doc| doc.doc_id).collect();
        let ordinals = doc_ids.iter().enumerate().map(|(i, id)| (*id, i as u32)).collect();
        Ok((Self { dictionary, postings, doc_ids, ordinals, files: files.clone() }, documents))
    }

    pub fn files(&self) -> &BaseFiles {
        &self.files
    }

    pub fn doc_count(&self) -> usize {
        self.doc_ids.len()
    }

    pub fn term_count(&self) -> usize {
        self.dictionary.len()
    }

    pub fn doc_id(&self, ordinal: u32) -> Option<DocId> {
        self.doc_ids.get(ordinal as usize).copied()
    }

    pub fn ordinal_of(&self, doc_id: DocId) -> Option<u32> {
        self.ordinals.get(&doc_id).copied()
    }

    /// The dictionary, for prefix and approximate term search.
    pub fn dictionary(&self) -> &Map<Mmap> {
        &self.dictionary
    }

    /// Postings of `term`, straight from the mapped files. Empty if absent.
    pub fn lookup(&self, term: &str, out: &mut Vec<Entry>) {
        match self.dictionary.get(term) {
            Some(value) => self.decode(value, out),
            None => out.clear(),
        }
    }

    fn decode(&self, value: u64, out: &mut Vec<Entry>) {
        out.clear();
        if value & INLINE != 0 {
            let ordinal = ((value >> 1) & 0xFF_FFFF) as u32;
            if (ordinal as usize) < self.doc_ids.len() {
                out.push((ordinal, field_mask(((value >> 25) & 3) as u8), (value >> 27) as u32));
            }
            return;
        }
        // Every read is bounds-checked: a damaged file ends the list early
        let mut pos = (value >> 1) as usize;
        let Some(count) = get_vint(&self.postings, &mut pos) else { return };
        let mut ordinal = 0u64;
        for _ in 0..count.min(self.doc_ids.len() as u64 * 4) {
            let Some(head) = get_vint(&self.postings, &mut pos) else { return };
            ordinal += head >> 3;
            let tf = if head & 1 == 1 {
                1
            } else {
                match get_vint(&self.postings, &mut pos) {
                    Some(extra) => extra.saturating_add(2).min(u32::MAX as u64) as u32,
                    None => return,
                }
            };
            if ordinal >= self.doc_ids.len() as u64 {
                return;
            }
            out.push((ordinal as u32, field_mask(((head >> 1) & 3) as u8), tf));
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(id: u64, path: &str) -> Document {
        Document { doc_id: DocId(id), path: path.to_string(), doc_len: 3, ..Default::default() }
    }

    fn table(docs: &[Document]) -> HashMap<DocId, Document> {
        docs.iter().map(|d| (d.doc_id, d.clone())).collect()
    }

    fn postings(list: &[(&str, u64, u8, u32)]) -> HashMap<String, Vec<Posting>> {
        let mut delta: HashMap<String, Vec<Posting>> = HashMap::new();
        for &(term, doc, mask, tf) in list {
            delta.entry(term.to_string()).or_default().push(Posting::new(DocId(doc), tf, mask));
        }
        delta
    }

    /// (path, field mask, tf) of every posting for a term, by path
    fn hits(base: &BaseIndex, docs: &[Document], term: &str) -> Vec<(String, u8, u32)> {
        let mut out = Vec::new();
        base.lookup(term, &mut out);
        out.iter().map(|&(ordinal, mask, tf)| (docs[ordinal as usize].path.clone(), mask, tf)).collect()
    }

    #[test]
    fn test_roundtrip_single_and_multiple_postings() {
        let dir = tempfile::tempdir().unwrap();
        let live = table(&[doc(10, "/b/two.txt"), doc(11, "/a/one.txt"), doc(12, "/c/three.txt")]);
        let delta = postings(&[
            ("report", 10, FIELD_CONTENT, 7),
            ("report", 11, FIELD_FILENAME, 1),
            ("report", 12, FIELD_FILENAME | FIELD_PATH, 300),
            ("unique", 12, FIELD_PATH, 1),
            ("café", 10, FIELD_CONTENT, 2),
        ]);

        let files = merge(dir.path(), 1, None, &delta, &live).unwrap();
        let (base, docs) = BaseIndex::open(dir.path(), &files, true).unwrap();

        // Ordinals follow path order, not DocId order
        assert_eq!(docs.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(), ["/a/one.txt", "/b/two.txt", "/c/three.txt"]);
        assert_eq!(base.doc_id(0), Some(DocId(11)));
        assert_eq!(base.ordinal_of(DocId(12)), Some(2));
        assert_eq!(
            hits(&base, &docs, "report"),
            [
                ("/a/one.txt".to_string(), FIELD_FILENAME, 1),
                ("/b/two.txt".to_string(), FIELD_CONTENT, 7),
                ("/c/three.txt".to_string(), FIELD_FILENAME | FIELD_PATH, 300),
            ]
        );
        assert_eq!(hits(&base, &docs, "unique"), [("/c/three.txt".to_string(), FIELD_PATH, 1)]);
        assert_eq!(hits(&base, &docs, "café"), [("/b/two.txt".to_string(), FIELD_CONTENT, 2)]);
        assert!(hits(&base, &docs, "absent").is_empty());
        assert_eq!(base.term_count(), 3);
    }

    #[test]
    fn test_merge_drops_dead_and_removed_documents_and_adds_delta() {
        let dir = tempfile::tempdir().unwrap();
        let first = [doc(1, "/a.txt"), doc(2, "/b.txt"), doc(3, "/c.txt")];
        let delta = postings(&[
            ("alpha", 1, FIELD_CONTENT, 1),
            ("alpha", 2, FIELD_CONTENT, 1),
            ("alpha", 3, FIELD_CONTENT, 1),
            ("only_b", 2, FIELD_CONTENT, 1),
        ]);
        let files = merge(dir.path(), 1, None, &delta, &table(&first)).unwrap();
        let (base, _) = BaseIndex::open(dir.path(), &files, true).unwrap();

        // b was edited (dead in the base, new postings in the delta), c was
        // deleted (gone from the table), d is new and sorts before everything.
        let mut dead = Bitset::new(base.doc_count());
        dead.set(base.ordinal_of(DocId(2)).unwrap());
        let live = table(&[doc(1, "/a.txt"), doc(2, "/b.txt"), doc(4, "/0-new.txt")]);
        let changes = postings(&[("beta", 2, FIELD_CONTENT, 5), ("alpha", 4, FIELD_FILENAME, 1), ("aaa", 4, FIELD_CONTENT, 1)]);

        let files = merge(dir.path(), 2, Some((&base, &dead)), &changes, &live).unwrap();
        let (merged, docs) = BaseIndex::open(dir.path(), &files, true).unwrap();

        assert_eq!(
            hits(&merged, &docs, "alpha"),
            [("/0-new.txt".to_string(), FIELD_FILENAME, 1), ("/a.txt".to_string(), FIELD_CONTENT, 1)]
        );
        assert_eq!(hits(&merged, &docs, "beta"), [("/b.txt".to_string(), FIELD_CONTENT, 5)]);
        assert_eq!(hits(&merged, &docs, "aaa"), [("/0-new.txt".to_string(), FIELD_CONTENT, 1)]);
        // A term whose only document died disappears from the dictionary
        assert!(hits(&merged, &docs, "only_b").is_empty());
        assert_eq!(merged.term_count(), 3);
        assert_eq!(merged.doc_count(), 3);
    }

    #[test]
    fn test_a_document_can_have_postings_in_several_fields() {
        let dir = tempfile::tempdir().unwrap();
        let live = table(&[doc(1, "/report.txt")]);
        let delta = postings(&[("report", 1, FIELD_FILENAME, 1), ("report", 1, FIELD_CONTENT, 4)]);
        let files = merge(dir.path(), 1, None, &delta, &live).unwrap();
        let (base, docs) = BaseIndex::open(dir.path(), &files, true).unwrap();

        assert_eq!(
            hits(&base, &docs, "report"),
            [("/report.txt".to_string(), FIELD_FILENAME, 1), ("/report.txt".to_string(), FIELD_CONTENT, 4)]
        );
    }

    #[test]
    fn test_empty_index_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let files = merge(dir.path(), 1, None, &HashMap::new(), &HashMap::new()).unwrap();
        let (base, docs) = BaseIndex::open(dir.path(), &files, true).unwrap();
        assert!(docs.is_empty());
        assert_eq!(base.term_count(), 0);
        assert!(hits(&base, &docs, "anything").is_empty());
    }

    #[test]
    fn test_damage_is_detected_or_survived() {
        let dir = tempfile::tempdir().unwrap();
        let live = table(&(0..200).map(|i| doc(i, &format!("/f{i:03}.txt"))).collect::<Vec<_>>());
        let delta = postings(&(0..200).map(|i| ("common", i, FIELD_CONTENT, 1 + (i % 5) as u32)).collect::<Vec<_>>());
        let files = merge(dir.path(), 1, None, &delta, &live).unwrap();

        // Flip bytes throughout the postings file
        let path = file_path(dir.path(), 1, "post");
        let mut bytes = fs::read(&path).unwrap();
        for byte in bytes.iter_mut().skip(1).step_by(3) {
            *byte ^= 0xFF;
        }
        fs::write(&path, bytes).unwrap();

        // With verification the damage is reported...
        assert!(BaseIndex::open(dir.path(), &files, true).is_err());
        // ...and without it, a lookup returns something bounded instead of panicking
        let (base, _) = BaseIndex::open(dir.path(), &files, false).unwrap();
        let mut out = Vec::new();
        base.lookup("common", &mut out);
        assert!(out.iter().all(|&(ordinal, _, _)| (ordinal as usize) < 200));

        // A damaged document table is always caught
        let docs_path = file_path(dir.path(), 1, "docs");
        let mut docs = fs::read(&docs_path).unwrap();
        let last = docs.len() - 1;
        docs[last] ^= 0xFF;
        fs::write(&docs_path, docs).unwrap();
        assert!(BaseIndex::open(dir.path(), &files, false).is_err());
    }

    #[test]
    fn test_remove_other_generations() {
        let dir = tempfile::tempdir().unwrap();
        for generation in 1..=3 {
            merge(dir.path(), generation, None, &HashMap::new(), &HashMap::new()).unwrap();
        }
        fs::write(dir.path().join("wal.log"), b"").unwrap();

        remove_other_generations(dir.path(), Some(2));

        let mut names: Vec<String> = fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["base_000002.docs", "base_000002.post", "base_000002.terms", "wal.log"]);
    }

    #[test]
    fn test_bitset() {
        let mut bits = Bitset::new(130);
        assert!(!bits.get(129));
        bits.set(129);
        bits.set(129);
        bits.set(0);
        assert!(bits.get(129) && bits.get(0) && !bits.get(64));
        assert_eq!(bits.count(), 2);
        // Out of range is ignored, not a panic
        bits.set(10_000);
        assert!(!bits.get(10_000));
    }
}
