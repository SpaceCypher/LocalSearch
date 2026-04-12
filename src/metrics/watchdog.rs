use std::thread;
use std::time::Duration;
use crate::metrics::threads::{ThreadRegistry, ThreadRole};

pub struct Watchdog;

impl Watchdog {
    /// Spawns the watchdog background thread.
    /// This thread monitors the global ThreadRegistry and logs warnings if any
    /// thread exceeds its heartbeat timeout.
    pub fn spawn() {
        thread::spawn(|| {
            let registry = ThreadRegistry::get();
            registry.register("Watchdog".to_string(), ThreadRole::Watchdog);
            
            log::info!("Watchdog thread started");
            
            loop {
                // Sleep for the check interval
                thread::sleep(Duration::from_secs(5));
                
                // Watchdog itself should update its heartbeat
                registry.heartbeat();
                
                // Check if any thread has been silent for more than 30 seconds
                let hangs = registry.check_health(Duration::from_secs(30));
                for (id, name) in hangs {
                    log::warn!(
                        "HEALTH ALERT: Thread '{}' ({:?}) heartbeat timeout! Possible hang detected.",
                        name, id
                    );
                    
                    // In a production environment, we might trigger a crash report 
                    // or a graceful restart here if the thread is critical (e.g., Query executor).
                }
            }
        });
    }
}
