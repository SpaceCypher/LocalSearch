use crate::index::bktree::BkTree;
use crate::index::delta::{DeltaIndex, DocId, Document, Posting, FIELD_FILENAME, FIELD_PATH};
use crate::index::trie::PathTrie;
use crate::index::trigram::TrigramIndex;
use crate::query::executor::QueryExecutor;
use crate::query::parser::{Query, Tokenizer};
use crate::query::phonetic::double_metaphone;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::SystemTime;
use walkdir::WalkDir;
use xxhash_rust::xxh3::xxh3_64;

#[repr(C)]
pub struct SearchResult {
    pub doc_id: u64,
    pub path: *const c_char,
    pub score: f32,
    pub snippet: *const c_char,
}

struct OwnedSearchResult {
    doc_id: u64,
    path: String,
    score: f32,
    snippet: String,
}

const MAX_RESULTS: usize = 100;
const MAX_SCANNED_ENTRIES: usize = 1_000_000;
const MAX_DEPTH: usize = 8;

struct SearchEngine {
    executor: QueryExecutor,
}

static SEARCH_ENGINE: OnceLock<Option<SearchEngine>> = OnceLock::new();
static ENGINE_INIT_STARTED: AtomicBool = AtomicBool::new(false);

fn ensure_engine_init_started() {
    if SEARCH_ENGINE.get().is_some() {
        return;
    }

    if ENGINE_INIT_STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        std::thread::spawn(|| {
            let _ = SEARCH_ENGINE.get_or_init(build_search_engine);
        });
    }
}

fn make_doc_id(path: &str) -> u64 {
    xxh3_64(path.as_bytes())
}

fn build_snippet(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "".to_string())
}

fn tokenize_query(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

fn search_roots() -> Vec<PathBuf> {
    if let Ok(root) = std::env::var("LOCALSEARCH_ROOT") {
        return vec![PathBuf::from(root)];
    }

    let mut roots = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(home);
        for dir in ["Downloads", "Documents", "Desktop"] {
            let candidate = home_path.join(dir);
            if candidate.exists() {
                roots.push(candidate);
            }
        }

        // Avoid scanning the entire home directory by default; it can cause
        // very high first-query latency in the desktop app.
        if roots.is_empty() {
            roots.push(home_path);
        }
    }

    if roots.is_empty() {
        roots.push(PathBuf::from("."));
    }

    roots
}

fn component_is_noise(component: &str) -> bool {
    matches!(
        component,
        ".git"
            | "node_modules"
            | ".build"
            | "target"
            | "dist"
            | "build"
            | "tmp"
            | "temp"
            | ".cache"
            | "cache"
            | "caches"
            | "deriveddata"
            | "modulecache"
            | ".trash"
            | "trash"
    )
}

fn is_noise_path(path: &Path) -> bool {
    path.components().any(|component| {
        let value = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        if value.starts_with('.') && value != "." && value != ".." {
            return true;
        }
        component_is_noise(&value)
    })
}

fn is_noise_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_ascii_lowercase())
        .unwrap_or_default();

    name.starts_with("~$") || name.starts_with('.')
}

fn preferred_path_boost(path: &Path) -> f32 {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    if lower.contains("/downloads/") {
        35.0
    } else if lower.contains("/documents/") {
        30.0
    } else if lower.contains("/desktop/") {
        25.0
    } else {
        0.0
    }
}

fn recency_boost(path: &Path) -> f32 {
    let Ok(metadata) = std::fs::metadata(path) else {
        return 0.0;
    };
    let Ok(modified) = metadata.modified() else {
        return 0.0;
    };
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return 0.0;
    };

    let days = age.as_secs() / 86_400;
    if days <= 1 {
        8.0
    } else if days <= 7 {
        5.0
    } else if days <= 30 {
        2.0
    } else {
        0.0
    }
}

fn score_candidate(path: &Path, query: &str, query_tokens: &[String]) -> f32 {
    let filename_raw = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let filename = filename_raw.to_ascii_lowercase();
    
    let stem_raw = path.file_stem().and_then(|n| n.to_str()).unwrap_or(filename_raw);
    let stem = stem_raw.to_ascii_lowercase();
    
    let path_lc = path.to_string_lossy().to_ascii_lowercase();

    // Dotted queries (e.g. "adaprag.pdf") should strictly match the filename or path.
    if query.contains('.') && !filename.contains(query) && !path_lc.contains(query) {
        return 0.0;
    }

    let mut lexical_score = 0.0;

    // Layer 1: Strong Phrase Matching (The "Big Wins")
    if filename == query || stem == query {
        lexical_score += 250.0; // Boosted
    }
    if filename.starts_with(query) || stem.starts_with(query) {
        lexical_score += 160.0; // Boosted
    }
    if filename.contains(query) || stem.contains(query) {
        lexical_score += 110.0; // Boosted
    }

    // Layer 2: Independent Token Matching
    let mut matched_tokens = 0usize;
    for token in query_tokens {
        if stem.starts_with(token) {
            lexical_score += 35.0;
            matched_tokens += 1;
        } else if stem.contains(token) || filename.contains(token) {
            lexical_score += 22.0;
            matched_tokens += 1;
        } else if path_lc.contains(token) {
            lexical_score += 6.0;
        }
    }

    // Multi-token bonus: if ALL tokens in a query (e.g. "cd" and "lab") match,
    // this document is highly relevant.
    if !query_tokens.is_empty() && matched_tokens >= query_tokens.len() {
        lexical_score += 80.0; // Boosted
    }
    
    // Proximity/Sequence bonus: if the tokens appear in the correct order
    // in the filename, add an extra boost.
    if query_tokens.len() > 1 {
        let mut last_pos = 0;
        let mut in_order = true;
        for token in query_tokens {
            if let Some(pos) = filename.find(token) {
                if pos < last_pos {
                    in_order = false;
                    break;
                }
                last_pos = pos;
            } else {
                in_order = false;
                break;
            }
        }
        if in_order {
            lexical_score += 50.0;
        }
    }

    // Never return a score for unrelated files.
    if lexical_score <= 0.0 {
        return 0.0;
    }

    lexical_score + preferred_path_boost(path) + recency_boost(path)
}


