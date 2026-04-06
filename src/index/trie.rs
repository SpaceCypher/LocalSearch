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
}

impl TrieNode {
    fn new() -> Self {
        Self {
            docs: RoaringBitmap::new(),
            children: HashMap::new(),
        }
    }
}

/// Path trie for scope-based queries
#[derive(Debug)]
pub struct PathTrie {
    root: TrieNode,
}

impl PathTrie {
    pub fn new() -> Self {
        Self {
            root: TrieNode::new(),
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
}
