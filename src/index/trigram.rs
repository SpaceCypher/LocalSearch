use crate::index::delta::DocId;
use std::collections::{HashMap, HashSet};

/// Trigram-based fuzzy index using Jaccard similarity
pub struct TrigramIndex {
    // Map from trigram → set of DocIds containing that trigram
    trigram_map: HashMap<String, HashSet<DocId>>,
    // Map from DocId → set of trigrams in that document's filename
    doc_trigrams: HashMap<DocId, HashSet<String>>,
}

impl TrigramIndex {
    pub fn new() -> Self {
        TrigramIndex {
            trigram_map: HashMap::new(),
            doc_trigrams: HashMap::new(),
        }
    }

    pub fn insert(&mut self, filename: &str, doc_id: DocId) {
        let trigrams = extract_trigrams(filename);
        
        // Store trigrams for this document
        self.doc_trigrams.insert(doc_id, trigrams.clone());
        
        // Add document to each trigram's posting list
        for trigram in trigrams {
            self.trigram_map
                .entry(trigram)
                .or_insert_with(HashSet::new)
                .insert(doc_id);
        }
    }

    pub fn search_jaccard(&self, query: &str, threshold: f32) -> Vec<DocId> {
        let query_trigrams = extract_trigrams(query);
        if query_trigrams.is_empty() {
            return Vec::new();
        }
        
        let mut results = Vec::new();
        
        // Check each document for Jaccard similarity
        for (doc_id, doc_trigrams) in &self.doc_trigrams {
            let jaccard = compute_jaccard(&query_trigrams, doc_trigrams);
            if jaccard >= threshold {
                results.push(*doc_id);
            }
        }
        
        results
    }
}

/// Extract trigrams from a string
fn extract_trigrams(s: &str) -> HashSet<String> {
    let s = s.to_lowercase();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 3 {
        return HashSet::new();
    }

    let mut trigrams = HashSet::new();
    
    for i in 0..=chars.len().saturating_sub(3) {
        let trigram: String = chars[i..i+3].iter().collect();
        trigrams.insert(trigram);
    }
    
    trigrams
}

/// Compute Jaccard similarity between two sets
fn compute_jaccard(set1: &HashSet<String>, set2: &HashSet<String>) -> f32 {
    if set1.is_empty() && set2.is_empty() {
        return 1.0;
    }
    
    let intersection = set1.intersection(set2).count();
    let union = set1.union(set2).count();
    
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
