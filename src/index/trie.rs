// Path Trie for efficient scope queries using Roaring Bitmaps
// Enables O(1) scope filtering per document

use roaring::RoaringBitmap;
use std::collections::HashMap;
use std::path::Path;

// Re-export DocId from delta module
use crate::index::delta::DocId;

/// Trie node for path components
#[derive(Debug)]
struct TrieNode {
    /// Documents at this exact path
    docs: RoaringBitmap,
    /// Child nodes (path component -> node)
    children: HashMap<String, TrieNode>,
    /// Precomputed top document IDs in this subtree
    top_docs: Vec<DocId>,
}

impl TrieNode {
    fn new() -> Self {
        Self {
            docs: RoaringBitmap::new(),
            children: HashMap::new(),
            top_docs: Vec::new(),
        }
    }
}

/// Path trie for scope-based queries
#[derive(Debug)]
pub struct PathTrie {
    root: TrieNode,
    /// Mapping from component prefix to top document IDs
    prefix_cache: HashMap<String, Vec<DocId>>,
}

impl PathTrie {
    pub fn new() -> Self {
        Self {
            root: TrieNode::new(),
            prefix_cache: HashMap::new(),
        }
    }

    pub fn insert(&mut self, path: &str, doc_id: DocId) {
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        
        let mut current = &mut self.root;
        
        // Traverse/create path in trie
        for component in &components {
            current = current
                .children
                .entry(component.to_string())
                .or_insert_with(TrieNode::new);
        }
        
        // Add document to the leaf node
        current.docs.insert(doc_id.0 as u32);
    }

    pub fn scope_query(&self, scope: &Path) -> RoaringBitmap {
        let scope_str = scope.to_str().unwrap_or("");
        let components: Vec<&str> = scope_str.split('/').filter(|s| !s.is_empty()).collect();
        
        // Navigate to the scope node
        let mut current = &self.root;
        for component in &components {
            match current.children.get(*component) {
                Some(node) => current = node,
                None => return RoaringBitmap::new(), // Scope not found
            }
        }
        
        // Collect all documents under this scope (recursive)
        self.collect_all_docs(current)
    }

    pub fn rebuild_prefix_cache(&mut self, top_k: usize) {
        // Clear existing cache
        self.prefix_cache.clear();
        
        // Recursively compute top_docs for each node and populate prefix_cache
        Self::rebuild_node_cache(&mut self.root, top_k, &mut self.prefix_cache);
    }

    fn rebuild_node_cache(
        node: &mut TrieNode,
        top_k: usize,
        prefix_cache: &mut HashMap<String, Vec<DocId>>,
    ) -> Vec<DocId> {
        let mut all_docs = Vec::new();

        // Add docs at this node
        for doc_id_u32 in node.docs.iter() {
            all_docs.push(DocId(doc_id_u32 as u64));
        }

        // Recursively process children
        for (name, child) in node.children.iter_mut() {
            let child_docs = Self::rebuild_node_cache(child, top_k, prefix_cache);
            all_docs.extend(child_docs);

            // Populate prefix cache for this component name
            // We only cache short prefixes to keep memory bounded
            let max_prefix = name.len().min(4);
            for i in 1..=max_prefix {
                let prefix = name[..i].to_lowercase();
                let entry = prefix_cache.entry(prefix).or_insert_with(Vec::new);
                
                // Add top child docs to this prefix
                for doc_id in &child.top_docs {
                    if !entry.contains(doc_id) {
                        entry.push(*doc_id);
                    }
                }
                entry.sort_by(|a, b| b.0.cmp(&a.0)); // DocId larger = more recent
                entry.truncate(top_k);
            }
        }

        // Sort by DocId descending and truncate
        all_docs.sort_by(|a, b| b.0.cmp(&a.0));
        all_docs.dedup();
        all_docs.truncate(top_k);
        
        node.top_docs = all_docs.clone();
        all_docs
    }

    pub fn prefix_cache_lookup(&self, prefix: &str) -> Option<Vec<DocId>> {
        self.prefix_cache.get(&prefix.to_lowercase()).cloned()
    }

    fn collect_all_docs(&self, node: &TrieNode) -> RoaringBitmap {
        let mut result = node.docs.clone();
        
        // Recursively collect from all children
        for child in node.children.values() {
            result |= self.collect_all_docs(child);
        }
        
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trie_scope_query_returns_correct_docs() {
        let mut trie = PathTrie::new();
        trie.insert("/Users/alice/Documents/report.pdf", DocId(1));
        trie.insert("/Users/alice/Desktop/todo.txt", DocId(2));
        trie.insert("/Users/bob/Documents/notes.md", DocId(3));

        let result = trie.scope_query(Path::new("/Users/alice/Documents"));
        assert!(result.contains(1u32));
        assert!(!result.contains(2u32));
        assert!(!result.contains(3u32));
    }

    #[test]
    fn test_trie_scope_query_includes_subdirectories() {
        let mut trie = PathTrie::new();
        trie.insert("/Users/alice/Documents/2024/report.pdf", DocId(1));
        trie.insert("/Users/alice/Documents/2023/old.pdf", DocId(2));
        trie.insert("/Users/alice/Desktop/todo.txt", DocId(3));

        let result = trie.scope_query(Path::new("/Users/alice/Documents"));
        assert!(result.contains(1u32));
        assert!(result.contains(2u32));
        assert!(!result.contains(3u32));
    }

    #[test]
    fn test_trie_scope_query_nonexistent_path() {
        let mut trie = PathTrie::new();
        trie.insert("/Users/alice/Documents/report.pdf", DocId(1));

        let result = trie.scope_query(Path::new("/Users/bob/Documents"));
        assert!(result.is_empty());
    }

    #[test]
    fn test_trie_multiple_docs_same_path() {
        let mut trie = PathTrie::new();
        trie.insert("/Users/alice/Documents/report1.pdf", DocId(1));
        trie.insert("/Users/alice/Documents/report2.pdf", DocId(2));
        trie.insert("/Users/alice/Documents/report3.pdf", DocId(3));

        let result = trie.scope_query(Path::new("/Users/alice/Documents"));
        assert!(result.contains(1u32));
        assert!(result.contains(2u32));
        assert!(result.contains(3u32));
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_prefix_cache_returns_precomputed_results() {
        let mut trie = PathTrie::new();
        trie.insert("/Users/alice/report.pdf", DocId(1));
        trie.insert("/Users/alice/readme.md", DocId(2));
        trie.insert("/Users/bob/random.txt", DocId(3));
        trie.rebuild_prefix_cache(100);

        let cached = trie.prefix_cache_lookup("re");
        assert!(cached.is_some());
        let hits = cached.unwrap();
        // "report.pdf" and "readme.md" start with "re"
        assert!(hits.contains(&DocId(1)));
        assert!(hits.contains(&DocId(2)));
        assert!(!hits.contains(&DocId(3)));
    }

    #[test]
    fn test_prefix_cache_15ms_warm() {
        let mut trie = PathTrie::new();
        for i in 0..1000 {
            trie.insert(&format!("/path/to/file_{}.txt", i), DocId(i as u64));
        }
        trie.rebuild_prefix_cache(100);

        let start = std::time::Instant::now();
        let _ = trie.prefix_cache_lookup("fi");
        let elapsed = start.elapsed();
        println!("Prefix cache lookup took: {:?}", elapsed);
        assert!(elapsed.as_millis() < 15);
    }
}
