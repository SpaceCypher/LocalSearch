use anyhow::Result;
use std::path::PathBuf;
use localsearch::startup;
use localsearch::metrics::dashboard::{HealthState, render_dashboard};

fn main() -> Result<()> {
    env_logger::init();
    
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--debug-panel") {
        // Render a mockup state for now, in production this would pull from components
        let state = HealthState {
            memory_state: "Normal".to_string(),
            segment_count: 3,
            delta_size_bytes: 10 * 1024 * 1024,
            wal_lag_events: 0,
            doc_count: 500_000,
            content_indexed_fraction: 0.98,
            last_compaction_ago_secs: 120,
            phantom_rate: 0.001,
            stale_rate: 0.005,
            query_p50_ms: 12.5,
            query_p99_ms: 85.0,
            zero_result_rate: 0.012,
        };
        println!("{}", render_dashboard(&state));
        return Ok(());
    }

    log::info!("LocalSearch starting...");
    
    // Determine data directory
    let data_dir = get_data_dir()?;
    
    // Detect startup path and execute appropriate handler
    let startup_path = startup::detect_startup_path(&data_dir)?;
    log::info!("Detected startup path: {:?}", startup_path);
    
    match startup_path {
        startup::StartupPath::FirstLaunch => {
            log::info!("First launch detected");
            startup::first_launch(&data_dir)?;
        }
        startup::StartupPath::WarmRestart => {
            log::info!("Warm restart detected");
            startup::warm_restart(&data_dir)?;
        }
        startup::StartupPath::CrashRecovery => {
            log::warn!("Crash recovery detected");
            startup::crash_recovery(&data_dir)?;
        }
    }
    
    log::info!("Startup complete, ready to serve queries");
    Ok(())
}

fn get_data_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| anyhow::anyhow!("HOME environment variable not set"))?;
    Ok(PathBuf::from(home).join(".localsearch"))
}
