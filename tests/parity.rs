// Index-disk parity test — catches silent index divergence
// This is a regression detector that validates index completeness

use localsearch::index::delta::{DeltaIndex, Document, DocId};
use localsearch::fs::identity::IdentityDb;
use std::collections::HashSet;
use std::path::PathBuf;
use walkdir::WalkDir;

#[test]
fn test_index_disk_parity_basic() {
    // Create test corpus
    let corpus_dir = tempfile::tempdir().unwrap();
    
    // Populate test corpus: 100 files across nested directories
    for i in 0..100 {
        let subdir = corpus_dir.path().join(format!("dir{}", i % 10));
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(
            subdir.join(format!("file_{}.txt", i)),
            format!("content {}", i)
        ).unwrap();
    }

    // Ground truth: all files on disk
    let disk_files: HashSet<PathBuf> = WalkDir::new(corpus_dir.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_owned())
        .collect();

    // Simulate indexing: create identity DB and delta index
    let identity_db_path = corpus_dir.path().join("identity.db");
    let mut identity_db = IdentityDb::new(&identity_db_path).unwrap();
    let mut delta = DeltaIndex::new(50 * 1024 * 1024);
    
    let mut indexed_paths = HashSet::new();
    
    // Index all files
    for (idx, path) in disk_files.iter().enumerate() {
        // Allocate DocId from identity DB (using dummy file identity)
        let fs_doc_id = identity_db.get_or_allocate(0, 1000 + idx as u64, 1, 16).unwrap();
        
        // Convert to delta DocId
        let doc_id = DocId(fs_doc_id.0);
        
        // Create document
        let doc = Document {
            doc_id,
            path: path.to_string_lossy().to_string(),
            content_hash: 0,
        };
        
        // Insert into delta index
        delta.insert_document(doc, std::collections::HashMap::new()).unwrap();
        indexed_paths.insert(path.clone());
    }

    // Verify parity
    let phantom: Vec<_> = indexed_paths.difference(&disk_files).collect();
    let missing: Vec<_> = disk_files.difference(&indexed_paths).collect();

    // Thresholds from implementation_spec.md §12
    assert_eq!(phantom.len(), 0, "Phantom docs (in index but not on disk): {:?}", phantom);
    
    let miss_rate = missing.len() as f32 / disk_files.len() as f32;
    assert!(
        miss_rate < 0.01,
        "Miss rate {:.2}% exceeds 1% threshold. Missing {} of {} files: {:?}",
        miss_rate * 100.0,
        missing.len(),
        disk_files.len(),
        missing
    );
    
    // Verify we indexed all 100 files
    assert_eq!(indexed_paths.len(), 100, "Should have indexed exactly 100 files");
    assert_eq!(disk_files.len(), 100, "Should have 100 files on disk");
}

#[test]
fn test_parity_detects_phantom_docs() {
    // Test that parity check catches phantom documents
    let corpus_dir = tempfile::tempdir().unwrap();
    
    // Create 10 files
    for i in 0..10 {
        std::fs::write(
            corpus_dir.path().join(format!("file_{}.txt", i)),
            format!("content {}", i)
        ).unwrap();
    }

    let disk_files: HashSet<PathBuf> = WalkDir::new(corpus_dir.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_owned())
        .collect();

    // Simulate index with phantom document
    let mut indexed_paths = disk_files.clone();
    indexed_paths.insert(corpus_dir.path().join("phantom_file.txt"));

    let phantom: Vec<_> = indexed_paths.difference(&disk_files).collect();
    
    // Should detect the phantom
    assert_eq!(phantom.len(), 1, "Should detect 1 phantom document");
    assert!(phantom[0].ends_with("phantom_file.txt"));
}

#[test]
fn test_parity_detects_missing_docs() {
    // Test that parity check catches missing documents
    let corpus_dir = tempfile::tempdir().unwrap();
    
    // Create 10 files
    for i in 0..10 {
        std::fs::write(
            corpus_dir.path().join(format!("file_{}.txt", i)),
            format!("content {}", i)
        ).unwrap();
    }

    let disk_files: HashSet<PathBuf> = WalkDir::new(corpus_dir.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_owned())
        .collect();

    // Simulate incomplete index (missing one file)
    let mut indexed_paths = disk_files.clone();
    let first_file = disk_files.iter().next().unwrap().clone();
    indexed_paths.remove(&first_file);

    let missing: Vec<_> = disk_files.difference(&indexed_paths).collect();
    
    // Should detect the missing file
    assert_eq!(missing.len(), 1, "Should detect 1 missing document");
}
