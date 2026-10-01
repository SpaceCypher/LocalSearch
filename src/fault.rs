use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::sync::OnceLock;
use serde::{Deserialize, Serialize};

#[repr(u8)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum FaultType {
    DiskFull,
    MemoryPressure,
    Panic,
    QueryTimeout,
    WalCorruption,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultConfig {
    /// Probability in range [0.0, 1.0].
    #[serde(default = "default_probability")]
    pub probability: f32,
    /// Whether this fault is enabled.
    #[serde(default)]
    pub active: bool,
    /// Do not inject until this call count is reached.
    #[serde(default)]
    pub after_n_calls: u64,
}

fn default_probability() -> f32 {
    1.0
}

pub struct FaultInjector {
    faults: RwLock<HashMap<FaultType, FaultConfig>>,
    call_counts: RwLock<HashMap<FaultType, u64>>,
}

static INJECTOR: OnceLock<Arc<FaultInjector>> = OnceLock::new();

impl FaultInjector {
    pub fn get() -> Arc<Self> {
        INJECTOR.get_or_init(|| {
            Arc::new(Self {
                faults: RwLock::new(HashMap::new()),
                call_counts: RwLock::new(HashMap::new()),
            })
        }).clone()
    }

    /// Configures a fault type with the given configuration.
    pub fn set_fault(&self, fault_type: FaultType, config: FaultConfig) {
        if let Ok(mut faults) = self.faults.write() {
            faults.insert(fault_type, config);
        }
        if let Ok(mut calls) = self.call_counts.write() {
            calls.insert(fault_type, 0);
        }
    }

    /// Loads and configures a fault from JSON.
    pub fn set_fault_from_json(&self, fault_type: FaultType, json: &str) -> Result<(), serde_json::Error> {
        let config: FaultConfig = serde_json::from_str(json)?;
        self.set_fault(fault_type, config);
        Ok(())
    }

    /// Checks if a fault should be triggered.
    pub fn should_fault(&self, fault_type: FaultType) -> bool {
        let call_num = {
            let mut calls = match self.call_counts.write() {
                Ok(c) => c,
                Err(_) => return false,
            };
            let entry = calls.entry(fault_type).or_insert(0);
            *entry += 1;
            *entry
        };

        if let Ok(faults) = self.faults.read() {
            if let Some(config) = faults.get(&fault_type) {
                if !config.active {
                    return false;
                }

                if call_num < config.after_n_calls {
                    return false;
                }

                if config.probability <= 0.0 {
                    return false;
                }
                if config.probability >= 1.0 {
                    return true;
                }

                // Deterministic pseudo-random sample from fault type + call number.
                // This keeps behavior reproducible in tests without extra RNG dependencies.
                let mix = splitmix64(call_num ^ ((fault_type as u64) << 32));
                let sample = (mix as f64) / (u64::MAX as f64);
                return sample < config.probability as f64;
            }
        }
        false
    }

    /// Clears all active faults.
    pub fn clear(&self) {
        if let Ok(mut faults) = self.faults.write() {
            faults.clear();
        }
        if let Ok(mut calls) = self.call_counts.write() {
            calls.clear();
        }
    }
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Macro to instrument a fault point in the code.
/// Usage: fault_point!(FaultType::Panic, { panic!("Simulated panic") });
#[macro_export]
macro_rules! fault_point {
    ($fault_type:expr, $block:block) => {
        if $crate::fault::FaultInjector::get().should_fault($fault_type) {
             $block
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn test_fault_injector_basic() {
        let _guard = test_lock().lock().unwrap();
        let injector = FaultInjector::get();
        injector.clear();
        
        assert!(!injector.should_fault(FaultType::Panic));
        
        injector.set_fault(
            FaultType::Panic,
            FaultConfig {
                probability: 1.0,
                active: true,
                after_n_calls: 1,
            },
        );
        assert!(injector.should_fault(FaultType::Panic));
        
        injector.clear();
        assert!(!injector.should_fault(FaultType::Panic));
    }

    #[test]
    fn test_fault_point_macro() {
        let _guard = test_lock().lock().unwrap();
        let injector = FaultInjector::get();
        injector.clear();
        
        let mut triggered = false;
        fault_point!(FaultType::Panic, {
            triggered = true;
        });
        assert!(!triggered);
        
        injector.set_fault(
            FaultType::Panic,
            FaultConfig {
                probability: 1.0,
                active: true,
                after_n_calls: 1,
            },
        );
        fault_point!(FaultType::Panic, {
            triggered = true;
        });
        assert!(triggered);
    }

    #[test]
    fn test_fault_after_n_calls() {
        let _guard = test_lock().lock().unwrap();
        let injector = FaultInjector::get();
        injector.clear();
        injector.set_fault(
            FaultType::WalCorruption,
            FaultConfig {
                probability: 1.0,
                active: true,
                after_n_calls: 5,
            },
        );

        for _ in 0..4 {
            assert!(!injector.should_fault(FaultType::WalCorruption));
        }
        assert!(injector.should_fault(FaultType::WalCorruption));
    }

    #[test]
    fn test_fault_probability_bounds() {
        let _guard = test_lock().lock().unwrap();
        let injector = FaultInjector::get();
        injector.clear();

        injector.set_fault(
            FaultType::DiskFull,
            FaultConfig {
                probability: 0.0,
                active: true,
                after_n_calls: 1,
            },
        );
        assert!(!injector.should_fault(FaultType::DiskFull));

        injector.set_fault(
            FaultType::DiskFull,
            FaultConfig {
                probability: 1.0,
                active: true,
                after_n_calls: 1,
            },
        );
        assert!(injector.should_fault(FaultType::DiskFull));
    }

    #[test]
    fn test_fault_config_from_json() {
        let _guard = test_lock().lock().unwrap();
        let injector = FaultInjector::get();
        injector.clear();

        let json = r#"{"probability":1.0,"active":true,"after_n_calls":3}"#;
        injector
            .set_fault_from_json(FaultType::QueryTimeout, json)
            .unwrap();

        assert!(!injector.should_fault(FaultType::QueryTimeout));
        assert!(!injector.should_fault(FaultType::QueryTimeout));
        assert!(injector.should_fault(FaultType::QueryTimeout));
    }
}
