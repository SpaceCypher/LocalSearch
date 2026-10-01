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
        // (term, weight): exact terms count fully, fuzzy/phonetic expansions half
        let mut expanded_terms: Vec<(String, f32)> = Vec::new();
        
        // Expansion logic (moved from executor.rs)
        for token_str in &query.tokens {
            let tokens = self.tokenizer.tokenize(token_str);
            for token in tokens {
                // Longer words tolerate two edits; short ones would match everything
                let max_distance = if token.term.chars().count() >= 5 { 2 } else { 1 };
                let mut fuzzy_matches = self.bk_tree.search(&token.term, max_distance);
                
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
                
                // The exact term always participates: content-only terms are
                // not in the BK-tree, so expansions alone would miss them.
                expanded_terms.push((token.term.clone(), 1.0));
                for term in fuzzy_matches {
                    if term != token.term && !expanded_terms.iter().any(|(t, _)| t == &term) {
                        expanded_terms.push((term, 0.5));
                    }
                }
            }
        }

        let mut doc_scores: HashMap<DocId, f32> = HashMap::new();
        for (term, weight) in expanded_terms {
            if let Some(postings) = self.delta_index.postings(&term) {
                let doc_freq = postings.len() as u64;
                for posting in postings {
                    if self.delta_index.is_deleted(posting.doc_id) {
                        continue;
                    }

                    let doc_len = self.delta_index.documents
                        .get(&posting.doc_id)
                        .map_or(1, |doc| doc.doc_len.max(1) as u64);
                    
                    let score = self.scorer.score(posting.term_freq as u64, doc_len, doc_freq);
                    
                    *doc_scores.entry(posting.doc_id).or_insert(0.0) +=
                        score * weight * field_boost(posting.field_mask);
                }
            }
        }

        doc_scores.into_iter()
            .map(|(doc_id, score)| RawResult { doc_id, score })
            .collect()
    }
}

/// A hit in the filename outweighs one in the directory path, which outweighs content.
fn field_boost(field_mask: u8) -> f32 {
    use crate::index::delta::{FIELD_FILENAME, FIELD_PATH};
    if field_mask & FIELD_FILENAME != 0 {
        3.0
    } else if field_mask & FIELD_PATH != 0 {
        1.5
    } else {
        1.0
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
    /// Scale scores into (0.0, 1.0] relative to the best result.
    ///
    /// Dividing by the maximum (rather than min-max scaling) keeps near-equal
    /// scores near-equal: min-max would stretch a negligible difference
    /// between two results into the full 0..1 range.
    pub fn normalize(results: &mut [RawResult]) {
        let max_score = results.iter().map(|r| r.score).fold(0.0f32, f32::max);
        if max_score > 0.0 {
            for r in results.iter_mut() {
                r.score /= max_score;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_preserves_ratios() {
        let mut results = vec![
            RawResult { doc_id: DocId(1), score: 4.0 },
            RawResult { doc_id: DocId(2), score: 3.9 },
            RawResult { doc_id: DocId(3), score: 1.0 },
        ];
        ScoreNormalizer::normalize(&mut results);
        assert_eq!(results[0].score, 1.0);
        assert!((results[1].score - 0.975).abs() < 1e-6);
        assert_eq!(results[2].score, 0.25);
    }

    #[test]
    fn test_normalize_handles_empty_and_zero() {
        ScoreNormalizer::normalize(&mut []);
        let mut results = vec![RawResult { doc_id: DocId(1), score: 0.0 }];
        ScoreNormalizer::normalize(&mut results);
        assert_eq!(results[0].score, 0.0);
    }
}
