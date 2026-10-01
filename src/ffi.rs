//! C ABI consumed by the Swift frontend (`frontend/Sources/Backend/FFIBackend.swift`).
//!
//! All functions are safe to call from any thread. The engine is created on
//! first use and indexes in the background; queries are answered from memory.

use crate::engine::{Engine, EngineConfig};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::sync::{Arc, OnceLock};

#[repr(C)]
pub struct SearchResult {
    pub doc_id: u64,
    pub path: *const c_char,
    pub score: f32,
    pub snippet: *const c_char,
}

/// Mirrors `engine::IndexStatus`. `phase`: 0 loading, 1 scanning, 2 extracting
/// content, 3 ready.
#[repr(C)]
pub struct IndexStatus {
    pub phase: u32,
    pub budget_exhausted: u32,
    pub doc_count: u64,
    pub work_done: u64,
    pub work_total: u64,
    pub elapsed_secs: u64,
}

static ENGINE: OnceLock<Option<Arc<Engine>>> = OnceLock::new();

fn engine() -> Option<&'static Arc<Engine>> {
    ENGINE
        .get_or_init(|| {
            let _ = env_logger::try_init();
            let data_dir = Engine::default_data_dir();
            let config = EngineConfig::load(&data_dir);
            match Engine::open(&data_dir, config) {
                Ok(engine) => {
                    engine.start();
                    Some(engine)
                }
                Err(e) => {
                    log::error!("search engine unavailable: {e:#}");
                    None
                }
            }
        })
        .as_ref()
}

/// Borrow a C string argument as UTF-8. Err carries the status code to return.
unsafe fn str_arg<'a>(ptr: *const c_char) -> Result<&'a str, i32> {
    if ptr.is_null() {
        return Err(1);
    }
    CStr::from_ptr(ptr).to_str().map_err(|_| 2)
}

