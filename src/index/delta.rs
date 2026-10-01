// Delta Index — In-memory inverted index with tombstone deletes
// Memory budget: 50MB
// Supports: insert, delete (tombstone), lookup

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Stable document identifier, never reused
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
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
}

impl Posting {
    pub fn new(doc_id: DocId, term_freq: u32, field_mask: u8) -> Self {
        Self {
            doc_id,
            term_freq,
            field_mask,
        }
    }
}

/// Document metadata
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Document {
    pub doc_id: DocId,
    pub path: String,
    pub content_hash: u64,
    /// Modification time and size at index time, used to detect stale entries
    pub mtime_ns: u64,
    pub size: u64,
    /// Number of indexed tokens (filename + path + content), for BM25 length normalisation
    pub doc_len: u32,
    /// True once content extraction has been attempted for this version of the file
    pub content_indexed: bool,
}

impl Document {
    #[cfg(test)]
    pub fn new_test(doc_id: DocId, path: &str) -> Self {
        Self {
            doc_id,
            path: path.to_string(),
            ..Default::default()
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
    /// Sum of `doc_len` over live documents
    total_doc_len: u64,
    /// Approximate heap bytes held by postings and documents
    approx_bytes: usize,
}

/// Heap cost of one posting
fn posting_bytes(_posting: &Posting) -> usize {
    std::mem::size_of::<Posting>()
}

impl DeltaIndex {
    pub fn new(memory_budget: usize) -> Self {
        Self {
            memory_budget,
            index: HashMap::new(),
            documents: HashMap::new(),
            tombstones: HashSet::new(),
            total_doc_len: 0,
            approx_bytes: 0,
        }
    }

    /// Rebuild an index from persisted parts (see `Segment`)
    pub fn from_parts(
        memory_budget: usize,
        index: HashMap<String, Vec<Posting>>,
        documents: HashMap<DocId, Document>,
    ) -> Self {
        let total_doc_len = documents.values().map(|d| d.doc_len as u64).sum();
        let approx_bytes = index
            .iter()
            .map(|(term, postings)| term.len() + 48 + postings.iter().map(posting_bytes).sum::<usize>())
            .sum::<usize>()
            + documents.values().map(|d| d.path.len() + 96).sum::<usize>();
        Self {
            memory_budget,
            index,
            documents,
            tombstones: HashSet::new(),
            total_doc_len,
            approx_bytes,
        }
    }

    pub fn memory_budget(&self) -> usize {
        self.memory_budget
    }

    pub fn set_memory_budget(&mut self, memory_budget: usize) {
        self.memory_budget = memory_budget;
    }

    pub fn approx_bytes(&self) -> usize {
        self.approx_bytes
    }

    pub fn is_over_budget(&self) -> bool {
        self.approx_bytes >= self.memory_budget
    }

    /// Number of live (non-tombstoned) documents
    pub fn live_doc_count(&self) -> usize {
        self.documents.len().saturating_sub(self.tombstones.len())
    }

    /// Average indexed token count per live document
    pub fn avg_doc_len(&self) -> f32 {
        let n = self.live_doc_count();
        if n == 0 {
            return 1.0;
        }
        (self.total_doc_len as f32 / n as f32).max(1.0)
    }

    /// Add postings to a document that is already in the index (e.g. content
    /// extracted after the filename was indexed) and grow its `doc_len`.
    pub fn append_postings(
        &mut self,
        doc_id: DocId,
        postings: HashMap<String, Posting>,
        added_len: u32,
        content_hash: u64,
    ) {
        let Some(doc) = self.documents.get_mut(&doc_id) else {
            return;
        };
        doc.doc_len = doc.doc_len.saturating_add(added_len);
        doc.content_hash = content_hash;
        doc.content_indexed = true;
        self.total_doc_len += added_len as u64;
        self.push_postings(postings);
    }

    /// Mark a document's content as handled without adding postings
    pub fn mark_content_indexed(&mut self, doc_id: DocId) {
        if let Some(doc) = self.documents.get_mut(&doc_id) {
            doc.content_indexed = true;
        }
    }

    fn push_postings(&mut self, postings: HashMap<String, Posting>) {
        for (term, posting) in postings {
            self.approx_bytes += posting_bytes(&posting);
            match self.index.get_mut(&term) {
                Some(list) => list.push(posting),
                None => {
                    self.approx_bytes += term.len() + 48;
                    self.index.insert(term, vec![posting]);
                }
            }
        }
    }

    /// Physically remove documents and all of their postings. One pass over the
    /// index regardless of how many documents are removed, so callers batch.
    pub fn purge(&mut self, doc_ids: &HashSet<DocId>) {
        if doc_ids.is_empty() {
            return;
        }
        let mut freed = 0usize;
        self.index.retain(|term, postings| {
            postings.retain(|p| {
                let keep = !doc_ids.contains(&p.doc_id);
                if !keep {
                    freed += posting_bytes(p);
                }
                keep
            });
            if postings.is_empty() {
                freed += term.len() + 48;
            }
            !postings.is_empty()
        });
        for doc_id in doc_ids {
            if let Some(doc) = self.documents.remove(doc_id) {
                freed += doc.path.len() + 96;
                if !self.tombstones.remove(doc_id) {
                    self.total_doc_len = self.total_doc_len.saturating_sub(doc.doc_len as u64);
                }
            } else {
                self.tombstones.remove(doc_id);
            }
        }
        self.approx_bytes = self.approx_bytes.saturating_sub(freed);
    }

    /// Borrowing lookup (no clone of the posting list)
    pub fn postings(&self, term: &str) -> Option<&[Posting]> {
        self.index.get(term).map(|p| p.as_slice())
    }

    pub fn insert_document(
        &mut self,
        doc: Document,
        postings: HashMap<String, Posting>,
    ) -> Result<(), String> {
        // Store document metadata
        self.total_doc_len += doc.doc_len as u64;
        self.approx_bytes += doc.path.len() + 96;
        self.tombstones.remove(&doc.doc_id);
        self.documents.insert(doc.doc_id, doc);

        // Insert postings into inverted index
        self.push_postings(postings);

        Ok(())
    }

    pub fn delete_document(&mut self, doc_id: DocId) -> Result<(), String> {
        // Add to tombstone set (don't actually remove from index)
        if self.tombstones.insert(doc_id) {
            if let Some(doc) = self.documents.get(&doc_id) {
                self.total_doc_len = self.total_doc_len.saturating_sub(doc.doc_len as u64);
            }
        }
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

    #[test]
    fn test_delta_purge_removes_postings_and_stats() {
        let mut delta = DeltaIndex::new(50 * 1024 * 1024);
        for id in 1..=2u64 {
            let mut doc = Document::new_test(DocId(id), "/f");
            doc.doc_len = 4;
            let mut postings = HashMap::new();
            postings.insert("shared".to_string(), Posting::new(DocId(id), 1, FIELD_FILENAME));
            delta.insert_document(doc, postings).unwrap();
        }
        assert_eq!(delta.avg_doc_len(), 4.0);

        delta.purge(&[DocId(1)].into_iter().collect());

        assert_eq!(delta.live_doc_count(), 1);
        assert_eq!(delta.postings("shared").unwrap().len(), 1);
        assert_eq!(delta.avg_doc_len(), 4.0);

        delta.purge(&[DocId(2)].into_iter().collect());
        assert!(delta.postings("shared").is_none());
        assert_eq!(delta.approx_bytes(), 0);
    }
}
