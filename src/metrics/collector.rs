// Metrics Collector — Query latency tracking + integrity checks
// Storage: SQLite ring buffer (7-day retention, 50MB cap)
// Integrity: 1000-doc sample parity check, phantom_rate alert (>5%)

use rusqlite::{Connection, params};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use anyhow::Result;

/// Integrity check report
#[derive(Debug, Clone)]
pub struct IntegrityReport {
    pub phantom_rate: f32,
    pub stale_rate: f32,
    pub needs_reconciliation: bool,
}

/// Integrity checker
pub struct IntegrityChecker {
    // For testing, we'll use a simple in-memory document store
    documents: Vec<(u64, String, u64)>, // (doc_id, path, mtime)
}

impl IntegrityChecker {
    pub fn new() -> Self {
        Self {
            documents: Vec::new(),
        }
    }

    /// Add document sample for parity checks.
    pub fn add_document(&mut self, doc_id: u64, path: String, mtime: u64) {
        self.documents.push((doc_id, path, mtime));
    }

    /// Run integrity check on 1000 random documents
    pub fn check(&self) -> Result<IntegrityReport> {
        let sample_size = self.documents.len().min(1000);
        let mut phantom_count = 0;
        let mut stale_count = 0;

        for (_doc_id, path, indexed_mtime) in self.documents.iter().take(sample_size) {
            match std::fs::metadata(path) {
                Ok(metadata) => {
                    let disk_mtime = metadata.modified()
                        .unwrap()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    
                    if disk_mtime != *indexed_mtime {
                        stale_count += 1;
                    }
                }
                Err(_) => {
                    // File doesn't exist on disk but is in index (phantom)
                    phantom_count += 1;
                }
            }
        }

        let phantom_rate = if sample_size > 0 {
            phantom_count as f32 / sample_size as f32
        } else {
            0.0
        };

        let stale_rate = if sample_size > 0 {
            stale_count as f32 / sample_size as f32
        } else {
            0.0
        };

        let needs_reconciliation = phantom_rate > 0.05; // 5% threshold

        Ok(IntegrityReport {
            phantom_rate,
            stale_rate,
            needs_reconciliation,
        })
    }
}

/// Metrics collector with SQLite ring buffer
pub struct MetricsCollector {
    db: Connection,
}

impl MetricsCollector {
    /// Create new metrics collector
    pub fn new(db_path: impl AsRef<Path>) -> Result<Self> {
        let db = Connection::open(db_path)?;
        
        // Create table if not exists
        db.execute(
            "CREATE TABLE IF NOT EXISTS query_metrics (
                ts INTEGER NOT NULL,
                latency_ms INTEGER NOT NULL,
                result_count INTEGER NOT NULL
            )",
            [],
        )?;

        // Create index for efficient cleanup
        db.execute(
            "CREATE INDEX IF NOT EXISTS idx_ts ON query_metrics(ts)",
            [],
        )?;