/// Run a query. On success (0) `*results` points to `*count` results, to be
/// released with `localsearch_free_results`; both are null/0 for no results.
/// Returns 1 for null arguments, 2 for invalid UTF-8, 3 if the engine is unavailable.
#[no_mangle]
pub extern "C" fn localsearch_query(
    query: *const c_char,
    results: *mut *mut SearchResult,
    count: *mut usize,
) -> i32 {
    if results.is_null() || count.is_null() {
        return 1;
    }
    let query_str = match unsafe { str_arg(query) } {
        Ok(q) => q,
        Err(code) => return code,
    };
    unsafe {
        *results = std::ptr::null_mut();
        *count = 0;
    }
    let Some(engine) = engine() else {
        return 3;
    };

    let mut ffi_results = Vec::new();
    for hit in engine.search(query_str) {
        let snippet = Path::new(&hit.path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Paths with interior NULs cannot cross the boundary; skip them
        let (Ok(path), Ok(snippet)) = (CString::new(hit.path), CString::new(snippet)) else {
            continue;
        };
        ffi_results.push(SearchResult {
            doc_id: hit.doc_id,
            path: path.into_raw(),
            score: hit.score,
            snippet: snippet.into_raw(),
        });
    }

    if ffi_results.is_empty() {
        return 0;
    }

    let len = ffi_results.len();
    let ptr = Box::into_raw(ffi_results.into_boxed_slice()) as *mut SearchResult;
    unsafe {
        *results = ptr;
        *count = len;
    }
    0
}

#[no_mangle]
pub extern "C" fn localsearch_free_results(results: *mut SearchResult, count: usize) {
    if results.is_null() || count == 0 {
        return;
    }

    unsafe {
        let slice = std::slice::from_raw_parts_mut(results, count);
        for item in slice.iter() {
            if !item.path.is_null() {
                let _ = CString::from_raw(item.path as *mut c_char);
            }
            if !item.snippet.is_null() {
                let _ = CString::from_raw(item.snippet as *mut c_char);
            }
        }

        let _ = Box::from_raw(slice as *mut [SearchResult]);
    }
}

/// The user opened `path` from the results. Feeds ranking.
#[no_mangle]
pub extern "C" fn localsearch_record_click(path: *const c_char) -> i32 {
    let path_str = match unsafe { str_arg(path) } {
        Ok(s) => s,
        Err(code) => return code,
    };
    let Some(engine) = engine() else {
        return 3;
    };
    engine.record_click(path_str);
    0
}

/// Set what gets indexed. `config_json` is an `EngineConfig` object; omitted
/// fields take their defaults, e.g.
/// `{"roots": ["~/Documents"], "excludes": ["node_modules"], "max_depth": 12,
///   "index_hidden": false, "index_content": true, "memory_budget_mb": 256}`.
/// The configuration is persisted and applied to the running engine.
/// Returns 4 if the JSON does not parse, 5 if it could not be saved.
#[no_mangle]
pub extern "C" fn localsearch_configure(config_json: *const c_char) -> i32 {
    let json = match unsafe { str_arg(config_json) } {
        Ok(s) => s,
        Err(code) => return code,
    };
    let Ok(config) = serde_json::from_str::<EngineConfig>(json) else {
        return 4;
    };

    let data_dir = Engine::default_data_dir();
    if let Err(e) = config.save(&data_dir) {
        log::error!("could not save configuration: {e:#}");
        return 5;
    }
    // If the engine is not running yet it will read the file just written.
    let already_running = ENGINE.get().is_some();
    let Some(engine) = engine() else {
        return 3;
    };
    if already_running {
        if let Err(e) = engine.reconfigure(EngineConfig::load(&data_dir)) {
            log::error!("could not apply configuration: {e:#}");
            return 5;
        }
    }
    0
}

/// Report indexing progress. Starts the engine if it is not running yet.
#[no_mangle]
pub extern "C" fn localsearch_index_status(status: *mut IndexStatus) -> i32 {
    if status.is_null() {
        return 1;
    }
    let Some(engine) = engine() else {
        return 3;
    };
    let current = engine.status();
    unsafe {
        *status = IndexStatus {
            phase: current.phase as u32,
            budget_exhausted: current.budget_exhausted as u32,
            doc_count: current.doc_count,
            work_done: current.work_done,
            work_total: current.work_total,
            elapsed_secs: current.elapsed_secs,
        };
    }
    0
}

/// Stop indexing and save the index. Call once, when the app is quitting.
#[no_mangle]
pub extern "C" fn localsearch_shutdown() {
    if let Some(Some(engine)) = ENGINE.get() {
        engine.shutdown();
    }
}

#[cfg(test)]
mod ffi_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn test_null_arguments_are_rejected() {
        let mut results: *mut SearchResult = std::ptr::null_mut();
        let mut count = 0usize;
        assert_eq!(localsearch_query(std::ptr::null(), &mut results, &mut count), 1);
        assert_eq!(localsearch_record_click(std::ptr::null()), 1);
        assert_eq!(localsearch_configure(std::ptr::null()), 1);
        assert_eq!(localsearch_index_status(std::ptr::null_mut()), 1);
        localsearch_free_results(std::ptr::null_mut(), 0);
    }

    /// The only test that touches the process-wide engine: it points it at
    /// temporary directories through the environment before first use.
    #[test]
    fn test_configure_query_click_shutdown_roundtrip() {
        let data = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        std::fs::write(root_path.join("ffi_roundtrip_notes.txt"), "hello").unwrap();
        std::env::set_var("LOCALSEARCH_DATA_DIR", data.path());

        let bad = CString::new("{not json").unwrap();
        assert_eq!(localsearch_configure(bad.as_ptr()), 4);

        let config = serde_json::json!({ "roots": [root_path] }).to_string();
        let config = CString::new(config).unwrap();
        assert_eq!(localsearch_configure(config.as_ptr()), 0);

        let mut status = IndexStatus { phase: 0, budget_exhausted: 0, doc_count: 0, work_done: 0, work_total: 0, elapsed_secs: 0 };
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            assert_eq!(localsearch_index_status(&mut status), 0);
            if status.phase == 3 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(status.phase, 3, "engine never became ready");
        assert_eq!(status.doc_count, 1);

        let query = CString::new("roundtrip").unwrap();
        let mut results: *mut SearchResult = std::ptr::null_mut();
        let mut count = 0usize;
        assert_eq!(localsearch_query(query.as_ptr(), &mut results, &mut count), 0);
        assert_eq!(count, 1);
        let path = unsafe { CStr::from_ptr((*results).path) }.to_owned();
        assert!(path.to_str().unwrap().ends_with("ffi_roundtrip_notes.txt"));
        localsearch_free_results(results, count);

        assert_eq!(localsearch_record_click(path.as_ptr()), 0);

        localsearch_shutdown();
        assert!(data.path().join(".clean_shutdown").exists());
        assert!(data.path().join("manifest.json").exists());
    }
}
