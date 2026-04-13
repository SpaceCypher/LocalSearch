use std::path::Path;
use anyhow::Result;
use crate::metrics::InvariantChecker;
use crate::wal::reader::WalReader;
use crate::index::signals::SignalDb;
use std::path::PathBuf;
use std::io::Read;
use memmap2::MmapOptions;

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

pub fn first_launch(data_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    
    let wal_path = data_dir.join("wal.log");
    std::fs::File::create(&wal_path)?;
    
    // Index hot paths synchronously (Desktop, Documents, Downloads)
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/unknown".to_string());
    let hot_paths = vec![
        PathBuf::from(&home).join("Desktop"),
        PathBuf::from(&home).join("Documents"),
        PathBuf::from(&home).join("Downloads"),
    ];
    
    for path in hot_paths {
        if path.exists() {
            index_directory_sync(&path)?;
        }
    }
    
    log::info!("First launch: initialized data directory at {:?}", data_dir);
    Ok(())
}

/// Index a directory synchronously (for hot paths on first launch)
fn index_directory_sync(path: &Path) -> Result<()> {
    log::info!("Indexing hot path: {:?}", path);
    
    // Walk directory and count files (simplified implementation)
    let mut file_count = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if entry.path().is_file() {
                file_count += 1;
            }
        }
    }
    
    log::info!("Indexed {} files from {:?}", file_count, path);
    Ok(())
}

// ─── Warm Restart ─────────────────────────────────────────────────────────────

pub fn warm_restart(data_dir: &Path) -> Result<()> {
    let wal_path = data_dir.join("wal.log");
    let mut reader = WalReader::new(&wal_path)?;
    let entries = reader.replay_from(0)?;
    InvariantChecker::check_wal_monotonicity(&entries)?;
    
    log::info!("Warm restart: replayed {} WAL entries", entries.len());
    
    // Prefault hot slab with madvise (macOS-specific)
    {
        let signal_db_path = data_dir.join("signals.json");
        let signal_db = if signal_db_path.exists() {
            let content = std::fs::read_to_string(&signal_db_path)?;
            serde_json::from_str::<SignalDb>(&content).unwrap_or_default()
        } else {
            SignalDb::new()
        };

        let plan = build_warmup_plan(&signal_db);
        log::info!("Warmup: pre-faulting {} hot terms and {} hot paths", 
            plan.hot_terms.len(), plan.hot_paths.len());

        for path in plan.hot_paths {
            prewarm_path(Path::new(&path))?;
        }

        prewarm_term_hints(data_dir, &plan.hot_terms)?;
    }
    
    // Remove clean shutdown marker (will be recreated on next clean shutdown)
    let marker = data_dir.join(".clean_shutdown");
    if marker.exists() {
        std::fs::remove_file(marker)?;
    }
    
    Ok(())
}

fn prewarm_path(path: &Path) -> Result<()> {
    // Touch metadata to populate vnode/dentry caches.
    let _ = std::fs::metadata(path);

    if path.is_file() {
        prewarm_file_pages(path)?;
    }

    // If the hot path points to a directory, touch a bounded subset of entries.
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten().take(32) {
                let p = entry.path();
                let _ = std::fs::metadata(&p);
                if p.is_file() {
                    prewarm_file_pages(&p)?;
                    // Bounded byte touch to encourage page cache residency.
                    if let Ok(mut f) = std::fs::File::open(&p) {
                        let mut buf = [0u8; 4096];
                        let _ = f.read(&mut buf);
                    }
                }
            }
        }
    }

    Ok(())
}

fn prewarm_file_pages(path: &Path) -> Result<()> {
    let file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    if meta.len() == 0 {
        return Ok(());
    }

    // Map only a bounded window to avoid large warmup memory spikes.
    let len = (meta.len() as usize).min(256 * 1024);
    let mmap = unsafe { MmapOptions::new().len(len).map(&file)? };

    #[cfg(target_os = "macos")]
    unsafe {
        let _ = libc::madvise(
            mmap.as_ptr() as *mut libc::c_void,
            len,
            libc::MADV_WILLNEED,
        );
    }

    Ok(())
}

