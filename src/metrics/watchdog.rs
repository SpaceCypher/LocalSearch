use std::thread;
use std::time::Duration;
use crate::metrics::threads::{ThreadRegistry, ThreadRole};

pub struct Watchdog;

impl Watchdog {
    pub fn spawn() {
        thread::spawn(|| {
            let registry = ThreadRegistry::get_instance();
            registry.register("Watchdog", ThreadRole::Watchdog);
            
            log::info!("Watchdog thread started");
            
            loop {
                thread::sleep(Duration::from_secs(5));
                
                registry.heartbeat("Watchdog");
                
                if let Err(msg) = registry.check_watchdog() {
                    log::warn!("HEALTH ALERT: {}", msg);
                    #[cfg(test)]
                    {
                        // Simulate SIGABRT in test builds as specified
                        panic!("Watchdog triggered: {}", msg);
                    }
                }

                let hangs = registry.check_health(30_000);
                for (id, name) in hangs {
                    log::warn!(
                        "HEALTH ALERT: Thread '{}' ({:?}) heartbeat timeout! Possible hang detected.",
                        name, id
                    );
                }
            }
        });
    }
}
