use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// Memory state enum representing system memory pressure levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MemoryState {
    Full = 0,      // All components in RAM, no pressure
    Reduced = 1,   // BK-tree evicted, >65% budget used
    Minimal = 2,   // Most indexes evicted, >80% budget used
    Critical = 3,  // Emergency mode, >95% budget used
}

impl From<u8> for MemoryState {
    fn from(v: u8) -> Self {
        match v {
            0 => MemoryState::Full,
            1 => MemoryState::Reduced,
            2 => MemoryState::Minimal,
            3 => MemoryState::Critical,
            _ => MemoryState::Full, // Default to Full for invalid values
        }
    }
}

/// Memory controller that monitors RSS and transitions between states
pub struct MemoryController {
    state: Arc<AtomicU8>,
    budget: usize,
    current_rss: usize,
}

impl MemoryController {
    pub fn new(budget: usize) -> Self {
        Self {
            state: Arc::new(AtomicU8::new(MemoryState::Full as u8)),
            budget,
            current_rss: 0,
        }
    }

    /// Get current memory state
    pub fn state(&self) -> MemoryState {
        MemoryState::from(self.state.load(Ordering::Relaxed))
    }

    /// Get shared atomic state for readers
    pub fn state_atomic(&self) -> Arc<AtomicU8> {
        Arc::clone(&self.state)
    }

    /// Update RSS and check for state transitions
    pub fn update(&mut self, rss: usize) {
        self.current_rss = rss;
        let pressure_ratio = rss as f64 / self.budget as f64;
        
        let new_state = match pressure_ratio {
            r if r > 0.95 => MemoryState::Critical,
            r if r > 0.80 => MemoryState::Minimal,
            r if r > 0.65 => MemoryState::Reduced,
            _ => MemoryState::Full,
        };
        
        let old_state = self.state();
        
        if new_state != old_state {
            self.transition(old_state, new_state);
        }
    }

    fn transition(&mut self, from: MemoryState, to: MemoryState) {
        log::info!("Memory state transition: {:?} → {:?}", from, to);
        self.state.store(to as u8, Ordering::Relaxed);
    }

    /// Get current RSS
    pub fn current_rss(&self) -> usize {
        self.current_rss
    }

    /// Get memory budget
    pub fn budget(&self) -> usize {
        self.budget
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_state_transitions_on_pressure() {
        let budget = 1000;
        let mut controller = MemoryController::new(budget);
        
        // Initial state should be Full
        assert_eq!(controller.state(), MemoryState::Full);
        
        // Simulate RSS > 80% budget → should transition to Minimal
        controller.update(850);
        assert_eq!(controller.state(), MemoryState::Minimal);
        
        // Simulate RSS drop below 50% → should recover to Full
        controller.update(400);
        assert_eq!(controller.state(), MemoryState::Full);
    }

    #[test]
    fn test_memory_state_reduced_at_65_percent() {
        let budget = 1000;
        let mut controller = MemoryController::new(budget);
        
        // Simulate RSS > 65% budget → should transition to Reduced
        controller.update(700);
        assert_eq!(controller.state(), MemoryState::Reduced);
    }

    #[test]
    fn test_memory_state_critical_at_95_percent() {
        let budget = 1000;
        let mut controller = MemoryController::new(budget);
        
        // Simulate RSS > 95% budget → should transition to Critical
        controller.update(970);
        assert_eq!(controller.state(), MemoryState::Critical);
    }

    #[test]
    fn test_recovery_only_when_pressure_below_50_percent() {
        let budget = 1000;
        let mut controller = MemoryController::new(budget);
        
        // Transition to Minimal
        controller.update(850);
        assert_eq!(controller.state(), MemoryState::Minimal);
        
        // Drop to 70% (still in Reduced range) → should transition to Reduced
        controller.update(700);
        assert_eq!(controller.state(), MemoryState::Reduced);
        
        // Drop to 60% (below 65%, in Full range) → should recover to Full
        controller.update(600);
        assert_eq!(controller.state(), MemoryState::Full);
    }

    #[test]
    fn test_state_atomic_shared_correctly() {
        let controller = MemoryController::new(1000);
        let state_atomic = controller.state_atomic();
        
        // Verify atomic state matches controller state
        assert_eq!(
            MemoryState::from(state_atomic.load(Ordering::Relaxed)),
            controller.state()
        );
    }
}
