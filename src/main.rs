use anyhow::Result;
use std::collections::HashSet;
use std::path::PathBuf;
use localsearch::startup;
use localsearch::fs::tcc::{TccMonitor, allowed_paths_for_scope};
use localsearch::fs::volumes::{VolumeMonitor, start_volume_mount_watcher};
use localsearch::wal::reader::WalReader;
use localsearch::metrics::collector::{IntegrityChecker, MetricsCollector};
use localsearch::metrics::dashboard::{HealthState, render_dashboard};
use localsearch::metrics::{ThreadRegistry, ThreadRole, Watchdog};
use localsearch::resource::memory::{MemoryController, MemoryState};

fn main() -> Result<()> {
    env_logger::init();
    
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--debug-panel") {
        let data_dir = get_data_dir()?;
        let state = collect_live_health_state(&data_dir)?;
        println!("{}", render_dashboard(&state));
        return Ok(());
    }

    log::info!("LocalSearch starting...");

    let registry = ThreadRegistry::get_instance();
    registry.register("main", ThreadRole::Maintenance);

    Watchdog::spawn();

    let mut tcc_monitor = TccMonitor::new();
    let scope = tcc_monitor.refresh_scope();
    log::info!("TCC scope: {:?}", scope);
    let allowed_paths = allowed_paths_for_scope(scope);
    log::info!("Allowed path roots: {}", allowed_paths.len());
    let scope_rx = tcc_monitor.start_scope_watcher(std::time::Duration::from_secs(5));

    let mut volume_monitor = VolumeMonitor::new();
    let volume_rx = start_volume_mount_watcher();
    if let Ok(entries) = std::fs::read_dir("/Volumes") {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let uuid = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown-volume")
                    .to_string();
                volume_monitor.simulate_mount(
                    &p.to_string_lossy(),
                    &uuid,
                    false,
                );
            }
        }
    }
    
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

    run_runtime_watchers(scope_rx, volume_rx, &mut volume_monitor);
    
    log::info!("Startup complete, ready to serve queries");
    Ok(())
}

fn get_data_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| anyhow::anyhow!("HOME environment variable not set"))?;
    let data_dir = PathBuf::from(home).join(".localsearch");
    std::fs::create_dir_all(&data_dir)?;
    Ok(data_dir)
}

fn collect_live_health_state(data_dir: &std::path::Path) -> Result<HealthState> {
    let mut segment_count = 0usize;
    let mut delta_size_bytes = 0u64;
    let mut newest_segment_mtime = 0u64;
    let mut wal_size_bytes = 0u64;

    if data_dir.exists() {
        for entry in std::fs::read_dir(data_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("seg") {
                segment_count += 1;
                if let Ok(meta) = entry.metadata() {
                    delta_size_bytes += meta.len();
                    if let Ok(modified) = meta.modified() {
                        if let Ok(secs) = modified.duration_since(std::time::UNIX_EPOCH) {
                            newest_segment_mtime = newest_segment_mtime.max(secs.as_secs());
                        }
                    }
                }
            }
        }
    }

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let last_compaction_ago_secs = if newest_segment_mtime == 0 {
        0
    } else {
        now_secs.saturating_sub(newest_segment_mtime)
    };

    let wal_path = data_dir.join("wal.log");
    if let Ok(meta) = std::fs::metadata(&wal_path) {
        wal_size_bytes = meta.len();
    }
    let mut unique_docs = HashSet::new();
    let mut wal_lag_events = 0usize;
    let mut checker = IntegrityChecker::new();

    if wal_path.exists() {
        let mut reader = WalReader::new(&wal_path)?;
        let entries = reader.replay_from(0)?;
        wal_lag_events = entries.len();
        for e in entries {
            unique_docs.insert(e.doc_id);
            checker.add_document(e.doc_id, e.path, e.mtime_ns / 1_000_000_000);
        }
    }

    let report = checker.check()?;

    let metrics = MetricsCollector::new(data_dir.join("metrics.db"))?;
    let (query_p50_ms, query_p99_ms) = metrics.get_latency_stats()?;
    let zero_result_rate = metrics.zero_result_rate()?;
    let metrics_db_size = metrics.db_size().unwrap_or(0);

    let rss_estimate = (delta_size_bytes + wal_size_bytes + metrics_db_size) as usize;
    let budget = std::env::var("LOCALSEARCH_MEMORY_BUDGET")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(512 * 1024 * 1024);
    let mut memory_controller = MemoryController::new(budget);
    memory_controller.update(rss_estimate);
    let memory_state: MemoryState = memory_controller.state();

    let registry = ThreadRegistry::get_instance();
    let thread_count = registry.thread_count();
    let watchdog_healthy = registry.check_watchdog().is_ok();

    let doc_count = unique_docs.len();
    let content_indexed_fraction = (1.0 - report.stale_rate - report.phantom_rate).clamp(0.0, 1.0);

    Ok(HealthState {
        memory_state: format!("{:?}", memory_state),
        segment_count,
        delta_size_bytes,
        wal_lag_events,
        doc_count,
        content_indexed_fraction,
        last_compaction_ago_secs,
        phantom_rate: report.phantom_rate,
        stale_rate: report.stale_rate,
        query_p50_ms,
        query_p99_ms,
        zero_result_rate,
        thread_count,
        watchdog_healthy,
    })
}

fn run_runtime_watchers(
    scope_rx: std::sync::mpsc::Receiver<localsearch::fs::tcc::TccScope>,
    volume_rx: std::sync::mpsc::Receiver<localsearch::fs::volumes::VolumeInfo>,
    volume_monitor: &mut VolumeMonitor,
) {
    let runtime_secs = std::env::var("LOCALSEARCH_RUNTIME_SECONDS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    if runtime_secs == 0 {
        return;
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(runtime_secs);
    while std::time::Instant::now() < deadline {
        while let Ok(new_vol) = volume_rx.try_recv() {
            log::info!(
                "Volume mounted: {} ({})",
                new_vol.path.display(),
                new_vol.uuid
            );
            volume_monitor.register_volume(new_vol);
        }

        match scope_rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(scope) => {
                log::warn!("TCC scope changed at runtime: {:?}", scope);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let jobs = volume_monitor.pending_reconciliation_jobs();
        for job in jobs {
            log::info!("Reconciling volume {} at {}", job.uuid, job.path.display());
            volume_monitor.mark_reconciled(&job.uuid);
        }
    }
}
