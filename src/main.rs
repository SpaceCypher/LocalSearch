use anyhow::Result;
use std::path::PathBuf;
use localsearch::startup;

fn main() -> Result<()> {
    env_logger::init();
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