        Ok(Self { db })
    }

    /// Record query metrics
    pub fn record_query(&self, latency: Duration, result_count: usize) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64;

        self.db.execute(
            "INSERT INTO query_metrics (ts, latency_ms, result_count) VALUES (?, ?, ?)",
            params![now, latency.as_millis() as i64, result_count as i64],
        )?;

        Ok(())
    }

    /// Cleanup old entries (7-day retention)
    pub fn cleanup_old_entries(&self) -> Result<usize> {
        let seven_days_ago = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64 - (7 * 24 * 60 * 60);

        let deleted = self.db.execute(
            "DELETE FROM query_metrics WHERE ts < ?",
            params![seven_days_ago],
        )?;

        Ok(deleted)
    }

    /// Get total entry count
    pub fn entry_count(&self) -> Result<usize> {
        let count: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM query_metrics",
            [],
            |row| row.get(0),
        )?;

        Ok(count as usize)
    }

    /// Get database size in bytes
    pub fn db_size(&self) -> Result<u64> {
        let page_count: i64 = self.db.query_row(
            "PRAGMA page_count",
            [],
            |row| row.get(0),
        )?;

        let page_size: i64 = self.db.query_row(
            "PRAGMA page_size",
            [],
            |row| row.get(0),
        )?;

        Ok((page_count * page_size) as u64)
    }

    /// Enforce 50MB cap by deleting oldest entries
    pub fn enforce_size_cap(&self) -> Result<()> {
        let max_size = 50 * 1024 * 1024; // 50MB
        let current_size = self.db_size()?;

        if current_size > max_size {
            // Delete oldest 10% of entries
            let total_count = self.entry_count()?;
            let delete_count = total_count / 10;

            self.db.execute(
                &format!(
                    "DELETE FROM query_metrics WHERE rowid IN (
                        SELECT rowid FROM query_metrics ORDER BY ts ASC LIMIT {}
                    )",
                    delete_count
                ),
                [],
            )?;
        }
        
        Ok(())
    }

    /// Get latency stats (P50, P99)
    pub fn get_latency_stats(&self) -> Result<(f64, f64)> {
        let mut p50 = 0.0;
        let mut p99 = 0.0;

        let mut latencies = Vec::new();
        let mut stmt = self.db.prepare("SELECT latency_ms FROM query_metrics ORDER BY latency_ms")?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;

        for row in rows {
            latencies.push(row? as f64);
        }

        if !latencies.is_empty() {
            let n = latencies.len();
            p50 = latencies[n / 2];
            p99 = latencies[(n * 99) / 100];
        }

        Ok((p50, p99))
    }

    /// Fraction of queries that returned zero results in [0.0, 1.0].
    pub fn zero_result_rate(&self) -> Result<f32> {
        let total: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM query_metrics",
            [],
            |row| row.get(0),
        )?;

        if total == 0 {
            return Ok(0.0);
        }

        let zero_count: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM query_metrics WHERE result_count = 0",
            [],
            |row| row.get(0),
        )?;

        Ok(zero_count as f32 / total as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_integrity_check_phantom_detection() {
        let mut checker = IntegrityChecker::new();
        
        // Add document that doesn't exist on disk (phantom)
        checker.add_document(1, "/nonexistent/file.txt".to_string(), 1234567890);
        
        let report = checker.check().unwrap();
        
        // Should detect 100% phantom rate
        assert_eq!(report.phantom_rate, 1.0);
        assert!(report.needs_reconciliation);
    }

    #[test]
    fn test_integrity_check_stale_detection() {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, b"content").unwrap();
        
        let mut checker = IntegrityChecker::new();
        
        // Add document with wrong mtime (stale)
        checker.add_document(1, test_file.to_str().unwrap().to_string(), 0);
        
        let report = checker.check().unwrap();
        
        // Should detect stale file
        assert_eq!(report.stale_rate, 1.0);
    }

    #[test]
    fn test_integrity_check_phantom_rate_threshold() {
        let mut checker = IntegrityChecker::new();
        
        // Add 100 documents, 4 phantoms (4% - below threshold)
        for i in 0..96 {
            let temp_file = tempfile::NamedTempFile::new().unwrap();
            checker.add_document(i, temp_file.path().to_str().unwrap().to_string(), 0);
            // Keep temp files alive
            std::mem::forget(temp_file);
        }
        for i in 96..100 {
            checker.add_document(i, format!("/nonexistent/{}.txt", i), 0);
        }
        
        let report = checker.check().unwrap();
        
        // 4% phantom rate - should NOT trigger reconciliation
        assert!(!report.needs_reconciliation);
        
        // Now add 2 more phantoms (6% - above threshold)
        checker.add_document(100, "/nonexistent/100.txt".to_string(), 0);
        checker.add_document(101, "/nonexistent/101.txt".to_string(), 0);
        
        let report = checker.check().unwrap();
        
        // 6% phantom rate - should trigger reconciliation
        assert!(report.needs_reconciliation);
    }

    #[test]
    fn test_metrics_record_query() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("metrics.db");
        
        let collector = MetricsCollector::new(&db_path).unwrap();
        
        // Record some queries
        collector.record_query(Duration::from_millis(25), 10).unwrap();
        collector.record_query(Duration::from_millis(80), 5).unwrap();
        
        let count = collector.entry_count().unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_metrics_cleanup_old_entries() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("metrics.db");
        
        let collector = MetricsCollector::new(&db_path).unwrap();
        
        // Insert old entry (8 days ago)
        let eight_days_ago = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64 - (8 * 24 * 60 * 60);
        
        collector.db.execute(
            "INSERT INTO query_metrics (ts, latency_ms, result_count) VALUES (?, ?, ?)",
            params![eight_days_ago, 100, 5],
        ).unwrap();
        
        // Insert recent entry
        collector.record_query(Duration::from_millis(50), 10).unwrap();
        
        assert_eq!(collector.entry_count().unwrap(), 2);
        
        // Cleanup old entries
        let deleted = collector.cleanup_old_entries().unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(collector.entry_count().unwrap(), 1);
    }

    #[test]
    fn test_metrics_size_cap_enforcement() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("metrics.db");
        
        let collector = MetricsCollector::new(&db_path).unwrap();
        
        // Insert many entries to grow database
        for _ in 0..1000 {
            collector.record_query(Duration::from_millis(50), 10).unwrap();
        }
        
        let initial_count = collector.entry_count().unwrap();
        assert_eq!(initial_count, 1000);
        
        // Size cap enforcement should work without error
        collector.enforce_size_cap().unwrap();
        
        // Should still have entries (cap is 50MB, we're nowhere near that)
        assert!(collector.entry_count().unwrap() > 0);
    }

    #[test]
    fn test_zero_result_rate() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("metrics.db");
        let collector = MetricsCollector::new(&db_path).unwrap();

        collector.record_query(Duration::from_millis(10), 0).unwrap();
        collector.record_query(Duration::from_millis(20), 5).unwrap();
        collector.record_query(Duration::from_millis(30), 0).unwrap();

        let rate = collector.zero_result_rate().unwrap();
        assert!((rate - (2.0 / 3.0)).abs() < 0.001);
    }
}
