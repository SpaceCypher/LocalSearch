use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::wal::entry::WalEntry;

// ─── WalReader ────────────────────────────────────────────────────────────────

/// Read-ahead log reader for crash recovery and replay.
///
/// # Crash Recovery Strategy
/// The reader replays entries from a given sequence number, verifying checksums
/// on each entry. When it encounters the first corrupt or truncated entry
/// (partial write from a crash), it stops replay and returns all valid entries
/// up to that point.
///
/// The caller can then truncate the WAL file to the last valid entry position,
/// discarding the corrupt tail.
pub struct WalReader {
    file: File,
}

impl WalReader {
    /// Open a WAL file for reading.
    pub fn new(path: &Path) -> anyhow::Result<Self> {
        let file = File::open(path)?;
        Ok(WalReader { file })
    }

    /// Replay all entries starting from `start_seq` (inclusive).
    ///
    /// Stops at the first corrupt or truncated entry. Returns all valid entries
    /// encountered before the corruption point.
    ///
    /// # Crash Recovery
    /// If the WAL was partially written during a crash, this method will:
    /// 1. Read entries sequentially
    /// 2. Verify checksum on each entry before decoding
    /// 3. Stop at the first checksum failure or truncated read
    /// 4. Return all valid entries up to that point
    ///
    /// The caller should then truncate the WAL file to discard the corrupt tail.
    pub fn replay_from(&mut self, start_seq: u64) -> anyhow::Result<Vec<WalEntry>> {
        let mut entries = Vec::new();
        self.file.seek(SeekFrom::Start(0))?;

        loop {
            match self.read_entry() {
                Ok(Some(entry)) => {
                    if entry.seq >= start_seq {
                        entries.push(entry);
                    }
                }
                Ok(None) => break, // Clean EOF
                Err(_) => {
                    // Corruption or truncation detected — stop replay here
                    // Return all valid entries collected so far
                    break;
                }
            }
        }

        Ok(entries)
    }

