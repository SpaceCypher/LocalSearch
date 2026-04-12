use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::thread::{self, ThreadId};
use std::time::{Instant, SystemTime};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRole {
    Query,
    Indexer,
    WAL,
    Watchdog,
    Maintenance,
}

#[derive(Debug, Clone)]
pub struct ThreadInfo {
    pub id: ThreadId,
    pub name: String,
    pub role: ThreadRole,
    pub last_heartbeat: Instant,
    pub started_at: SystemTime,
}

pub struct ThreadRegistry {
    threads: RwLock<HashMap<ThreadId, ThreadInfo>>,
}

static REGISTRY: OnceLock<Arc<ThreadRegistry>> = OnceLock::new();

impl ThreadRegistry {
    pub fn get() -> Arc<Self> {
        REGISTRY.get_or_init(|| {
            Arc::new(Self {
                threads: RwLock::new(HashMap::new()),
            })
        }).clone()
    }

    /// Register the current thread in the global registry.
    pub fn register(&self, name: String, role: ThreadRole) {
        if let Ok(mut threads) = self.threads.write() {
            threads.insert(thread::current().id(), ThreadInfo {
                id: thread::current().id(),
                name,
                role,
                last_heartbeat: Instant::now(),
                started_at: SystemTime::now(),
            });
        }
    }

    /// Update the heartbeat for the current thread.
    pub fn heartbeat(&self) {
        if let Ok(mut threads) = self.threads.write() {
            if let Some(info) = threads.get_mut(&thread::current().id()) {
                info.last_heartbeat = Instant::now();
            }
        }
    }

    /// Remove the current thread from the registry.
    pub fn unregister(&self) {
        if let Ok(mut threads) = self.threads.write() {
            threads.remove(&thread::current().id());
        }
    }

    /// Check for threads that haven't updated their heartbeat within the timeout.
    pub fn check_health(&self, timeout: std::time::Duration) -> Vec<(ThreadId, String)> {
        match self.threads.read() {
            Ok(threads) => {
                let now = Instant::now();
                threads.iter()
                    .filter(|(_, info)| now.duration_since(info.last_heartbeat) > timeout)
                    .map(|(id, info)| (*id, info.name.clone()))
                    .collect()
            }
            Err(_) => Vec::new(),
        }
    }

    /// Get count of registered threads.
    pub fn thread_count(&self) -> usize {
        self.threads.read().map(|t| t.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_thread_registry_registration() {
        let registry = ThreadRegistry::get();
        registry.register("TestThread".to_string(), ThreadRole::Maintenance);
        
        assert!(registry.thread_count() >= 1);
        
        let hangs = registry.check_health(Duration::from_secs(1));
        assert!(hangs.is_empty());
        
        registry.unregister();
    }
}