fn build_search_engine() -> Option<SearchEngine> {
    let mut delta_index = DeltaIndex::new(256 * 1024 * 1024);
    let mut bk_tree = BkTree::new();
    let mut path_trie = PathTrie::new();
    let mut trigram_index = TrigramIndex::new();
    let mut phonetic_index: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let tokenizer = Tokenizer::new();

    let roots = search_roots();
    let mut scanned = 0usize;
    let mut next_doc_id = 1u64;

    for root in roots {
        if scanned > MAX_SCANNED_ENTRIES {
            break;
        }

        for entry in WalkDir::new(&root)
            .follow_links(false)
            .max_depth(MAX_DEPTH)
            .into_iter()
            .filter_entry(|entry| {
                let path = entry.path();
                if path == root {
                    return true;
                }
                if path.is_dir() {
                    !is_noise_path(path)
                } else {
                    true
                }
            })
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if is_noise_file(path) || is_noise_path(path) {
                continue;
            }
            
            scanned += 1;
            if scanned > MAX_SCANNED_ENTRIES {
                break;
            }

            let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            let path_str = path.to_string_lossy().to_string();
            let doc_id = DocId(next_doc_id);
            next_doc_id = next_doc_id.saturating_add(1);

            let mut postings_by_term: std::collections::HashMap<String, (u32, Vec<u32>)> =
                std::collections::HashMap::new();

            let token_source = format!("{} {}", filename, path_str);
            let tokens = tokenizer.tokenize(&token_source);
            for token in tokens {
                let entry = postings_by_term
                    .entry(token.term)
                    .or_insert_with(|| (0, Vec::new()));
                entry.0 = entry.0.saturating_add(1);
                entry.1.push(token.position);
            }

            if postings_by_term.is_empty() {
                continue;
            }

            let mut postings: std::collections::HashMap<String, Posting> = std::collections::HashMap::new();
            for (term, (freq, positions)) in postings_by_term {
                bk_tree.insert(&term);
                trigram_index.insert(&term, doc_id);
                let code = double_metaphone(&term);
                let entry = phonetic_index.entry(code.primary).or_insert_with(Vec::new);
                if !entry.contains(&term) {
                    entry.push(term.clone());
                }

                postings.insert(
                    term,
                    Posting {
                        doc_id,
                        term_freq: freq,
                        field_mask: FIELD_FILENAME | FIELD_PATH,
                        positions,
                    },
                );
            }

            let doc = Document {
                doc_id,
                path: path_str.clone(),
                content_hash: 0,
            };
            let _ = delta_index.insert_document(doc, postings);
            path_trie.insert(&path_str, doc_id);

            // Also index raw filename for substring-heavy fuzzy fallback.
            trigram_index.insert(filename, doc_id);
        }
    }

    Some(SearchEngine {
        executor: QueryExecutor::new(
            delta_index,
            bk_tree,
            path_trie,
            trigram_index,
            phonetic_index,
        ),
    })
}

