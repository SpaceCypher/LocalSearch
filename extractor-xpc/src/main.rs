/// LocalSearch Content Extractor XPC Service
///
/// This process runs sandboxed (read-only file access, no network).
/// It receives extraction jobs over stdin/XPC channel, extracts text,
/// and returns results. Crashes here are contained — the main process
/// detects them via the XPC connection interruption handler.
fn main() {
    env_logger::init();
    log::info!("localsearch-extractor XPC service starting...");
    // XPC event loop will be wired in Task 25.
    // Task 21 FFI entrypoints currently live in the main `localsearch` crate.
}
