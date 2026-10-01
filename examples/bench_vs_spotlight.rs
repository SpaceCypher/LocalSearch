//! Benchmark LocalSearch against Spotlight on this machine's real files.
//!
//!     cargo run --release --example bench_vs_spotlight > docs/benchmarks/spotlight.md
//!
//! Both engines are queried in-process (Spotlight through the MDQuery API, so
//! no `mdfind` process start-up is counted), over the same folders, with the
//! same result cap. A fresh LocalSearch index is built in a scratch directory;
//! the app's own index in ~/.localsearch is not touched.
//!
//! The report contains timings and result counts only, never file names.

use localsearch::engine::{Engine, EngineConfig, Phase};
use std::ffi::{c_void, CString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const RESULT_CAP: usize = 100;
const WARMUP_RUNS: usize = 3;
const MEASURED_RUNS: usize = 30;
/// Stop repeating a query once it has used this much time (slow Spotlight content queries)
const PER_QUERY_BUDGET: Duration = Duration::from_secs(20);

// ─── Spotlight (MDQuery) ──────────────────────────────────────────────────────

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn MDQueryCreate(alloc: *const c_void, query: *const c_void, values: *const c_void, sorting: *const c_void) -> *mut c_void;
    fn MDQuerySetSearchScope(query: *mut c_void, scope: *const c_void, options: u32);
    fn MDQuerySetMaxCount(query: *mut c_void, size: isize);
    fn MDQueryExecute(query: *mut c_void, flags: usize) -> u8;
    fn MDQueryGetResultCount(query: *mut c_void) -> isize;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeArrayCallBacks: c_void;
    fn CFStringCreateWithCString(alloc: *const c_void, c_str: *const std::os::raw::c_char, encoding: u32) -> *const c_void;
    fn CFArrayCreate(alloc: *const c_void, values: *const *const c_void, count: isize, callbacks: *const c_void) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

const UTF8: u32 = 0x0800_0100;
const MD_QUERY_SYNCHRONOUS: usize = 1;

fn cf_string(s: &str) -> *const c_void {
    let c = CString::new(s).unwrap();
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) }
}

/// Run a Spotlight query synchronously over `roots`. Returns the hit count
/// (capped at RESULT_CAP), or None if Spotlight rejected the query.
fn spotlight(query: &str, roots: &[PathBuf]) -> Option<usize> {
    unsafe {
        let query_string = cf_string(query);
        let md_query = MDQueryCreate(std::ptr::null(), query_string, std::ptr::null(), std::ptr::null());
        CFRelease(query_string);
        if md_query.is_null() {
            return None;
        }
        let scope: Vec<*const c_void> = roots.iter().map(|r| cf_string(&r.to_string_lossy())).collect();
        let scope_array = CFArrayCreate(std::ptr::null(), scope.as_ptr(), scope.len() as isize, &kCFTypeArrayCallBacks);
        MDQuerySetSearchScope(md_query, scope_array, 0);
        MDQuerySetMaxCount(md_query, RESULT_CAP as isize);
        let ok = MDQueryExecute(md_query, MD_QUERY_SYNCHRONOUS) != 0;
        let count = MDQueryGetResultCount(md_query);
        CFRelease(scope_array);
        for item in scope {
            CFRelease(item);
        }
        CFRelease(md_query as *const c_void);
        ok.then_some(count.max(0) as usize)
    }
}

/// Every word must appear in the file name (what `mdfind -name` does)
fn spotlight_name_query(text: &str) -> String {
    text.split_whitespace()
        .map(|word| format!("kMDItemFSName == \"*{word}*\"cd"))
        .collect::<Vec<_>>()
        .join(" && ")
}

/// Every word must appear in the file name or in the content: the closest
/// equivalent of what LocalSearch searches by default
fn spotlight_everything_query(text: &str) -> String {
    text.split_whitespace()
        .map(|word| format!("(kMDItemFSName == \"*{word}*\"cd || kMDItemTextContent == \"{word}*\"cdw)"))
        .collect::<Vec<_>>()
        .join(" && ")
}

