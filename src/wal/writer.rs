use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::thread;

use crate::wal::entry::{EventType, WalEntry};

// ─── Constants ────────────────────────────────────────────────────────────────

/// Maximum entries to buffer before forcing a flush (ADR-005).
pub const BATCH_SIZE: usize = 64;

/// Write a checkpoint record after every N entries flushed.
pub const CHECKPOINT_INTERVAL: u64 = 256;

/// Default deadline between automatic flushes (ADR-005).
pub const DEFAULT_DEADLINE: Duration = Duration::from_millis(10);

// ─── Internal state ───────────────────────────────────────────────────────────

struct WriterState {
    file:            File,
    /// Serialized bytes not yet written to disk.
    pending:         Vec<u8>,
    /// Number of WalEntry records in `pending`.
    pending_count:   usize,
    /// Total O_DSYNC flush calls (tracks batching, used in tests).
    flush_count:     u64,
    /// Seq field of the last appended entry.
    last_seq:        u64,
    /// Total entries flushed at the time the last checkpoint fired.
    checkpoint_seq:  u64,
    /// Running total of entries written to disk.
    total_flushed:   u64,
}

impl WriterState {
    /// Drain pending buffer to disk with a single `sync_data()` (O_DSYNC equivalent).
    /// Writes a checkpoint record when the 256-entry boundary is crossed.
    /// Only increments `flush_count` when there is something to flush.
    fn do_flush(&mut self) -> std::io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }

        let entries_in_batch = self.pending_count as u64;
        let prev_total = self.total_flushed;

        // Write buffered bytes + data sync (single system call)
        self.file.write_all(&self.pending)?;
        self.file.sync_data()?; // O_DSYNC: data reaches disk, no metadata wait

        self.total_flushed  += entries_in_batch;
        self.pending.clear();
        self.pending_count   = 0;
        self.flush_count    += 1;

        // Check if this flush crossed a CHECKPOINT_INTERVAL boundary
        let curr_total = self.total_flushed;
        if curr_total / CHECKPOINT_INTERVAL > prev_total / CHECKPOINT_INTERVAL {
            self.checkpoint_seq = (curr_total / CHECKPOINT_INTERVAL) * CHECKPOINT_INTERVAL;
            self.write_checkpoint_record()?;
        }

        Ok(())
    }

    /// Write a durable checkpoint record and full fsync (stronger than sync_data).
    fn write_checkpoint_record(&mut self) -> std::io::Result<()> {
        let record = WalEntry {
            seq:          self.checkpoint_seq,
            timestamp_us: 0,
            event_type:   EventType::Checkpoint,
            flags:        0,
            doc_id:       0,
            inode:        0,
            volume_uuid:  0,
            mtime_ns:     0,
            size:         0,
            mode:         0,
            uid:          0,
            gid:          0,
            path:         String::new(),
        };
        let mut buf = Vec::new();
        record.serialize(&mut buf);
        self.file.write_all(&buf)?;
        self.file.sync_all()?; // full fsync for checkpoint records
        Ok(())
    }
}

// ─── Public WalWriter ─────────────────────────────────────────────────────────

/// Write-ahead log writer.
///
/// # ADR-005 — Batched O_DSYNC
/// Entries are buffered in memory. A flush occurs when:
/// - `BATCH_SIZE` (64) entries have accumulated, OR
/// - `deadline` duration has elapsed since the last flush
///   (enforced by a background timer thread), OR
/// - `flush()` is called explicitly.
///
/// This reduces the number of `sync_data()` syscalls from O(events) to
/// O(events / 64) under normal load, and avoids saturating disk I/O during
/// FSEvent storms.
pub struct WalWriter {
    state:            Arc<Mutex<WriterState>>,
    stop:             Arc<AtomicBool>,
    // Kept alive so the deadline thread runs for the lifetime of WalWriter.
    _deadline_thread: Option<thread::JoinHandle<()>>,
}

impl WalWriter {
    /// Create a writer with the default 10ms deadline.
    pub fn new(path: &Path) -> anyhow::Result<Self> {
        Self::new_with_deadline(path, DEFAULT_DEADLINE)
    }

    /// Create a writer with a custom flush deadline (useful for tests).
    pub fn new_with_deadline(path: &Path, deadline: Duration) -> anyhow::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        let state = Arc::new(Mutex::new(WriterState {
            file,
            pending:        Vec::with_capacity(BATCH_SIZE * 256),
            pending_count:  0,
            flush_count:    0,
            last_seq:       0,
            checkpoint_seq: 0,
            total_flushed:  0,
        }));

        let stop        = Arc::new(AtomicBool::new(false));
        let state_clone = Arc::clone(&state);
        let stop_clone  = Arc::clone(&stop);

        let handle = thread::Builder::new()
            .name("wal-deadline-flush".to_string())
            .spawn(move || {
                loop {
                    thread::sleep(deadline);
                    if stop_clone.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Ok(mut s) = state_clone.lock() {
                        if s.pending_count > 0 {
                            let _ = s.do_flush();
                        }
                    }
                }
            })?;

