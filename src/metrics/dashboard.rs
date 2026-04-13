pub struct HealthState {
    pub memory_state: String,
    pub segment_count: usize,
    pub delta_size_bytes: u64,
    pub wal_lag_events: usize,
    pub doc_count: usize,
    pub content_indexed_fraction: f32,
    pub last_compaction_ago_secs: u64,
    pub phantom_rate: f32,
    pub stale_rate: f32,
    pub query_p50_ms: f64,
    pub query_p99_ms: f64,
    pub zero_result_rate: f32,
    pub thread_count: usize,
    pub watchdog_healthy: bool,
}

pub fn render_dashboard(state: &HealthState) -> String {
    let status = if state.phantom_rate > 0.05 || state.query_p99_ms > 350.0 {
        "UNHEALTHY"
    } else if state.phantom_rate > 0.02 || state.query_p99_ms > 150.0 {
        "DEGRADED"
    } else {
        "HEALTHY"
    };

    format!(
        "┌──────────────────────────────────────────────────────────────────────────┐\n\
         │ LocalSearch Health Dashboard                                   [{:<9}] │\n\
         ├──────────────────────────────────────────────────────────────────────────┤\n\
         │ INDEX STATUS                                                             │\n\
         │   Documents: {:<15} Content Indexed: {:<5.1}%               │\n\
         │   Segments:  {:<15} Delta Size:      {:<10} bytes         │\n\
         │   Last Comp: {:<5}s ago          WAL Lag:          {:<10} items     │\n\
         ├──────────────────────────────────────────────────────────────────────────┤\n\
         │ PERFORMANCE                                                              │\n\
         │   Latency P50: {:<12.1}ms  Latency P99: {:<12.1}ms          │\n\
         │   Zero Results: {:<5.1}%                                                │\n\
         │   Threads: {:<18} Watchdog: {:<17} │\n\
         ├──────────────────────────────────────────────────────────────────────────┤\n\
         │ INTEGRITY                                                                │\n\
         │   Phantom Rate: {:<12.1}%  Stale Rate:  {:<12.1}%          │\n\
         └──────────────────────────────────────────────────────────────────────────┘",
        status,
        state.doc_count, state.content_indexed_fraction * 100.0,
        state.segment_count, state.delta_size_bytes,
        state.last_compaction_ago_secs, state.wal_lag_events,
        state.query_p50_ms, state.query_p99_ms,
        state.zero_result_rate * 100.0,
        state.thread_count,
        if state.watchdog_healthy { "OK" } else { "ALERT" },
        state.phantom_rate * 100.0, state.stale_rate * 100.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashboard_renders_health_status() {
        let state = HealthState {
            memory_state: "Full".to_string(),
            segment_count: 4,
            delta_size_bytes: 12_800_000,
            wal_lag_events: 0,
            doc_count: 1_247_832,
            content_indexed_fraction: 0.943,
            last_compaction_ago_secs: 240,
            phantom_rate: 0.003,
            stale_rate: 0.011,
            query_p50_ms: 48.0,
            query_p99_ms: 87.0,
            zero_result_rate: 0.021,
            thread_count: 7,
            watchdog_healthy: true,
        };
        let output = render_dashboard(&state);
        assert!(output.contains("HEALTHY"));
        assert!(output.contains("1247832"));
        assert!(output.contains("48.0"));
    }
}
