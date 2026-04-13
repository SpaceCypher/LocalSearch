use crate::index::delta::DeltaIndex;
use crate::index::delta::DocId;
use crate::index::trie::PathTrie;
use crate::wal::entry::WalEntry;
use anyhow::{Result, anyhow};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use xxhash_rust::xxh3::xxh3_64;

pub struct InvariantChecker;

impl InvariantChecker {
    /// Verifies cross-index consistency between DeltaIndex and PathTrie.
    /// Every document in the DeltaIndex must be reachable in the PathTrie.
    pub fn check_cross_index_consistency(delta_index: &DeltaIndex, path_trie: &PathTrie) -> Result<()> {
        let mut checked = 0;
        for (doc_id, doc) in &delta_index.documents {
            // A scope query for the exact path should return the doc_id
            let results = path_trie.scope_query(Path::new(&doc.path));
            if !results.contains(doc_id.0 as u32) {
                return Err(anyhow!(
                    "INVARIANT VIOLATION: DocId {:?} at path '{}' is missing from PathTrie mapping.",
                    doc_id, doc.path
                ));
            }
            checked += 1;
        }
        
        log::debug!("Cross-index consistency check passed for {} documents", checked);
        Ok(())
    }

    /// Checks for basic structural sanity of the DeltaIndex.
    pub fn check_delta_sanity(delta_index: &DeltaIndex) -> Result<()> {
        let doc_count = delta_index.documents.len();
        let posting_count = delta_index.index.len();

        if doc_count > 0 && posting_count == 0 {
            return Err(anyhow!(
                "INVARIANT VIOLATION: DeltaIndex has {} documents but 0 indexed terms.",
                doc_count
            ));
        }

        Ok(())
    }

    /// WAL sequence numbers must be strictly monotonic without gaps.
    pub fn check_wal_monotonicity(entries: &[WalEntry]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        for pair in entries.windows(2) {
            let prev = pair[0].seq;
            let next = pair[1].seq;
            if next != prev + 1 {
                return Err(anyhow!(
                    "INVARIANT VIOLATION: WAL sequence gap/non-monotonic transition {} -> {}",
                    prev,
                    next
                ));
            }
        }

        Ok(())
    }

    /// Doc IDs present in the delta index must be unique.
    pub fn check_doc_id_uniqueness(delta_index: &DeltaIndex) -> Result<()> {
        let mut seen = HashSet::with_capacity(delta_index.documents.len());
        for doc_id in delta_index.documents.keys() {
            if !seen.insert(*doc_id) {
                return Err(anyhow!(
                    "INVARIANT VIOLATION: duplicate DocId {:?} in DeltaIndex",
                    doc_id
                ));
            }
        }
        Ok(())
    }

    /// A doc should not be simultaneously present in delta and immutable segments.
    pub fn check_delta_segment_disjoint(
        delta_index: &DeltaIndex,
        segment_doc_ids: &HashSet<DocId>,
    ) -> Result<()> {
        for doc_id in delta_index.documents.keys() {
            if segment_doc_ids.contains(doc_id) {
                return Err(anyhow!(
                    "INVARIANT VIOLATION: DocId {:?} present in both delta and segment sets",
                    doc_id
                ));
            }
        }
        Ok(())
    }

    /// Segment file bytes must match expected immutable hash.
    pub fn check_segment_immutability(path: &Path, expected_hash: u64) -> Result<()> {
        let bytes = fs::read(path)?;
        let actual = xxh3_64(&bytes);
        if actual != expected_hash {
            return Err(anyhow!(
                "INVARIANT VIOLATION: segment hash mismatch for {} (expected {}, got {})",
                path.display(),
                expected_hash,
                actual
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wal::entry::EventType;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;
    use crate::index::delta::{DocId, Document};

    #[test]
    fn test_invariant_consistency_check() {
        let mut delta = DeltaIndex::new(1024);
        let mut trie = PathTrie::new();

        let doc_id = DocId(1);
        let path = "/test/path.txt";
        
        delta.insert_document(Document { doc_id, path: path.to_string(), content_hash: 0 }, std::collections::HashMap::new()).unwrap();
        trie.insert(path, doc_id);

        assert!(InvariantChecker::check_cross_index_consistency(&delta, &trie).is_ok());
    }

    #[test]
    fn test_invariant_consistency_violation() {
        let mut delta = DeltaIndex::new(1024);
        let trie = PathTrie::new(); // Empty trie

        let doc_id = DocId(1);
        let path = "/test/path.txt";
        
        delta.insert_document(Document { doc_id, path: path.to_string(), content_hash: 0 }, std::collections::HashMap::new()).unwrap();
        // Skip trie insert to cause violation

        assert!(InvariantChecker::check_cross_index_consistency(&delta, &trie).is_err());
    }

    #[test]
    fn test_invariant_wal_monotonicity_violation() {
        let mut e1 = WalEntry::new_test();
        e1.seq = 1;
        e1.event_type = EventType::Created;
        let mut e2 = WalEntry::new_test();
        e2.seq = 2;
        e2.event_type = EventType::Modified;
        let mut e3 = WalEntry::new_test();
        e3.seq = 4; // gap
        e3.event_type = EventType::Deleted;

        let entries = vec![e1, e2, e3];
        assert!(InvariantChecker::check_wal_monotonicity(&entries).is_err());
    }

    #[test]
    fn test_invariant_doc_id_uniqueness_ok() {
        let mut delta = DeltaIndex::new(1024);
        delta.insert_document(
            Document { doc_id: DocId(1), path: "/a".to_string(), content_hash: 0 },
            std::collections::HashMap::new(),
        ).unwrap();
        delta.insert_document(
            Document { doc_id: DocId(2), path: "/b".to_string(), content_hash: 0 },
            std::collections::HashMap::new(),
        ).unwrap();

        assert!(InvariantChecker::check_doc_id_uniqueness(&delta).is_ok());
    }

    #[test]
    fn test_invariant_delta_segment_disjoint_violation() {
        let mut delta = DeltaIndex::new(1024);
        delta.insert_document(
            Document { doc_id: DocId(7), path: "/x".to_string(), content_hash: 0 },
            std::collections::HashMap::new(),
        ).unwrap();

        let mut seg = HashSet::new();
        seg.insert(DocId(7));

        assert!(InvariantChecker::check_delta_segment_disjoint(&delta, &seg).is_err());
    }

    #[test]
    fn test_invariant_segment_immutability_hash() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("segment.bin");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"segment-bytes").unwrap();

        let expected = xxh3_64(b"segment-bytes");
        assert!(InvariantChecker::check_segment_immutability(&path, expected).is_ok());
        assert!(InvariantChecker::check_segment_immutability(&path, expected.wrapping_add(1)).is_err());
    }
}
