use std::path::{Path, PathBuf};
use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;
#[cfg(target_os = "macos")]
use notify::{RecursiveMode, Watcher};

#[derive(Debug, Clone, PartialEq)]
pub enum WatchStrategy {
    FSEvents,
    SMBv3Notify,
    PollEvery(Duration),
}

#[derive(Debug, Clone)]
pub struct VolumeInfo {
    pub uuid: String,
    pub path: PathBuf,
    pub is_network: bool,
    pub is_ssd: bool,
    pub content_indexing_opted_in: bool,
}

pub struct VolumeMonitor {
    volumes: Vec<VolumeInfo>,
    reconciliation_queue: Vec<VolumeInfo>,
}

impl VolumeMonitor {
    pub fn new() -> Self {
        Self {
            volumes: Vec::new(),
            reconciliation_queue: Vec::new(),
        }
    }

    pub fn register_volume(&mut self, info: VolumeInfo) {
        self.reconciliation_queue.push(info.clone());
        self.volumes.push(info);
    }

    pub fn volumes(&self) -> &[VolumeInfo] {
        &self.volumes
    }

    pub fn select_strategy(&self, vol: &VolumeInfo) -> WatchStrategy {
        if vol.is_network {
            // For network volumes, if they don't support SMB3 notifications (simplified check)
            // we fall back to polling.
            WatchStrategy::PollEvery(Duration::from_secs(300))
        } else {
            WatchStrategy::FSEvents
        }
    }

    pub fn should_content_index(&self, vol: &VolumeInfo) -> bool {
        if vol.is_network {
            vol.content_indexing_opted_in
        } else {
            true
        }
    }

    // Mock for DiskArbitration / NSWorkspace notifications
    pub fn simulate_mount(&mut self, path: &str, uuid: &str, is_network: bool) {
        self.register_volume(VolumeInfo {
            uuid: uuid.to_string(),
            path: PathBuf::from(path),
            is_network,
            is_ssd: true, // simplified
            content_indexing_opted_in: false,
        });
    }

    pub fn pending_reconciliation_jobs(&self) -> Vec<VolumeInfo> {
        self.reconciliation_queue.clone()
    }

    pub fn mark_reconciled(&mut self, volume_uuid: &str) {
        self.reconciliation_queue
            .retain(|v| v.uuid != volume_uuid);
    }
}

pub fn select_watch_strategy(vol: &VolumeInfo) -> WatchStrategy {
    if vol.is_network {
        WatchStrategy::PollEvery(Duration::from_secs(300))
    } else {
        WatchStrategy::FSEvents
    }
}

pub fn should_content_index(vol: &VolumeInfo) -> bool {
    if vol.is_network {
        vol.content_indexing_opted_in
    } else {
        true
    }
}

pub fn start_volume_mount_watcher() -> Receiver<VolumeInfo> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut seen = HashSet::new();

        #[cfg(target_os = "macos")]
        let _watcher = {
            let notify_tx = tx.clone();
            let mut watcher = match notify::recommended_watcher(move |_| {
                // Any event under /Volumes triggers a bounded rescan.
                if let Ok(entries) = std::fs::read_dir("/Volumes") {
                    for entry in entries.flatten().take(128) {
                        let p = entry.path();
                        if p.is_dir() {
                            let key = p.to_string_lossy().to_string();
                            let uuid = p
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("unknown-volume")
                                .to_string();
                            let info = VolumeInfo {
                                uuid,
                                path: p,
                                is_network: false,
                                is_ssd: true,
                                content_indexing_opted_in: false,
                            };
                            let _ = notify_tx.send(info);
                            let _ = key;
                        }
                    }
                }
            }) {
                Ok(w) => w,
                Err(_) => {
                    // Fallback polling if watcher cannot be created.
                    loop {
                        std::thread::sleep(Duration::from_secs(10));
                        if let Ok(entries) = std::fs::read_dir("/Volumes") {
                            for entry in entries.flatten().take(128) {
                                let p = entry.path();
                                if p.is_dir() {
                                    let key = p.to_string_lossy().to_string();
                                    if seen.insert(key.clone()) {
                                        let info = VolumeInfo {
                                            uuid: key,
                                            path: p,
                                            is_network: false,
                                            is_ssd: true,
                                            content_indexing_opted_in: false,
                                        };
                                        if tx.send(info).is_err() {
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            };

            let _ = watcher.watch(Path::new("/Volumes"), RecursiveMode::NonRecursive);
            watcher
        };

        loop {
            std::thread::sleep(Duration::from_secs(10));
            if let Ok(entries) = std::fs::read_dir("/Volumes") {
                for entry in entries.flatten().take(128) {
                    let p = entry.path();
                    if p.is_dir() {
                        let key = p.to_string_lossy().to_string();
                        if seen.insert(key.clone()) {
                            let info = VolumeInfo {
                                uuid: key,
                                path: p,
                                is_network: false,
                                is_ssd: true,
                                content_indexing_opted_in: false,
                            };
                            if tx.send(info).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        }
    });

    rx
}

/// Executor-level guard for paths on network-mounted volumes.
/// If `LOCALSEARCH_NETWORK_CONTENT_OPTIN=true`, paths on /Volumes are queryable.
pub fn is_path_queryable(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    if path_str.starts_with("/Volumes/") {
        let opt_in = std::env::var("LOCALSEARCH_NETWORK_CONTENT_OPTIN")
            .unwrap_or_else(|_| "false".to_string())
            .to_lowercase();
        return opt_in == "1" || opt_in == "true" || opt_in == "yes";
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_mount_triggers_reconciliation_logic() {
        let mut vmon = VolumeMonitor::new();
        vmon.simulate_mount("/Volumes/ExternalDrive", "test-uuid-1234", false);
        let jobs = vmon.pending_reconciliation_jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].uuid, "test-uuid-1234");
        vmon.mark_reconciled("test-uuid-1234");
        assert!(vmon.pending_reconciliation_jobs().is_empty());
    }

    #[test]
    fn test_network_volume_falls_back_to_polling() {
        let vol = VolumeInfo {
            uuid: "nas-1".to_string(),
            path: PathBuf::from("/Volumes/NAS"),
            is_network: true,
            is_ssd: false,
            content_indexing_opted_in: false,
        };
        let strategy = select_watch_strategy(&vol);
        assert_eq!(strategy, WatchStrategy::PollEvery(Duration::from_secs(300)));
    }

    #[test]
    fn test_network_volume_content_indexing_opt_in_only() {
        let mut vol = VolumeInfo {
            uuid: "nas-1".to_string(),
            path: PathBuf::from("/Volumes/NAS"),
            is_network: true,
            is_ssd: false,
            content_indexing_opted_in: false,
        };
        assert!(!should_content_index(&vol));
        
        vol.content_indexing_opted_in = true;
        assert!(should_content_index(&vol));
    }

    #[test]
    fn test_is_path_queryable_for_local_paths() {
        assert!(is_path_queryable(Path::new("/Users/test/Documents/a.txt")));
    }
}
