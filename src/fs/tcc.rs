use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Scope of filesystem access granted by macOS TCC (Transparency, Consent, and Control).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TccScope {
    /// App has Full Disk Access permission.
    FullDiskAccess,
    /// App is restricted to user-accessible paths (Home, Documents, etc.).
    HotPathsOnly,
    /// App access is critically restricted.
    NoAccess,
}

/// Monitors TCC permission state and manages graceful degradation.
pub struct TccMonitor {
    scope: TccScope,
    is_suspended: Arc<AtomicBool>,
}

impl TccMonitor {
    /// Creates a new TccMonitor, defaulting to FullDiskAccess until checked.
    pub fn new() -> Self {
        TccMonitor {
            scope: TccScope::FullDiskAccess,
            is_suspended: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Performs a probe to determine the actual TCC scope.
    pub fn check_current_scope(&self) -> TccScope {
        #[cfg(target_os = "macos")]
        {
            // PROBE: Try to access a path that requires Full Disk Access.
            // ~/Library/Safari is a classic high-integrity path.
            if let Some(home) = dirs::home_dir() {
                let safari_path = home.join("Library/Safari");
                if std::fs::read_dir(&safari_path).is_err() {
                    // If we can't read Safari bookmarks/history, we likely don't have FDA.
                    return TccScope::HotPathsOnly;
                }
            }
        }
        
        TccScope::FullDiskAccess
    }

    /// Updates the internal scope and toggles suspension if needed.
    pub fn update_scope(&mut self, new_scope: TccScope) {
        self.scope = new_scope;
        if new_scope == TccScope::NoAccess {
            self.is_suspended.store(true, Ordering::SeqCst);
        } else {
            self.is_suspended.store(false, Ordering::SeqCst);
        }
    }

    /// Returns true if background crawling should be suspended.
    pub fn is_crawl_suspended(&self) -> bool {
        self.is_suspended.load(Ordering::SeqCst)
    }

    /// Returns the currently active scope.
    pub fn current_scope(&self) -> TccScope {
        self.scope
    }

    /// Simulates a revocation for testing purposes.
    #[cfg(test)]
    pub fn simulate_revocation(&mut self) {
        self.update_scope(TccScope::NoAccess);
    }
}

/// Returns the primary directories allowed under a specific TCC scope.
pub fn allowed_paths_for_scope(scope: TccScope) -> Vec<PathBuf> {
    match scope {
        TccScope::FullDiskAccess => vec![PathBuf::from("/")],
        TccScope::HotPathsOnly => {
            let mut paths = Vec::new();
            if let Some(home) = dirs::home_dir() {
                paths.push(home.join("Documents"));
                paths.push(home.join("Desktop"));
                paths.push(home.join("Downloads"));
            }
            paths
        }
        TccScope::NoAccess => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tcc_scope_allowed_paths() {
        let home = dirs::home_dir().expect("Home dir not found");
        
        // Full Access
        let full = allowed_paths_for_scope(TccScope::FullDiskAccess);
        assert!(full.contains(&PathBuf::from("/")));
        
        // Hot Paths
        let hot = allowed_paths_for_scope(TccScope::HotPathsOnly);
        assert!(hot.contains(&home.join("Documents")));
        assert!(hot.contains(&home.join("Desktop")));
        
        // No Access
        let none = allowed_paths_for_scope(TccScope::NoAccess);
        assert!(none.is_empty());
    }

    #[test]
    fn test_tcc_monitor_suspension_logic() {
        let mut monitor = TccMonitor::new();
        assert!(!monitor.is_crawl_suspended());
        
        monitor.update_scope(TccScope::NoAccess);
        assert!(monitor.is_crawl_suspended());
        
        monitor.update_scope(TccScope::HotPathsOnly);
        assert!(!monitor.is_crawl_suspended());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_tcc_probe_behavior() {
        let monitor = TccMonitor::new();
        let scope = monitor.check_current_scope();
        // We can't guarantee FDA in test environment, but we can verify it returns a valid variant.
        match scope {
            TccScope::FullDiskAccess | TccScope::HotPathsOnly | TccScope::NoAccess => {}
        }
    }
}
