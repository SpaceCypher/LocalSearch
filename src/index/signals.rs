use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SignalDb {
    /// term -> frequency/click count
    pub hot_terms: HashMap<String, u32>,
    /// path prefix -> frequency/click count
    pub hot_paths: HashMap<String, u32>,
}

impl SignalDb {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_click(&mut self, term: &str, path: &str) {
        *self.hot_terms.entry(term.to_string()).or_insert(0) += 1;
        
        // Record path segments as prefixes
        let mut prefix = String::new();
        for segment in path.split('/') {
            if segment.is_empty() { continue; }
            prefix.push('/');
            prefix.push_str(segment);
            *self.hot_paths.entry(prefix.clone()).or_insert(0) += 1;
        }
    }

    pub fn get_top_terms(&self, limit: usize) -> Vec<String> {
        let mut terms: Vec<_> = self.hot_terms.iter().collect();
        terms.sort_by(|a, b| b.1.cmp(a.1));
        terms.into_iter().take(limit).map(|(k, _)| k.clone()).collect()
    }

    pub fn get_top_paths(&self, limit: usize) -> Vec<String> {
        let mut paths: Vec<_> = self.hot_paths.iter().collect();
        paths.sort_by(|a, b| b.1.cmp(a.1));
        paths.into_iter().take(limit).map(|(k, _)| k.clone()).collect()
    }
}
