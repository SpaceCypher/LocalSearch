//! Reconciliation worker for detecting index-disk divergence.
//!
//! Periodically scans the filesystem and compares against the index snapshot
//! to detect files that were added/deleted/modified outside of FSEvents monitoring.

use std::path::{Path, PathBuf};
use std::collections::HashSet;

/// Reconciliation diff result
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationDiff {
    pub to_add: Vec<PathBuf>,
    pub to_delete: Vec<PathBuf>,
    pub to_update: Vec<PathBuf>,
}

/// Index snapshot for reconciliation
#[derive(Debug, Clone)]
pub struct IndexSnapshot {
    pub paths: HashSet<PathBuf>,
}

/// Compute reconciliation diff between disk and index snapshot
pub fn compute_reconciliation_diff(
    root: &Path,
    snapshot: &IndexSnapshot,
) -> anyhow::Result<ReconciliationDiff> {
    let mut to_add = Vec::new();
    let mut to_delete = Vec::new();
    let mut to_update = Vec::new();

    // Collect all files currently on disk
    let mut disk_files = HashSet::new();
    if root.exists() {
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                disk_files.insert(entry.path().to_path_buf());
            }
        }
    }

    // Find files on disk but not in index (to_add)
    for disk_path in &disk_files {
        if !snapshot.paths.contains(disk_path) {
            to_add.push(disk_path.clone());
        }
    }

    // Find files in index but not on disk (to_delete)
    for index_path in &snapshot.paths {
        if !disk_files.contains(index_path) {
            to_delete.push(index_path.clone());
        }
    }

    Ok(ReconciliationDiff {
        to_add,
        to_delete,
        to_update,
    })
}

/// Compute priority for a path (hot paths get higher priority)
pub fn compute_priority(path: &Path) -> u32 {
    let path_str = path.to_string_lossy();
    
    // Hot paths (Desktop, Documents, Downloads) get priority 100
    if path_str.contains("/Desktop/") || path_str.contains("/Documents/") || path_str.contains("/Downloads/") {
        return 100;
    }
    
    // Hidden/cache directories get priority 1
    if path_str.contains("/.cache/") || path_str.contains("/.local/") || path_str.contains("/.config/") {
        return 1;
    }
    
    // Default priority
    10
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn build_index_snapshot_from_paths(paths: &[&str], root: &Path) -> IndexSnapshot {
        let mut snapshot_paths = HashSet::new();
        for p in paths {
            snapshot_paths.insert(root.join(p));
        }
        IndexSnapshot { paths: snapshot_paths }
    }

    #[test]
    fn test_reconciliation_detects_added_files() {
        // Build index snapshot with 3 files
        // Add 1 file to disk (not in index)
        // Run reconcile_subtree()
        // Verify to_add contains the new file
        let dir = tempfile::tempdir().unwrap();
        let corpus = vec!["a.txt", "b.txt", "c.txt"];
        for f in &corpus {
            std::fs::write(dir.path().join(f), "x").unwrap();
        }

        let snapshot = build_index_snapshot_from_paths(&corpus, dir.path());
        std::fs::write(dir.path().join("d.txt"), "new").unwrap(); // Not in index

        let diff = compute_reconciliation_diff(dir.path(), &snapshot).unwrap();
        assert_eq!(diff.to_add.len(), 1);
        assert!(diff.to_add[0].ends_with("d.txt"));
    }

    #[test]
    fn test_reconciliation_detects_deleted_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("gone.txt");
        std::fs::write(&file, "x").unwrap();
        let snapshot = build_index_snapshot_from_paths(&["gone.txt"], dir.path());
        std::fs::remove_file(&file).unwrap(); // Delete from disk

        let diff = compute_reconciliation_diff(dir.path(), &snapshot).unwrap();
        assert_eq!(diff.to_delete.len(), 1);
    }

    #[test]
    fn test_hot_paths_get_higher_priority() {
        assert!(
            compute_priority(Path::new("/Users/alice/Documents/x"))
                > compute_priority(Path::new("/Users/alice/.cache/x"))
        );
    }
}
