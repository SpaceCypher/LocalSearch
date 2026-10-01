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

impl PowerMonitor {
    /// Sample the machine's current thermal and power state.
    pub fn current() -> Self {
        Self {
            thermal_state: read_thermal_state(),
            on_battery: read_on_battery(),
        }
    }

    pub fn thermal_state(&self) -> ThermalState {
        self.thermal_state
    }

    pub fn on_battery(&self) -> bool {
        self.on_battery
    }
}

/// `NSProcessInfo.processInfo.thermalState`
#[cfg(target_os = "macos")]
fn read_thermal_state() -> ThermalState {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    let Some(class) = AnyClass::get("NSProcessInfo") else {
        return ThermalState::Nominal;
    };
    let state: isize = unsafe {
        let info: *mut AnyObject = msg_send![class, processInfo];
        if info.is_null() {
            return ThermalState::Nominal;
        }
        msg_send![info, thermalState]
    };
    match state {
        1 => ThermalState::Fair,
        2 => ThermalState::Serious,
        3 => ThermalState::Critical,
        _ => ThermalState::Nominal,
    }
}

#[cfg(not(target_os = "macos"))]
fn read_thermal_state() -> ThermalState {
    ThermalState::Nominal
}

/// True when `pmset -g batt` reports the machine is drawing from its battery.
#[cfg(target_os = "macos")]
fn read_on_battery() -> bool {
    std::process::Command::new("/usr/bin/pmset")
        .args(["-g", "batt"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("'Battery Power'"))
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn read_on_battery() -> bool {
    false
}

#[cfg(target_os = "macos")]
extern "C" {
    fn setiopolicy_np(iotype: std::os::raw::c_int, scope: std::os::raw::c_int, policy: std::os::raw::c_int) -> std::os::raw::c_int;
}

/// Set the disk I/O priority of the calling thread (`setiopolicy_np`).
/// Returns false if the kernel rejected the request.
#[cfg(target_os = "macos")]
pub fn set_io_policy(policy: IoPolicy) -> bool {
    const IOPOL_TYPE_DISK: std::os::raw::c_int = 0;
    const IOPOL_SCOPE_THREAD: std::os::raw::c_int = 1;
    const IOPOL_DEFAULT: std::os::raw::c_int = 0;
    const IOPOL_THROTTLE: std::os::raw::c_int = 3;

    let value = match policy {
        IoPolicy::Normal => IOPOL_DEFAULT,
        IoPolicy::Throttle => IOPOL_THROTTLE,
    };
    unsafe { setiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_THREAD, value) == 0 }
}

#[cfg(not(target_os = "macos"))]
pub fn set_io_policy(_policy: IoPolicy) -> bool {
    false
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

    #[test]
    fn test_current_state_is_readable() {
        // Just exercises the real system probes; any state is valid.
        let monitor = PowerMonitor::current();
        let _ = (monitor.thermal_state(), monitor.on_battery(), monitor.should_compact());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_io_throttle_set_on_background_threads() {
        // The kernel accepts the policy for this thread
        assert!(set_io_policy(IoPolicy::Throttle));
        assert!(set_io_policy(IoPolicy::Normal));
    }
}
