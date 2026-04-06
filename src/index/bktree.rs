// BK-Tree for fuzzy string matching
// Uses Damerau-Levenshtein edit distance on Unicode code points

use std::collections::HashMap;

/// BK-Tree node
#[derive(Debug)]
struct Node {
    word: String,
    children: HashMap<usize, Box<Node>>,
}

/// BK-Tree for efficient fuzzy string search
#[derive(Debug)]
pub struct BkTree {
    root: Option<Box<Node>>,
}

impl BkTree {
    pub fn new() -> Self {
        Self { root: None }
    }

    pub fn insert(&mut self, word: &str) {
        let word = word.to_string();

        if self.root.is_none() {
            self.root = Some(Box::new(Node {
                word,
                children: HashMap::new(),
            }));
            return;
        }

        let mut current = self.root.as_mut().unwrap();
        loop {
            let distance = damerau_levenshtein(&current.word, &word);

            if distance == 0 {
                // Word already exists
                return;
            }

            // Check if child exists at this distance
            if current.children.contains_key(&distance) {
                current = current.children.get_mut(&distance).unwrap();
            } else {
                // Insert new child
                current.children.insert(
                    distance,
                    Box::new(Node {
                        word,
                        children: HashMap::new(),
                    }),
                );
                return;
            }
        }
    }

    pub fn search(&self, query: &str, max_distance: usize) -> Vec<String> {
        let mut results = Vec::new();

        if let Some(root) = &self.root {
            self.search_recursive(root, query, max_distance, &mut results);
        }

        results
    }

    fn search_recursive(
        &self,
        node: &Node,
        query: &str,
        max_distance: usize,
        results: &mut Vec<String>,
    ) {
        let distance = damerau_levenshtein(&node.word, query);

        if distance <= max_distance {
            results.push(node.word.clone());
        }

        // Search children within the distance range
        let min_child_distance = distance.saturating_sub(max_distance);
        let max_child_distance = distance + max_distance;

        for (&child_distance, child) in &node.children {
            if child_distance >= min_child_distance && child_distance <= max_child_distance {
                self.search_recursive(child, query, max_distance, results);
            }
        }
    }
}

/// Compute Damerau-Levenshtein edit distance between two strings
/// Operates on Unicode code points, not bytes
fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    // Create distance matrix
    let mut d = vec![vec![0; b_len + 1]; a_len + 1];

    // Initialize first column and row
    for i in 0..=a_len {
        d[i][0] = i;
    }
    for j in 0..=b_len {
        d[0][j] = j;
    }

    // Compute distances
    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };

            d[i][j] = std::cmp::min(
                std::cmp::min(
                    d[i - 1][j] + 1,     // deletion
                    d[i][j - 1] + 1,     // insertion
                ),
                d[i - 1][j - 1] + cost,  // substitution
            );

            // Transposition
            if i > 1 && j > 1 && a_chars[i - 1] == b_chars[j - 2] && a_chars[i - 2] == b_chars[j - 1]
            {
                d[i][j] = std::cmp::min(d[i][j], d[i - 2][j - 2] + cost);
            }
        }
    }

    d[a_len][b_len]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bktree_exact_match() {
        let mut tree = BkTree::new();
        tree.insert("hello");
        assert_eq!(tree.search("hello", 0), vec!["hello"]);
    }

    #[test]
    fn test_bktree_edit_distance_1() {
        let mut tree = BkTree::new();
        tree.insert("hello");
        tree.insert("world");
        let results = tree.search("helo", 1); // 1 deletion
        assert!(results.contains(&"hello".to_string()));
        assert!(!results.contains(&"world".to_string()));
    }

    #[test]
    fn test_bktree_edit_distance_2() {
        let mut tree = BkTree::new();
        tree.insert("quarterly");
        let results = tree.search("quartely", 2); // 1 deletion
        assert!(results.contains(&"quarterly".to_string()));
    }

    #[test]
    fn test_bktree_unicode_safe() {
        // Must operate on Unicode code points, not bytes
        let mut tree = BkTree::new();
        tree.insert("café");
        let results = tree.search("cafe", 1);
        assert!(results.contains(&"café".to_string()));
    }
}
