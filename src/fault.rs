use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::sync::OnceLock;
use serde::{Deserialize, Serialize};

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
    pub probability: f32, // 0.0 to 1.0 (currently used as binary active toggle)
    pub active: bool,
}

pub struct FaultInjector {
    faults: RwLock<HashMap<FaultType, FaultConfig>>,
}

static INJECTOR: OnceLock<Arc<FaultInjector>> = OnceLock::new();

impl FaultInjector {
    pub fn get() -> Arc<Self> {
        INJECTOR.get_or_init(|| {
            Arc::new(Self {
                faults: RwLock::new(HashMap::new()),
            })
        }).clone()
    }

    /// Configures a fault type with the given configuration.
    pub fn set_fault(&self, fault_type: FaultType, config: FaultConfig) {
        if let Ok(mut faults) = self.faults.write() {
            faults.insert(fault_type, config);
        }
    }

    /// Checks if a fault should be triggered.
    /// In this initial version, it simply checks if the fault is active.
    pub fn should_fault(&self, fault_type: FaultType) -> bool {
        if let Ok(faults) = self.faults.read() {
            if let Some(config) = faults.get(&fault_type) {
                return config.active;
            }
        }
        false
    }

    /// Clears all active faults.
    pub fn clear(&self) {
        if let Ok(mut faults) = self.faults.write() {
            faults.clear();
        }
    }
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

    #[test]
    fn test_fault_injector_basic() {
        let injector = FaultInjector::get();
        injector.clear();
        
        assert!(!injector.should_fault(FaultType::Panic));
        
        injector.set_fault(FaultType::Panic, FaultConfig { probability: 1.0, active: true });
        assert!(injector.should_fault(FaultType::Panic));
        
        injector.clear();
        assert!(!injector.should_fault(FaultType::Panic));
    }

    #[test]
    fn test_fault_point_macro() {
        let injector = FaultInjector::get();
        injector.clear();
        
        let mut triggered = false;
        fault_point!(FaultType::Panic, {
            triggered = true;
        });
        assert!(!triggered);
        
        injector.set_fault(FaultType::Panic, FaultConfig { probability: 1.0, active: true });
        fault_point!(FaultType::Panic, {
            triggered = true;
        });
        assert!(triggered);
    }
}