// ─── Measurement ──────────────────────────────────────────────────────────────

struct Timing {
    first_ms: f64,
    median_ms: f64,
    p95_ms: f64,
    hits: usize,
    runs: usize,
}

fn measure(mut run: impl FnMut() -> usize) -> Timing {
    let started = Instant::now();
    let t = Instant::now();
    let hits = run();
    let first_ms = t.elapsed().as_secs_f64() * 1000.0;

    for _ in 1..WARMUP_RUNS {
        if started.elapsed() > PER_QUERY_BUDGET {
            break;
        }
        run();
    }
    let mut samples = Vec::new();
    while samples.len() < MEASURED_RUNS && (samples.is_empty() || started.elapsed() < PER_QUERY_BUDGET) {
        let t = Instant::now();
        run();
        samples.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Timing {
        first_ms,
        median_ms: samples[samples.len() / 2],
        p95_ms: samples[((samples.len() * 95) / 100).min(samples.len() - 1)],
        hits,
        runs: samples.len(),
    }
}

fn ms(value: f64) -> String {
    if value >= 100.0 {
        format!("{value:.0}")
    } else if value >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

fn hits(count: usize) -> String {
    if count >= RESULT_CAP { format!("{RESULT_CAP}+") } else { count.to_string() }
}

fn command_output(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

fn rss_mb() -> f64 {
    command_output("/bin/ps", &["-o", "rss=", "-p", &std::process::id().to_string()])
        .parse::<f64>()
        .map_or(0.0, |kb| kb / 1024.0)
}

fn dir_size_mb(dir: &Path, extension: &str) -> f64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|ext| ext == extension))
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len() as f64)
                .sum::<f64>()
        })
        .unwrap_or(0.0)
        / (1024.0 * 1024.0)
}

