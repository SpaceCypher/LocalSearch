use crate::index::delta::{DocId, DeltaIndex};
use crate::query::parser::Query;
use crate::query::ranker::BM25Scorer;
use std::collections::HashMap;

/// A raw search result before normalization and final ranking.
#[derive(Debug, Clone)]
pub struct RawResult {
    pub doc_id: DocId,
    pub score: f32,
}

/// A trait for components that can provide search results for a query.
pub trait ResultProvider {
    /// Name of the provider (for debugging and weighting).
    fn name(&self) -> &'static str;

    /// Execute search and return raw results.
    /// Scores should be internal to the provider's logic (e.g. raw BM25).
    fn provide(&self, query: &Query) -> Vec<RawResult>;
}

// ─── KeywordProvider ──────────────────────────────────────────────────────────

/// Provides results based on BM25 keyword matching in the DeltaIndex.
pub struct KeywordProvider<'a> {
    delta_index: &'a DeltaIndex,
    bk_tree: &'a crate::index::bktree::BkTree,
    phonetic_index: &'a HashMap<String, Vec<String>>,
    scorer: &'a BM25Scorer,
    tokenizer: crate::query::parser::Tokenizer,
}

impl<'a> KeywordProvider<'a> {
    pub fn new(
        delta_index: &'a DeltaIndex,
        bk_tree: &'a crate::index::bktree::BkTree,
        phonetic_index: &'a HashMap<String, Vec<String>>,
        scorer: &'a BM25Scorer,
    ) -> Self {
        Self {
            delta_index,
            bk_tree,
            phonetic_index,
            scorer,
            tokenizer: crate::query::parser::Tokenizer::new(),
        }
    }
}

impl<'a> ResultProvider for KeywordProvider<'a> {
    fn name(&self) -> &'static str { "keyword" }

    fn provide(&self, query: &Query) -> Vec<RawResult> {
        let mut expanded_terms = Vec::new();
        let mut doc_lengths: HashMap<DocId, u64> = HashMap::new();

        for postings in self.delta_index.index.values() {
            for posting in postings {
                *doc_lengths.entry(posting.doc_id).or_insert(0) += posting.term_freq as u64;
            }
        }
        
        // Expansion logic (moved from executor.rs)
        for token_str in &query.tokens {
            let tokens = self.tokenizer.tokenize(token_str);
            for token in tokens {
                let mut fuzzy_matches = self.bk_tree.search(&token.term, 1);
                
                if fuzzy_matches.len() < 3 {
                    let phonetic_code = crate::query::phonetic::double_metaphone(&token.term);
                    if let Some(phonetic_terms) = self.phonetic_index.get(&phonetic_code.primary) {
                        for term in phonetic_terms {
                            if !fuzzy_matches.contains(term) {
                                fuzzy_matches.push(term.clone());
                            }
                        }
                    }
                }
                
                if fuzzy_matches.is_empty() {
                    expanded_terms.push(token.term.clone());
                } else {
                    expanded_terms.extend(fuzzy_matches);
                }
            }
        }

        let mut doc_scores: HashMap<DocId, f32> = HashMap::new();
        for term in expanded_terms {
            if let Some(posting_list) = self.delta_index.lookup(&term) {
                for posting in &posting_list.postings {
                    if self.delta_index.is_deleted(posting.doc_id) {
                        continue;
                    }

                    let doc_len = doc_lengths.get(&posting.doc_id).copied().unwrap_or(1);
                    
                    let score = self.scorer.score(
                        posting.term_freq as u64,
                        doc_len,
                        posting_list.postings.len() as u64,
                    );
                    
                    *doc_scores.entry(posting.doc_id).or_insert(0.0) += score;
                }
            }
        }

        doc_scores.into_iter()
            .map(|(doc_id, score)| RawResult { doc_id, score })
            .collect()
    }
}

// ─── TrigramProvider ──────────────────────────────────────────────────────────

use crate::index::trigram::TrigramIndex;

/// Provides results based on trigram Jaccard similarity.
pub struct TrigramProvider<'a> {
    index: &'a TrigramIndex,
    threshold: f32,
}

impl<'a> TrigramProvider<'a> {
    pub fn new(index: &'a TrigramIndex, threshold: f32) -> Self {
        Self { index, threshold }
    }
}

impl<'a> ResultProvider for TrigramProvider<'a> {
    fn name(&self) -> &'static str { "trigram" }

    fn provide(&self, query: &Query) -> Vec<RawResult> {
        let mut results = HashMap::new();
        
        for token in &query.tokens {
            let matches = self.index.search_jaccard(token, self.threshold);
            for doc_id in matches {
                // Score is simple match count or similar
                *results.entry(doc_id).or_insert(0.0) += 1.0;
            }
        }

        results.into_iter()
            .map(|(doc_id, score)| RawResult { doc_id, score })
            .collect()
    }
}

// ─── ScoreNormalizer ──────────────────────────────────────────────────────────

pub struct ScoreNormalizer;

impl ScoreNormalizer {
    /// Normalize scores to [0.0, 1.0] using min-max scaling.
    pub fn normalize(results: &mut [RawResult]) {
        if results.is_empty() { return; }

        let mut max_score = f32::MIN;
        let mut min_score = f32::MAX;

        for r in results.iter() {
            if r.score > max_score { max_score = r.score; }
            if r.score < min_score { min_score = r.score; }
        }

        let range = max_score - min_score;
        if range > 0.0 {
            for r in results.iter_mut() {
                r.score = (r.score - min_score) / range;
            }
        } else if max_score > 0.0 {
            // All results have same positive score
            for r in results.iter_mut() {
                r.score = 1.0;
            }
        }
    }
}
