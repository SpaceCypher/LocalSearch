use std::collections::HashSet;

/// Rank-Biased Overlap (RBO) implementation
/// Compares two ranked lists and returns a similarity score in [0, 1]
pub struct RankBiasedOverlap {
    p: f64, // Persistence parameter (typical values: 0.9 or 0.98)
}

impl RankBiasedOverlap {
    pub fn new(p: f64) -> Self {
        Self { p }
    }

    /// Calculates RBO score for two rankings
    pub fn calculate<T: Eq + std::hash::Hash>(&self, list1: &[T], list2: &[T]) -> f64 {
        let n1 = list1.len();
        let n2 = list2.len();
        let n = n1.max(n2);
        
        if n == 0 { return 1.0; }

        let mut intersection_count = 0;
        let mut set1 = HashSet::new();
        let mut set2 = HashSet::new();
        let mut sum_agreement = 0.0;
        
        for d in 1..=n {
            if d <= n1 {
                let item = &list1[d-1];
                if set2.contains(item) {
                    intersection_count += 1;
                }
                set1.insert(item);
            }
            
            if d <= n2 {
                let item = &list2[d-1];
                if set1.contains(item) {
                    intersection_count += 1;
                }
                set2.insert(item);
            }
            
            let agreement = (intersection_count as f64) / (d as f64);
            sum_agreement += agreement * self.p.powi((d - 1) as i32);
        }

        let rbo_min = (1.0 - self.p) * sum_agreement;
        let agreement_n = (intersection_count as f64) / (n as f64);
        rbo_min + (agreement_n * self.p.powi(n as i32))
    }
}

pub fn percentile(scores: &mut [f64], p: usize) -> f64 {
    if scores.is_empty() { return 0.0; }
    scores.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = (p * (scores.len() - 1)) / 100;
    scores[idx]
}

pub fn compute_rbo_all<T: Eq + std::hash::Hash>(
    baseline: &[Vec<T>],
    candidate: &[Vec<T>],
    p: f64,
) -> Vec<f64>
where
    T: Clone,
{
    let rbo = RankBiasedOverlap::new(p);
    baseline
        .iter()
        .zip(candidate.iter())
        .map(|(b, c)| rbo.calculate(b, c))
        .collect()
}

pub fn passes_shadow_threshold(scores: &mut [f64], min_p10: f64) -> bool {
    percentile(scores, 10) >= min_p10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rbo_identical_lists() {
        let rbo = RankBiasedOverlap::new(0.9);
        let list1 = vec!["a", "b", "c"];
        let list2 = vec!["a", "b", "c"];
        let score = rbo.calculate(&list1, &list2);
        assert!(score > 0.99); // Should be very close to 1.0
    }

    #[test]
    fn test_rbo_different_order() {
        let rbo = RankBiasedOverlap::new(0.9);
        let list1 = vec!["a", "b", "c"];
        let list2 = vec!["b", "a", "c"];
        let score1 = rbo.calculate(&list1, &list2);
        
        let list3 = vec!["c", "b", "a"];
        let score2 = rbo.calculate(&list1, &list3);
        
        assert!(score1 > score2); // Swapping top items should hurt more
    }

    #[test]
    fn test_shadow_ranking_p10_threshold() {
        let baseline = vec![
            vec!["a", "b", "c", "d"],
            vec!["w", "x", "y", "z"],
            vec!["m", "n", "o", "p"],
            vec!["r", "s", "t", "u"],
            vec!["k", "l", "q", "v"],
        ];

        let candidate = vec![
            vec!["a", "b", "d", "c"],
            vec!["w", "x", "z", "y"],
            vec!["m", "o", "n", "p"],
            vec!["r", "t", "s", "u"],
            vec!["k", "l", "v", "q"],
        ];

        let mut scores = compute_rbo_all(&baseline, &candidate, 0.9);
        assert!(passes_shadow_threshold(&mut scores, 0.7));
    }
}
