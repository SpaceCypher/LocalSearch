use crate::index::delta::DocId;
use std::collections::{HashMap, HashSet};

/// A trigram packed into one integer (three 21-bit code points), which is
/// far smaller than a `String` per trigram per document.
type Trigram = u64;

/// Trigram-based fuzzy index using Jaccard similarity
pub struct TrigramIndex {
    // Map from trigram → set of DocIds containing that trigram
    trigram_map: HashMap<Trigram, HashSet<DocId>>,
    // Map from DocId → sorted, distinct trigrams of that document's filename
    doc_trigrams: HashMap<DocId, Box<[Trigram]>>,
}

impl TrigramIndex {
    pub fn new() -> Self {
        TrigramIndex {
            trigram_map: HashMap::new(),
            doc_trigrams: HashMap::new(),
        }
    }

    pub fn insert(&mut self, filename: &str, doc_id: DocId) {
        // Re-inserting a document replaces its previous trigrams
        self.remove(doc_id);
        let trigrams = extract_trigrams(filename);

        // Add document to each trigram's posting list
        for trigram in trigrams.iter() {
            self.trigram_map.entry(*trigram).or_default().insert(doc_id);
        }

        // Store trigrams for this document
        self.doc_trigrams.insert(doc_id, trigrams);
    }

    pub fn search_jaccard(&self, query: &str, threshold: f32) -> Vec<DocId> {
        let query_trigrams = extract_trigrams(query);
        if query_trigrams.is_empty() {
            return Vec::new();
        }

        // Only documents sharing at least one trigram can have non-zero similarity
        let mut candidates: HashSet<DocId> = HashSet::new();
        for trigram in query_trigrams.iter() {
            if let Some(docs) = self.trigram_map.get(trigram) {
                candidates.extend(docs.iter().copied());
            }
        }

        let mut results = Vec::new();
        for doc_id in candidates {
            let Some(doc_trigrams) = self.doc_trigrams.get(&doc_id) else {
                continue;
            };
            let jaccard = compute_jaccard(&query_trigrams, doc_trigrams);
            if jaccard >= threshold {
                results.push(doc_id);
            }
        }

        results
    }

    pub fn remove(&mut self, doc_id: DocId) {
        let Some(trigrams) = self.doc_trigrams.remove(&doc_id) else {
            return;
        };
        for trigram in trigrams.iter() {
            if let Some(docs) = self.trigram_map.get_mut(trigram) {
                docs.remove(&doc_id);
                if docs.is_empty() {
                    self.trigram_map.remove(trigram);
                }
            }
        }
    }
}

/// Extract the sorted, distinct trigrams of a string (case-insensitive)
fn extract_trigrams(s: &str) -> Box<[Trigram]> {
    let chars: Vec<char> = s.to_lowercase().chars().collect();
    let mut trigrams: Vec<Trigram> = chars
        .windows(3)
        .map(|w| ((w[0] as u64) << 42) | ((w[1] as u64) << 21) | w[2] as u64)
        .collect();
    trigrams.sort_unstable();
    trigrams.dedup();
    trigrams.into_boxed_slice()
}

/// Compute Jaccard similarity between two sorted, distinct trigram lists
fn compute_jaccard(a: &[Trigram], b: &[Trigram]) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }

    let (mut i, mut j, mut intersection) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                intersection += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let union = a.len() + b.len() - intersection;

    if union == 0 {
        return 0.0;
    }

    intersection as f32 / union as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trigram_jaccard_above_threshold_matches() {
        let mut idx = TrigramIndex::new();
        idx.insert("finder", DocId(1));
        // "finde" shares {fin, ind, nde} with "finder" → Jaccard > 0.5
        let results = idx.search_jaccard("finde", 0.5);
        assert!(results.contains(&DocId(1)));
    }

    #[test]
    fn test_trigram_no_match_below_threshold() {
        let mut idx = TrigramIndex::new();
        idx.insert("finder", DocId(1));
        // "xyz" shares no trigrams with "finder" → Jaccard = 0
        let results = idx.search_jaccard("xyz", 0.5);
        assert!(!results.contains(&DocId(1)));
    }

    #[test]
    fn test_trigram_exact_match() {
        let mut idx = TrigramIndex::new();
        idx.insert("test", DocId(1));
        // Exact match should have Jaccard = 1.0
        let results = idx.search_jaccard("test", 0.9);
        assert!(results.contains(&DocId(1)));
    }
}
