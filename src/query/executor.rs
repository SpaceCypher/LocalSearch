use crate::query::parser::{Query, Tokenizer};
use crate::query::ranker::BM25Scorer;
use crate::index::delta::{DeltaIndex, Document, Posting, DocId};
use crate::index::bktree::BkTree;
use crate::index::trie::PathTrie;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub doc_id: DocId,
    pub path: String,
    pub score: f32,
}

pub struct QueryExecutor {
    delta_index: DeltaIndex,
    bk_tree: BkTree,
    path_trie: PathTrie,
    scorer: BM25Scorer,
    tokenizer: Tokenizer,
}

impl QueryExecutor {
    pub fn new(delta_index: DeltaIndex, bk_tree: BkTree, path_trie: PathTrie) -> Self {
        let total_docs = 1000; // TODO: get from index
        let avg_doc_len = 50.0; // TODO: calculate from index
        Self {
            delta_index,
            bk_tree,
            path_trie,
            scorer: BM25Scorer::new(total_docs, avg_doc_len),
            tokenizer: Tokenizer::new(),
        }
    }

    pub fn execute(&self, query: Query) -> Result<Vec<SearchResult>, String> {
        // Step 1: Tokenize query terms
        let mut expanded_terms = Vec::new();
        for token_str in &query.tokens {
            // Tokenize to get stemmed form
            let tokens = self.tokenizer.tokenize(token_str);
            for token in tokens {
                // Fuzzy expand using BK-tree (max edit distance 1)
                let fuzzy_matches = self.bk_tree.search(&token.term, 1);
                if fuzzy_matches.is_empty() {
                    // No fuzzy matches, use original term
                    expanded_terms.push(token.term.clone());
                } else {
                    // Add all fuzzy matches
                    expanded_terms.extend(fuzzy_matches);
                }
            }
        }

        // Step 2: Apply scope filter if present
        let scope_doc_ids = if let Some(ref scope_path) = query.scope {
            Some(self.path_trie.scope_query(scope_path))
        } else {
            None
        };

        // Step 3: Query delta index for each expanded term
        let mut doc_scores: HashMap<DocId, f32> = HashMap::new();
        for term in expanded_terms {
            if let Some(posting_list) = self.delta_index.lookup(&term) {
                for posting in &posting_list.postings {
                    // Skip if document is deleted
                    if self.delta_index.is_deleted(posting.doc_id) {
                        continue;
                    }
                    
                    // Skip if not in scope
                    if let Some(ref scope_ids) = scope_doc_ids {
                        if !scope_ids.contains(posting.doc_id.0 as u32) {
                            continue;
                        }
                    }
                    
                    // Calculate BM25 score
                    let score = self.scorer.score(
                        posting.term_freq as u64,
                        100, // TODO: get actual doc length
                        posting_list.postings.len() as u64,
                    );
                    
                    // Accumulate scores for multi-term queries
                    *doc_scores.entry(posting.doc_id).or_insert(0.0) += score;
                }
            }
        }

        // Step 4: Get document metadata and create results
        let mut results: Vec<SearchResult> = doc_scores
            .into_iter()
            .filter_map(|(doc_id, score)| {
                // Get document path from delta index
                self.delta_index.documents.get(&doc_id).map(|doc| SearchResult {
                    doc_id,
                    path: doc.path.clone(),
                    score,
                })
            })
            .collect();

        // Step 5: Sort by score (descending) and return top-K
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        results.truncate(20); // Top 20 results

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::delta::FIELD_FILENAME;

    fn build_test_executor(docs: &[(&str, &str)]) -> QueryExecutor {
        let mut delta_index = DeltaIndex::new(50 * 1024 * 1024);
        let mut bk_tree = BkTree::new();
        let mut path_trie = PathTrie::new();
        let tokenizer = Tokenizer::new();

        for (doc_id, (path, content)) in docs.iter().enumerate() {
            let doc_id = DocId((doc_id + 1) as u64);
            
            // Tokenize content
            let tokens = tokenizer.tokenize(content);
            
            // Add terms to BK-tree for fuzzy matching
            for token in &tokens {
                bk_tree.insert(&token.term);
            }
            
            // Create postings
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
            
            // Insert into delta index
            let doc = Document {
                doc_id,
                path: path.to_string(),
            };
            delta_index.insert_document(doc, postings).unwrap();
            
            // Insert into path trie
            path_trie.insert(path, doc_id);
        }

        QueryExecutor::new(delta_index, bk_tree, path_trie)
    }

    #[test]
    fn test_executor_end_to_end() {
        // Index 3 files, query for one by name with 1 typo
        let executor = build_test_executor(&[
            ("/test/quarterly_report.pdf", "quarterly report Q3"),
            ("/test/budget.xlsx", "budget 2024"),
            ("/test/notes.txt", "meeting notes"),
        ]);

        let results = executor.execute(Query::parse("quartely report").unwrap()).unwrap();
        assert_eq!(results[0].path, "/test/quarterly_report.pdf");
    }
}
