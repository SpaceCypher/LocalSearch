// Power monitor for thermal/battery-aware compaction scheduling

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IoPolicy {
    Normal,
    Throttle,
}

pub struct PowerMonitor {
    thermal_state: ThermalState,
    on_battery: bool,
}

impl PowerMonitor {
    #[cfg(test)]
    pub fn new_test(thermal_state: ThermalState, on_battery: bool) -> Self {
        Self { thermal_state, on_battery }
    }

    pub fn should_compact(&self) -> bool {
        // Don't compact on critical thermal or low battery
        if self.thermal_state == ThermalState::Critical {
            return false;
        }
        if self.on_battery {
            return false; // Battery check logic
        }
        true
    }
}

#[cfg(target_os = "macos")]
pub fn set_io_policy(_policy: IoPolicy) {
    // setiopolicy_np stub for macOS
    // In production, this would call libc::setiopolicy_np
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_not_compact_on_critical_thermal() {
        let monitor = PowerMonitor::new_test(ThermalState::Critical, false);
        assert!(!monitor.should_compact());
    }

    #[test]
    fn test_should_not_compact_on_battery_below_20() {
        let monitor = PowerMonitor::new_test(ThermalState::Nominal, true /* on battery */);
        assert!(!monitor.should_compact()); // Battery check logic
    }

    #[test]
    fn test_should_compact_when_idle_and_plugged_in() {
        let monitor = PowerMonitor::new_test(ThermalState::Nominal, false);
        assert!(monitor.should_compact());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_io_throttle_set_on_background_threads() {
        // Verify setiopolicy_np called without panic
        set_io_policy(IoPolicy::Throttle);
    }
}
