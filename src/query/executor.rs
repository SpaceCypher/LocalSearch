use crate::query::parser::Query;
use crate::query::ranker::Ranker;
use crate::index::delta::{DeltaIndex, DocId};
use crate::index::bktree::BkTree;
use crate::index::trie::PathTrie;
use crate::index::trigram::TrigramIndex;
use std::collections::{HashSet, HashMap};

use crate::query::spotlight_fallback::ResultSource;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub doc_id: DocId,
    pub path: String,
    pub score: f32,
    pub source: ResultSource,
    /// False when the document matched only through approximate (typo,
    /// phonetic, trigram) expansion of the query
    pub exact: bool,
}

const MAX_RESULTS: usize = 100;

pub struct QueryExecutor {
    pub(crate) delta_index: DeltaIndex,
    /// The on-disk index, if one has been written, and which of its
    /// documents have since been superseded or removed
    pub(crate) base: Option<crate::index::base::BaseIndex>,
    pub(crate) base_dead: crate::index::base::Bitset,
    pub(crate) bk_tree: BkTree,
    pub(crate) path_trie: PathTrie,
    pub(crate) trigram_index: TrigramIndex,
    pub(crate) phonetic_index: HashMap<String, Vec<String>>, // phonetic code → terms
    pub ranker: Ranker,
    pub spotlight_fallback: crate::query::spotlight_fallback::SpotlightFallback,
    pub is_warming: bool,
}

impl QueryExecutor {
    pub fn new(
        delta_index: DeltaIndex,
        bk_tree: BkTree,
        path_trie: PathTrie,
        trigram_index: TrigramIndex,
        phonetic_index: HashMap<String, Vec<String>>,
    ) -> Self {
        let total_docs = delta_index.live_doc_count().max(1) as u64;
        let avg_doc_len = delta_index.avg_doc_len();
        Self {
            delta_index,
            base: None,
            base_dead: crate::index::base::Bitset::default(),
            bk_tree,
            path_trie,
            trigram_index,
            phonetic_index,
            ranker: Ranker::new(total_docs, avg_doc_len),
            spotlight_fallback: crate::query::spotlight_fallback::SpotlightFallback::new(),
            is_warming: false,
        }
    }

    /// Recalibrate BM25 against the current corpus. Call after the index changes.
    pub fn refresh_stats(&mut self) {
        self.ranker.scorer.set_stats(
            self.delta_index.live_doc_count() as u64,
            self.delta_index.avg_doc_len(),
        );
    }

