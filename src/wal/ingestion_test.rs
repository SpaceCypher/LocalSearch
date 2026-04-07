use super::*;
use crate::fs::identity::{IdentityDb, DocId};
use crate::wal::writer::WalWriter;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_wal_ingestion_pipeline() {
    // Setup: temp directory for WAL and identity DB
    let temp_dir = TempDir::new().unwrap();
    let wal_path = temp_dir.path().join("test.wal");
    let identity_db_path = temp_dir.path().join("identity.db");
    
    // Create identity DB and WAL writer
    let mut identity_db = IdentityDb::new(&identity_db_path).unwrap();
    let wal_writer = WalWriter::new(&wal_path).unwrap();
    
    // Simulate file identity (volume_uuid, inode, generation, device_id)
    // In real usage, these would come from file system metadata
    let volume_uuid = 0x1234567890ABCDEF_u128;
    let inode = 42_u64;
    let generation = 1_u32;
    let device_id = 1_u32;
    
    // Get or allocate DocId through identity DB
    let doc_id = identity_db.get_or_allocate(volume_uuid, inode, generation, device_id).unwrap();
    
    // Create WAL entry
    let entry = entry::WalEntry {
        seq: 1,
        timestamp_us: 1234567890,
        event_type: entry::EventType::Created,
        flags: 0,
        doc_id: doc_id.0,
        inode: inode,
        volume_uuid: volume_uuid as u64,
        mtime_ns: 1234567890_u64,
        size: 1024,
        mode: 0o644,
        uid: 501,
        gid: 20,
        path: "/test/document.txt".to_string(),
    };
    
    // Write to WAL
    wal_writer.append(entry).unwrap();
    wal_writer.flush().unwrap();
    
    // Verify: read WAL and check entry exists with correct doc_id
    let mut wal_reader = reader::WalReader::new(&wal_path).unwrap();
    let entries = wal_reader.replay_from(0).unwrap();
    
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].doc_id, doc_id.0);
    assert_eq!(entries[0].path, "/test/document.txt");
    assert_eq!(entries[0].event_type, entry::EventType::Created);
    assert_eq!(entries[0].mtime_ns, 1234567890);
    assert_eq!(entries[0].size, 1024);
}

#[cfg(target_os = "macos")]
#[test]
fn test_fsevent_detection() {
    use crate::fs::events::FsEventWatcher;
    use crossbeam::channel;
    use std::time::Duration;
    
    // Setup: temp directory for test files
    let temp_dir = TempDir::new().unwrap();
    let watch_dir = temp_dir.path().join("watch");
    fs::create_dir(&watch_dir).unwrap();
    
    // Create FSEvents watcher with crossbeam channel
    let (tx, rx) = channel::unbounded();
    let _watcher = FsEventWatcher::new(&watch_dir, tx).unwrap();
    
    // Write a test file
    let test_file = watch_dir.join("test_document.txt");
    fs::write(&test_file, "test content").unwrap();
    
    // Wait for FSEvent to arrive
    let event = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    
    // Verify event was detected
    // Note: FSEvents may report the parent directory instead of the specific file
    // This is expected behavior on macOS
    let event_path_canon = event.path.canonicalize().unwrap();
    let test_file_canon = test_file.canonicalize().unwrap();
    let watch_dir_canon = watch_dir.canonicalize().unwrap();
    
    // Accept either the file path or the parent directory path
    assert!(
        event_path_canon == test_file_canon || event_path_canon == watch_dir_canon,
        "Event path should be either the file or its parent directory. Got: {:?}, expected: {:?} or {:?}",
        event_path_canon, test_file_canon, watch_dir_canon
    );
    assert_eq!(event.event_type, entry::EventType::Created);
}