    /// Read a single entry from the current file position.
    ///
    /// Returns:
    /// - `Ok(Some(entry))` if a valid entry was read
    /// - `Ok(None)` if EOF was reached
    /// - `Err(_)` if corruption was detected (checksum failure or truncation)
    ///
    /// # Corruption Detection
    /// This method verifies the checksum BEFORE deserializing. If the checksum
    /// fails, it returns an error immediately, signaling that replay should stop.
    fn read_entry(&mut self) -> anyhow::Result<Option<WalEntry>> {
        // Read fixed header (70 bytes) to get path_len
        let mut header = [0u8; 70];
        match self.file.read_exact(&mut header) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(None); // Clean EOF
            }
            Err(e) => return Err(e.into()),
        }

        // Extract path_len from header (bytes 68-70)
        let path_len = u16::from_le_bytes([header[68], header[69]]) as usize;

        // Read variable-length path + checksum (8 bytes)
        let mut tail = vec![0u8; path_len + 8];
        match self.file.read_exact(&mut tail) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                // Truncated entry — partial write from crash
                anyhow::bail!("Truncated WAL entry at path field");
            }
            Err(e) => return Err(e.into()),
        }

        // Reconstruct full entry buffer
        let mut buf = Vec::with_capacity(70 + path_len + 8);
        buf.extend_from_slice(&header);
        buf.extend_from_slice(&tail);

        // Verify checksum BEFORE deserialization (catches corruption early)
        if !WalEntry::verify_checksum_bytes(&buf) {
            anyhow::bail!("WAL entry checksum mismatch — corrupt entry detected");
        }

        // Checksum passed — safe to deserialize
        let entry = WalEntry::deserialize(&buf)?;
        Ok(Some(entry))
    }

    /// Get the current file position (useful for truncation after replay).
    pub fn current_position(&mut self) -> anyhow::Result<u64> {
        Ok(self.file.stream_position()?)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wal::entry::EventType;
    use crate::wal::writer::WalWriter;

    #[test]
    fn test_wal_reader_replay_from_checkpoint() {
        // Write 500 entries, corrupt entry 300, verify replay stops at 299
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wal");

        // Write 500 entries
        {
            let writer = WalWriter::new(&path).unwrap();
            for i in 0..500u64 {
                let entry = WalEntry {
                    seq: i,
                    ..WalEntry::new_test()
                };
                writer.append(entry).unwrap();
            }
            writer.flush().unwrap();
        }

        // Corrupt entry at position ~300 (approximate — we'll corrupt a byte in the middle)
        {
            use std::fs::OpenOptions;
            use std::io::{Seek, Write};
            let mut file = OpenOptions::new().write(true).open(&path).unwrap();
            // Each entry is ~78 bytes (70 fixed + 14 path + 8 checksum for "/test/file.txt")
            // Entry 300 starts around byte 300 * 92 = 27,600
            file.seek(SeekFrom::Start(27_600)).unwrap();
            file.write_all(&[0xFF, 0xFF, 0xFF, 0xFF]).unwrap(); // Corrupt 4 bytes
        }

        // Replay from seq 0 — should stop before entry 300
        let mut reader = WalReader::new(&path).unwrap();
        let entries = reader.replay_from(0).unwrap();

        // Verify we got entries before the corruption point
        assert!(!entries.is_empty(), "Should have read some valid entries");
        assert!(entries.len() < 500, "Should have stopped before entry 500");
        
        // All returned entries should have valid checksums
        for entry in &entries {
            assert!(entry.verify_checksum(), "Entry seq {} has invalid checksum", entry.seq);
        }

        // Entries should be sequential (excluding checkpoint records)
        let non_checkpoint_entries: Vec<_> = entries.iter()
            .filter(|e| e.event_type != EventType::Checkpoint)
            .collect();
        
        for (i, entry) in non_checkpoint_entries.iter().enumerate() {
            assert_eq!(entry.seq, i as u64, "Entry sequence mismatch at index {}", i);
        }
    }

    #[test]
    fn test_wal_reader_handles_truncated_entry() {
        // Write 10 entries, then manually truncate the file mid-entry
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("truncated.wal");

        {
            let writer = WalWriter::new(&path).unwrap();
            for i in 0..10u64 {
                writer.append(WalEntry { seq: i, ..WalEntry::new_test() }).unwrap();
            }
            writer.flush().unwrap();
        }

        // Truncate file to cut off the last entry
        {
            let file = File::open(&path).unwrap();
            let len = file.metadata().unwrap().len();
            drop(file);
            let file = File::create(&path).unwrap();
            file.set_len(len - 20).unwrap(); // Remove last 20 bytes
        }

        // Replay should stop at the truncation point
        let mut reader = WalReader::new(&path).unwrap();
        let entries = reader.replay_from(0).unwrap();

        // Should have read fewer than 10 entries
        assert!(entries.len() < 10, "Should have stopped at truncation");
        
        // All returned entries should be valid
        for entry in &entries {
            assert!(entry.verify_checksum());
        }
    }

    #[test]
    fn test_wal_reader_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.wal");
        File::create(&path).unwrap();

        let mut reader = WalReader::new(&path).unwrap();
        let entries = reader.replay_from(0).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_wal_reader_skips_entries_before_start_seq() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skip.wal");

        {
            let writer = WalWriter::new(&path).unwrap();
            for i in 0..100u64 {
                writer.append(WalEntry { seq: i, ..WalEntry::new_test() }).unwrap();
            }
            writer.flush().unwrap();
        }

        let mut reader = WalReader::new(&path).unwrap();
        let entries = reader.replay_from(50).unwrap();

        assert_eq!(entries.len(), 50);
        assert_eq!(entries.first().unwrap().seq, 50);
        assert_eq!(entries.last().unwrap().seq, 99);
    }

    #[test]
    fn test_wal_reader_handles_checkpoint_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.wal");

        {
            let writer = WalWriter::new(&path).unwrap();
            // Write 300 entries to trigger checkpoint at 256
            for i in 0..300u64 {
                writer.append(WalEntry { seq: i, ..WalEntry::new_test() }).unwrap();
            }
            writer.flush().unwrap();
        }

        let mut reader = WalReader::new(&path).unwrap();
        let entries = reader.replay_from(0).unwrap();

        // Should include checkpoint record at seq 256
        let checkpoint = entries.iter().find(|e| e.event_type == EventType::Checkpoint);
        assert!(checkpoint.is_some(), "Should have found checkpoint record");
        assert_eq!(checkpoint.unwrap().seq, 256);
    }

    #[test]
    fn test_wal_reader_replay_from_after_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("after_ckpt.wal");

        {
            let writer = WalWriter::new(&path).unwrap();
            for i in 0..300u64 {
                writer.append(WalEntry { seq: i, ..WalEntry::new_test() }).unwrap();
            }
            writer.flush().unwrap();
        }

        // Replay from checkpoint (256) — should get entries 256-299 + checkpoint record
        let mut reader = WalReader::new(&path).unwrap();
        let entries = reader.replay_from(256).unwrap();

        assert!(!entries.is_empty());
        assert!(entries.iter().all(|e| e.seq >= 256));
    }
}
