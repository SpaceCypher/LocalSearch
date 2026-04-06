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
        let numerator = (self.total_docs - doc_freq) as f32 + 0.5;
        let denominator = doc_freq as f32 + 0.5;
        (numerator / denominator + 1.0).ln()
    }

    pub fn score(&self, term_freq: u64, doc_len: u64, doc_freq: u64) -> f32 {
        let idf = self.idf(doc_freq);
        let tf = term_freq as f32;
        let dl = doc_len as f32;
        
        let numerator = tf * (self.k1 + 1.0);
        let denominator = tf + self.k1 * (1.0 - self.b + self.b * dl / self.avg_doc_len);
        
        idf * (numerator / denominator)
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
}
