use std::fs::OpenOptions;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::wal::entry::WalEntry;

// ─── Replay outcome ────────────────────────────────────────────────────────────

/// Result of a WAL replay operation.
#[derive(Debug)]
pub struct ReplayResult {
    /// All valid entries read from the WAL file, in order.
    pub entries: Vec<WalEntry>,
    /// Number of bytes that were valid and replayed.
    pub valid_bytes: u64,
    /// Whether a truncated (partial-write) tail was detected and removed.
    pub truncated: bool,
    /// Number of corrupt or truncated bytes trimmed from the file.
    pub trimmed_bytes: u64,
}

// ─── Reader errors ─────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ReaderError {
    Io(std::io::Error),
    TruncateFailed(std::io::Error),
}

impl std::fmt::Display for ReaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReaderError::Io(e)             => write!(f, "WAL I/O error: {e}"),
            ReaderError::TruncateFailed(e) => write!(f, "WAL truncation failed: {e}"),
        }
    }
}

impl std::error::Error for ReaderError {}

impl From<std::io::Error> for ReaderError {
    fn from(e: std::io::Error) -> Self {
        ReaderError::Io(e)
    }
}

// ─── WAL reader constants ──────────────────────────────────────────────────────

/// Minimum bytes needed to read the fixed header (up to path_len field at offset 68).
const MIN_HEADER_BYTES: usize = 70;
/// Checksum trailer size.
const CHECKSUM_BYTES: usize = 8;

// ─── WalReader ────────────────────────────────────────────────────────────────

/// Reads and replays a WAL file on startup or after a crash.
///
/// # Crash Recovery Protocol
///
/// The WAL may be partially written if the process crashed mid-flush. The
/// recovery procedure is:
///
/// 1. Stream entries one by one from the start of file.
/// 2. For each entry: read the fixed 70-byte header, extract `path_len`,
///    read the remaining path + 8-byte checksum into one buffer.
/// 3. Call `WalEntry::verify_checksum_bytes()` on the full entry bytes.
/// 4. If the checksum passes: deserialize and add to `entries`.
/// 5. If the checksum fails OR bytes are truncated: **stop**. Record the
///    byte offset of the last valid entry as `valid_bytes`.
/// 6. Truncate the file to `valid_bytes` — this removes the partial write
///    and leaves the WAL in a consistent state.
///
/// This is an **atomic recovery**: the WAL is either fully consistent
/// after `replay()`, or the function returns an error. It never leaves
/// the file in a partially-truncated state (truncate is atomic at the
/// OS level on macOS/Linux).
pub struct WalReader;

impl WalReader {
    /// Open a WAL file, replay all valid entries, and truncate any corrupt tail.
    ///
    /// Returns `Ok(ReplayResult)` if the file was successfully read (even if
    /// some tail bytes were trimmed). Returns `Err` only on I/O failures.
    pub fn replay(path: &Path) -> Result<ReplayResult, ReaderError> {
        // Open for read + write so we can truncate if needed.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;

        let total_bytes = file.metadata()?.len();
        let mut reader   = BufReader::new(&file);
        let mut entries  = Vec::new();
        let mut cursor: u64 = 0;

        loop {
            // ── Step 1: read fixed header (70 bytes) ──────────────────────
            let mut header = [0u8; MIN_HEADER_BYTES];
            match reader.read_exact(&mut header) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // Normal EOF or truncated header
                    break;
                }
                Err(e) => return Err(ReaderError::Io(e)),
            }

            // ── Step 2: extract path_len from offset 68..70 ───────────────
            let path_len = u16::from_le_bytes([header[68], header[69]]) as usize;