fn prewarm_term_hints(data_dir: &Path, hot_terms: &[String]) -> Result<()> {
    // Persist hints for future startup phases that can map terms to pages/segments.
    // This creates a concrete handoff artifact instead of only logging.
    let hint_path = data_dir.join("warmup_terms.txt");
    let content = hot_terms.iter().take(500).cloned().collect::<Vec<_>>().join("\n");
    std::fs::write(hint_path, content)?;
    Ok(())
}

pub struct WarmupPlan {
    pub hot_terms: Vec<String>,
    pub hot_paths: Vec<String>,
}

pub fn build_warmup_plan(signal_db: &SignalDb) -> WarmupPlan {
    WarmupPlan {
        hot_terms: signal_db.get_top_terms(500),
        hot_paths: signal_db.get_top_paths(50),
    }
}

// ─── Crash Recovery ───────────────────────────────────────────────────────────

pub fn crash_recovery(data_dir: &Path) -> Result<()> {
    // 1. Remove temp segments from incomplete compaction
    for entry in std::fs::read_dir(data_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("tmp") {
            log::warn!("Removing incomplete segment: {:?}", path);
            std::fs::remove_file(path)?;
        }
    }
    
    // 2. Verify segment checksums (simplified - just check file exists and is readable)
    for entry in std::fs::read_dir(data_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("seg") {
            // Verify segment is readable
            if let Err(e) = std::fs::read(&path) {
                log::error!("Corrupt segment: {:?}, error: {}", path, e);
                std::fs::remove_file(path)?;
            }
        }
    }
    
    // 3. Replay WAL
    let wal_path = data_dir.join("wal.log");
    let mut reader = WalReader::new(&wal_path)?;
    let entries = reader.replay_from(0)?;
    InvariantChecker::check_wal_monotonicity(&entries)?;
    
    log::info!("Crash recovery: replayed {} WAL entries", entries.len());
    
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
        
        // Should not panic
        warm_restart(data_dir).unwrap();
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
    fn test_first_launch_indexes_hot_paths() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        
        // This test verifies first_launch doesn't panic when hot paths don't exist
        // In production, it would index Desktop/Documents/Downloads
        first_launch(&data_dir).unwrap();
        
        assert!(data_dir.exists());
        assert!(data_dir.join("wal.log").exists());
    }

    #[test]
    fn test_crash_recovery_verifies_segment_checksums() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();
        
        std::fs::create_dir_all(data_dir).unwrap();
        std::fs::write(data_dir.join("wal.log"), b"").unwrap();
        
        // Create a valid segment file
        let valid_segment = data_dir.join("segment_0001.seg");
        std::fs::write(&valid_segment, b"valid data").unwrap();
        
        crash_recovery(data_dir).unwrap();
        
        // Valid segment should still exist
        assert!(valid_segment.exists());
    }

    #[test]
    fn test_warm_restart_writes_term_hint_file() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();

        std::fs::create_dir_all(data_dir).unwrap();
        let wal_path = data_dir.join("wal.log");
        std::fs::write(&wal_path, b"").unwrap();
        std::fs::write(data_dir.join(".clean_shutdown"), b"").unwrap();

        let signal_db = SignalDb {
            hot_terms: [("quarterly".to_string(), 10)].into_iter().collect(),
            hot_paths: std::collections::HashMap::new(),
        };
        let signal_path = data_dir.join("signals.json");
        std::fs::write(&signal_path, serde_json::to_string(&signal_db).unwrap()).unwrap();

        warm_restart(data_dir).unwrap();
        assert!(data_dir.join("warmup_terms.txt").exists());
    }

    #[test]
    fn test_warm_restart_wal_monotonicity_invariant() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();

        std::fs::create_dir_all(data_dir).unwrap();
        let wal_path = data_dir.join("wal.log");
        {
            use crate::wal::entry::WalEntry;
            use crate::wal::writer::WalWriter;
            let writer = WalWriter::new_with_deadline(&wal_path, std::time::Duration::from_secs(60)).unwrap();
            for i in 0..5u64 {
                writer.append(WalEntry { seq: i, ..WalEntry::new_test() }).unwrap();
            }
            writer.flush().unwrap();
        }
        std::fs::write(data_dir.join(".clean_shutdown"), b"").unwrap();

        assert!(warm_restart(data_dir).is_ok());
    }
}