    pub fn execute(&self, query: Query, cancel_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>) -> Result<Vec<SearchResult>, String> {
        // Check cancellation early
        let is_cancelled = || {
            cancel_token.as_ref().map_or(false, |t| t.load(std::sync::atomic::Ordering::Relaxed))
        };

        let mut merged_scores: HashMap<DocId, (f32, bool)> = HashMap::new();

        // Step 0: Prefix cache seeds candidates for short single-token queries.
        // They are ranked with everything else in Step 3 rather than returned as-is.
        if query.tokens.len() == 1 && query.scope.is_none() && query.filters.is_empty() {
            let prefix = query.tokens[0].to_lowercase();
            if let Some(doc_ids) = self.path_trie.prefix_cache_lookup(&prefix) {
                for doc_id in doc_ids {
                    let Some(doc) = self.delta_index.documents.get(&doc_id) else { continue };
                    let filename = doc.path.rsplit('/').next().unwrap_or("").to_lowercase();
                    if filename.starts_with(&prefix) {
                        merged_scores.insert(doc_id, (1.0, true));
                    }
                }
            }
        }

        // Step 1: Collect results from multiple providers
        use crate::query::provider::{ResultProvider, KeywordProvider, TrigramProvider, ScoreNormalizer, RawResult};
        
        let providers: Vec<Box<dyn ResultProvider>> = vec![
            Box::new(KeywordProvider::new(
                &self.delta_index,
                self.base.as_ref().map(|base| (base, &self.base_dead)),
                &self.bk_tree,
                &self.phonetic_index,
                &self.ranker.scorer,
            )),
            Box::new(TrigramProvider::new(&self.trigram_index, 0.5)),
        ];

        let mut all_provider_results: Vec<Vec<RawResult>> = Vec::new();
        for provider in providers {
            if is_cancelled() { return Ok(Vec::new()); }
            let mut results = provider.provide(&query);
            ScoreNormalizer::normalize(&mut results);
            all_provider_results.push(results);
        }

        // Step 2: Merge results and apply bonuses
        for results in all_provider_results {
            for res in results {
                let entry = merged_scores.entry(res.doc_id).or_insert((0.0, false));
                entry.0 += res.score;
                entry.1 |= res.exact;
            }
        }

        // Apply Scope Filter if present
        let scope_doc_ids = if let Some(ref scope_path) = query.scope {
            Some(self.path_trie.scope_query(scope_path))
        } else {
            None
        };

        // Step 3: Global Ranking Signals & FSI Filter
        let query_token_count = query.tokens.len();
        let mut final_results = Vec::new();

        for (doc_id, (base_score, exact)) in merged_scores {
            if is_cancelled() { break; }
            
            // FSI Filter: Document Validity & Volume Isolation
            // 1. Check if document is deleted in DeltaIndex
            if self.delta_index.is_deleted(doc_id) {
                continue;
            }

            // 2. Placeholder for Volume Isolation (Task 40 will implement VolumeMonitor)
            if !self.is_path_accessible(&doc_id) {
                continue;
            }

            // Scope filter check
            if let Some(ref scope_ids) = scope_doc_ids {
                if !scope_ids.contains(doc_id.0 as u32) {
                    continue;
                }
            }

            let mut final_score = base_score;

            // Bonuses & Global Ranking Signals
            if let Some(doc) = self.delta_index.documents.get(&doc_id) {
                // Apply Directory Proximity Boost
                final_score *= self.ranker.calculate_proximity_boost(&doc.path);

                let lower_path = doc.path.to_lowercase();
                let mut matched_query_tokens = 0;
                for token in &query.tokens {
                    if lower_path.contains(&token.to_lowercase()) {
                        final_score += 0.2; // Normalized bonus
                        matched_query_tokens += 1;
                    }
                }

                // Multi-token match bonus (heuristic)
                if query_token_count > 1 && matched_query_tokens > 1 {
                    let match_ratio = matched_query_tokens as f32 / query_token_count as f32;
                    final_score += 1.0 * match_ratio;
                }

                final_results.push(SearchResult {
                    doc_id,
                    path: doc.path.clone(),
                    score: final_score,
                    source: ResultSource::LocalIndex,
                    exact,
                });
            }
        }

        // Sort and truncate
        final_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        final_results.truncate(MAX_RESULTS); 

        // Step 4: Spotlight Fallback while the local index is still warming up
        if self.is_warming && final_results.len() < 5 && !query.tokens.is_empty() {
            let query_str = query.tokens.join(" ");
            let mut spotlight_results = self.spotlight_fallback.query(&query_str);
            
            // Deduplicate: don't add if path already in final_results
            let existing_paths: HashSet<_> = final_results.iter().map(|r| &r.path).collect();
            spotlight_results.retain(|r| !existing_paths.contains(&r.path));
            
            final_results.extend(spotlight_results);
            final_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
            final_results.truncate(MAX_RESULTS);
        }

        Ok(final_results)
    }