            // ── Step 3: read path bytes + 8-byte checksum ─────────────────
            let tail_len = path_len + CHECKSUM_BYTES;
            let mut tail = vec![0u8; tail_len];
            match reader.read_exact(&mut tail) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // Truncated at path or checksum — partial write, stop here.
                    break;
                }
                Err(e) => return Err(ReaderError::Io(e)),
            }

            // ── Step 4: verify checksum over the complete entry bytes ──────
            let mut entry_bytes = Vec::with_capacity(MIN_HEADER_BYTES + tail_len);
            entry_bytes.extend_from_slice(&header);
            entry_bytes.extend_from_slice(&tail);

            if !WalEntry::verify_checksum_bytes(&entry_bytes) {
                // Corrupt entry — treat everything from here as garbage, stop.
                break;
            }

            // ── Step 5: deserialize (checksum already verified) ───────────
            let entry = WalEntry::deserialize(&entry_bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

            cursor += entry_bytes.len() as u64;
            entries.push(entry);
        }

        // ── Step 6: truncate corrupt tail if any ──────────────────────────
        let valid_bytes   = cursor;
        let truncated     = valid_bytes < total_bytes;
        let trimmed_bytes = total_bytes.saturating_sub(valid_bytes);

        if truncated {
            // Drop the BufReader borrow before re-opening for truncation.
            drop(reader);
            let mut f = OpenOptions::new().write(true).open(path)?;
            f.seek(SeekFrom::Start(0))?;
            f.set_len(valid_bytes)
                .map_err(ReaderError::TruncateFailed)?;
        }

        Ok(ReplayResult {
            entries,
            valid_bytes,
            truncated,
            trimmed_bytes,
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wal::entry::EventType;
    use crate::wal::writer::WalWriter;
    use std::time::Duration;

    /// Write N entries to a WAL and flush everything to disk.
    fn write_entries(path: &Path, count: u64) {
        let writer = WalWriter::new_with_deadline(path, Duration::from_secs(60)).unwrap();
        for i in 0..count {
            let entry = WalEntry { seq: i, ..WalEntry::new_test() };
            writer.append(entry).unwrap();
        }
        writer.flush().unwrap();
    }

    #[test]
    fn test_wal_replay_clean_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clean.wal");
        write_entries(&path, 10);

        let result = WalReader::replay(&path).unwrap();

        assert_eq!(result.entries.len(), 10);
        assert!(!result.truncated);
        assert_eq!(result.trimmed_bytes, 0);
        // Entries must be in seq order
        for (i, entry) in result.entries.iter().enumerate() {
            assert_eq!(entry.seq, i as u64);
        }
    }

    #[test]
    fn test_wal_replay_checkpoint_entries_included() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.wal");
        // 256 entries → checkpoint fires, checkpoint record written to disk
        write_entries(&path, 256);

        let result = WalReader::replay(&path).unwrap();

        // 256 data entries + 1 checkpoint record
        assert_eq!(result.entries.len(), 257);
        assert!(result.entries.iter().any(|e| e.event_type == EventType::Checkpoint));
    }

    #[test]
    fn test_wal_replay_truncated_tail_trimmed() {
        // Simulate a crash mid-write: write valid entries, then append garbage bytes.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash.wal");
        write_entries(&path, 5);

        // Append partial/corrupt bytes (simulate incomplete flush from a crash)
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x00]).unwrap();
        }

        let file_size_before = std::fs::metadata(&path).unwrap().len();

        let result = WalReader::replay(&path).unwrap();

        assert_eq!(result.entries.len(), 5, "Should recover exactly the 5 valid entries");
        assert!(result.truncated, "Must detect and truncate the corrupt tail");
        assert!(result.trimmed_bytes > 0);

        // Verify file was actually truncated on disk
        let file_size_after = std::fs::metadata(&path).unwrap().len();
        assert!(file_size_after < file_size_before,
            "File must be truncated: before={} after={}", file_size_before, file_size_after);
        assert_eq!(file_size_after, result.valid_bytes);
    }

    #[test]
    fn test_wal_replay_single_corrupt_entry_mid_file() {
        // Write 3 valid entries, corrupt entry 2, write 2 more.
        // Reader should return entries 0 and 1 only (stop at corruption).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("midcorrupt.wal");

        // Serialize all 5 entries into one buffer with byte offsets recorded
        let mut full_bytes = Vec::new();
        let entries: Vec<WalEntry> = (0..5u64)
            .map(|i| WalEntry { seq: i, ..WalEntry::new_test() })
            .collect();
        let mut offsets = Vec::new();
        for e in &entries {
            offsets.push(full_bytes.len());
            e.serialize(&mut full_bytes);
        }

        // Corrupt the checksum of entry 2: flip the first byte of its checksum
        let entry2_start = offsets[2];
        let entry2_end   = offsets.get(3).copied().unwrap_or(full_bytes.len());
        let checksum_start = entry2_end - 8;
        full_bytes[checksum_start] ^= 0xFF;

        std::fs::write(&path, &full_bytes).unwrap();

        let result = WalReader::replay(&path).unwrap();

        assert_eq!(result.entries.len(), 2, "Only entries 0 and 1 should survive");
        assert_eq!(result.entries[0].seq, 0);
        assert_eq!(result.entries[1].seq, 1);
        assert!(result.truncated, "Corrupt tail must be truncated");
        let _ = entry2_start; // suppress unused warning
    }

    #[test]
    fn test_wal_replay_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.wal");
        std::fs::File::create(&path).unwrap();

        let result = WalReader::replay(&path).unwrap();

        assert!(result.entries.is_empty());
        assert!(!result.truncated);
        assert_eq!(result.trimmed_bytes, 0);
    }

    #[test]
    fn test_wal_replay_after_truncation_is_clean() {
        // After a crash + replay, a second replay on the truncated file
        // must produce zero truncation (idempotent).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("idempotent.wal");
        write_entries(&path, 4);

        // Simulate crash: add garbage
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&[0xFF; 20]).unwrap();
        }

        // First replay: truncates
        let r1 = WalReader::replay(&path).unwrap();
        assert!(r1.truncated);

        // Second replay on same file: must be clean
        let r2 = WalReader::replay(&path).unwrap();
        assert!(!r2.truncated, "Second replay on truncated file must find no corruption");
        assert_eq!(r2.entries.len(), r1.entries.len());
    }

    #[test]
    fn test_wal_replay_preserves_all_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fields.wal");

        let original = WalEntry {
            seq:          42,
            timestamp_us: 1_234_567,
            event_type:   EventType::Renamed,
            flags:        0b1010,
            doc_id:       99,
            inode:        77777,
            volume_uuid:  0xCAFE_BABE,
            mtime_ns:     9_876_543,
            size:         65536,
            mode:         0o755,
            uid:          1000,
            gid:          1000,
            path:         "/Users/sanidhya/Documents/重要.pdf".to_string(),
        };

        {
            let writer = WalWriter::new_with_deadline(&path, Duration::from_secs(60)).unwrap();
            writer.append(original.clone()).unwrap();
            writer.flush().unwrap();
        }

        let result = WalReader::replay(&path).unwrap();
        assert_eq!(result.entries.len(), 1);
        let decoded = &result.entries[0];

        assert_eq!(decoded.seq,          original.seq);
        assert_eq!(decoded.timestamp_us, original.timestamp_us);
        assert_eq!(decoded.event_type,   original.event_type);
        assert_eq!(decoded.flags,        original.flags);
        assert_eq!(decoded.doc_id,       original.doc_id);
        assert_eq!(decoded.inode,        original.inode);
        assert_eq!(decoded.volume_uuid,  original.volume_uuid);
        assert_eq!(decoded.mtime_ns,     original.mtime_ns);
        assert_eq!(decoded.size,         original.size);
        assert_eq!(decoded.mode,         original.mode);
        assert_eq!(decoded.uid,          original.uid);
        assert_eq!(decoded.gid,          original.gid);
        assert_eq!(decoded.path,         original.path);
    }
}
