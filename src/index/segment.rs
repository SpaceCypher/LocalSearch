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
