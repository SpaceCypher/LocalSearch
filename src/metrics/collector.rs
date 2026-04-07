use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct MetricsCollector {
    db: Connection,
}

pub struct IntegrityReport {
    pub phantom_rate: f32,
    pub stale_rate: f32,
}

impl MetricsCollector {
    pub fn new(db_path: &Path) -> Result<Self> {
        let db = Connection::open(db_path)?;
        
        db.execute(
            "CREATE TABLE IF NOT EXISTS query_metrics (
                ts INTEGER NOT NULL,
                latency_ms INTEGER NOT NULL,
                result_count INTEGER NOT NULL
            )",
            [],
        )?;
        
        Ok(Self { db })
    }

    pub fn record_query(&self, latency: Duration, result_count: usize) -> Result<()> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs();
        
        self.db.execute(
            "INSERT INTO query_metrics (ts, latency_ms, result_count) VALUES (?, ?, ?)",
            params![ts, latency.as_millis() as i64, result_count],
        )?;
        
        Ok(())
    }

    pub fn cleanup_old_metrics(&self) -> Result<()> {
        let seven_days_ago = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() - (7 * 24 * 3600);
        
        self.db.execute(
            "DELETE FROM query_metrics WHERE ts < ?",
            params![seven_days_ago],
        )?;
        
        Ok(())
    }
}

pub struct IntegrityChecker;

impl IntegrityChecker {
    pub fn check<F>(sample_docs: F) -> Result<IntegrityReport>
    where
        F: Fn() -> Vec<(u64, PathBuf, u64)>,
    {
        let docs = sample_docs();
        let total = docs.len() as f32;
        
        let mut phantom_count = 0;
        let mut stale_count = 0;
        
        for (_doc_id, path, expected_mtime) in docs {
            match std::fs::metadata(&path) {
                Ok(metadata) => {
                    let actual_mtime = metadata.modified()?
                        .duration_since(UNIX_EPOCH)?
                        .as_secs();
                    
                    if actual_mtime != expected_mtime {
                        stale_count += 1;
                    }
                }
                Err(_) => {
                    phantom_count += 1;
                }
            }
        }
        
        Ok(IntegrityReport {
            phantom_rate: phantom_count as f32 / total,
            stale_rate: stale_count as f32 / total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_integrity_check_detects_phantom_files() {
        // RED: This test should fail
        let sample_docs = || {
            vec![
                (1, PathBuf::from("/nonexistent/file1.txt"), 1000),
                (2, PathBuf::from("/nonexistent/file2.txt"), 2000),
                (3, PathBuf::from("/nonexistent/file3.txt"), 3000),
            ]
        };

        let report = IntegrityChecker::check(sample_docs).unwrap();
        
        // All 3 files don't exist, so phantom_rate should be 3/3 = 1.0
        assert_eq!(report.phantom_rate, 1.0);
        assert_eq!(report.stale_rate, 0.0);
    }

    #[test]
    fn test_integrity_check_detects_stale_files() {
        // RED: This test should fail
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        std::fs::write(&file_path, "content").unwrap();
        
        // Get actual mtime
        let metadata = std::fs::metadata(&file_path).unwrap();
        let actual_mtime = metadata.modified().unwrap()
            .duration_since(UNIX_EPOCH).unwrap().as_secs();
        
        // Provide wrong mtime (1 second off)
        let sample_docs = move || {
            vec![
                (1, file_path.clone(), actual_mtime - 1),
            ]
        };

        let report = IntegrityChecker::check(sample_docs).unwrap();
        
        // File exists but mtime is wrong, so stale_rate should be 1.0
        assert_eq!(report.phantom_rate, 0.0);
        assert_eq!(report.stale_rate, 1.0);
    }

    #[test]
    fn test_integrity_check_phantom_rate_threshold() {
        // RED: This test should fail
        // 60 phantom files out of 1000 = 6% > 5% threshold
        let mut docs = Vec::new();
        for i in 0..60 {
            docs.push((i, PathBuf::from(format!("/nonexistent/file{}.txt", i)), 1000));
        }
        for i in 60..1000 {
            docs.push((i, PathBuf::from("/dev/null"), 1000));
        }
        
        let sample_docs = move || docs.clone();
        let report = IntegrityChecker::check(sample_docs).unwrap();
        
        assert!(report.phantom_rate > 0.05, "phantom_rate should exceed 5% threshold");
    }

    #[test]
    fn test_metrics_collector_records_query() {
        // RED: This test should fail
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("metrics.db");
        
        let collector = MetricsCollector::new(&db_path).unwrap();
        
        collector.record_query(Duration::from_millis(50), 10).unwrap();
        collector.record_query(Duration::from_millis(100), 20).unwrap();
        
        // Verify records were inserted
        let count: i64 = collector.db.query_row(
            "SELECT COUNT(*) FROM query_metrics",
            [],
            |row| row.get(0)
        ).unwrap();
        
        assert_eq!(count, 2);
    }

    #[test]
    fn test_metrics_cleanup_7_day_retention() {
        // RED: This test should fail
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("metrics.db");
        
        let collector = MetricsCollector::new(&db_path).unwrap();
        
        // Insert old record (8 days ago)
        let eight_days_ago = SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap().as_secs() - (8 * 24 * 3600);
        
        collector.db.execute(
            "INSERT INTO query_metrics (ts, latency_ms, result_count) VALUES (?, ?, ?)",
            params![eight_days_ago, 50, 10],
        ).unwrap();
        
        // Insert recent record
        collector.record_query(Duration::from_millis(50), 10).unwrap();
        
        // Run cleanup
        collector.cleanup_old_metrics().unwrap();
        
        // Only recent record should remain
        let count: i64 = collector.db.query_row(
            "SELECT COUNT(*) FROM query_metrics",
            [],
            |row| row.get(0)
        ).unwrap();
        
        assert_eq!(count, 1);
    }
}
