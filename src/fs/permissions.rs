use std::path::{Path, PathBuf};
use std::collections::HashMap;
use rusqlite::{Connection, params};
use anyhow::Result;

/// Represents the accessibility state of a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    Accessible,
    Revoked,
    Hidden,
}

/// Tracks path accessibility state transitions.
pub struct PermissionTracker {
    states: HashMap<PathBuf, PermissionState>,
}

impl PermissionTracker {
    pub fn new() -> Self {
        PermissionTracker {
            states: HashMap::new(),
        }
    }

    pub fn mark_accessible(&mut self, path: PathBuf) {
        self.states.insert(path, PermissionState::Accessible);
    }

    pub fn mark_revoked(&mut self, path: PathBuf) {
        self.states.insert(path, PermissionState::Revoked);
    }

    pub fn mark_hidden(&mut self, path: PathBuf) {
        self.states.insert(path, PermissionState::Hidden);
    }

    pub fn state(&self, path: &Path) -> PermissionState {
        self.states.get(path).copied().unwrap_or(PermissionState::Accessible)
    }
}

/// Returns true if the path contains sensitive patterns that should NEVER be indexed.
pub fn is_never_index_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    
    // Security-sensitive patterns (e.g., credentials, keys, private configs)
    let sensitive_patterns = [
        "/.ssh/",
        "/Library/Keychains/",
        "/.aws/",
        "/.config/gcloud/",
        "/.gnupg/",
        "/Keychains/",
    ];
    
    for pattern in &sensitive_patterns {
        if path_str.contains(pattern) {
            return true;
        }
    }
    
    // File extensions for secrets/certificates
    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        match ext_str.as_str() {
            "pem" | "key" | "p12" | "pfx" | "asc" => return true,
            _ => {}
        }
    }
    
    // Exact filename matches for sensitive files
    if let Some(filename) = path.file_name() {
        let name_str = filename.to_string_lossy().to_lowercase();
        match name_str.as_str() {
            ".env" | "id_rsa" | "id_ed25519" | "id_dsa" | "id_ecdsa" => return true,
            _ => {}
        }
    }
    
    false
}

/// SQLite-backed audit log for permission transitions.
pub struct PermissionAuditLog {
    conn: Connection,
}

impl PermissionAuditLog {
    pub fn new(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS permission_audit (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                transition TEXT NOT NULL,
                error_code TEXT,
                timestamp INTEGER NOT NULL
            )",
            [],
        )?;
        Ok(PermissionAuditLog { conn })
    }

    pub fn record(&self, path: &Path, error_code: Option<&str>, transition: &str) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        
        self.conn.execute(
            "INSERT INTO permission_audit (path, transition, error_code, timestamp)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                path.to_string_lossy(),
                transition,
                error_code,
                now as i64,
            ],
        )?;
        Ok(())
    }

    pub fn recent(&self, days: u32) -> Result<Vec<AuditEntry>> {
        let cutoff = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() - (days as u64 * 86400);
            
        let mut stmt = self.conn.prepare(
            "SELECT path, transition, error_code, timestamp FROM permission_audit 
             WHERE timestamp > ?1 ORDER BY timestamp DESC"
        )?;
        
        let rows = stmt.query_map(params![cutoff as i64], |row| {
            Ok(AuditEntry {
                path: row.get(0)?,
                transition: row.get(1)?,
                error_code: row.get(2)?,
                timestamp: row.get(3)?,
            })
        })?;
        
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub path: String,
    pub transition: String,
    pub error_code: Option<String>,
    pub timestamp: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_permission_state_transitions() {
        let mut tracker = PermissionTracker::new();
        let path = PathBuf::from("/Users/test/secret.txt");
        
        tracker.mark_accessible(path.clone());
        assert_eq!(tracker.state(&path), PermissionState::Accessible);
        
        tracker.mark_revoked(path.clone());
        assert_eq!(tracker.state(&path), PermissionState::Revoked);
        
        tracker.mark_hidden(path.clone());
        assert_eq!(tracker.state(&path), PermissionState::Hidden);
    }

    #[test]
    fn test_never_index_path_patterns() {
        assert!(is_never_index_path(Path::new("/Users/test/.ssh/id_rsa")));
        assert!(is_never_index_path(Path::new("/Users/test/Library/Keychains/login.keychain")));
        assert!(is_never_index_path(Path::new("/Users/test/.aws/credentials")));
        assert!(is_never_index_path(Path::new("/Users/test/secret.pem")));
        assert!(is_never_index_path(Path::new("/Users/test/private.key")));
        assert!(is_never_index_path(Path::new("/Users/test/.env")));
        
        assert!(!is_never_index_path(Path::new("/Users/test/Documents/report.pdf")));
        assert!(!is_never_index_path(Path::new("/Users/test/Desktop/notes.txt")));
    }

    #[test]
    fn test_permission_audit_log_persistence() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("audit.db");
        let log = PermissionAuditLog::new(&log_path).unwrap();
        
        let path = Path::new("/Users/test/denied.txt");
        log.record(path, Some("EACCES"), "ACCESSIBLE→REVOKED").unwrap();
        
        let recent = log.recent(7).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].path, "/Users/test/denied.txt");
        assert_eq!(recent[0].transition, "ACCESSIBLE→REVOKED");
        assert_eq!(recent[0].error_code, Some("EACCES".to_string()));
    }
}
