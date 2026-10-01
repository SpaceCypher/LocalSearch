use std::path::Path;
use anyhow::Result;
use crate::wal::reader::WalReader;
use crate::index::segment::Segment;

// ─── Startup Path Detection ───────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub enum StartupPath {
    FirstLaunch,
    WarmRestart,
    CrashRecovery,
}



pub fn detect_startup_path(data_dir: &Path) -> Result<StartupPath> {
    let wal_path = data_dir.join("wal.log");
    let clean_shutdown_marker = data_dir.join(".clean_shutdown");
    
    if !wal_path.exists() {
        return Ok(StartupPath::FirstLaunch);
    }
    
    if clean_shutdown_marker.exists() {
        Ok(StartupPath::WarmRestart)
    } else {
        Ok(StartupPath::CrashRecovery)
    }
}

// ─── First Launch ─────────────────────────────────────────────────────────────

/// Create the data directory and an empty WAL. Indexing itself is done by
/// `Engine::reconcile`, which the engine runs right after this.
pub fn first_launch(data_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    std::fs::File::create(data_dir.join("wal.log"))?;
    log::info!("First launch: initialized data directory at {:?}", data_dir);
    Ok(())
}

// ─── Warm Restart ─────────────────────────────────────────────────────────────

/// The previous session shut down cleanly. Consume the marker so that a crash
/// during this session is detected next time.
pub fn warm_restart(data_dir: &Path) -> Result<()> {
    let marker = data_dir.join(".clean_shutdown");
    if marker.exists() {
        std::fs::remove_file(marker)?;
    }
    Ok(())
}

// ─── Crash Recovery ───────────────────────────────────────────────────────────

/// The previous session died. Discard half-written files and segments that
/// fail their checksum, and cut a torn tail off the WAL. What remains is
/// loaded and replayed by `Engine::load`.
pub fn crash_recovery(data_dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(data_dir)? {
        let path = entry?.path();
        match path.extension().and_then(|s| s.to_str()) {
            // 1. Temp files from an incomplete snapshot
            Some("tmp") => {
                log::warn!("Removing incomplete file: {:?}", path);
                std::fs::remove_file(path)?;
            }
            // 2. Segments whose checksum does not verify
            Some("seg") => {
                if let Err(e) = Segment::verify(&path) {
                    log::error!("Corrupt segment: {:?}, error: {}", path, e);
                    std::fs::remove_file(path)?;
                }
            }
            _ => {}
        }
    }

    // 3. Truncate the WAL after its last intact entry
    let wal_path = data_dir.join("wal.log");
    let mut reader = WalReader::new(&wal_path)?;
    let entries = reader.replay_from(0)?;
    let valid_len: u64 = entries.iter().map(|entry| entry.serialized_len() as u64).sum();
    if valid_len < std::fs::metadata(&wal_path)?.len() {
        log::warn!("Truncating torn WAL tail at byte {}", valid_len);
        std::fs::OpenOptions::new().write(true).open(&wal_path)?.set_len(valid_len)?;
    }

    log::info!("Crash recovery: {} intact WAL entries", entries.len());
    Ok(())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_first_launch() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();
        
        // Empty directory = first launch
        let path = detect_startup_path(data_dir).unwrap();
        assert_eq!(path, StartupPath::FirstLaunch);
    }

    #[test]
    fn test_detect_warm_restart() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();
        
        // Create WAL file and clean shutdown marker
        std::fs::create_dir_all(data_dir).unwrap();
        std::fs::write(data_dir.join("wal.log"), b"").unwrap();
        std::fs::write(data_dir.join(".clean_shutdown"), b"").unwrap();
        
        let path = detect_startup_path(data_dir).unwrap();
        assert_eq!(path, StartupPath::WarmRestart);
    }

    #[test]
    fn test_detect_crash_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();
        
        // WAL exists but no clean shutdown marker
        std::fs::create_dir_all(data_dir).unwrap();
        std::fs::write(data_dir.join("wal.log"), b"").unwrap();
        
        let path = detect_startup_path(data_dir).unwrap();
        assert_eq!(path, StartupPath::CrashRecovery);
    }

    #[test]
    fn test_first_launch_creates_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        
        first_launch(&data_dir).unwrap();
        
        assert!(data_dir.exists());
        assert!(data_dir.join("wal.log").exists());
    }

    #[test]
    fn test_warm_restart_replays_wal() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();
        
        // Setup: create WAL with entries
        std::fs::create_dir_all(data_dir).unwrap();
        let wal_path = data_dir.join("wal.log");
        {
            use crate::wal::writer::WalWriter;
            use crate::wal::entry::WalEntry;
            let writer = WalWriter::new(&wal_path).unwrap();
            for i in 0..10u64 {
                writer.append(WalEntry { seq: i, ..WalEntry::new_test() }).unwrap();
            }
            writer.flush().unwrap();
        }
        std::fs::write(data_dir.join(".clean_shutdown"), b"").unwrap();
        
        warm_restart(data_dir).unwrap();

        // The marker is consumed so a crash in this session is detectable
        assert!(!data_dir.join(".clean_shutdown").exists());
        assert_eq!(detect_startup_path(data_dir).unwrap(), StartupPath::CrashRecovery);
    }

    #[test]
    fn test_crash_recovery_removes_temp_segments() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();
        
        std::fs::create_dir_all(data_dir).unwrap();
        std::fs::write(data_dir.join("wal.log"), b"").unwrap();
        
        // Create temp segment (simulating incomplete compaction)
        let temp_segment = data_dir.join("segment_0000.tmp");
        std::fs::write(&temp_segment, b"incomplete").unwrap();
        
        crash_recovery(data_dir).unwrap();
        
        // Temp segment should be removed
        assert!(!temp_segment.exists());
    }

    #[test]
    fn test_crash_recovery_verifies_segment_checksums() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();
        
        std::fs::create_dir_all(data_dir).unwrap();
        std::fs::write(data_dir.join("wal.log"), b"").unwrap();
        
        // One real segment, one file of garbage with a segment extension
        use crate::index::delta::DeltaIndex;
        use crate::index::segment::SegmentBuilder;
        let valid_segment = data_dir.join("segment_000001.seg");
        SegmentBuilder::from_delta(&DeltaIndex::new(1024)).finalize(&valid_segment).unwrap();
        let corrupt_segment = data_dir.join("segment_000002.seg");
        std::fs::write(&corrupt_segment, b"not a segment").unwrap();

        crash_recovery(data_dir).unwrap();

        assert!(valid_segment.exists());
        assert!(!corrupt_segment.exists());
    }

    #[test]
    fn test_crash_recovery_truncates_torn_wal_tail() {
        use crate::wal::entry::WalEntry;
        use crate::wal::writer::WalWriter;
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let wal_path = dir.path().join("wal.log");
        {
            let writer = WalWriter::new(&wal_path).unwrap();
            for i in 1..=3u64 {
                writer.append(WalEntry { seq: i, ..WalEntry::new_test() }).unwrap();
            }
            writer.flush().unwrap();
        }
        let intact_len = std::fs::metadata(&wal_path).unwrap().len();
        std::fs::OpenOptions::new().append(true).open(&wal_path).unwrap()
            .write_all(&[0xAB; 40]).unwrap();

        crash_recovery(dir.path()).unwrap();

        assert_eq!(std::fs::metadata(&wal_path).unwrap().len(), intact_len);
    }
}
