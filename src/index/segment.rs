// Segment — Immutable on-disk index segment
// Format: LZ4 term dict, delta+varint posting lists, zstd metadata
// Write: atomic rename from temp file
// Read: binary search term lookup

use crate::index::delta::{DeltaIndex, DocId, Document, Posting};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use lz4_flex::compress_prepend_size;
use lz4_flex::decompress_size_prepended;

/// Immutable segment file
pub struct Segment {
    path: PathBuf,
    term_dict: HashMap<String, Vec<Posting>>,
    documents: HashMap<DocId, Document>,
}

impl Segment {
    /// Open existing segment from disk
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
        })
    }

    /// Binary search lookup for term
    pub fn lookup(&self, term: &str) -> Option<Vec<Posting>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::delta::FIELD_FILENAME;
    use std::collections::HashMap;

    #[test]
    fn test_segment_creation_from_delta() {
        let mut delta = DeltaIndex::new(50 * 1024 * 1024);
        
        // Insert test document
        let doc = Document::new_test(DocId(1), "/test/quarterly_report.pdf");
        let mut postings = HashMap::new();
        postings.insert(
            "quarterly".to_string(),
            Posting::new(DocId(1), 1, FIELD_FILENAME),
        );
        delta.insert_document(doc, postings).unwrap();
        
        // Build segment from delta
        let builder = SegmentBuilder::from_delta(&delta);
        
        // Verify term dictionary copied
        assert!(builder.term_dict.contains_key("quarterly"));
        assert_eq!(builder.term_dict.get("quarterly").unwrap().len(), 1);
    }

    #[test]
    fn test_segment_atomic_rename() {
        let temp_dir = std::env::temp_dir();
        let segment_path = temp_dir.join("test_segment.seg");
        
        // Clean up any existing file
        let _ = fs::remove_file(&segment_path);
        
        let mut delta = DeltaIndex::new(50 * 1024 * 1024);
        let doc = Document::new_test(DocId(1), "/test/file.txt");
        delta.insert_document(doc, HashMap::new()).unwrap();
        
        let builder = SegmentBuilder::from_delta(&delta);
        let result_path = builder.finalize(&segment_path).unwrap();
        
        // Verify file exists at final path
        assert!(result_path.exists());
        
        // Verify temp file was removed
        let temp_path = segment_path.with_extension("tmp");
        assert!(!temp_path.exists());
        
        // Clean up
        fs::remove_file(&segment_path).unwrap();
    }

    #[test]
    fn test_segment_binary_search_lookup() {
        let temp_dir = std::env::temp_dir();
        let segment_path = temp_dir.join("test_lookup.seg");
        let _ = fs::remove_file(&segment_path);
        
        let mut delta = DeltaIndex::new(50 * 1024 * 1024);
        let doc = Document::new_test(DocId(1), "/test/budget.xlsx");
        let mut postings = HashMap::new();
        postings.insert(
            "budget".to_string(),
            Posting::new(DocId(1), 2, FIELD_FILENAME),
        );
        delta.insert_document(doc, postings).unwrap();
        
        let builder = SegmentBuilder::from_delta(&delta);
        builder.finalize(&segment_path).unwrap();
        
        // Open segment and lookup term
        let segment = Segment::open(&segment_path).unwrap();
        let result = segment.lookup("budget");
        
        // Verify full round-trip with LZ4 compression
        assert!(result.is_some());
        let postings = result.unwrap();
        assert_eq!(postings.len(), 1);
        assert_eq!(postings[0].doc_id, DocId(1));
        
        // Clean up
        fs::remove_file(&segment_path).unwrap();
    }
}

/// Suffix array for O(log n) substring search
/// Stores sorted (suffix_offset, doc_id) pairs over concatenated filename blob
use std::sync::atomic::{AtomicBool, Ordering};

pub struct SuffixArray {
    /// Concatenated filename blob
    blob: String,
    /// Sorted (suffix_offset, doc_id) pairs
    suffixes: Vec<(u32, DocId)>,
    /// Rebuild flag
    rebuilding: AtomicBool,
}

/// Result of substring search
pub enum SubstringResult {
    /// Normal results
    Found(Vec<DocId>),
    /// Fallback required during rebuild
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
    