fn run_engine_query(engine: &SearchEngine, query: &str) -> Option<Vec<OwnedSearchResult>> {
    let parsed = Query::parse(query).ok()?;
    let results = engine.executor.execute(parsed, None).ok()?;
    Some(
        results
            .into_iter()
            .map(|result| {
                let path = result.path;
                let doc_id = result.doc_id.0;
                let snippet = Path::new(&path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(ToOwned::to_owned)
                    .unwrap_or_default();

                OwnedSearchResult {
                    doc_id,
                    path,
                    score: result.score,
                    snippet,
                }
            })
            .collect(),
    )
}

fn run_query(query: &str) -> Vec<OwnedSearchResult> {
    let query_owned = query.trim().to_ascii_lowercase();
    if query_owned.is_empty() {
        return Vec::new();
    }

    // Dotted/path-like queries (e.g. "foo.pdf", "src/main.rs") and
    // structured filename queries (e.g. "1_advanced") are better served by
    // direct filename/path matching than token BM25.
    let prefer_fallback = query_owned.contains('.') || query_owned.contains('/') || query_owned.contains('_');

    // Keep first-query latency low by serving fallback results immediately,
    // while the heavier full-text engine warms up in the background.
    ensure_engine_init_started();

    if !prefer_fallback {
        if let Some(engine_opt) = SEARCH_ENGINE.get() {
            if let Some(engine) = engine_opt.as_ref() {
                if let Some(results) = run_engine_query(engine, &query_owned) {
                    if !results.is_empty() {
                        return results;
                    }
                }
            }
        }
    }

    let query_tokens = tokenize_query(&query_owned);
    let roots = search_roots();
    let structured_query = query_owned.contains('_');
    let traversal_depth = if structured_query { 4 } else { MAX_DEPTH };

    let mut results = Vec::new();
    let mut scanned = 0usize;

    'roots: for root in roots {
        if scanned > MAX_SCANNED_ENTRIES {
            break;
        }

        for entry in WalkDir::new(&root)
            .follow_links(false)
            .max_depth(traversal_depth)
            .into_iter()
            .filter_entry(|entry| {
                let path = entry.path();
                if path == root {
                    return true;
                }
                if path.is_dir() {
                    !is_noise_path(path)
                } else {
                    true
                }
            })
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if is_noise_file(path) || is_noise_path(path) {
                continue;
            }
            
            scanned += 1;
            if scanned > MAX_SCANNED_ENTRIES {
                break;
            }

            let score = score_candidate(path, &query_owned, &query_tokens);
            if score <= 0.0 {
                continue;
            }

            let path_str = path.to_string_lossy().to_string();
            results.push(OwnedSearchResult {
                doc_id: make_doc_id(&path_str),
                path: path_str,
                score,
                snippet: build_snippet(path),
            });

            // Strong exact-like hits should return immediately for responsive UX.
            if score >= 220.0 {
                results.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                results.truncate(MAX_RESULTS);
                return results;
            }

            // If enough candidates are gathered, avoid scanning the entire tree.
            if results.len() >= MAX_RESULTS && scanned >= 8_000 && !prefer_fallback {
                break 'roots;
            }
        }
    }

    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(MAX_RESULTS);
    results
}

#[no_mangle]
pub extern "C" fn localsearch_query(
    query: *const c_char,
    results: *mut *mut SearchResult,
    count: *mut usize,
) -> i32 {
    if query.is_null() || results.is_null() || count.is_null() {
        return 1;
    }

    let query_cstr = unsafe { CStr::from_ptr(query) };
    let query_str = match query_cstr.to_str() {
        Ok(q) => q,
        Err(_) => return 2,
    };

    let owned_results = run_query(query_str);

    if owned_results.is_empty() {
        unsafe {
            *results = std::ptr::null_mut();
            *count = 0;
        }
        return 0;
    }

    let mut ffi_results = Vec::with_capacity(owned_results.len());
    for result in owned_results {
        let path_ptr = match CString::new(result.path) {
            Ok(s) => s.into_raw(),
            Err(_) => continue,
        };

        let snippet_ptr = match CString::new(result.snippet) {
            Ok(s) => s.into_raw(),
            Err(_) => {
                unsafe {
                    let _ = CString::from_raw(path_ptr);
                }
                continue;
            }
        };

        ffi_results.push(SearchResult {
            doc_id: result.doc_id,
            path: path_ptr,
            score: result.score,
            snippet: snippet_ptr,
        });
    }

    if ffi_results.is_empty() {
        unsafe {
            *results = std::ptr::null_mut();
            *count = 0;
        }
        return 0;
    }

    let len = ffi_results.len();
    let boxed = ffi_results.into_boxed_slice();
    let ptr = Box::into_raw(boxed) as *mut SearchResult;

    unsafe {
        *results = ptr;
        *count = len;
    }

    0
}

#[no_mangle]
pub extern "C" fn localsearch_record_click(path: *const c_char) -> i32 {
    if path.is_null() {
        return 1;
    }

    let path_cstr = unsafe { CStr::from_ptr(path) };
    let path_str = match path_cstr.to_str() {
        Ok(s) => s,
        Err(_) => return 2,
    };

    if let Some(engine_opt) = SEARCH_ENGINE.get() {
        if let Some(engine) = engine_opt.as_ref() {
            engine.executor.ranker.record_click(PathBuf::from(path_str));
            return 0;
        }
    }

    3 // Engine not initialized
}

#[no_mangle]
pub extern "C" fn localsearch_free_results(results: *mut SearchResult, count: usize) {
    if results.is_null() || count == 0 {
        return;
    }

    unsafe {
        let slice = std::slice::from_raw_parts_mut(results, count);
        for item in slice {
            if !item.path.is_null() {
                let _ = CString::from_raw(item.path as *mut c_char);
            }
            if !item.snippet.is_null() {
                let _ = CString::from_raw(item.snippet as *mut c_char);
            }
        }

        let _ = Vec::from_raw_parts(results, count, count);
    }
}
