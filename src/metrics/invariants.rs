use crate::index::delta::DeltaIndex;
use crate::index::trie::PathTrie;
use anyhow::{Result, anyhow};
use std::path::Path;

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
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