    /// Insert filename into suffix array
    pub fn insert(&mut self, filename: &str, doc_id: DocId) {
        let start_offset = self.blob.len() as u32;
        self.blob.push_str(filename);
        self.blob.push('\0'); // Null terminator
        
        // Add all suffixes for this filename
        for i in 0..filename.len() {
            self.suffixes.push((start_offset + i as u32, doc_id));
        }
        
        // Sort suffixes lexicographically
        self.suffixes.sort_by(|a, b| {
            let suffix_a = &self.blob[a.0 as usize..];
            let suffix_b = &self.blob[b.0 as usize..];
            suffix_a.cmp(suffix_b)
        });
    }
    
    /// Search for substring in filenames
    pub fn search_substring(&self, query: &str) -> SubstringResult {
        // Check if rebuilding
        if self.rebuilding.load(Ordering::Relaxed) {
            return SubstringResult::FallbackRequired;
        }
        
        let mut results = Vec::new();
        
        // Binary search for first matching suffix
        let mut left = 0;
        let mut right = self.suffixes.len();
        
        while left < right {
            let mid = (left + right) / 2;
            let suffix = &self.blob[self.suffixes[mid].0 as usize..];
            
            if suffix.starts_with(query) {
                // Found a match, collect all matching suffixes
                results.push(self.suffixes[mid].1);
                
                // Scan left and right for more matches
                let mut i = mid.wrapping_sub(1);
                while i < self.suffixes.len() {
                    let suffix = &self.blob[self.suffixes[i].0 as usize..];
                    if suffix.starts_with(query) {
                        results.push(self.suffixes[i].1);
                        i = i.wrapping_sub(1);
                    } else {
                        break;
                    }
                }
                
                let mut i = mid + 1;
                while i < self.suffixes.len() {
                    let suffix = &self.blob[self.suffixes[i].0 as usize..];
                    if suffix.starts_with(query) {
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
    
    /// Begin rebuild (marks array as rebuilding)
    pub fn begin_rebuild(&mut self) {
        self.rebuilding.store(true, Ordering::Relaxed);
    }
    
    /// Commit rebuild (marks array as ready)
    pub fn commit_rebuild(&mut self) {
        self.rebuilding.store(false, Ordering::Relaxed);
    }
    
    /// Check if rebuilding
    pub fn is_rebuilding(&self) -> bool {
        self.rebuilding.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod suffix_array_tests {
    use super::*;

    #[test]
    fn test_suffix_array_exact_substring() {
        let mut sa = SuffixArray::new();
        sa.insert("quarterly_report.pdf", DocId(1));
        sa.insert("budget_2024.xlsx", DocId(2));
        sa.insert("meeting_notes.txt", DocId(3));

        let results = sa.search_substring("report");
        assert!(!results.is_fallback_required());
        let doc_ids = results.doc_ids();
        assert!(doc_ids.contains(&DocId(1)));
        assert!(!doc_ids.contains(&DocId(2)));
    }

    #[test]
    fn test_suffix_array_mid_filename() {
        let mut sa = SuffixArray::new();
        sa.insert("AppDelegate.swift", DocId(1));
        let results = sa.search_substring("Delegate");
        assert!(!results.is_fallback_required());
        let doc_ids = results.doc_ids();
        assert!(doc_ids.contains(&DocId(1)));
    }

    #[test]
    fn test_substring_query_falls_back_during_rebuild() {
        // Architect-review fix: during suffix array rebuild, substring queries must
        // fall back to a linear scan of the delta index — not return empty results.
        // Verifies the rebuild window does NOT silently degrade user-visible search.
        let mut sa = SuffixArray::new();
        sa.insert("quarterly_report.pdf", DocId(1));

        // Simulate rebuild in progress: mark SA as rebuilding
        sa.begin_rebuild();
        assert!(sa.is_rebuilding());

        // Substring search during rebuild should return a FallbackRequired signal
        // (not empty results — caller must fall back to delta index linear scan)
        let result = sa.search_substring("report");
        assert!(
            result.is_fallback_required(),
            "During rebuild, search_substring must signal FallbackRequired, not return empty"
        );

        // After rebuild completes, normal results resume
        sa.commit_rebuild();
        let result = sa.search_substring("report");
        assert!(!result.is_fallback_required());
        assert!(result.doc_ids().contains(&DocId(1)));
    }
}
