//! The live search engine: owns the index, keeps it on disk, keeps it current,
//! and answers queries from memory.
//!
//! Lifecycle of the data directory:
//!
//! ```text
//! config.json        roots / exclusions / limits (written by `reconfigure`)
//! identity.db        (device, inode, birthtime) → stable DocId
//! base_NNNNNN.terms  the on-disk index (see index/base.rs): term dictionary,
//! base_NNNNNN.post   postings and document table. Memory-mapped, immutable.
//! base_NNNNNN.docs
//! manifest.json      which base is current, its checksums, the WAL seq it covers
//! wal.log            filesystem changes applied since that base was written
//! signals.json       click history used for ranking
//! metrics.db         query latency ring buffer
//! ```
//!
//! The bulk of the index lives on disk and is paged in on demand. Memory
//! holds the document table, the name-matching structures, and a small delta:
//! the postings of files added or changed since the base was written. A
//! changed file is marked dead in the base and re-indexed into the delta;
//! a merge (`snapshot`) writes the next base from the two and empties the delta.
//!
//! On start the base is mapped, WAL entries newer than it are re-applied,
//! and a reconcile walk picks up anything that changed while the app was not
//! running. From then on FSEvents drive incremental updates, each logged to
//! the WAL before it is applied, with periodic merges truncating the log.
//!
//! Failure policy: nothing here may take the host app down. Locks are taken
//! poison-tolerantly, the worker restarts itself after a panic, and anything
//! unreadable on disk is rebuilt from the filesystem rather than trusted.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use crossbeam::channel::{Receiver, RecvTimeoutError};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::extract::client::ExtractionClient;
use crate::extract::extractors::{content_kind, ContentKind};
use crate::fs::events::{FsEvent, FsEventWatcher};
use crate::fs::identity::IdentityDb;
use crate::index::bktree::BkTree;
use crate::index::delta::{
    DeltaIndex, DocId, Document, Posting, FIELD_CONTENT, FIELD_FILENAME, FIELD_PATH,
};
use crate::index::base::{self, BaseFiles, BaseIndex, Bitset};
use crate::index::signals::SignalDb;
use crate::index::trie::PathTrie;
use crate::index::trigram::TrigramIndex;
use crate::metrics::collector::{IntegrityChecker, MetricsCollector};
use crate::query::executor::QueryExecutor;
use crate::query::parser::{Query, Tokenizer};
use crate::query::phonetic::double_metaphone;
use crate::query::spotlight_fallback::ResultSource;
use crate::resource::power::{set_io_policy, IoPolicy, PowerMonitor};
use crate::startup::{self, StartupPath};
use crate::wal::entry::{EventType, WalEntry};
use crate::wal::reader::WalReader;
use crate::wal::writer::WalWriter;

const MAX_RESULTS: usize = 100;
/// The in-memory delta is merged into the on-disk base once its postings
/// reach this size (or the configured memory budget, if that is smaller)
const DELTA_MERGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_DOCS: usize = 2_000_000;
/// Documents committed to the index per write-lock acquisition during scans
const COMMIT_BATCH: usize = 512;
/// Quiet period before filesystem events are applied, so files are read after
/// the writer has finished with them
const EVENT_DEBOUNCE: Duration = Duration::from_millis(300);
/// Snapshot at most this often while changes trickle in...
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// ...but never let more than this many changes or this much time go unsaved,
/// even on battery
const SNAPSHOT_MAX_DIRTY: u64 = 5_000;
const SNAPSHOT_MAX_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// Weight of the index (BM25 / fuzzy) score relative to filename matching
const INDEX_SCORE_WEIGHT: f32 = 20.0;
/// Boost per (log-scaled) past open of a result; one click outweighs index-score noise
const CLICK_WEIGHT: f32 = 40.0;
/// Approximate (typo-tolerant) matches are a fallback: they are shown only
/// when fewer than this many results matched the query as typed...
const APPROX_FALLBACK_BELOW: usize = 5;
/// ...and then only the closest ones
const APPROX_MAX: usize = 20;
const APPROX_MIN_SHARE_OF_BEST: f32 = 0.5;
/// Longer queries are cut here; nobody types a kilobyte into a search field
const MAX_QUERY_CHARS: usize = 256;
/// Queries slower than this are logged with a per-stage breakdown
const SLOW_QUERY: Duration = Duration::from_millis(250);
/// FSEvents can drop events under load; a periodic full reconcile bounds how
/// long a missed change can stay missed
const RECONCILE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// How many times the worker restarts after panicking before it gives up
const MAX_WORKER_RESTARTS: u32 = 3;

// ─── Configuration ────────────────────────────────────────────────────────────

fn default_excludes() -> Vec<String> {
    [
        "node_modules", "target", "dist", "build", "tmp", "temp", "cache", "caches",
        "deriveddata", "modulecache", "trash", "__pycache__", "venv", "site-packages", "pods",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn default_max_depth() -> usize {
    12
}

fn default_true() -> bool {
    true
}

fn default_memory_budget_mb() -> usize {
    256
}

/// What gets indexed. Persisted as `config.json` in the data directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Directories to index, recursively
    #[serde(default = "EngineConfig::default_roots")]
    pub roots: Vec<PathBuf>,
    /// File or directory names to skip (case-insensitive, matched per path component)
    #[serde(default = "default_excludes")]
    pub excludes: Vec<String>,
    /// How many levels below a root are indexed
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    /// Index names starting with a dot
    #[serde(default)]
    pub index_hidden: bool,
    /// Extract and index file contents, not just names
    #[serde(default = "default_true")]
    pub index_content: bool,
    /// Ceiling for the in-memory index; content extraction stops when reached
    #[serde(default = "default_memory_budget_mb")]
    pub memory_budget_mb: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            roots: Self::default_roots(),
            excludes: default_excludes(),
            max_depth: default_max_depth(),
            index_hidden: false,
            index_content: true,
            memory_budget_mb: default_memory_budget_mb(),
        }
    }
}

impl EngineConfig {
    pub fn default_roots() -> Vec<PathBuf> {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        ["Downloads", "Documents", "Desktop"]
            .iter()
            .map(|dir| home.join(dir))
            .filter(|path| path.exists())
            .collect()
    }

    /// Read `config.json`, falling back to defaults. `LOCALSEARCH_ROOT`
    /// overrides the configured roots for this process.
    pub fn load(data_dir: &Path) -> Self {
        let mut config: Self = fs::read_to_string(data_dir.join("config.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if let Ok(root) = std::env::var("LOCALSEARCH_ROOT") {
            if !root.is_empty() {
                config.roots = vec![PathBuf::from(root)];
            }
        }
        config.normalized()
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        fs::create_dir_all(data_dir)?;
        write_atomically(&data_dir.join("config.json"), &serde_json::to_vec_pretty(self)?)
    }

    /// Expand `~`, resolve symlinks (FSEvents reports resolved paths), drop
    /// duplicate and nested roots, lower-case exclusions.
    pub fn normalized(mut self) -> Self {
        let home = dirs::home_dir();
        let mut roots: Vec<PathBuf> = Vec::new();
        for root in &self.roots {
            let expanded = match (root.strip_prefix("~"), &home) {
                (Ok(rest), Some(home)) => home.join(rest),
                _ => root.clone(),
            };
            let resolved = fs::canonicalize(&expanded).unwrap_or(expanded);
            if !roots.contains(&resolved) {
                roots.push(resolved);
            }
        }
        let all = roots.clone();
        roots.retain(|root| !all.iter().any(|other| other != root && root.starts_with(other)));
        self.roots = roots;

        self.excludes = self
            .excludes
            .iter()
            .map(|name| name.trim().to_lowercase())
            .filter(|name| !name.is_empty())
            .collect();
        self.max_depth = self.max_depth.clamp(1, 64);
        self.memory_budget_mb = self.memory_budget_mb.max(16);
        self
    }

    fn is_excluded_name(&self, name: &str) -> bool {
        if name.starts_with("~$") || name == ".DS_Store" {
            return true;
        }
        if !self.index_hidden && name.starts_with('.') {
            return true;
        }
        let lower = name.to_lowercase();
        self.excludes.iter().any(|excluded| excluded == &lower)
    }

    /// The root containing `path`, if the path is in scope: below a root,
    /// within the depth limit, and with no excluded component on the way.
    fn root_of(&self, path: &Path) -> Option<&Path> {
        for root in &self.roots {
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let mut depth = 0;
            for component in relative.components() {
                depth += 1;
                let name = component.as_os_str().to_string_lossy();
                if self.is_excluded_name(&name) {
                    return None;
                }
            }
            if depth == 0 || depth > self.max_depth {
                return None;
            }
            // Contents of packages are not indexed, only the package itself
            let inside_package = relative
                .parent()
                .into_iter()
                .flat_map(|parent| parent.components())
                .any(|c| is_package_name(&c.as_os_str().to_string_lossy()));
            if inside_package {
                return None;
            }
            return Some(root);
        }
        None
    }
}

/// Directories macOS presents as a single file; indexed by name, not descended into
fn is_package_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".app", ".photoslibrary", ".framework", ".bundle", ".xcodeproj", ".xcworkspace"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Visit every in-scope entry below `root` (not `root` itself).
fn walk_root(root: &Path, config: &EngineConfig, mut visit: impl FnMut(&Path, &fs::Metadata) -> bool) {
    let mut walker = WalkDir::new(root)
        .follow_links(false)
        .min_depth(1)
        .max_depth(config.max_depth)
        .into_iter();
    while let Some(entry) = walker.next() {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy();
        let is_dir = entry.file_type().is_dir();
        if config.is_excluded_name(&name) {
            if is_dir {
                walker.skip_current_dir();
            }
            continue;
        }
        if is_dir && is_package_name(&name) {
            walker.skip_current_dir();
        }
        let Ok(metadata) = entry.metadata() else { continue };
        if !visit(entry.path(), &metadata) {
            return;
        }
    }
}

// ─── Persistence records ──────────────────────────────────────────────────────

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    /// The current on-disk base: its generation and file checksums.
    /// Absent before the first merge, and in manifests from older formats,
    /// in which case the index is rebuilt from the filesystem.
    #[serde(default)]
    base: Option<BaseFiles>,
    generation: u64,
    /// Every WAL entry with seq <= this is contained in the base
    snapshot_seq: u64,
    doc_count: u64,
    written_at_secs: u64,
}

