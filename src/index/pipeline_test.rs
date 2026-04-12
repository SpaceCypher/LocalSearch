use crate::wal::{entry, writer::WalWriter, reader::WalReader};
use crate::query::parser::Tokenizer;
use crate::index::delta::{DeltaIndex, Document, Posting, DocId};
use tempfile::TempDir;
use std::collections::HashMap;

#[test]
fn test_wal_to_delta_index_pipeline() {
    // Setup: temp directory for WAL
    let temp_dir = TempDir::new().unwrap();
    let wal_path = temp_dir.path().join("test.wal");
    
    // Step 1: Write WAL entries manually
    let wal_writer = WalWriter::new(&wal_path).unwrap();
    
    // Create 3 test documents
    let entries = vec![
        entry::WalEntry {
            seq: 1,
            timestamp_us: 1000,
            event_type: entry::EventType::Created,
            flags: 0,
            doc_id: 1,
            inode: 100,
            volume_uuid: 0x1234,
            mtime_ns: 1000,
            size: 1024,
            mode: 0o644,
            uid: 501,
            gid: 20,
            path: "/test/quarterly_report.pdf".to_string(),
        },
        entry::WalEntry {
            seq: 2,
            timestamp_us: 2000,
            event_type: entry::EventType::Created,
            flags: 0,
            doc_id: 2,
            inode: 101,
            volume_uuid: 0x1234,
            mtime_ns: 2000,
            size: 2048,
            mode: 0o644,
            uid: 501,
            gid: 20,
            path: "/test/budget.xlsx".to_string(),
        },
        entry::WalEntry {
            seq: 3,
            timestamp_us: 3000,
            event_type: entry::EventType::Created,
            flags: 0,
            doc_id: 3,
            inode: 102,
            volume_uuid: 0x1234,
            mtime_ns: 3000,
            size: 512,
            mode: 0o644,
            uid: 501,
            gid: 20,
            path: "/test/meeting_notes.txt".to_string(),
        },
    ];
    
    for entry in entries {
        wal_writer.append(entry).unwrap();
    }
    wal_writer.flush().unwrap();
    
    // Step 2: Run indexing pipeline
    let mut wal_reader = WalReader::new(&wal_path).unwrap();
    let wal_entries = wal_reader.replay_from(0).unwrap();
    
    let mut delta_index = DeltaIndex::new(50 * 1024 * 1024); // 50MB budget
    let tokenizer = Tokenizer::new();
    
    for entry in wal_entries {
        // Extract filename from path
        let filename = std::path::Path::new(&entry.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        
        // Tokenize filename
        let tokens = tokenizer.tokenize(filename);
        
        // Create document
        let doc = Document {
            doc_id: DocId(entry.doc_id),
            path: entry.path.clone(),
            content_hash: 0,
        };
        
        // Create postings from tokens
        let mut postings = HashMap::new();
        for token in tokens.iter() {
            postings.insert(
                token.term.clone(),
                Posting {
                    doc_id: DocId(entry.doc_id),
                    term_freq: 1,
                    field_mask: 0x01, // FIELD_FILENAME
                    positions: vec![token.position],
                },
            );
        }
        
        // Insert into delta index
        delta_index.insert_document(doc, postings).unwrap();
    }
    
    // Step 3: Query delta index and verify results
    
    // Query for "quarterly" - should find doc 1 (stemmed to "quarter")
    let results = delta_index.lookup("quarter");
    assert!(results.is_some(), "Should find 'quarter' in delta index");
    let postings = results.unwrap();
    assert_eq!(postings.postings.len(), 1);
    assert_eq!(postings.postings[0].doc_id, DocId(1));
    
    // Query for "budget" - should find doc 2
    let results = delta_index.lookup("budget");
    assert!(results.is_some(), "Should find 'budget' in delta index");
    let postings = results.unwrap();
    assert_eq!(postings.postings.len(), 1);
    assert_eq!(postings.postings[0].doc_id, DocId(2));
    
    // Query for "meeting" - should find doc 3 (stemmed to "meet")
    let results = delta_index.lookup("meet");
    assert!(results.is_some(), "Should find 'meet' in delta index");
    let postings = results.unwrap();
    assert_eq!(postings.postings.len(), 1);
    assert_eq!(postings.postings[0].doc_id, DocId(3));
    
    // Query for non-existent term
    let results = delta_index.lookup("nonexistent");
    assert!(results.is_none());
}
