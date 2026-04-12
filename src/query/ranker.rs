use std::path::{Path, PathBuf};
use std::sync::RwLock;

pub struct BM25Scorer {
    total_docs: u64,
    avg_doc_len: f32,
    k1: f32,
    b: f32,
}

impl BM25Scorer {
    pub fn new(total_docs: u64, avg_doc_len: f32) -> Self {
        Self {
            total_docs,
            avg_doc_len,
            k1: 1.2,
            b: 0.75,
        }
    }

    pub fn idf(&self, doc_freq: u64) -> f32 {
        let numerator = self.total_docs.saturating_sub(doc_freq) as f32 + 0.5;
        let denominator = doc_freq as f32 + 0.5;
        (numerator / denominator + 1.0).ln()
    }

    pub fn score(&self, term_freq: u64, doc_len: u64, doc_freq: u64) -> f32 {
        let idf = self.idf(doc_freq);
        let tf = term_freq as f32;
        let dl = doc_len as f32;
        let avg_dl = self.avg_doc_len.max(1.0);
        
        let numerator = tf * (self.k1 + 1.0);
        let denominator = tf + self.k1 * (1.0 - self.b + self.b * dl / avg_dl);
        
        idf * (numerator / denominator)
    }
}

pub struct Ranker {
    pub scorer: BM25Scorer,
    session_clicked_paths: RwLock<Vec<PathBuf>>,
}

impl Ranker {
    pub fn new(total_docs: u64, avg_doc_len: f32) -> Self {
        Self {
            scorer: BM25Scorer::new(total_docs, avg_doc_len),
            session_clicked_paths: RwLock::new(Vec::with_capacity(5)),
        }
    }

    /// Records a user click on a search result to update the session context.
    pub fn record_click(&self, path: PathBuf) {
        if let Ok(mut paths) = self.session_clicked_paths.write() {
            if let Some(pos) = paths.iter().position(|p| p == &path) {
                paths.remove(pos);
            }
            paths.insert(0, path);
            if paths.len() > 5 {
                paths.pop();
            }
        }
    }

    /// Calculates a multiplier boost based on directory proximity to recently clicked paths.
    /// η=1.5 if the file shares a parent directory with one of the most recent 3 clicks.
    pub fn calculate_proximity_boost(&self, doc_path: &str) -> f32 {
        let doc_path_p = Path::new(doc_path);
        let doc_parent = doc_path_p.parent();
        
        if let Ok(clicked_paths) = self.session_clicked_paths.read() {
            for clicked in clicked_paths.iter().take(3) {
                if let Some(clicked_parent) = clicked.parent() {
                    if doc_parent == Some(clicked_parent) {
                        return 1.5;
                    }
                }
            }
        }
        1.0
    }

    /// Helper for testing to check session state
    #[cfg(test)]
    pub fn clicked_paths_count(&self) -> usize {
        self.session_clicked_paths.read().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bm25_higher_freq_higher_score() {
        let scorer = BM25Scorer::new(1000, 50.0);
        let s1 = scorer.score(5, 100, 200); // tf=5
        let s2 = scorer.score(1, 100, 200); // tf=1
        assert!(s1 > s2);
    }

    #[test]
    fn test_bm25_rare_term_higher_idf() {
        let scorer = BM25Scorer::new(1000, 50.0);
        // term in 10 docs vs term in 900 docs
        let s_rare = scorer.idf(10);
        let s_common = scorer.idf(900);
        assert!(s_rare > s_common);
    }

    #[test]
    fn test_ranker_proximity_boost() {
        let ranker = Ranker::new(100, 10.0);
        let path1 = PathBuf::from("/Users/alice/Projects/App1/main.rs");
        let path2 = PathBuf::from("/Users/alice/Projects/App1/utils.rs");
        let path3 = PathBuf::from("/Users/alice/Documents/notes.txt");
        
        ranker.record_click(path1);
        
        // Sibling file should get boost
        assert_eq!(ranker.calculate_proximity_boost(p2_str(&path2)), 1.5);
        // Unrelated file should not
        assert_eq!(ranker.calculate_proximity_boost(p2_str(&path3)), 1.0);
    }

    #[test]
    fn test_ranker_click_history_order() {
        let ranker = Ranker::new(100, 10.0);
        let p1 = PathBuf::from("/p1");
        let p2 = PathBuf::from("/p2");
        
        ranker.record_click(p1.clone());
        ranker.record_click(p2.clone());
        
        let paths = ranker.session_clicked_paths.read().unwrap();
        assert_eq!(paths[0], p2);
        assert_eq!(paths[1], p1);
    }

    fn p2_str(p: &PathBuf) -> &str {
        p.to_str().unwrap()
    }
}