impl Manifest {
    fn load(data_dir: &Path) -> Self {
        fs::read_to_string(data_dir.join("manifest.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }
}

// ─── Status and results ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    /// Reading the snapshot and WAL
    Loading = 0,
    /// Walking the roots, indexing names
    Scanning = 1,
    /// Extracting and indexing file contents
    Extracting = 2,
    /// Up to date; applying live changes
    Ready = 3,
}

impl Phase {
    fn from_u8(value: u8) -> Self {
        match value {
            0 => Phase::Loading,
            1 => Phase::Scanning,
            2 => Phase::Extracting,
            _ => Phase::Ready,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct IndexStatus {
    pub phase: Phase,
    pub doc_count: u64,
    /// Progress within the current phase (files handled / files queued).
    /// `work_total` is 0 while the amount of work is not yet known.
    pub work_done: u64,
    pub work_total: u64,
    pub elapsed_secs: u64,
    /// Always false: the index is on disk and has no content ceiling. Kept
    /// so the FFI status layout does not change.
    pub budget_exhausted: bool,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub doc_id: u64,
    pub path: String,
    pub score: f32,
    pub source: ResultSource,
}

/// Real numbers behind the health dashboard
#[derive(Debug, Clone)]
pub struct EngineStats {
    pub doc_count: usize,
    /// Fraction of content-indexable files whose content has been processed
    pub content_indexed_fraction: f32,
    pub segment_count: usize,
    /// Size of the on-disk index
    pub index_bytes: u64,
    /// Postings held in memory, waiting for the next merge
    pub delta_bytes: u64,
    /// Documents in the on-disk index that have since changed or gone
    pub dead_in_base: usize,
    pub over_budget: bool,
    /// WAL entries not yet covered by a snapshot
    pub wal_lag_events: usize,
    pub last_snapshot_age_secs: u64,
    pub phantom_rate: f32,
    pub stale_rate: f32,
    pub query_p50_ms: f64,
    pub query_p99_ms: f64,
    pub zero_result_rate: f32,
}

// ─── Index state ──────────────────────────────────────────────────────────────

struct State {
    executor: QueryExecutor,
    /// path → DocId for live documents
    path_ids: HashMap<String, DocId>,
    signals: SignalDb,
}

impl State {
    fn new(delta_index: DeltaIndex) -> Self {
        Self {
            executor: QueryExecutor::new(
                delta_index,
                BkTree::new(),
                PathTrie::new(),
                TrigramIndex::new(),
                HashMap::new(),
            ),
            path_ids: HashMap::new(),
            signals: SignalDb::new(),
        }
    }

    fn docs(&self) -> &HashMap<DocId, Document> {
        &self.executor.delta_index.documents
    }

    /// Register a document's name with the lookup structures that sit beside
    /// the inverted index (fuzzy, phonetic, substring, scope).
    fn register(&mut self, doc_id: DocId, path: &str, name_terms: impl Iterator<Item = String>) {
        for term in name_terms {
            self.executor.bk_tree.insert(&term);
            let code = double_metaphone(&term).primary;
            let terms = self.executor.phonetic_index.entry(code).or_default();
            if !terms.contains(&term) {
                terms.push(term);
            }
        }
        let filename = path.rsplit('/').next().unwrap_or(path);
        self.executor.trigram_index.insert(filename, doc_id);
        self.executor.path_trie.insert(path, doc_id);
        self.path_ids.insert(path.to_string(), doc_id);
    }
}

/// A file or directory that has been stat'ed and tokenized, ready to commit
struct Prepared {
    doc: Document,
    postings: HashMap<String, Posting>,
    wants_content: bool,
}

// ─── Engine ───────────────────────────────────────────────────────────────────

pub struct Engine {
    data_dir: PathBuf,
    config: RwLock<EngineConfig>,
    state: RwLock<State>,
    identity: Mutex<IdentityDb>,
    wal: Mutex<Option<WalWriter>>,
    extractor: Mutex<ExtractionClient>,
    metrics: Mutex<Option<MetricsCollector>>,
    /// Query timings waiting to be written to `metrics`. Queries only push
    /// here; the database write happens off the query path.
    pending_metrics: Mutex<Vec<(Duration, usize)>>,
    last_query: Mutex<String>,
    /// Exclusive lock on the data directory, held while this engine writes to it
    dir_lock: Mutex<Option<fs::File>>,

    /// Seq of the last WAL entry written
    last_seq: AtomicU64,
    generation: AtomicU64,
    /// Changes applied since the last snapshot
    dirty: AtomicU64,
    /// Bumped on every index mutation, so a merge can tell whether the index
    /// changed while it was writing
    mutations: AtomicU64,
    /// One merge at a time
    merge_lock: Mutex<()>,

    phase: AtomicU8,
    work_done: AtomicU64,
    work_total: AtomicU64,
    budget_exhausted: AtomicBool,
    /// True until the index covers the roots; queries may use Spotlight meanwhile
    warming: AtomicBool,
    started: Instant,

    stop: AtomicBool,
    reconfigured: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
    /// Test hook: make the next reconcile panic, to exercise worker recovery
    #[cfg(test)]
    panic_in_next_reconcile: AtomicBool,
}

impl Engine {
    /// The default data directory: `LOCALSEARCH_DATA_DIR` or `~/.localsearch`.
    pub fn default_data_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("LOCALSEARCH_DATA_DIR") {
            if !dir.is_empty() {
                return PathBuf::from(dir);
            }
        }
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".localsearch")
    }

    /// Open an engine over `data_dir`. Nothing is loaded or indexed yet; call
    /// `start` to run it in the background, or drive it step by step with
    /// `load` / `reconcile` / `index_content` / `snapshot`.
    pub fn open(data_dir: &Path, config: EngineConfig) -> Result<Arc<Self>> {
        fs::create_dir_all(data_dir)
            .with_context(|| format!("creating data directory {}", data_dir.display()))?;
        // The index contains the names and words of the user's files:
        // nobody else on the machine gets to read it.
        fs::set_permissions(data_dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting access to {}", data_dir.display()))?;
        let config = config.normalized();
        let identity = IdentityDb::new(&data_dir.join("identity.db"))?;
        let metrics = MetricsCollector::new(data_dir.join("metrics.db"))
            .map_err(|e| log::warn!("query metrics disabled: {e}"))
            .ok();

        let mut state = State::new(DeltaIndex::new(config.memory_budget_mb * 1024 * 1024));
        state.executor.spotlight_fallback.set_enabled(true);
        state.executor.spotlight_fallback.set_roots(config.roots.clone());

        Ok(Arc::new(Self {
            data_dir: data_dir.to_path_buf(),
            config: RwLock::new(config),
            state: RwLock::new(state),
            identity: Mutex::new(identity),
            wal: Mutex::new(None),
            extractor: Mutex::new(ExtractionClient::new()),
            metrics: Mutex::new(metrics),
            pending_metrics: Mutex::new(Vec::new()),
            last_query: Mutex::new(String::new()),
            dir_lock: Mutex::new(None),
            last_seq: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            dirty: AtomicU64::new(0),
            mutations: AtomicU64::new(0),
            merge_lock: Mutex::new(()),
            phase: AtomicU8::new(Phase::Loading as u8),
            work_done: AtomicU64::new(0),
            work_total: AtomicU64::new(0),
            budget_exhausted: AtomicBool::new(false),
            warming: AtomicBool::new(true),
            started: Instant::now(),
            stop: AtomicBool::new(false),
            reconfigured: AtomicBool::new(false),
            worker: Mutex::new(None),
            #[cfg(test)]
            panic_in_next_reconcile: AtomicBool::new(false),
        }))
    }

    pub fn config(&self) -> EngineConfig {
        self.config.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn set_phase(&self, phase: Phase, total: u64) {
        self.work_done.store(0, Ordering::Relaxed);
        self.work_total.store(total, Ordering::Relaxed);
        self.phase.store(phase as u8, Ordering::Relaxed);
    }

    pub fn status(&self) -> IndexStatus {
        IndexStatus {
            phase: Phase::from_u8(self.phase.load(Ordering::Relaxed)),
            doc_count: self.doc_count() as u64,
            work_done: self.work_done.load(Ordering::Relaxed),
            work_total: self.work_total.load(Ordering::Relaxed),
            elapsed_secs: self.started.elapsed().as_secs(),
            budget_exhausted: self.budget_exhausted.load(Ordering::Relaxed),
        }
    }

    pub fn doc_count(&self) -> usize {
        self.state.read().unwrap_or_else(PoisonError::into_inner).executor.delta_index.live_doc_count()
    }

    // ─── Loading ──────────────────────────────────────────────────────────────

    /// Map the current on-disk base, if any, and load its document table.
    /// Read-only on disk. Returns the number of documents loaded.
    pub fn load_snapshot(&self) -> Result<usize> {
        self.load_base(false)
    }

    /// `verify` checks every index file against its checksum first, which
    /// reads the whole index: done after an unclean shutdown.
    fn load_base(&self, verify: bool) -> Result<usize> {
        let manifest = Manifest::load(&self.data_dir);
        self.generation.store(manifest.generation, Ordering::Relaxed);
        self.last_seq.store(manifest.snapshot_seq, Ordering::Relaxed);

        let Some(files) = manifest.base else {
            return Ok(0);
        };
        let (base, documents) = match BaseIndex::open(&self.data_dir, &files, verify) {
            Ok(opened) => opened,
            Err(e) => {
                // A missing or damaged index is not fatal: reconcile rebuilds
                // it from the filesystem.
                log::error!("index generation {} unusable ({e}); rebuilding from disk", files.generation);
                self.last_seq.store(0, Ordering::Relaxed);
                return Ok(0);
            }
        };
        let config = self.config();
        let tokenizer = Tokenizer::new();

        let loaded = documents.len();
        let table: HashMap<DocId, Document> = documents.into_iter().map(|doc| (doc.doc_id, doc)).collect();
        let mut state = State::new(DeltaIndex::from_parts(config.memory_budget_mb * 1024 * 1024, HashMap::new(), table));
        state.executor.base_dead = Bitset::new(base.doc_count());
        state.executor.base = Some(base);
        state.executor.spotlight_fallback.set_enabled(true);
        state.executor.spotlight_fallback.set_roots(config.roots.clone());

        let paths: Vec<(DocId, String)> = state
            .docs()
            .iter()
            .map(|(id, doc)| (*id, doc.path.clone()))
            .collect();
        for (doc_id, path) in &paths {
            let (filename_tokens, dir_tokens) = name_tokens(&tokenizer, Path::new(path), &config);
            state.register(*doc_id, path, filename_tokens.into_iter().chain(dir_tokens));
        }
        state.executor.refresh_stats();
        state.executor.path_trie.rebuild_prefix_cache(20);
        state.signals = fs::read_to_string(self.data_dir.join("signals.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();

        *self.state.write().unwrap_or_else(PoisonError::into_inner) = state;
        if loaded > 0 {
            self.warming.store(false, Ordering::Relaxed);
        }
        Ok(loaded)
    }

    /// Bring the engine up from whatever is on disk: run the startup path for
    /// this launch, load the snapshot, and re-apply WAL entries newer than it.
    /// Returns (documents loaded from snapshot, WAL entries re-applied).
    pub fn load(&self) -> Result<(usize, usize)> {
        self.set_phase(Phase::Loading, 0);
        self.lock_data_dir()?;
        let crashed = match startup::detect_startup_path(&self.data_dir)? {
            StartupPath::FirstLaunch => {
                startup::first_launch(&self.data_dir)?;
                false
            }
            StartupPath::WarmRestart => {
                startup::warm_restart(&self.data_dir)?;
                false
            }
            StartupPath::CrashRecovery => {
                log::warn!("previous session did not shut down cleanly; recovering");
                startup::crash_recovery(&self.data_dir)?;
                true
            }
        };

        // After a crash the index files are checked against their checksums
        let loaded = self.load_base(crashed)?;
        self.remove_superseded_files();

        // Changes logged after the snapshot was taken: re-examine each path.
        let snapshot_seq = self.last_seq.load(Ordering::Relaxed);
        let entries: Vec<WalEntry> = WalReader::new(&self.wal_path())
            .and_then(|mut reader| reader.replay_from(snapshot_seq + 1))
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| entry.event_type != EventType::Checkpoint)
            .collect();
        if let Some(max_seq) = entries.iter().map(|entry| entry.seq).max() {
            self.last_seq.store(max_seq, Ordering::Relaxed);
        }
        let replayed = entries.len();
        if replayed > 0 {
            log::info!("replaying {replayed} WAL entries");
            let paths = entries.into_iter().map(|entry| PathBuf::from(entry.path)).collect();
            self.apply_paths(paths, false);
        }

        *self.wal.lock().unwrap_or_else(PoisonError::into_inner) = Some(self.open_wal()?);
        Ok((loaded, replayed))
    }

    /// Two processes writing one index would corrupt it. The lock is released
    /// by `shutdown`, or by the OS when the process exits.
    fn lock_data_dir(&self) -> Result<()> {
        let mut guard = self.dir_lock.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.is_some() {
            return Ok(());
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(self.data_dir.join("lock"))?;
        const LOCK_EX: i32 = 2;
        const LOCK_NB: i32 = 4;
        if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } != 0 {
            anyhow::bail!(
                "the index in {} is in use by another LocalSearch process",
                self.data_dir.display()
            );
        }
        *guard = Some(file);
        Ok(())
    }

    /// Delete index files the manifest does not point at: older generations,
    /// a generation whose merge never completed, and pre-base segment files.
    fn remove_superseded_files(&self) {
        let current = Manifest::load(&self.data_dir).base.map(|files| files.generation);
        base::remove_other_generations(&self.data_dir, current);
        for entry in fs::read_dir(&self.data_dir).into_iter().flatten().flatten() {
            if entry.path().extension().is_some_and(|ext| ext == "seg") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    fn wal_path(&self) -> PathBuf {
        self.data_dir.join("wal.log")
    }

    fn open_wal(&self) -> Result<WalWriter> {
        WalWriter::new_with_deadline(&self.wal_path(), Duration::from_millis(250))
    }

    // ─── Indexing ─────────────────────────────────────────────────────────────

    /// The DocId of `path` if the index already holds this exact version of it.
    fn unchanged(&self, path: &str, metadata: &fs::Metadata) -> Option<DocId> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        let doc_id = *state.path_ids.get(path)?;
        let doc = state.docs().get(&doc_id)?;
        (doc.mtime_ns == mtime_ns(metadata) && doc.size == metadata.len()).then_some(doc_id)
    }

    /// Stat-derived identity → stable DocId, then tokenize the name.
    fn prepare(
        &self,
        tokenizer: &Tokenizer,
        config: &EngineConfig,
        path: &Path,
        metadata: &fs::Metadata,
    ) -> Option<Prepared> {
        let path_str = path.to_str()?;
        let birth_secs = metadata
            .created()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as u32);
        let doc_id = self
            .identity
            .lock()
            .unwrap()
            .get_or_allocate(metadata.dev() as u128, metadata.ino(), birth_secs, metadata.dev() as u32)
            .map_err(|e| log::warn!("identity allocation failed for {path_str}: {e}"))
            .ok()?;
        let doc_id = DocId(doc_id.0);

        let (filename_tokens, dir_tokens) = name_tokens(tokenizer, path, config);
        let doc_len = (filename_tokens.len() + dir_tokens.len()) as u32;
        let mut postings: HashMap<String, Posting> = HashMap::new();
        for (terms, field) in [(filename_tokens, FIELD_FILENAME), (dir_tokens, FIELD_PATH)] {
            for term in terms {
                let posting = postings
                    .entry(term)
                    .or_insert_with(|| Posting::new(doc_id, 0, 0));
                posting.term_freq += 1;
                posting.field_mask |= field;
            }
        }

        let wants_content = metadata.is_file() && content_kind(path) != ContentKind::Unsupported;
        Some(Prepared {
            doc: Document {
                doc_id,
                path: path_str.to_string(),
                content_hash: 0,
                mtime_ns: mtime_ns(metadata),
                size: metadata.len(),
                doc_len,
                // Nothing to extract counts as done
                content_indexed: !wants_content,
            },
            postings,
            wants_content,
        })
    }

    /// Apply a batch of inserts and removals under one write lock.
    /// A document being re-inserted (modified, or renamed: same inode, new
    /// path) has its old postings purged first.
    fn commit(&self, batch: Vec<Prepared>, mut removed: HashSet<DocId>) {
        if batch.is_empty() && removed.is_empty() {
            return;
        }
        let changes = (batch.len() + removed.len()) as u64;
        let mut guard = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let state = &mut *guard;

        for prepared in &batch {
            if state.docs().contains_key(&prepared.doc.doc_id) {
                removed.insert(prepared.doc.doc_id);
            }
        }
        for doc_id in &removed {
            if let Some(doc) = state.executor.delta_index.documents.get(doc_id) {
                if state.path_ids.get(&doc.path) == Some(doc_id) {
                    state.path_ids.remove(&doc.path);
                }
            }
            state.executor.trigram_index.remove(*doc_id);
            // Its postings in the on-disk base no longer count
            let ordinal = state.executor.base.as_ref().and_then(|base| base.ordinal_of(*doc_id));
            if let Some(ordinal) = ordinal {
                state.executor.base_dead.set(ordinal);
            }
        }
        // Only the delta is scanned here; the base is never rewritten in place
        state.executor.delta_index.purge(&removed);
        self.mutations.fetch_add(1, Ordering::Relaxed);

        for prepared in batch {
            if state.docs().len() >= MAX_DOCS {
                break;
            }
            let Prepared { doc, postings, .. } = prepared;
            // The same file reached by a second path (hard link): first path wins
            if state.docs().contains_key(&doc.doc_id) {
                continue;
            }
            let (doc_id, path) = (doc.doc_id, doc.path.clone());
            let name_terms: Vec<String> = postings.keys().cloned().collect();
            let _ = state.executor.delta_index.insert_document(doc, postings);
            state.register(doc_id, &path, name_terms.into_iter());
        }
        state.executor.refresh_stats();
        self.dirty.fetch_add(changes, Ordering::Relaxed);
    }

    /// Walk the configured roots and make the index match the filesystem:
    /// add what is new, re-index what changed, drop what is gone or no longer
    /// in scope. Unchanged files cost one stat.
    pub fn reconcile(&self) {
        let config = self.config();
        let tokenizer = Tokenizer::new();
        self.set_phase(Phase::Scanning, 0);
        #[cfg(test)]
        if self.panic_in_next_reconcile.swap(false, Ordering::Relaxed) {
            panic!("injected reconcile failure");
        }

        let mut seen: HashSet<DocId> = HashSet::new();
        let mut batch: Vec<Prepared> = Vec::new();
        let mut aborted = false;

        // A root that cannot be read right now (unmounted volume, permission
        // revoked) is not evidence that its files were deleted.
        let unavailable: Vec<String> = config
            .roots
            .iter()
            .filter(|root| fs::read_dir(root).is_err())
            .map(|root| format!("{}/", root.to_string_lossy()))
            .collect();
        for root in &unavailable {
            log::warn!("{root} is not readable; keeping its indexed entries");
        }

        let _ = self.identity.lock().unwrap_or_else(PoisonError::into_inner).begin_batch();
        for root in &config.roots {
            walk_root(root, &config, |path, metadata| {
                if self.stop.load(Ordering::Relaxed) || self.reconfigured.load(Ordering::Relaxed) {
                    aborted = true;
                    return false;
                }
                self.work_done.fetch_add(1, Ordering::Relaxed);
                let Some(path_str) = path.to_str() else { return true };

                if let Some(doc_id) = self.unchanged(path_str, metadata) {
                    seen.insert(doc_id);
                } else if let Some(prepared) = self.prepare(&tokenizer, &config, path, metadata) {
                    seen.insert(prepared.doc.doc_id);
                    batch.push(prepared);
                }
                if batch.len() >= COMMIT_BATCH {
                    self.commit(std::mem::take(&mut batch), HashSet::new());
                    self.merge_if_delta_is_large();
                    let mut identity = self.identity.lock().unwrap_or_else(PoisonError::into_inner);
                    let _ = identity.commit_batch();
                    let _ = identity.begin_batch();
                }
                true
            });
            if aborted {
                break;
            }
        }
        self.commit(batch, HashSet::new());
        let _ = self.identity.lock().unwrap_or_else(PoisonError::into_inner).commit_batch();

        // Only a complete walk tells us what is gone.
        if !aborted {
            let stale: HashSet<DocId> = {
                let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
                state
                    .docs()
                    .values()
                    .filter(|doc| !seen.contains(&doc.doc_id))
                    .filter(|doc| !unavailable.iter().any(|root| doc.path.starts_with(root)))
                    .map(|doc| doc.doc_id)
                    .collect()
            };
            if !stale.is_empty() {
                log::info!("reconcile: removing {} documents no longer on disk or in scope", stale.len());
            }
            self.commit(Vec::new(), stale);
            self.state.write().unwrap_or_else(PoisonError::into_inner).executor.path_trie.rebuild_prefix_cache(20);
            self.warming.store(false, Ordering::Relaxed);
        }
    }

    /// Extract and index the content of one document.
    fn index_one_content(&self, tokenizer: &Tokenizer, doc_id: DocId, path: &str) -> bool {
        let extracted = self.extractor.lock().unwrap_or_else(PoisonError::into_inner).extract(path);
        let (postings, token_count, content_hash) = match extracted {
            Ok(result) if !result.text.is_empty() => {
                let tokens = tokenizer.tokenize(&result.text);
                let token_count = tokens.len() as u32;
                let mut postings: HashMap<String, Posting> = HashMap::new();
                for token in tokens {
                    postings
                        .entry(token.term)
                        .or_insert_with(|| Posting::new(doc_id, 0, FIELD_CONTENT))
                        .term_freq += 1;
                }
                (postings, token_count, result.content_hash)
            }
            _ => (HashMap::new(), 0, 0),
        };

        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        // The file may have been removed or replaced while we were reading it
        let still_current = state.docs().get(&doc_id).is_some_and(|doc| doc.path == path && !doc.content_indexed);
        if still_current {
            if postings.is_empty() {
                state.executor.delta_index.mark_content_indexed(doc_id);
            } else {
                state.executor.delta_index.append_postings(doc_id, postings, token_count, content_hash);
            }
            self.mutations.fetch_add(1, Ordering::Relaxed);
            self.dirty.fetch_add(1, Ordering::Relaxed);
        }
        true
    }

    /// Fold the delta into the on-disk base if it has grown past its limit.
    /// This is what keeps memory flat however much there is to index.
    fn merge_if_delta_is_large(&self) {
        let (posting_bytes, limit) = {
            let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
            let delta = &state.executor.delta_index;
            (delta.posting_bytes(), DELTA_MERGE_BYTES.min(delta.memory_budget()))
        };
        // Not ours to write: a read-only engine keeps whatever it has in memory
        let owner = self.dir_lock.lock().unwrap_or_else(PoisonError::into_inner).is_some();
        if posting_bytes >= limit && owner {
            if let Err(e) = self.snapshot() {
                log::error!("merge failed: {e:#}");
            }
        }
    }

    /// Extract content for every document still waiting for it.
    /// `pump` is called between files so live changes keep flowing.
    pub fn index_content(&self, mut pump: impl FnMut()) {
        if !self.config().index_content {
            return;
        }
        let queue: Vec<(DocId, String)> = {
            let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
            state
                .docs()
                .values()
                .filter(|doc| !doc.content_indexed)
                .map(|doc| (doc.doc_id, doc.path.clone()))
                .collect()
        };
        if queue.is_empty() {
            return;
        }
        self.set_phase(Phase::Extracting, queue.len() as u64);

        let tokenizer = Tokenizer::new();
        for (index, (doc_id, path)) in queue.iter().enumerate() {
            if self.stop.load(Ordering::Relaxed) || self.reconfigured.load(Ordering::Relaxed) {
                break;
            }
            if !self.index_one_content(&tokenizer, *doc_id, path) {
                break;
            }
            self.work_done.fetch_add(1, Ordering::Relaxed);
            if index % 16 == 15 {
                self.merge_if_delta_is_large();
            }
            if index % 64 == 63 {
                self.state.write().unwrap_or_else(PoisonError::into_inner).executor.refresh_stats();
                pump();
            }
        }
        self.state.write().unwrap_or_else(PoisonError::into_inner).executor.refresh_stats();
    }

    // ─── Live changes ─────────────────────────────────────────────────────────

    /// Re-examine paths reported by the filesystem watcher (or the WAL) and
    /// update the index to match what is on disk now.
    pub fn apply_changes(&self, paths: Vec<PathBuf>) {
        self.apply_paths(paths, true);
    }

    fn apply_paths(&self, paths: Vec<PathBuf>, log_to_wal: bool) {
        let config = self.config();
        let tokenizer = Tokenizer::new();
        let paths: BTreeSet<PathBuf> = paths.into_iter().collect();

        let mut removed: HashSet<DocId> = HashSet::new();
        let mut batch: Vec<Prepared> = Vec::new();

        for path in paths {
            if config.root_of(&path).is_none() {
                continue;
            }
            let Some(path_str) = path.to_str() else { continue };
            let metadata = fs::symlink_metadata(&path).ok();

            if log_to_wal {
                self.log_change(path_str, metadata.as_ref());
            }

            match metadata {
                None => {
                    // Gone: the path itself and, if it was a directory, everything under it
                    let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
                    if let Some(doc_id) = state.path_ids.get(path_str) {
                        removed.insert(*doc_id);
                    }
                    let prefix = format!("{path_str}/");
                    for id in state.executor.path_trie.scope_query(&path) {
                        let doc_id = DocId(id as u64);
                        if state.docs().get(&doc_id).is_some_and(|doc| doc.path.starts_with(&prefix)) {
                            removed.insert(doc_id);
                        }
                    }
                }
                Some(metadata) => {
                    if self.unchanged(path_str, &metadata).is_some() {
                        continue;
                    }
                    let is_new = !self.state.read().unwrap_or_else(PoisonError::into_inner).path_ids.contains_key(path_str);
                    batch.extend(self.prepare(&tokenizer, &config, &path, &metadata));

                    // A directory that appears whole (moved or copied in) may
                    // not produce an event per child.
                    if metadata.is_dir() && is_new && !is_package_name(path_str) {
                        let depth = path.strip_prefix(config.root_of(&path).unwrap()).map_or(0, |r| r.components().count());
                        let mut sub = config.clone();
                        sub.max_depth = config.max_depth.saturating_sub(depth).max(1);
                        if depth < config.max_depth {
                            walk_root(&path, &sub, |child, child_meta| {
                                let known = child.to_str().is_some_and(|s| self.unchanged(s, child_meta).is_some());
                                if !known {
                                    batch.extend(self.prepare(&tokenizer, &config, child, child_meta));
                                }
                                true
                            });
                        }
                    }
                }
            }
        }

        let content: Vec<(DocId, String)> = batch
            .iter()
            .filter(|prepared| prepared.wants_content)
            .map(|prepared| (prepared.doc.doc_id, prepared.doc.path.clone()))
            .collect();
        // Removals first so a rename (old path gone, same inode at new path)
        // ends with the document present under its new path.
        self.commit(batch, removed);

        if config.index_content {
            for (doc_id, path) in content {
                if !self.index_one_content(&tokenizer, doc_id, &path) {
                    break;
                }
            }
            self.state.write().unwrap_or_else(PoisonError::into_inner).executor.refresh_stats();
        }
        // A burst (a branch switch, an unpacked archive) can fill the delta
        if log_to_wal {
            self.merge_if_delta_is_large();
        }
    }

    fn log_change(&self, path: &str, metadata: Option<&fs::Metadata>) {
        let guard = self.wal.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(wal) = guard.as_ref() else { return };
        if path.len() > u16::MAX as usize {
            return;
        }
        let entry = WalEntry {
            seq: self.last_seq.fetch_add(1, Ordering::Relaxed) + 1,
            timestamp_us: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_micros() as u64),
            event_type: if metadata.is_some() { EventType::Modified } else { EventType::Deleted },
            flags: 0,
            doc_id: 0,
            inode: metadata.map_or(0, |m| m.ino()),
            volume_uuid: metadata.map_or(0, |m| m.dev() as u64),
            mtime_ns: metadata.map_or(0, mtime_ns),
            size: metadata.map_or(0, |m| m.len()),
            mode: metadata.map_or(0, |m| m.mode() as u16),
            uid: metadata.map_or(0, |m| m.uid()),
            gid: metadata.map_or(0, |m| m.gid()),
            path: path.to_string(),
        };
        if let Err(e) = wal.append(entry) {
            log::error!("WAL append failed: {e}");
        }
    }

    // ─── Snapshots ────────────────────────────────────────────────────────────

    /// Merge: write the next on-disk base from the current base (minus its
    /// dead documents) and the delta, switch to it, empty the delta, and
    /// truncate the WAL it supersedes.
    ///
    /// Queries keep running while the files are written; they are excluded
    /// only for the switch itself.
    pub fn snapshot(&self) -> Result<()> {
        if self.dir_lock.lock().unwrap_or_else(PoisonError::into_inner).is_none() {
            anyhow::bail!("this engine does not own {} and will not write to it", self.data_dir.display());
        }
        let _one_at_a_time = self.merge_lock.lock().unwrap_or_else(PoisonError::into_inner);

        // Everything logged so far is applied: logging and applying happen
        // together, and the WAL is flushed before the index is read.
        if let Some(wal) = self.wal.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            wal.flush()?;
        }
        let snapshot_seq = self.last_seq.load(Ordering::Relaxed);
        let generation = self.generation.load(Ordering::Relaxed) + 1;

        let write = |state: &State| -> Result<BaseFiles> {
            let executor = &state.executor;
            let old = executor.base.as_ref().map(|base| (base, &executor.base_dead));
            Ok(base::merge(&self.data_dir, generation, old, &executor.delta_index.index, &executor.delta_index.documents)?)
        };

        // Write under a read lock, so searches are not held up...
        let (mut files, seen) = {
            let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
            (write(&state)?, self.mutations.load(Ordering::Relaxed))
        };

        // ...then switch under the write lock.
        let (doc_count, signals) = {
            let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
            if self.mutations.load(Ordering::Relaxed) != seen {
                // The index changed while the files were being written. Write
                // them again with writers excluded; this is rare and brief.
                files = write(&state)?;
            }
            let (new_base, _) = BaseIndex::open(&self.data_dir, &files, false)?;
            state.executor.base_dead = Bitset::new(new_base.doc_count());
            state.executor.base = Some(new_base);
            state.executor.delta_index.clear_postings();
            (state.executor.delta_index.live_doc_count() as u64, serde_json::to_vec(&state.signals)?)
        };

        // The files only count once the manifest names them
        let manifest = Manifest {
            base: Some(files),
            generation,
            snapshot_seq,
            doc_count,
            written_at_secs: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        };
        write_atomically(&self.data_dir.join("manifest.json"), &serde_json::to_vec_pretty(&manifest)?)?;
        self.generation.store(generation, Ordering::Relaxed);

        // Older generations and the WAL entries this one covers are no longer needed
        self.remove_superseded_files();
        {
            let mut wal = self.wal.lock().unwrap_or_else(PoisonError::into_inner);
            if wal.is_some() {
                *wal = None;
                fs::File::create(self.wal_path())?;
                *wal = Some(self.open_wal()?);
            }
        }
        write_atomically(&self.data_dir.join("signals.json"), &signals)?;

        self.dirty.store(0, Ordering::Relaxed);
        log::info!("merged into base generation {generation}: {doc_count} documents");
        Ok(())
    }

    // ─── Background worker ────────────────────────────────────────────────────

    /// Run the engine in the background: load, reconcile, extract content,
    /// then follow filesystem events until `shutdown`.
    pub fn start(self: &Arc<Self>) {
        let mut worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        if worker.is_some() {
            return;
        }
        let engine = Arc::clone(self);
        *worker = std::thread::Builder::new()
            .name("localsearch-indexer".to_string())
            .spawn(move || engine.run_supervised())
            .map_err(|e| log::error!("could not start indexer thread: {e}"))
            .ok();
    }

    /// Run the worker; if it panics, log it and start over from what is on
    /// disk. A bug in indexing must degrade to "index is a little stale",
    /// never to a crashed app.
    fn run_supervised(&self) {
        let mut restarts = 0;
        loop {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run()));
            if outcome.is_ok() || self.stop.load(Ordering::Relaxed) {
                return;
            }
            restarts += 1;
            if restarts > MAX_WORKER_RESTARTS {
                log::error!("indexer panicked {restarts} times; live updates are off until restart");
                // Whatever is in memory stays searchable
                self.warming.store(false, Ordering::Relaxed);
                self.set_phase(Phase::Ready, 0);
                return;
            }
            log::error!("indexer panicked; restarting ({restarts}/{MAX_WORKER_RESTARTS})");
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    fn run(&self) {
        // Indexing must never compete with the user's foreground I/O
        set_io_policy(IoPolicy::Throttle);

        // Another process owns this index: serve what it last saved, read-only.
        // Indexing here as well would have two writers on one set of files.
        if let Err(e) = self.lock_data_dir() {
            log::error!("{e:#}; this process will search the saved index without updating it");
            if let Err(e) = self.load_snapshot() {
                log::error!("could not load the saved index: {e:#}");
            }
            self.warming.store(false, Ordering::Relaxed);
            self.set_phase(Phase::Ready, 0);
            return;
        }

        match self.load() {
            Ok((loaded, replayed)) => log::info!("loaded {loaded} documents, replayed {replayed} WAL entries"),
            Err(e) => log::error!("load failed, starting from an empty index: {e:#}"),
        }

        while !self.stop.load(Ordering::Relaxed) {
            self.reconfigured.store(false, Ordering::Relaxed);
            let config = self.config();

            // Watch before walking so nothing that changes mid-walk is missed
            let (sender, events) = crossbeam::channel::unbounded::<FsEvent>();
            let _watchers: Vec<FsEventWatcher> = config
                .roots
                .iter()
                .filter(|root| root.is_dir())
                .filter_map(|root| {
                    FsEventWatcher::new(root, sender.clone())
                        .map_err(|e| log::error!("cannot watch {}: {e}", root.display()))
                        .ok()
                })
                .collect();

            self.reconcile();
            self.save_if_dirty();
            self.index_content(|| self.pump(&events));
            self.save_if_dirty();

            self.set_phase(Phase::Ready, 0);
            self.follow(&events);
        }
    }

    fn save_if_dirty(&self) {
        let never_saved = self.generation.load(Ordering::Relaxed) == 0;
        if self.dirty.load(Ordering::Relaxed) > 0 || never_saved {
            if let Err(e) = self.snapshot() {
                log::error!("snapshot failed: {e:#}");
            }
        }
    }

    /// Apply whatever events are queued right now, without waiting.
    fn pump(&self, events: &Receiver<FsEvent>) {
        let paths: Vec<PathBuf> = events.try_iter().map(|event| event.path).collect();
        if !paths.is_empty() {
            self.apply_paths(paths, true);
        }
    }

    /// Steady state: apply filesystem events as they settle, snapshot now and then.
    fn follow(&self, events: &Receiver<FsEvent>) {
        let mut pending: Vec<PathBuf> = Vec::new();
        let mut oldest_pending = Instant::now();
        let mut last_snapshot = Instant::now();
        let entered = Instant::now();

        while !self.stop.load(Ordering::Relaxed) && !self.reconfigured.load(Ordering::Relaxed) {
            // Returning sends the worker round its loop: re-watch, reconcile
            if entered.elapsed() >= RECONCILE_INTERVAL {
                return;
            }
            self.flush_metrics();
            match events.recv_timeout(EVENT_DEBOUNCE) {
                Ok(event) => {
                    if pending.is_empty() {
                        oldest_pending = Instant::now();
                    }
                    pending.push(event.path);
                    // Under a steady stream of events, don't wait for quiet forever
                    if oldest_pending.elapsed() < Duration::from_secs(2) {
                        continue;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }

            if !pending.is_empty() {
                self.apply_paths(std::mem::take(&mut pending), true);
            }

            let dirty = self.dirty.load(Ordering::Relaxed);
            let since = last_snapshot.elapsed();
            let due = dirty > 0
                && ((since >= SNAPSHOT_INTERVAL && PowerMonitor::current().should_compact())
                    || since >= SNAPSHOT_MAX_INTERVAL
                    || dirty >= SNAPSHOT_MAX_DIRTY);
            if due {
                self.save_if_dirty();
                self.state.write().unwrap_or_else(PoisonError::into_inner).executor.path_trie.rebuild_prefix_cache(20);
                last_snapshot = Instant::now();
            }
        }
    }

    /// Write queued query timings to the metrics database. Called from the
    /// worker and before reading stats, never from a query.
    fn flush_metrics(&self) {
        let pending = std::mem::take(&mut *self.pending_metrics.lock().unwrap_or_else(PoisonError::into_inner));
        if pending.is_empty() {
            return;
        }
        let metrics = self.metrics.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(collector) = metrics.as_ref() {
            for (latency, result_count) in pending {
                let _ = collector.record_query(latency, result_count);
            }
            let _ = collector.cleanup_old_entries();
            let _ = collector.enforce_size_cap();
        }
    }

    /// Replace the configuration for this session (persisting it is the
    /// caller's choice, see `EngineConfig::save`). The worker re-reconciles
    /// against the new roots and exclusions; documents that fell out of scope
    /// are removed.
    pub fn reconfigure(&self, config: EngineConfig) -> Result<()> {
        let config = config.normalized();
        {
            let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
            state.executor.spotlight_fallback.set_roots(config.roots.clone());
            state.executor.delta_index.set_memory_budget(config.memory_budget_mb * 1024 * 1024);
        }
        let changed = {
            let mut current = self.config.write().unwrap_or_else(PoisonError::into_inner);
            let changed = *current != config;
            *current = config;
            changed
        };
        if changed {
            self.reconfigured.store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Stop the worker, save the index, and mark the shutdown as clean.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.lock().unwrap_or_else(PoisonError::into_inner).take() {
            let _ = worker.join();
        }
        self.save_if_dirty();
        self.flush_metrics();
        *self.wal.lock().unwrap_or_else(PoisonError::into_inner) = None;
        // Only an engine that owned the directory may vouch for its state
        let owned = self.dir_lock.lock().unwrap_or_else(PoisonError::into_inner).take().is_some();
        if owned {
            if let Err(e) = fs::write(self.data_dir.join(".clean_shutdown"), b"") {
                log::error!("could not write clean-shutdown marker: {e}");
            }
        }
    }

    // ─── Queries ──────────────────────────────────────────────────────────────

    /// Answer a query from the in-memory index. Never touches the filesystem
    /// (Spotlight is consulted only while the index is still being built).
    pub fn search(&self, query: &str) -> Vec<SearchHit> {
        let started = Instant::now();
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let query: String = query.chars().take(MAX_QUERY_CHARS).collect();
        let query = query.as_str();
        *self.last_query.lock().unwrap_or_else(PoisonError::into_inner) = query.to_string();

        let query_lc = query.to_lowercase();
        let query_tokens = tokenize_query(query);
        // "report.pdf" or "src/main.rs" name a file; fuzzy and content matches
        // on the fragments would only add noise.
        let literal = query_lc.contains('.') || query_lc.contains('/');

        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        let lock_wait = started.elapsed();

        /// A candidate's score and whether it matched the query as typed
        struct Candidate {
            score: f32,
            exact: bool,
            source: ResultSource,
        }
        let mut candidates: HashMap<DocId, Candidate> = HashMap::new();

        if !literal {
            if let Ok(parsed) = Query::parse(query) {
                for result in state.executor.execute(parsed, None).unwrap_or_default() {
                    candidates.insert(
                        result.doc_id,
                        Candidate {
                            score: result.score * INDEX_SCORE_WEIGHT,
                            exact: result.exact,
                            source: result.source,
                        },
                    );
                }
            }
        }
        let index_done = started.elapsed();

        // Filename matching over every document, in memory and in parallel.
        let now_ns = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        let lexical: Vec<(DocId, f32)> = state
            .docs()
            .par_iter()
            .filter_map(|(doc_id, doc)| {
                let haystack = if literal { doc.path.as_str() } else { doc.path.rsplit('/').next().unwrap_or("") };
                let candidate = contains_ignore_ascii_case(haystack, &query_lc)
                    || query_tokens.iter().any(|token| contains_ignore_ascii_case(haystack, token));
                if !candidate {
                    return None;
                }
                let score = lexical_score(&doc.path, &query_lc, &query_tokens);
                (score > 0.0).then(|| (*doc_id, score + recency_boost(doc.mtime_ns, now_ns)))
            })
            .collect();
        for (doc_id, score) in lexical {
            let candidate = candidates
                .entry(doc_id)
                .or_insert(Candidate { score: 0.0, exact: true, source: ResultSource::LocalIndex });
            candidate.score += score;
            // The query text appears in the name: that is a match as typed
            candidate.exact = true;
        }
        let scan_done = started.elapsed();

        let to_hit = |doc_id: &DocId, candidate: &Candidate| -> Option<SearchHit> {
            let doc = state.docs().get(doc_id)?;
            // Files opened from results before rank higher next time
            let clicks = state.signals.hot_paths.get(&doc.path).copied().unwrap_or(0);
            Some(SearchHit {
                doc_id: doc_id.0,
                path: doc.path.clone(),
                score: candidate.score + CLICK_WEIGHT * (1.0 + clicks as f32).ln(),
                source: candidate.source,
            })
        };
        let by_score = |a: &SearchHit, b: &SearchHit| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
        };

        let mut hits: Vec<SearchHit> = candidates
            .iter()
            .filter(|(_, candidate)| candidate.exact)
            .filter_map(|(doc_id, candidate)| to_hit(doc_id, candidate))
            .collect();

        // Approximate matches only stand in when the query as typed found
        // little, and then only the ones close to the best of them.
        if hits.len() < APPROX_FALLBACK_BELOW {
            let mut approximate: Vec<SearchHit> = candidates
                .iter()
                .filter(|(_, candidate)| !candidate.exact)
                .filter_map(|(doc_id, candidate)| to_hit(doc_id, candidate))
                .collect();
            approximate.sort_by(by_score);
            if let Some(best) = approximate.first().map(|hit| hit.score) {
                approximate.retain(|hit| hit.score >= best * APPROX_MIN_SHARE_OF_BEST);
                approximate.truncate(APPROX_MAX);
                hits.extend(approximate);
            }
        }

        if self.warming.load(Ordering::Relaxed) && hits.len() < 5 {
            let known: HashSet<String> = hits.iter().map(|hit| hit.path.clone()).collect();
            for result in state.executor.spotlight_fallback.query(query) {
                if !known.contains(&result.path) {
                    hits.push(SearchHit {
                        doc_id: result.doc_id.0,
                        score: result.score * INDEX_SCORE_WEIGHT,
                        path: result.path,
                        source: result.source,
                    });
                }
            }
        }
        drop(state);

        hits.sort_by(by_score);
        hits.truncate(MAX_RESULTS);

        // Timings are queued, not written: no disk I/O on the query path
        let elapsed = started.elapsed();
        self.pending_metrics
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((elapsed, hits.len()));
        if elapsed >= SLOW_QUERY {
            log::warn!(
                "slow query ({} chars): {:?} total; lock wait {:?}, index {:?}, name scan {:?}, assemble {:?}",
                query.chars().count(),
                elapsed,
                lock_wait,
                index_done - lock_wait,
                scan_done - index_done,
                elapsed - scan_done,
            );
        }
        hits
    }

    /// The passage of `path`'s content that matches `query`, for showing under
    /// a result: one line of about `SNIPPET_CHARS` characters with each match
    /// wrapped in `SNIPPET_MARK_START` / `SNIPPET_MARK_END`.
    ///
    /// Returns None when the file is not in the index, has no extractable
    /// content, or matched on its name alone. Reads the file, so call it off
    /// the query path and only for results that are on screen.
    pub fn snippet(&self, path: &str, query: &str) -> Option<String> {
        if content_kind(Path::new(path)) == ContentKind::Unsupported {
            return None;
        }
        // Only files the index knows about; this is not a general file reader
        if !self.state.read().unwrap_or_else(PoisonError::into_inner).path_ids.contains_key(path) {
            return None;
        }
        let tokenizer = Tokenizer::new();
        let words: Vec<String> = query
            .split_whitespace()
            .filter(|part| !part.contains(':') && !part.starts_with('-'))
            .flat_map(tokenize_query)
            .collect();
        let stems: HashSet<String> = tokenizer
            .tokenize(&words.join(" "))
            .into_iter()
            .map(|token| token.term)
            .collect();
        if words.is_empty() {
            return None;
        }

        let text = self.extractor.lock().unwrap_or_else(PoisonError::into_inner).extract(path).ok()?.text;
        build_snippet(&text, &words, &stems, &tokenizer)
    }

    /// The user opened `path` from the results of the most recent query.
    pub fn record_click(&self, path: &str) {
        let query = self.last_query.lock().unwrap_or_else(PoisonError::into_inner).to_lowercase();
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.executor.ranker.record_click(PathBuf::from(path));
        state.signals.record_click(&query, path);
        self.dirty.fetch_add(1, Ordering::Relaxed);
    }

    // ─── Health ───────────────────────────────────────────────────────────────

    pub fn stats(&self) -> EngineStats {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        let delta = &state.executor.delta_index;

        let (mut eligible, mut processed) = (0usize, 0usize);
        let mut checker = IntegrityChecker::new();
        for (index, doc) in state.docs().values().enumerate() {
            if content_kind(Path::new(&doc.path)) != ContentKind::Unsupported {
                eligible += 1;
                processed += doc.content_indexed as usize;
            }
            if index < 1000 {
                checker.add_document(doc.doc_id.0, doc.path.clone(), doc.mtime_ns / 1_000_000_000);
            }
        }
        let integrity = checker.check().ok();

        let snapshot_seq = Manifest::load(&self.data_dir).snapshot_seq;
        let wal_lag_events = WalReader::new(&self.wal_path())
            .and_then(|mut reader| reader.replay_from(snapshot_seq + 1))
            .map_or(0, |entries| {
                entries.iter().filter(|entry| entry.event_type != EventType::Checkpoint).count()
            });

        let segments: Vec<PathBuf> = fs::read_dir(&self.data_dir)
            .map(|dir| {
                dir.flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "terms"))
                    .collect()
            })
            .unwrap_or_default();
        let last_snapshot_age_secs = segments
            .iter()
            .filter_map(|path| fs::metadata(path).ok()?.modified().ok()?.elapsed().ok())
            .map(|age| age.as_secs())
            .min()
            .unwrap_or(0);

        self.flush_metrics();
        let metrics = self.metrics.lock().unwrap_or_else(PoisonError::into_inner);
        let (query_p50_ms, query_p99_ms) = metrics
            .as_ref()
            .and_then(|collector| collector.get_latency_stats().ok())
            .unwrap_or((0.0, 0.0));
        let zero_result_rate = metrics
            .as_ref()
            .and_then(|collector| collector.zero_result_rate().ok())
            .unwrap_or(0.0);

        EngineStats {
            doc_count: delta.live_doc_count(),
            content_indexed_fraction: if eligible == 0 { 1.0 } else { processed as f32 / eligible as f32 },
            segment_count: segments.len(),
            index_bytes: state.executor.base.as_ref().map_or(0, |base| base.files().bytes),
            delta_bytes: delta.posting_bytes() as u64,
            dead_in_base: state.executor.base_dead.count(),
            over_budget: false,
            wal_lag_events,
            last_snapshot_age_secs,
            phantom_rate: integrity.as_ref().map_or(0.0, |report| report.phantom_rate),
            stale_rate: integrity.as_ref().map_or(0.0, |report| report.stale_rate),
            query_p50_ms,
            query_p99_ms,
            zero_result_rate,
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

extern "C" {
    fn flock(fd: std::os::raw::c_int, operation: std::os::raw::c_int) -> std::os::raw::c_int;
}

/// Write a file so that a crash leaves either the old or the new version,
/// never a torn one: temp file, fsync, rename.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn mtime_ns(metadata: &fs::Metadata) -> u64 {
    (metadata.mtime().max(0) as u64) * 1_000_000_000 + metadata.mtime_nsec().max(0) as u64
}

/// Index terms for a path: (filename terms, terms of the directories between
/// the root and the file). Directories above the root are shared by every
/// document and carry no signal, so they are not indexed.
fn name_tokens(tokenizer: &Tokenizer, path: &Path, config: &EngineConfig) -> (Vec<String>, Vec<String>) {
    let filename = path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
    let filename_terms = tokenizer.tokenize(&filename).into_iter().map(|t| t.term).collect();

    let dir_terms = config
        .roots
        .iter()
        .find_map(|root| path.strip_prefix(root).ok())
        .and_then(|relative| relative.parent())
        .map(|parent| tokenizer.tokenize(&parent.to_string_lossy()).into_iter().map(|t| t.term).collect())
        .unwrap_or_default();

    (filename_terms, dir_terms)
}

pub const SNIPPET_MARK_START: char = '\u{1}';
pub const SNIPPET_MARK_END: char = '\u{2}';
const SNIPPET_CHARS: usize = 140;
/// Characters of context kept before the first match
const SNIPPET_LEAD_CHARS: usize = 40;

/// Alphanumeric runs of `text` with their byte ranges
fn word_spans(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut chars = text.char_indices().peekable();
    std::iter::from_fn(move || {
        while let Some(&(start, ch)) = chars.peek() {
            if !ch.is_alphanumeric() {
                chars.next();
                continue;
            }
            let mut end = start;
            while let Some(&(index, ch)) = chars.peek() {
                if !ch.is_alphanumeric() {
                    break;
                }
                end = index + ch.len_utf8();
                chars.next();
            }
            return Some((start, end));
        }
        None
    })
}

fn build_snippet(
    text: &str,
    words: &[String],
    stems: &HashSet<String>,
    tokenizer: &Tokenizer,
) -> Option<String> {
    // A word matches if it is a query word, starts with one, or shares its stem
    let is_match = |word: &str| {
        let lower = word.to_lowercase();
        words.iter().any(|w| lower == *w || (w.chars().count() >= 3 && lower.starts_with(w.as_str())))
            || tokenizer.tokenize(&lower).iter().any(|token| stems.contains(&token.term))
    };

    let (first_start, _) = word_spans(text).find(|&(start, end)| is_match(&text[start..end]))?;

    // The window: the matching line, starting a little before the match
    let line_start = text[..first_start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[first_start..].find('\n').map_or(text.len(), |i| first_start + i);
    let mut window_start = text[line_start..first_start]
        .char_indices()
        .rev()
        .nth(SNIPPET_LEAD_CHARS)
        .map_or(line_start, |(i, _)| line_start + i);
    // Don't begin mid-word
    if window_start > line_start {
        window_start = text[window_start..first_start]
            .find(char::is_whitespace)
            .map_or(window_start, |i| window_start + i + 1);
    }
    let window_end = text[window_start..line_end]
        .char_indices()
        .nth(SNIPPET_CHARS)
        .map_or(line_end, |(i, _)| window_start + i);
    let window = &text[window_start..window_end];

    let mut snippet = String::with_capacity(window.len() + 16);
    if window_start > line_start {
        snippet.push('…');
    }
    let mut cursor = 0;
    for (start, end) in word_spans(window) {
        if is_match(&window[start..end]) {
            snippet.push_str(&window[cursor..start]);
            snippet.push(SNIPPET_MARK_START);
            snippet.push_str(&window[start..end]);
            snippet.push(SNIPPET_MARK_END);
            cursor = end;
        }
    }
    snippet.push_str(&window[cursor..]);
    if window_end < line_end {
        snippet.push('…');
    }

    // One tidy line: control characters and runs of whitespace become a space
    let mut tidy = String::with_capacity(snippet.len());
    let mut last_was_space = true;
    for ch in snippet.chars() {
        let is_mark = ch == SNIPPET_MARK_START || ch == SNIPPET_MARK_END;
        if ch.is_whitespace() || (ch.is_control() && !is_mark) {
            if !last_was_space {
                tidy.push(' ');
            }
            last_was_space = true;
        } else {
            tidy.push(ch);
            last_was_space = false;
        }
    }
    Some(tidy.trim_end().to_string())
}

fn contains_ignore_ascii_case(haystack: &str, needle_lc: &str) -> bool {
    let (haystack, needle) = (haystack.as_bytes(), needle_lc.as_bytes());
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|window| window.eq_ignore_ascii_case(needle))
}

fn split_camel_case(word: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut prev_was_lower = false;

    for ch in word.chars() {
        if ch.is_uppercase() && prev_was_lower && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        current.push(ch);
        prev_was_lower = ch.is_lowercase();
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Lower-cased alphanumeric runs of the query, camelCase split, not stemmed
fn tokenize_query(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .flat_map(split_camel_case)
        .map(|part| part.to_lowercase())
        .collect()
}

fn preferred_path_boost(path_lc: &str) -> f32 {
    if path_lc.contains("/downloads/") {
        35.0
    } else if path_lc.contains("/documents/") {
        30.0
    } else if path_lc.contains("/desktop/") {
        25.0
    } else {
        0.0
    }
}

fn recency_boost(mtime_ns: u64, now_ns: u64) -> f32 {
    let days = now_ns.saturating_sub(mtime_ns) / 1_000_000_000 / 86_400;
    match days {
        0..=1 => 8.0,
        2..=7 => 5.0,
        8..=30 => 2.0,
        _ => 0.0,
    }
}

/// How well a path's filename matches the query text. 0 means no match.
fn lexical_score(path: &str, query: &str, query_tokens: &[String]) -> f32 {
    let filename_raw = path.rsplit('/').next().unwrap_or("");
    let filename = filename_raw.to_lowercase();
    let stem_raw = Path::new(filename_raw)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(filename_raw);
    let stem = stem_raw.to_lowercase();
    let path_lc = path.to_lowercase();

    // Dotted queries (e.g. "adaprag.pdf") should strictly match the filename or path.
    if query.contains('.') && !filename.contains(query) && !path_lc.contains(query) {
        return 0.0;
    }

    let mut score = 0.0;
    let stem_tokens = tokenize_query(stem_raw);

    // Layer 1: the query as a phrase
    if filename == query || stem == query {
        score += 250.0;
    } else if stem_tokens.iter().any(|token| token == query) {
        score += 180.0;
    }
    if filename.starts_with(query) || stem.starts_with(query) {
        score += 160.0;
    }
    if filename.contains(query) || stem.contains(query) {
        score += 110.0;
    } else if query.contains('/') && path_lc.contains(query) {
        score += 110.0;
    }

    // Layer 2: each query token on its own
    let mut matched_tokens = 0usize;
    for token in query_tokens {
        if stem_tokens.contains(token) {
            score += 50.0;
            matched_tokens += 1;
        } else if stem.starts_with(token.as_str()) {
            score += 35.0;
            matched_tokens += 1;
        } else if stem.contains(token.as_str()) || filename.contains(token.as_str()) {
            score += 22.0;
            matched_tokens += 1;
        } else if path_lc.contains(token.as_str()) {
            score += 6.0;
        }
    }

    // Every token of a multi-word query matched
    if !query_tokens.is_empty() && matched_tokens >= query_tokens.len() {
        score += 80.0;
    }

    // Tokens appear in the filename in query order
    if query_tokens.len() > 1 {
        let mut last_pos = 0;
        let in_order = query_tokens.iter().all(|token| match filename.find(token.as_str()) {
            Some(pos) if pos >= last_pos => {
                last_pos = pos;
                true
            }
            _ => false,
        });
        if in_order {
            score += 50.0;
        }
    }

    if score <= 0.0 {
        return 0.0;
    }
    score + preferred_path_boost(&path_lc)
}

#[cfg(test)]
#[path = "engine_test.rs"]
mod tests;
