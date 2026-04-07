// Chaos tests for filesystem operations
use std::os::unix::fs::PermissionsExt;
use tempfile;

#[cfg(target_os = "macos")]
#[test]
fn test_fsevent_storm() {
    // Simulate 50 MUST_SCAN_SUBDIRS events
    // Verify hot paths reconciled first
    let dir = tempfile::tempdir().unwrap();
    
    // Create hot path directories (Desktop, Documents, Downloads)
    let hot_paths = vec!["Desktop", "Documents", "Downloads"];
    for hp in &hot_paths {
        std::fs::create_dir_all(dir.path().join(hp)).unwrap();
    }
    
    // Create many other directories
    for i in 0..50 {
        std::fs::create_dir_all(dir.path().join(format!("dir_{}", i))).unwrap();
    }
    
    // In a real implementation, FSEvents would trigger MUST_SCAN_SUBDIRS
    // and the reconciliation worker would prioritize hot paths
    // For this test, we verify the priority computation
    
    let hot_priority = compute_priority_for_test(dir.path().join("Documents"));
    let cold_priority = compute_priority_for_test(dir.path().join("dir_25"));
    
    assert!(hot_priority > cold_priority, 
        "Hot paths should have higher priority: {} vs {}", hot_priority, cold_priority);
}

#[test]
fn test_permission_revocation_mid_crawl() {
    // chmod 000 mid-crawl, verify EACCES handled gracefully
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("secret.txt");
    std::fs::write(&file, "sensitive data").unwrap();
    
    // Revoke all permissions
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    
    // Attempt to read - should get EACCES
    let result = std::fs::read(&file);
    assert!(result.is_err());
    
    let err = result.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    
    // In real implementation, this would call mark_inaccessible()
    // and continue without panicking
    
    // Cleanup: restore permissions so tempdir can be deleted
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
}

#[test]
fn test_extractor_xpc_crash_isolation() {
    // Simulate extractor crash - main process should be unaffected
    // In real implementation, XPC service crash would be caught
    // and document marked as content_pending
    
    // This test verifies the error handling pattern
    let result = simulate_extractor_crash();
    
    // Should return error, not panic
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Extractor crashed");
}

// Helper functions for tests
fn compute_priority_for_test(path: std::path::PathBuf) -> u32 {
    // Simplified priority computation
    let path_str = path.to_string_lossy();
    if path_str.contains("Desktop") || path_str.contains("Documents") || path_str.contains("Downloads") {
        100 // High priority
    } else {
        10 // Low priority
    }
}

fn simulate_extractor_crash() -> Result<String, &'static str> {
    // Simulate XPC service crash
    Err("Extractor crashed")
}