        Ok(WalWriter {
            state,
            stop,
            _deadline_thread: Some(handle),
        })
    }

    /// Append an entry. Automatically flushes when the batch reaches BATCH_SIZE.
    /// `last_seq` is updated immediately (before flush).
    pub fn append(&self, entry: WalEntry) -> anyhow::Result<()> {
        let mut s = self.state.lock()
            .map_err(|_| anyhow::anyhow!("WalWriter mutex poisoned"))?;
        s.last_seq = entry.seq;
        entry.serialize(&mut s.pending);
        s.pending_count += 1;

        if s.pending_count >= BATCH_SIZE {
            s.do_flush()
                .map_err(|e| anyhow::anyhow!("WAL batch flush failed: {e}"))?;
        }
        Ok(())
    }

    /// Explicitly flush any buffered entries to disk.
    /// No-op (and does NOT increment flush_count) if pending is empty.
    pub fn flush(&self) -> anyhow::Result<()> {
        let mut s = self.state.lock()
            .map_err(|_| anyhow::anyhow!("WalWriter mutex poisoned"))?;
        s.do_flush()
            .map_err(|e| anyhow::anyhow!("WAL explicit flush failed: {e}"))
    }

    // ─── Accessors (used by tests and WAL reader) ─────────────────────────

    /// Total number of O_DSYNC flushes performed so far.
    pub fn flush_count(&self) -> u64 {
        self.state.lock().unwrap().flush_count
    }

    /// Seq field of the last entry passed to `append()`.
    pub fn last_seq(&self) -> u64 {
        self.state.lock().unwrap().last_seq
    }

    /// Total entries flushed at the time the last checkpoint record was written.
    /// Returns 0 if no checkpoint has fired yet.
    pub fn checkpoint_seq(&self) -> u64 {
        self.state.lock().unwrap().checkpoint_seq
    }
}

impl Drop for WalWriter {
    fn drop(&mut self) {
        // Signal deadline thread to exit
        self.stop.store(true, Ordering::Relaxed);
        // Flush any remaining pending entries before closing the file
        if let Ok(mut s) = self.state.lock() {
            let _ = s.do_flush();
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wal_writer_append_and_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wal");
        let writer = WalWriter::new_with_deadline(&path, Duration::from_secs(60)).unwrap();

        for i in 0..300u64 {
            let entry = WalEntry { seq: i, ..WalEntry::new_test() };
            writer.append(entry).unwrap();
        }
        writer.flush().unwrap();

        // Checkpoint fires when total_flushed crosses 256
        assert_eq!(writer.checkpoint_seq(), 256);
        // last_seq tracks every append, not just flushes
        assert_eq!(writer.last_seq(), 299);
    }

    #[test]
    fn test_wal_batched_flush_under_storm() {
        // ADR-005: batch up to 64 entries before O_DSYNC — not per-entry.
        // 128 appends with BATCH_SIZE=64 → exactly 2 auto-flushes (at entry 64 and 128).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("storm.wal");
        // Use a very long deadline so the timer thread never fires during this test
        let writer = WalWriter::new_with_deadline(&path, Duration::from_secs(60)).unwrap();

        for i in 0..128u64 {
            writer.append(WalEntry { seq: i, ..WalEntry::new_test() }).unwrap();
        }
        writer.flush().unwrap(); // pending is empty — must NOT increment flush_count

        assert_eq!(
            writer.flush_count(), 2,
            "Expected exactly 2 O_DSYNC flushes for 128 entries (batch_size={})",
            BATCH_SIZE
        );
        assert_eq!(writer.last_seq(), 127);
    }

    #[test]
    fn test_wal_deadline_flush_fires_within_10ms() {
        // ADR-005: even a single entry must reach disk within 10ms via deadline thread.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deadline.wal");
        let writer = WalWriter::new_with_deadline(
            &path,
            Duration::from_millis(10),
        ).unwrap();

        writer.append(WalEntry { seq: 0, ..WalEntry::new_test() }).unwrap();
        // Wait longer than the deadline
        thread::sleep(Duration::from_millis(50));
        // Explicit flush with empty pending — must NOT increment flush_count
        writer.flush().unwrap();

        assert_eq!(writer.flush_count(), 1,
            "Deadline thread should have flushed the 1 pending entry within 10ms");
    }

    #[test]
    fn test_empty_flush_does_not_increment_flush_count() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.wal");
        let writer = WalWriter::new(&path).unwrap();
        writer.flush().unwrap(); // nothing pending
        assert_eq!(writer.flush_count(), 0);
    }

    #[test]
    fn test_wal_file_is_non_empty_after_append_and_flush() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("content.wal");
        let writer = WalWriter::new(&path).unwrap();
        writer.append(WalEntry::new_test()).unwrap();
        writer.flush().unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert!(meta.len() > 0, "WAL file should have bytes after flush");
    }

    #[test]
    fn test_checkpoint_fires_at_correct_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ckpt.wal");
        // Long deadline so only batch flushes fire
        let writer = WalWriter::new_with_deadline(&path, Duration::from_secs(60)).unwrap();

        // Append exactly 256 entries in 4 batches of 64
        for i in 0..256u64 {
            writer.append(WalEntry { seq: i, ..WalEntry::new_test() }).unwrap();
        }

        assert_eq!(writer.checkpoint_seq(), 256,
            "Checkpoint must fire exactly at the 256-entry boundary");
        // Append 64 more — total 320, still only 1 checkpoint
        for i in 256..320u64 {
            writer.append(WalEntry { seq: i, ..WalEntry::new_test() }).unwrap();
        }
        // Next checkpoint fires at 512, not yet reached
        assert_eq!(writer.checkpoint_seq(), 256);
    }
}
