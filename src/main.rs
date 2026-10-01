use anyhow::Result;
use localsearch::engine::{Engine, EngineConfig, Phase};
use localsearch::metrics::dashboard::{HealthState, render_dashboard};
use localsearch::metrics::ThreadRegistry;
use std::time::Duration;

const USAGE: &str = "\
Usage: localsearch <command>

  index            Build or update the index, then exit
  query <text>     Search the saved index
  --debug-panel    Show index health for the data directory

The data directory is $LOCALSEARCH_DATA_DIR or ~/.localsearch.
Indexed folders come from its config.json, or $LOCALSEARCH_ROOT.";

fn main() -> Result<()> {
    env_logger::init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let data_dir = Engine::default_data_dir();
    let engine = Engine::open(&data_dir, EngineConfig::load(&data_dir))?;

    match args.first().map(String::as_str) {
        Some("--debug-panel") => {
            // Read-only: safe to run while the app has the same directory open
            engine.load_snapshot()?;
            let stats = engine.stats();
            let registry = ThreadRegistry::get_instance();
            let state = HealthState {
                memory_state: if stats.over_budget { "Over budget" } else { "Normal" }.to_string(),
                segment_count: stats.segment_count,
                delta_size_bytes: stats.index_bytes,
                wal_lag_events: stats.wal_lag_events,
                doc_count: stats.doc_count,
                content_indexed_fraction: stats.content_indexed_fraction,
                last_compaction_ago_secs: stats.last_snapshot_age_secs,
                phantom_rate: stats.phantom_rate,
                stale_rate: stats.stale_rate,
                query_p50_ms: stats.query_p50_ms,
                query_p99_ms: stats.query_p99_ms,
                zero_result_rate: stats.zero_result_rate,
                thread_count: registry.thread_count(),
                watchdog_healthy: registry.check_watchdog().is_ok(),
            };
            println!("{}", render_dashboard(&state));
        }
        Some("index") => {
            engine.start();
            let mut last_phase = None;
            loop {
                let status = engine.status();
                if last_phase != Some(status.phase) {
                    eprintln!("{:?}: {} documents", status.phase, status.doc_count);
                    last_phase = Some(status.phase);
                }
                if status.phase == Phase::Ready {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            engine.shutdown();
            println!("Indexed {} documents into {}", engine.doc_count(), data_dir.display());
        }
        Some("query") if args.len() > 1 => {
            if engine.load_snapshot()? == 0 {
                anyhow::bail!("no index in {}; run `localsearch index` first", data_dir.display());
            }
            for hit in engine.search(&args[1..].join(" ")) {
                println!("{:8.1}  {}", hit.score, hit.path);
            }
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
    Ok(())
}
