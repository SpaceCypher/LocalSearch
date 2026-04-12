use crate::index::delta::{DeltaIndex, DocId, Document, Posting};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use lz4_flex::compress_prepend_size;
use lz4_flex::decompress_size_prepended;
#[cfg(unix)]
use std::os::unix::fs::FileExt;
use std::sync::atomic::{AtomicBool, Ordering};

/// Immutable segment file
pub struct Segment {
    path: PathBuf,
    term_dict: HashMap<String, Vec<Posting>>,
    documents: HashMap<DocId, Document>,
    is_cold: bool,
}

impl Segment {
    /// Open existing segment from disk (fully loads into RAM)
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let compressed = fs::read(&path)?;
        
        // Decompress with LZ4
        let decompressed = decompress_size_prepended(&compressed)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        
        // Deserialize term dictionary
        let term_dict: HashMap<String, Vec<Posting>> = bincode::deserialize(&decompressed)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        
        Ok(Self {
            path,
            term_dict,
            documents: HashMap::new(),
            is_cold: false,
        })
    }

    /// Open segment without fully loading its dictionary (Cold Segment)
    pub fn open_cold(path: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            term_dict: HashMap::new(),
            documents: HashMap::new(),
            is_cold: true,
        })
    }

    /// Prefetches a portion of the segment using pread to avoid blocking
    pub fn prefetch(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            let file = fs::File::open(&self.path)?;
            // Use pread to fault in the first 4KB (header/dictionary start)
            let mut buf = [0u8; 4096];
            let _ = file.read_at(&mut buf, 0);
        }
        Ok(())
    }

    /// Binary search lookup for term
    pub fn lookup(&self, term: &str) -> Option<Vec<Posting>> {
        if self.is_cold {
            // In a full implementation, this might trigger an async load
            // For now, we return None if it hasn't been warmed up yet
            return None;
        }
        self.term_dict.get(term).cloned()
    }
}

/// Builder for creating segment from delta index
pub struct SegmentBuilder {
    term_dict: HashMap<String, Vec<Posting>>,
    documents: HashMap<DocId, Document>,
}

impl SegmentBuilder {
    pub fn new() -> Self {
        Self {
            term_dict: HashMap::new(),
            documents: HashMap::new(),
        }
    }

    /// Create builder from delta index
    pub fn from_delta(delta: &DeltaIndex) -> Self {
        let mut builder = Self::new();
        
        // Copy term dictionary
        for (term, postings) in &delta.index {
            builder.term_dict.insert(term.clone(), postings.clone());
        }
        
        // Copy documents
        for (doc_id, doc) in &delta.documents {
            builder.documents.insert(*doc_id, doc.clone());
        }
        
        builder
    }

    /// Finalize segment to disk with atomic rename
    pub fn finalize(self, final_path: impl AsRef<Path>) -> io::Result<PathBuf> {
        let final_path = final_path.as_ref();
        
        // Write to temp file first
        let temp_path = final_path.with_extension("tmp");
        let mut file = fs::File::create(&temp_path)?;
        
        // Serialize term dictionary
        let serialized = bincode::serialize(&self.term_dict)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        
        // Compress with LZ4
        let compressed = compress_prepend_size(&serialized);
        
        file.write_all(&compressed)?;
        file.sync_all()?;
        
        // Atomic rename
        fs::rename(&temp_path, final_path)?;
        
        Ok(final_path.to_path_buf())
    }
}

/// Suffix array for O(log n) substring search
pub struct SuffixArray {
    blob: String,
    suffixes: Vec<(u32, DocId)>,
    rebuilding: AtomicBool,
}

pub enum SubstringResult {
    Found(Vec<DocId>),
    FallbackRequired,
}

impl SubstringResult {
    pub fn is_fallback_required(&self) -> bool {
        matches!(self, SubstringResult::FallbackRequired)
    }
    
    pub fn doc_ids(&self) -> Vec<DocId> {
        match self {
            SubstringResult::Found(ids) => ids.clone(),
            SubstringResult::FallbackRequired => vec![],
        }
    }
}

impl SuffixArray {
    pub fn new() -> Self {
        Self {
            blob: String::new(),
            suffixes: Vec::new(),
            rebuilding: AtomicBool::new(false),
        }
    }
    
