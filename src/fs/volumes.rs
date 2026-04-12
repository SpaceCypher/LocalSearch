use std::path::{Path, PathBuf};
use std::time::Duration;
use crate::fs::reconcile::ReconciliationDiff;

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
}

impl VolumeMonitor {
    pub fn new() -> Self {
        Self { volumes: Vec::new() }
    }

    pub fn register_volume(&mut self, info: VolumeInfo) {
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
        // In a real impl, this would return volumes that were just mounted
        // or haven't been reconciled in a while.
        self.volumes.clone()
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
}
