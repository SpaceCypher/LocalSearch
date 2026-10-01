use crate::index::base::{BaseIndex, Bitset};
use crate::index::delta::{DocId, DeltaIndex};
use crate::query::parser::Query;
use crate::query::ranker::BM25Scorer;
use std::collections::HashMap;

/// A raw search result before normalization and final ranking.
#[derive(Debug, Clone)]
pub struct RawResult {
    pub doc_id: DocId,
    pub score: f32,
    /// The document contains a query term as typed (after stemming), as
    /// opposed to matching only through typo/phonetic/trigram expansion
    pub exact: bool,
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

/// Provides results based on BM25 keyword matching over the on-disk base
/// (minus its dead documents) and the in-memory delta.
pub struct KeywordProvider<'a> {
    delta_index: &'a DeltaIndex,
    base: Option<(&'a BaseIndex, &'a Bitset)>,
    bk_tree: &'a crate::index::bktree::BkTree,
    phonetic_index: &'a HashMap<String, Vec<String>>,
    scorer: &'a BM25Scorer,
}

impl<'a> KeywordProvider<'a> {
    pub fn new(
        delta_index: &'a DeltaIndex,
        base: Option<(&'a BaseIndex, &'a Bitset)>,
        bk_tree: &'a crate::index::bktree::BkTree,
        phonetic_index: &'a HashMap<String, Vec<String>>,
        scorer: &'a BM25Scorer,
    ) -> Self {
        Self {
            delta_index,
            base,
            bk_tree,
            phonetic_index,
            scorer,
        }
    }
}

impl<'a> ResultProvider for KeywordProvider<'a> {
    fn name(&self) -> &'static str { "keyword" }

    fn provide(&self, query: &Query) -> Vec<RawResult> {
        // (term, weight): exact terms count fully, fuzzy/phonetic expansions half
        let mut expanded_terms: Vec<(String, f32)> = Vec::new();
        
        // Expansion logic (moved from executor.rs)
        // Query tokens arrive already normalised and stemmed by `Query::parse`.
        // They must not be stemmed again: stemming is not idempotent
        // ("everywhere" -> "everywher" -> "everywh"), and the second pass
        // produced a term that is not in the index.
        for term in &query.tokens {
            {
                let token = crate::query::parser::Token { term: term.clone(), position: 0 };
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
                // An expansion counts for less the further it is from what was
                // typed: one edit is probably the intended word, two edits or
                // a sound-alike is a long shot.
                for term in fuzzy_matches {
                    if term != token.term && !expanded_terms.iter().any(|(t, _)| t == &term) {
                        let weight = match crate::index::bktree::damerau_levenshtein(&token.term, &term) {
                            0 | 1 => 0.6,
                            2 => 0.25,
                            _ => 0.15,
                        };
                        expanded_terms.push((term, weight));
                    }
                }
            }
        }

        let mut doc_scores: HashMap<DocId, (f32, bool)> = HashMap::new();
        // (document, term frequency, field mask) for one term, from both parts
        let mut hits: Vec<(DocId, u32, u8)> = Vec::new();
        let mut decoded = Vec::new();
        for (term, weight) in expanded_terms {
            hits.clear();
            if let Some((base, dead)) = self.base {
                base.lookup(&term, &mut decoded);
                hits.extend(decoded.iter().filter(|entry| !dead.get(entry.0)).filter_map(|&(ordinal, mask, tf)| {
                    base.doc_id(ordinal).map(|doc_id| (doc_id, tf, mask))
                }));
            }
            if let Some(postings) = self.delta_index.postings(&term) {
                hits.extend(
                    postings
                        .iter()
                        .filter(|posting| !self.delta_index.is_deleted(posting.doc_id))
                        .map(|posting| (posting.doc_id, posting.term_freq, posting.field_mask)),
                );
            }

            let doc_freq = hits.len() as u64;
            for &(doc_id, term_freq, mask) in &hits {
                // A document missing from the table was removed after the base was written
                let Some(doc) = self.delta_index.documents.get(&doc_id) else { continue };
                let score = self.scorer.score(term_freq as u64, doc.doc_len.max(1) as u64, doc_freq);

                let entry = doc_scores.entry(doc_id).or_insert((0.0, false));
                entry.0 += score * weight * field_boost(mask);
                entry.1 |= weight >= 1.0;
            }
        }

        doc_scores.into_iter()
            .map(|(doc_id, (score, exact))| RawResult { doc_id, score, exact })
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
            .map(|(doc_id, score)| RawResult { doc_id, score, exact: false })
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
            RawResult { doc_id: DocId(1), score: 4.0, exact: true },
            RawResult { doc_id: DocId(2), score: 3.9, exact: true },
            RawResult { doc_id: DocId(3), score: 1.0, exact: true },
        ];
        ScoreNormalizer::normalize(&mut results);
        assert_eq!(results[0].score, 1.0);
        assert!((results[1].score - 0.975).abs() < 1e-6);
        assert_eq!(results[2].score, 0.25);
    }

    #[test]
    fn test_normalize_handles_empty_and_zero() {
        ScoreNormalizer::normalize(&mut []);
        let mut results = vec![RawResult { doc_id: DocId(1), score: 0.0, exact: true }];
        ScoreNormalizer::normalize(&mut results);
        assert_eq!(results[0].score, 0.0);
    }
}