    pub fn insert(&mut self, filename: &str, doc_id: DocId) {
        let start_offset = self.blob.len() as u32;
        self.blob.push_str(filename);
        self.blob.push('\0');
        
        for i in 0..filename.len() {
            self.suffixes.push((start_offset + i as u32, doc_id));
        }
        
        self.suffixes.sort_by(|a, b| {
            let suffix_a = &self.blob[a.0 as usize..];
            let suffix_b = &self.blob[b.0 as usize..];
            suffix_a.cmp(suffix_b)
        });
    }
    
    pub fn search_substring(&self, query: &str) -> SubstringResult {
        if self.rebuilding.load(Ordering::Relaxed) {
            return SubstringResult::FallbackRequired;
        }
        
        let mut results = Vec::new();
        let mut left = 0;
        let mut right = self.suffixes.len();
        
        while left < right {
            let mid = (left + right) / 2;
            let suffix = &self.blob[self.suffixes[mid].0 as usize..];
            
            if suffix.starts_with(query) {
                results.push(self.suffixes[mid].1);
                
                let mut i = mid.wrapping_sub(1);
                while i < self.suffixes.len() {
                    let s = &self.blob[self.suffixes[i].0 as usize..];
                    if s.starts_with(query) {
                        results.push(self.suffixes[i].1);
                        i = i.wrapping_sub(1);
                    } else {
                        break;
                    }
                }
                
                let mut i = mid + 1;
                while i < self.suffixes.len() {
                    let s = &self.blob[self.suffixes[i].0 as usize..];
                    if s.starts_with(query) {
                        results.push(self.suffixes[i].1);
                        i += 1;
                    } else {
                        break;
                    }
                }
                break;
            } else if suffix < query {
                left = mid + 1;
            } else {
                right = mid;
            }
        }
        
        results.sort();
        results.dedup();
        SubstringResult::Found(results)
    }
    
    pub fn begin_rebuild(&mut self) {
        self.rebuilding.store(true, Ordering::Relaxed);
    }
    
    pub fn commit_rebuild(&mut self) {
        self.rebuilding.store(false, Ordering::Relaxed);
    }
    
    pub fn is_rebuilding(&self) -> bool {
        self.rebuilding.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::delta::FIELD_FILENAME;

    #[test]
    fn test_segment_creation_from_delta() {
        let mut delta = DeltaIndex::new(50 * 1024 * 1024);
        let doc = Document::new_test(DocId(1), "/test/report.pdf");
        let mut postings = HashMap::new();
        postings.insert("report".to_string(), Posting::new(DocId(1), 1, FIELD_FILENAME));
        delta.insert_document(doc, postings).unwrap();
        
        let builder = SegmentBuilder::from_delta(&delta);
        assert!(builder.term_dict.contains_key("report"));
    }

    #[test]
    fn test_segment_binary_search_lookup() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("test_seg.seg");
        let _ = fs::remove_file(&path);
        
        let mut delta = DeltaIndex::new(50 * 1024 * 1024);
        let doc = Document::new_test(DocId(1), "/test/budget.xlsx");
        let mut postings = HashMap::new();
        postings.insert("budget".to_string(), Posting::new(DocId(1), 2, FIELD_FILENAME));
        delta.insert_document(doc, postings).unwrap();
        
        let builder = SegmentBuilder::from_delta(&delta);
        builder.finalize(&path).unwrap();
        
        let segment = Segment::open(&path).unwrap();
        let result = segment.lookup("budget");
        assert!(result.is_some());
        assert_eq!(result.unwrap()[0].doc_id, DocId(1));
        
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_cold_segment_behavior() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("test_cold.seg");
        fs::write(&path, b"dummy content").unwrap();
        
        let segment = Segment::open_cold(&path).unwrap();
        assert!(segment.is_cold);
        assert!(segment.lookup("any").is_none());
        
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_suffix_array_search() {
        let mut sa = SuffixArray::new();
        sa.insert("AppDelegate.swift", DocId(1));
        let results = sa.search_substring("Delegate");
        assert!(results.doc_ids().contains(&DocId(1)));
    }
}