    fn is_path_accessible(&self, doc_id: &DocId) -> bool {
        // Verification logic for FSI Filter
        self.delta_index.documents.get(doc_id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::delta::{Document, FIELD_FILENAME};
    use crate::query::parser::Tokenizer;
    use crate::query::phonetic::double_metaphone;
    use crate::index::delta::Posting;

    fn build_test_executor(docs: &[(&str, &str)]) -> QueryExecutor {
        let mut delta_index = DeltaIndex::new(50 * 1024 * 1024);
        let mut bk_tree = BkTree::new();
        let mut path_trie = PathTrie::new();
        let mut trigram_index = TrigramIndex::new();
        let mut phonetic_index: HashMap<String, Vec<String>> = HashMap::new();
        let tokenizer = Tokenizer::new();

        for (doc_id, (path, content)) in docs.iter().enumerate() {
            let doc_id = DocId((doc_id + 1) as u64);
            
            let tokens = tokenizer.tokenize(content);
            let path_tokens = tokenizer.tokenize(path);
            
            for token in &tokens {
                bk_tree.insert(&token.term);
            }
            
            for token in &path_tokens {
                bk_tree.insert(&token.term);
                trigram_index.insert(&token.term, doc_id);
                
                let phonetic_code = double_metaphone(&token.term);
                let entry = phonetic_index
                    .entry(phonetic_code.primary)
                    .or_insert_with(Vec::new);
                if !entry.contains(&token.term) {
                    entry.push(token.term.clone());
                }
            }
            
            let mut postings = HashMap::new();
            for token in tokens {
                postings.insert(
                    token.term.clone(),
                    Posting {
                        doc_id,
                        term_freq: 1,
                        field_mask: FIELD_FILENAME,
                    },
                );
            }
            
            for token in path_tokens {
                postings.entry(token.term.clone()).or_insert_with(|| Posting {
                    doc_id,
                    term_freq: 1,
                    field_mask: FIELD_FILENAME,
                });
            }
            
            let doc = Document {
                doc_id,
                path: path.to_string(),
                content_hash: 0,
                ..Default::default()
            };
            delta_index.insert_document(doc, postings).unwrap();
            path_trie.insert(path, doc_id);
        }

        QueryExecutor::new(delta_index, bk_tree, path_trie, trigram_index, phonetic_index)
    }

    #[test]
    fn test_executor_end_to_end() {
        let executor = build_test_executor(&[
            ("/test/quarterly_report.pdf", "quarterly report Q3"),
            ("/test/budget.xlsx", "budget 2024"),
            ("/test/notes.txt", "meeting notes"),
        ]);

        let results = executor.execute(Query::parse("quartely report").unwrap(), None).unwrap();
        assert_eq!(results[0].path, "/test/quarterly_report.pdf");
    }

    #[test]
    fn test_phonetic_expansion_fires_when_bk_returns_few() {
        let executor = build_test_executor(&[
            ("/test/meyer_report.pdf", "quarterly report"),
            ("/test/jones_notes.pdf", "annual notes"),
        ]);

        let results = executor.execute(Query::parse("mayer").unwrap(), None).unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.path.contains("meyer")));
    }

    #[test]
    fn test_bm25_stats_come_from_index() {
        let mut executor = build_test_executor(&[
            ("/test/a.txt", "alpha"),
            ("/test/b.txt", "beta"),
            ("/test/c.txt", "gamma"),
        ]);
        executor.refresh_stats();

        // idf(doc_freq = total_docs) is ln(1 + 0.5 / (N + 0.5)); with N = 3 that is
        // far from what a hardcoded N = 1000 would give.
        let idf = executor.ranker.scorer.idf(3);
        assert!((idf - (1.0f32 + 0.5 / 3.5).ln()).abs() < 1e-5, "idf = {idf}");
    }

    #[test]
    fn test_prefix_cache_hits_are_ranked_not_flat() {
        let mut executor = build_test_executor(&[
            ("/test/quarterly_report.pdf", "data"),
            ("/test/quiz.txt", "data"),
            ("/quotes/other.txt", "data"),
        ]);
        executor.path_trie.rebuild_prefix_cache(10);
        let results = executor.execute(Query::parse("qu").unwrap(), None).unwrap();

        // Only files whose own name starts with the prefix are seeded by the cache;
        // "/quotes/other.txt" merely lives under a matching directory.
        assert!(results.iter().all(|r| !r.path.ends_with("other.txt") || r.score < 1.0));
        assert!(results.iter().any(|r| r.path.ends_with("quiz.txt")));
        assert!(results.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn test_executor_uses_prefix_cache() {
        let mut executor = build_test_executor(&[
            ("/test/quarterly_report.pdf", "data"),
            ("/test/budget.xlsx", "data"),
        ]);
        
        executor.path_trie.rebuild_prefix_cache(10);
        let results = executor.execute(Query::parse("qu").unwrap(), None).unwrap();
        
        assert!(!results.is_empty());
        assert_eq!(results[0].path, "/test/quarterly_report.pdf");
        assert!(results[0].score >= 1.0);
    }
}
