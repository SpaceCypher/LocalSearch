use crate::query::parser::{Query, Tokenizer};
use crate::query::ranker::{BM25Scorer, Ranker};
use crate::query::phonetic::double_metaphone;
use crate::index::delta::{DeltaIndex, Document, DocId};
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
}

const MAX_RESULTS: usize = 100;

pub struct QueryExecutor {
    delta_index: DeltaIndex,
    bk_tree: BkTree,
    path_trie: PathTrie,
    trigram_index: TrigramIndex,
    phonetic_index: HashMap<String, Vec<String>>, // phonetic code → terms
    pub ranker: Ranker,
    tokenizer: Tokenizer,
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
        let total_docs = 1000; // TODO: get from index
        let avg_doc_len = 50.0; // TODO: calculate from index
        Self {
            delta_index,
            bk_tree,
            path_trie,
            trigram_index,
            phonetic_index,
            ranker: Ranker::new(total_docs, avg_doc_len),
            tokenizer: Tokenizer::new(),
            spotlight_fallback: crate::query::spotlight_fallback::SpotlightFallback::new(),
            is_warming: false,
        }
    }

    pub fn execute(&self, query: Query, cancel_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>) -> Result<Vec<SearchResult>, String> {
        // Check cancellation early
        let is_cancelled = || {
            cancel_token.as_ref().map_or(false, |t| t.load(std::sync::atomic::Ordering::Relaxed))
        };

        // Step 0: Check prefix cache (fast path)
        if query.tokens.len() == 1 && query.scope.is_none() && query.filters.is_empty() {
            if let Some(doc_ids) = self.path_trie.prefix_cache_lookup(&query.tokens[0]) {
                let results: Vec<SearchResult> = doc_ids.iter().filter_map(|doc_id| {
                    self.delta_index.documents.get(doc_id).map(|doc| SearchResult {
                        doc_id: *doc_id,
                        path: doc.path.clone(),
                        score: 1.0, 
                        source: ResultSource::LocalIndex,
                    })
                }).collect();
                
                if !results.is_empty() {
                    return Ok(results);
                }
            }
        }

        // Step 1: Collect results from multiple providers
        use crate::query::provider::{ResultProvider, KeywordProvider, TrigramProvider, ScoreNormalizer, RawResult};
        
        let providers: Vec<Box<dyn ResultProvider>> = vec![
            Box::new(KeywordProvider::new(&self.delta_index, &self.bk_tree, &self.phonetic_index, &self.ranker.scorer)),
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
        let mut merged_scores: HashMap<DocId, f32> = HashMap::new();
        
        for results in all_provider_results {
            for res in results {
                *merged_scores.entry(res.doc_id).or_insert(0.0) += res.score;
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

        for (doc_id, base_score) in merged_scores {
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
                for token in &query.tokens {
                    if lower_path.contains(&token.to_lowercase()) {
                        final_score += 0.2; // Normalized bonus
                    }
                }

                // Multi-token match bonus (heuristic)
                if query_token_count > 1 && final_score > 0.5 {
                    final_score += 0.3;
                }

                final_results.push(SearchResult {
                    doc_id,
                    path: doc.path.clone(),
                    score: final_score,
                    source: ResultSource::LocalIndex,
                });
            }
        }

        // Sort and truncate
        final_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        final_results.truncate(MAX_RESULTS); 

        // Step 4: Spotlight Fallback (if warming and results are few)
        if (self.is_warming || final_results.len() < 5) && !query.tokens.is_empty() {
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
    use crate::index::delta::FIELD_FILENAME;
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
                        positions: vec![token.position],
                    },
                );
            }
            
            for token in path_tokens {
                postings.entry(token.term.clone()).or_insert_with(|| Posting {
                    doc_id,
                    term_freq: 1,
                    field_mask: FIELD_FILENAME,
                    positions: vec![token.position],
                });
            }
            
            let doc = Document {
                doc_id,
                path: path.to_string(),
                content_hash: 0,
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
    fn test_executor_uses_prefix_cache() {
        let mut executor = build_test_executor(&[
            ("/test/quarterly_report.pdf", "data"),
            ("/test/budget.xlsx", "data"),
        ]);
        
        executor.path_trie.rebuild_prefix_cache(10);
        let results = executor.execute(Query::parse("qu").unwrap(), None).unwrap();
        
        assert!(!results.is_empty());
        assert_eq!(results[0].path, "/test/quarterly_report.pdf");
        assert_eq!(results[0].score, 1.0);
    }
}
