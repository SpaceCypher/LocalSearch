// Delta Index — In-memory inverted index with tombstone deletes
// Memory budget: 50MB
// Supports: insert, delete (tombstone), lookup

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Stable document identifier, never reused
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct DocId(pub u64);

/// Field mask bits for term occurrences
pub const FIELD_FILENAME: u8 = 1 << 0;
pub const FIELD_PATH: u8 = 1 << 1;
pub const FIELD_CONTENT: u8 = 1 << 2;
pub const FIELD_TAGS: u8 = 1 << 3;

/// Posting entry for a single term in a document
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Posting {
    pub doc_id: DocId,
    pub term_freq: u32,
    pub field_mask: u8,
    pub positions: Vec<u32>,
}

impl Posting {
    pub fn new(doc_id: DocId, term_freq: u32, field_mask: u8) -> Self {
        Self {
            doc_id,
            term_freq,
            field_mask,
            positions: Vec::new(),
        }
    }
}

/// Document metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub doc_id: DocId,
    pub path: String,
}

impl Document {
    #[cfg(test)]
    pub fn new_test(doc_id: DocId, path: &str) -> Self {
        Self {
            doc_id,
            path: path.to_string(),
        }
    }
}

/// Posting list for a term
#[derive(Debug, Clone)]
pub struct PostingList {
    pub postings: Vec<Posting>,
}

/// In-memory delta index
pub struct DeltaIndex {
    memory_budget: usize,
    /// Inverted index: term -> list of postings
    pub index: HashMap<String, Vec<Posting>>,
    /// Document metadata: doc_id -> document
    pub documents: HashMap<DocId, Document>,
    /// Tombstone set for deleted documents
    tombstones: HashSet<DocId>,
}

impl DeltaIndex {
    pub fn new(memory_budget: usize) -> Self {
        Self {
            memory_budget,
            index: HashMap::new(),
            documents: HashMap::new(),
            tombstones: HashSet::new(),
        }
    }

    pub fn insert_document(
        &mut self,
        doc: Document,
        postings: HashMap<String, Posting>,
    ) -> Result<(), String> {
        // Store document metadata
        self.documents.insert(doc.doc_id, doc);

        // Insert postings into inverted index
        for (term, posting) in postings {
            self.index
                .entry(term)
                .or_insert_with(Vec::new)
                .push(posting);
        }

        Ok(())
    }

    pub fn delete_document(&mut self, doc_id: DocId) -> Result<(), String> {
        // Add to tombstone set (don't actually remove from index)
        self.tombstones.insert(doc_id);
        Ok(())
    }

    pub fn is_deleted(&self, doc_id: DocId) -> bool {
        self.tombstones.contains(&doc_id)
    }

    pub fn lookup(&self, term: &str) -> Option<PostingList> {
        self.index.get(term).map(|postings| PostingList {
            postings: postings.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delta_insert_and_lookup() {
        let mut delta = DeltaIndex::new(50 * 1024 * 1024);
        let doc = Document::new_test(DocId(1), "/test/file.txt");
        let mut postings = HashMap::new();
        postings.insert(
            "quarterly".to_string(),
            Posting::new(DocId(1), 3, FIELD_FILENAME),
        );
        delta.insert_document(doc, postings).unwrap();

        let result = delta.lookup("quarterly");
        assert!(result.is_some());
        assert_eq!(result.unwrap().postings[0].doc_id, DocId(1));
    }

    #[test]
    fn test_delta_delete_tombstone() {
        let mut delta = DeltaIndex::new(50 * 1024 * 1024);
        // insert then delete
        delta
            .insert_document(Document::new_test(DocId(1), "/f"), HashMap::new())
            .unwrap();
        delta.delete_document(DocId(1)).unwrap();
        assert!(delta.is_deleted(DocId(1)));
    }
}
