use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::thread::{self, ThreadId};
use std::time::{SystemTime, UNIX_EPOCH};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRole {
    Query,
    Indexer,
    WAL,
    Watchdog,
    Maintenance,
}

pub struct ThreadInfo {
    pub id: ThreadId,
    pub name: String,
    pub role: ThreadRole,
    pub last_heartbeat_ms: AtomicU64,
    pub op_start_ms: AtomicU64,
    pub current_op: RwLock<String>,
    pub io_wait_us: AtomicU64,
}

impl ThreadInfo {
    pub fn new(id: ThreadId, name: String, role: ThreadRole) -> Self {
        let now_ms = current_time_ms();
        Self {
            id,
            name,
            role,
            last_heartbeat_ms: AtomicU64::new(now_ms),
            op_start_ms: AtomicU64::new(now_ms),
            current_op: RwLock::new("Idle".to_string()),
            io_wait_us: AtomicU64::new(0),
        }
    }
}

pub struct ThreadRegistry {
    threads: RwLock<HashMap<String, Arc<ThreadInfo>>>,
}

static REGISTRY: OnceLock<Arc<ThreadRegistry>> = OnceLock::new();

pub fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl ThreadRegistry {
    pub fn new() -> Self {
        Self {
            threads: RwLock::new(HashMap::new()),
        }
    }

    pub fn get_instance() -> Arc<Self> {
        REGISTRY.get_or_init(|| Arc::new(Self::new())).clone()
    }

    pub fn register(&self, name: &str, role: ThreadRole) {
        let mut threads = self.threads.write().unwrap();
        threads.insert(name.to_string(), Arc::new(ThreadInfo::new(thread::current().id(), name.to_string(), role)));
    }

    pub fn unregister(&self, name: &str) {
        if let Ok(mut threads) = self.threads.write() {
            threads.remove(name);
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<ThreadInfo>> {
        self.threads.read().unwrap().get(name).cloned()
    }

    pub fn heartbeat(&self, name: &str) {
        if let Some(info) = self.get(name) {
            info.last_heartbeat_ms.store(current_time_ms(), Ordering::Release);
        }
    }

    pub fn set_current_op(&self, name: &str, op: &str) {
        if let Some(info) = self.get(name) {
            if let Ok(mut current) = info.current_op.write() {
                *current = op.to_string();
                info.op_start_ms.store(current_time_ms(), Ordering::Release);
            }
        }
    }

    #[cfg(test)]
    pub fn set_op_start_backdated(&self, name: &str, elapsed_ms: u64) {
        if let Some(info) = self.get(name) {
            let now = current_time_ms();
            info.op_start_ms.store(now.saturating_sub(elapsed_ms), Ordering::Release);
        }
    }

    pub fn check_watchdog(&self) -> Result<(), String> {
        let now = current_time_ms();
        let threads = self.threads.read().unwrap();
        for (name, info) in threads.iter() {
            let start = info.op_start_ms.load(Ordering::Acquire);
            if start > 0 && now.saturating_sub(start) > 500 {
                return Err(format!("Thread '{}' stuck in op '{}' for >500ms", name, *info.current_op.read().unwrap()));
            }
        }
        Ok(())
    }

    pub fn check_health(&self, timeout_ms: u64) -> Vec<(ThreadId, String)> {
        let mut hangs = Vec::new();
        if let Ok(threads) = self.threads.read() {
            let now = current_time_ms();
            for (name, info) in threads.iter() {
                let hb = info.last_heartbeat_ms.load(Ordering::Acquire);
                if now.saturating_sub(hb) > timeout_ms {
                    hangs.push((info.id, name.clone()));
                }
            }
        }
        hangs
    }

    pub fn thread_count(&self) -> usize {
        self.threads.read().map(|t| t.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thread_registry_tracks_active_op() {
        let registry = ThreadRegistry::new();
        registry.register("query-worker-1", ThreadRole::Query);
        registry.set_current_op("query-worker-1", "executing BM25 scorer");
        let info = registry.get("query-worker-1").unwrap();
        assert_eq!(*info.current_op.read().unwrap(), "executing BM25 scorer");
    }

    #[test]
    fn test_watchdog_fires_on_lock_held_too_long() {
        let registry = ThreadRegistry::new();
        registry.register("query-worker-1", ThreadRole::Query);
        registry.set_op_start_backdated("query-worker-1", 600); // 600ms ago
        assert!(registry.check_watchdog().is_err());
    }
}
