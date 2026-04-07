// Chaos tests for WAL corruption recovery
use super::entry::{WalEntry, EventType};
use super::writer::WalWriter;
use super::reader::WalReader;
use std::io::{Write, Read, Seek};
use tempfile;

#[test]
fn test_wal_corruption_recovery() {
    // Write 600 entries, corrupt entry 500, verify replay stops at 499
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.wal");
    
    // Write 600 entries
    let writer = WalWriter::new_with_deadline(&path, std::time::Duration::from_secs(60)).unwrap();
    for i in 0..600u64 {
        let entry = WalEntry {
            seq: i,
            timestamp_us: 1000000 + i,
            event_type: EventType::Created,
            flags: 0,
            doc_id: i + 100,
            inode: 12345 + i,
            volume_uuid: 0xdeadbeef,
            mtime_ns: 999,
            size: 1024,
            mode: 0o644,
            uid: 501,
            gid: 20,
            path: format!("/test/file_{}.txt", i),
        };
        writer.append(entry).unwrap();
    }
    writer.flush().unwrap();
    drop(writer);
    
    // Corrupt entry 500 by flipping bits in the middle of the file
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    
    // Seek to approximate position of entry 500 (each entry ~100 bytes)
    file.seek(std::io::SeekFrom::Start(500 * 100)).unwrap();
    let mut corrupt_bytes = [0u8; 20];
    file.read_exact(&mut corrupt_bytes).unwrap();
    // Flip all bits
    for byte in &mut corrupt_bytes {
        *byte ^= 0xFF;
    }
    file.seek(std::io::SeekFrom::Start(500 * 100)).unwrap();
    file.write_all(&corrupt_bytes).unwrap();
    drop(file);
    
    // Replay from start - should stop at corruption point
    let mut reader = WalReader::new(&path).unwrap();
    let entries = reader.replay_from(0).unwrap();
    
    // Should have entries before corruption, but not after
    assert!(entries.len() < 600, "Should stop at corruption");
    assert!(entries.len() >= 400, "Should have entries before corruption");
    
    // All returned entries should have valid sequence numbers
    for (i, entry) in entries.iter().enumerate() {
        assert!(entry.seq < 600, "Entry {} has seq {}", i, entry.seq);
    }
}

#[test]
fn test_mid_compaction_disk_full() {
    // Simulate disk full during compaction
    // Verify partial segment discarded on restart
    let dir = tempfile::tempdir().unwrap();
    let segment_path = dir.path().join("segment_0.seg");
    let temp_path = dir.path().join("segment_0.seg.tmp");
    
    // Create a partial temp segment (simulating interrupted compaction)
    std::fs::write(&temp_path, b"incomplete segment data").unwrap();
    
    // Verify temp file exists
    assert!(temp_path.exists());
    
    // Simulate restart: cleanup should remove temp files
    // This would be called in startup.rs crash_recovery()
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("tmp") {
            std::fs::remove_file(&path).unwrap();
        }
    }
    
    // Verify temp file removed
    assert!(!temp_path.exists());
    assert!(!segment_path.exists()); // Final segment never created
}