fn wait_for(engine: &Engine, phase: Phase) {
    while engine.status().phase != phase && engine.status().phase != Phase::Ready {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// (label, query). Generic words only, so the report says nothing about
/// what is on the machine it ran on.
const QUERIES: &[(&str, &str)] = &[
    ("Common word", "report"),
    ("Common word", "test"),
    ("Common word", "config"),
    ("Common word", "notes"),
    ("Common word", "screenshot"),
    ("Short prefix", "re"),
    ("Short prefix", "doc"),
    ("Exact file name", "README.md"),
    ("Exact file name", "package.json"),
    ("Exact file name", "index.html"),
    ("Two words", "project plan"),
    ("Two words", "meeting notes"),
    ("Content word", "function"),
    ("Content word", "license"),
    ("Content word", "copyright"),
    ("Rare word", "zebrafish"),
    ("Typo", "reprot"),
    ("Typo", "screnshot"),
    ("Typo", "licnese"),
];

/// Child-process mode: load a saved index and print this process's memory,
/// so the figure is not inflated by whatever the indexing run left behind.
fn report_loaded_memory(data_dir: &Path) -> anyhow::Result<()> {
    let engine = Engine::open(data_dir, EngineConfig::default())?;
    engine.load_snapshot()?;
    engine.search("report");
    println!("{:.0}", rss_mb());
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--loaded-memory" {
        return report_loaded_memory(Path::new(&args[2]));
    }

    let data_dir = std::env::temp_dir().join(format!("localsearch-bench-{}", std::process::id()));
    let config = EngineConfig::default().normalized();
    let roots = config.roots.clone();

    // ── Build a fresh index, timing each phase ───────────────────────────────
    let engine = Engine::open(&data_dir, config)?;
    let build_started = Instant::now();
    engine.start();
    wait_for(&engine, Phase::Scanning);
    wait_for(&engine, Phase::Extracting);
    let names_secs = build_started.elapsed().as_secs_f64();
    let names_docs = engine.doc_count();
    wait_for(&engine, Phase::Ready);
    let build_secs = build_started.elapsed().as_secs_f64();
    let status = engine.status();
    let stats = engine.stats();
    let rss_after_build = rss_mb();

    // ── Does saving the index get in the way of searching? ──────────────────
    eprintln!("save");
    let stop = std::sync::atomic::AtomicBool::new(false);
    let (save_secs, during_save, before_save) = std::thread::scope(|scope| {
        let sampler = scope.spawn(|| {
            let mut samples: Vec<(Instant, f64)> = Vec::new();
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let t = Instant::now();
                engine.search("report");
                samples.push((t, t.elapsed().as_secs_f64() * 1000.0));
            }
            samples
        });
        std::thread::sleep(Duration::from_secs(2));
        let save_started = Instant::now();
        let saved = engine.snapshot();
        let save_secs = save_started.elapsed().as_secs_f64();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Err(e) = saved {
            eprintln!("snapshot failed: {e:#}");
        }
        let samples = sampler.join().unwrap();
        let split = |during: bool| -> Vec<f64> {
            let mut values: Vec<f64> = samples
                .iter()
                .filter(|(at, _)| (*at >= save_started) == during)
                .map(|(_, ms)| *ms)
                .collect();
            values.sort_by(|a, b| a.partial_cmp(b).unwrap());
            values
        };
        (save_secs, split(true), split(false))
    });
    eprintln!("save took {save_secs:.2}s; worst query during save {:.1} ms", during_save.last().copied().unwrap_or(0.0));

    // ── Queries ──────────────────────────────────────────────────────────────
    struct Row {
        label: &'static str,
        query: &'static str,
        local: Timing,
        spot_name: Option<Timing>,
        spot_all: Option<Timing>,
    }
    let mut rows = Vec::new();
    for &(label, query) in QUERIES {
        eprintln!("query: {query}");
        let local = measure(|| engine.search(query).len());
        let name_query = spotlight_name_query(query);
        let all_query = spotlight_everything_query(query);
        let spot_name = spotlight(&name_query, &roots).map(|_| measure(|| spotlight(&name_query, &roots).unwrap_or(0)));
        let spot_all = spotlight(&all_query, &roots).map(|_| measure(|| spotlight(&all_query, &roots).unwrap_or(0)));
        rows.push(Row { label, query, local, spot_name, spot_all });
    }

    // ── Freshness: how long until a new file can be found ────────────────────
    eprintln!("freshness");
    let mut fresh_local = Vec::new();
    let mut fresh_spot = Vec::new();
    if let Some(root) = roots.iter().find(|r| r.ends_with("Documents")).or(roots.first()) {
        for i in 0..5 {
            let token = format!("lsbench{}x{}", std::process::id(), i);
            let path = root.join(format!("{token}.txt"));
            let name_query = spotlight_name_query(&token);
            let created = Instant::now();
            std::fs::write(&path, "benchmark probe file, safe to delete")?;
            let (mut local_at, mut spot_at) = (None, None);
            while created.elapsed() < Duration::from_secs(30) && (local_at.is_none() || spot_at.is_none()) {
                if local_at.is_none() && engine.search(&token).iter().any(|hit| hit.path.contains(&token)) {
                    local_at = Some(created.elapsed().as_secs_f64());
                }
                if spot_at.is_none() && spotlight(&name_query, &roots).unwrap_or(0) > 0 {
                    spot_at = Some(created.elapsed().as_secs_f64());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            std::fs::remove_file(&path)?;
            fresh_local.push(local_at);
            fresh_spot.push(spot_at);
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    // ── Restart: save, reopen, load ──────────────────────────────────────────
    eprintln!("restart");
    engine.shutdown();
    let segment_mb = dir_size_mb(&data_dir, "seg");
    drop(engine);
    let reopened = Engine::open(&data_dir, EngineConfig::default())?;
    let load_started = Instant::now();
    let loaded = reopened.load_snapshot()?;
    let load_secs = load_started.elapsed().as_secs_f64();
    drop(reopened);
    let loaded_rss = std::env::current_exe()
        .ok()
        .map(|exe| command_output(&exe.to_string_lossy(), &["--loaded-memory", &data_dir.to_string_lossy()]))
        .unwrap_or_default();
    let _ = std::fs::remove_dir_all(&data_dir);

    // ── Report ───────────────────────────────────────────────────────────────
    let median = |values: &[Option<f64>]| -> String {
        let mut found: Vec<f64> = values.iter().flatten().copied().collect();
        if found.len() < values.len() {
            return format!("not found within 30 s in {} of {} trials", values.len() - found.len(), values.len());
        }
        found.sort_by(|a, b| a.partial_cmp(b).unwrap());
        format!("{:.2} s (median of {}; range {:.2}–{:.2} s)", found[found.len() / 2], found.len(), found[0], found[found.len() - 1])
    };
    let ram_gb = command_output("/usr/sbin/sysctl", &["-n", "hw.memsize"]).parse::<f64>().unwrap_or(0.0) / (1024.0 * 1024.0 * 1024.0);

    println!("# LocalSearch vs Spotlight\n");
    println!("Generated by `cargo run --release --example bench_vs_spotlight`.\n");
    println!("## Setup\n");
    println!("- Machine: {}, {:.0} GB RAM", command_output("/usr/sbin/sysctl", &["-n", "machdep.cpu.brand_string"]), ram_gb);
    println!("- macOS {}", command_output("/usr/bin/sw_vers", &["-productVersion"]));
    println!("- LocalSearch commit `{}`, release build", command_output("/usr/bin/git", &["rev-parse", "--short", "HEAD"]));
    println!("- Scope for both: `~/Downloads`, `~/Documents`, `~/Desktop` ({} items in the LocalSearch index)", status.doc_count);
    println!("- Both are called in-process: LocalSearch through `Engine::search`, Spotlight through `MDQueryExecute` (synchronous). No `mdfind` process start-up is included.");
    println!("- Both are capped at {RESULT_CAP} results per query.");
    println!("- Each query: {WARMUP_RUNS} warm-up runs, then up to {MEASURED_RUNS} measured runs (fewer if the query used more than {} s in total). Times are milliseconds.\n", PER_QUERY_BUDGET.as_secs());

    println!("## Query latency\n");
    println!("LocalSearch searches names and contents in one query. Spotlight is shown twice: names only (its fastest mode, what `mdfind -name` does) and names plus contents (the like-for-like comparison).\n");
    println!("| Kind | Query | LocalSearch median | p95 | hits | Spotlight names median | p95 | hits | Spotlight names+contents median | p95 | hits |");
    println!("|---|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|");
    let cell = |t: &Option<Timing>| match t {
        Some(t) => format!("{} | {} | {}", ms(t.median_ms), ms(t.p95_ms), hits(t.hits)),
        None => "query rejected | – | –".to_string(),
    };
    for row in &rows {
        println!(
            "| {} | `{}` | {} | {} | {} | {} | {} |",
            row.label, row.query, ms(row.local.median_ms), ms(row.local.p95_ms), hits(row.local.hits),
            cell(&row.spot_name), cell(&row.spot_all)
        );
    }

    let overall = |pick: &dyn Fn(&Row) -> Option<f64>| -> String {
        let mut values: Vec<f64> = rows.iter().filter_map(pick).collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        format!("{} / {} / {}", ms(values[0]), ms(values[values.len() / 2]), ms(values[values.len() - 1]))
    };
    println!("\nAcross the {} queries, per-query medians (fastest / middle / slowest):\n", rows.len());
    println!("- LocalSearch: {} ms", overall(&|r| Some(r.local.median_ms)));
    println!("- Spotlight, names only: {} ms", overall(&|r| r.spot_name.as_ref().map(|t| t.median_ms)));
    println!("- Spotlight, names + contents: {} ms", overall(&|r| r.spot_all.as_ref().map(|t| t.median_ms)));

    println!("\n### First run of each query\n");
    println!("The first execution, before any repetition, which is closer to what a person typing a new query sees.\n");
    println!("| Query | LocalSearch | Spotlight names | Spotlight names+contents |");
    println!("|---|--:|--:|--:|");
    let first = |t: &Option<Timing>| t.as_ref().map_or("–".to_string(), |t| ms(t.first_ms));
    for row in &rows {
        println!("| `{}` | {} | {} | {} |", row.query, ms(row.local.first_ms), first(&row.spot_name), first(&row.spot_all));
    }
    let short_runs: Vec<String> = rows
        .iter()
        .flat_map(|r| [("LocalSearch", Some(&r.local)), ("Spotlight names", r.spot_name.as_ref()), ("Spotlight names+contents", r.spot_all.as_ref())]
            .into_iter()
            .filter_map(move |(who, t)| t.filter(|t| t.runs < MEASURED_RUNS).map(|t| format!("`{}` on {} ({} runs)", r.query, who, t.runs))))
        .collect();
    if !short_runs.is_empty() {
        println!("\nQueries that hit the time budget and were measured fewer than {MEASURED_RUNS} times: {}.", short_runs.join(", "));
    }

    println!("\n## Time until a new file is findable\n");
    println!("A file with a unique name is created in an indexed folder and both engines are polled every 10 ms until they return it.\n");
    println!("- LocalSearch: {}", median(&fresh_local));
    println!("- Spotlight: {}", median(&fresh_spot));

    println!("\n## Searching while the index is being saved\n");
    println!("LocalSearch periodically writes its whole index to disk. One query (`report`) is repeated continuously on another thread before and during a save.\n");
    let summarise = |values: &[f64]| -> String {
        if values.is_empty() {
            return "no queries completed".to_string();
        }
        format!(
            "median {} ms, worst {} ms ({} queries)",
            ms(values[values.len() / 2]),
            ms(values[values.len() - 1]),
            values.len()
        )
    };
    println!("- Before the save: {}", summarise(&before_save));
    println!("- During the save ({:.1} s): {}", save_secs, summarise(&during_save));

    println!("\n## LocalSearch resource cost\n");
    println!("Spotlight's index is built and held by the system for every app, so there is no equivalent per-app figure to set beside these.\n");
    println!("- First index, names only: {:.1} s for {} items (search is usable from here)", names_secs, names_docs);
    println!("- First index, names and contents: {:.1} s in total", build_secs);
    println!("- Share of text, code and PDF files whose contents were indexed: {:.0}%{}", stats.content_indexed_fraction * 100.0,
        if status.budget_exhausted { " (stopped at the 256 MB in-memory index budget)" } else { "" });
    println!("- Process memory right after the first index: {:.0} MB", rss_after_build);
    println!("- Process memory after loading the saved index in a fresh process: {} MB", loaded_rss);
    println!("- Index on disk: {:.0} MB", segment_mb);
    println!("- Loading the saved index on a later launch: {:.2} s for {} items", load_secs, loaded);

    println!("\n## What this does not show\n");
    println!("- One machine, one set of files. Numbers will differ elsewhere.");
    println!("- Spotlight indexes the whole machine and many more file types (Office documents, mail, images); LocalSearch here covers three folders and reads only text, code and PDF contents. Hit counts are therefore not a measure of which is more complete.");
    println!("- Spotlight needs no index build by the user and no extra memory in the app.");
    println!("- The typo queries are included because LocalSearch tolerates misspellings and Spotlight matches literally; zero Spotlight hits there is expected behaviour, not a failure.");
    println!("- The engines do not interpret a query the same way. For several words LocalSearch returns files matching any of them (ranked by how many match); the Spotlight queries here require every word. LocalSearch also returns approximate matches, so its hit counts include loose ones.");
    println!("- Result quality (whether the best file is ranked first) is not measured.");
    Ok(())
}
