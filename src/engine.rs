//! The live search engine: owns the index, keeps it on disk, keeps it current,
//! and answers queries from memory.
//!
//! Lifecycle of the data directory:
//!
//! ```text
//! config.json        roots / exclusions / limits (written by `reconfigure`)
//! identity.db        (device, inode, birthtime) → stable DocId
//! segment_NNNNNN.seg full snapshot of the index (postings + documents)
//! manifest.json      which segment is current and the WAL seq it covers
//! wal.log            filesystem changes applied since that snapshot
//! signals.json       click history used for ranking
//! metrics.db         query latency ring buffer
//! ```
//!
//! On start the snapshot is loaded, WAL entries newer than it are re-applied,
//! and a reconcile walk picks up anything that changed while the app was not
//! running. From then on FSEvents drive incremental updates, each logged to
//! the WAL before it is applied, with periodic snapshots truncating the log.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock};
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
use crate::index::segment::{Segment, SegmentBuilder};
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
        let tmp = data_dir.join("config.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(tmp, data_dir.join("config.json"))?;
        Ok(())
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
    /// File name of the current segment inside the data directory
    segment: Option<String>,
    generation: u64,
    /// Every WAL entry with seq <= this is contained in the segment
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
    /// Content extraction stopped because the memory budget was reached
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
    pub index_bytes: u64,
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
    last_query: Mutex<String>,

    /// Seq of the last WAL entry written
    last_seq: AtomicU64,
    generation: AtomicU64,
    /// Changes applied since the last snapshot
    dirty: AtomicU64,

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
            last_query: Mutex::new(String::new()),
            last_seq: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            dirty: AtomicU64::new(0),
            phase: AtomicU8::new(Phase::Loading as u8),
            work_done: AtomicU64::new(0),
            work_total: AtomicU64::new(0),
            budget_exhausted: AtomicBool::new(false),
            warming: AtomicBool::new(true),
            started: Instant::now(),
            stop: AtomicBool::new(false),
            reconfigured: AtomicBool::new(false),
            worker: Mutex::new(None),
        }))
    }

    pub fn config(&self) -> EngineConfig {
        self.config.read().unwrap().clone()
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
        self.state.read().unwrap().executor.delta_index.live_doc_count()
    }

    // ─── Loading ──────────────────────────────────────────────────────────────

    /// Load the current snapshot, if any, into memory. Read-only on disk.
    /// Returns the number of documents loaded.
    pub fn load_snapshot(&self) -> Result<usize> {
        let manifest = Manifest::load(&self.data_dir);
        self.generation.store(manifest.generation, Ordering::Relaxed);
        self.last_seq.store(manifest.snapshot_seq, Ordering::Relaxed);

        let Some(name) = manifest.segment else {
            return Ok(0);
        };
        let segment = match Segment::open(self.data_dir.join(&name)) {
            Ok(segment) => segment,
            Err(e) => {
                // A missing or corrupt snapshot is not fatal: reconcile rebuilds
                // the index from the filesystem.
                log::error!("snapshot {name} unusable ({e}); rebuilding from disk");
                return Ok(0);
            }
        };
        let (term_dict, documents) = segment.into_parts();
        let config = self.config();
        let tokenizer = Tokenizer::new();

        let mut state = State::new(DeltaIndex::from_parts(
            config.memory_budget_mb * 1024 * 1024,
            term_dict,
            documents,
        ));
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

        let loaded = paths.len();
        *self.state.write().unwrap() = state;
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
        match startup::detect_startup_path(&self.data_dir)? {
            StartupPath::FirstLaunch => startup::first_launch(&self.data_dir)?,
            StartupPath::WarmRestart => startup::warm_restart(&self.data_dir)?,
            StartupPath::CrashRecovery => {
                log::warn!("previous session did not shut down cleanly; recovering");
                startup::crash_recovery(&self.data_dir)?
            }
        }

        let loaded = self.load_snapshot()?;

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

        *self.wal.lock().unwrap() = Some(self.open_wal()?);
        Ok((loaded, replayed))
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
        let state = self.state.read().unwrap();
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
            for (position, term) in terms.into_iter().enumerate() {
                let posting = postings
                    .entry(term)
                    .or_insert_with(|| Posting::new(doc_id, 0, 0));
                posting.term_freq += 1;
                posting.field_mask |= field;
                posting.positions.push(position as u32);
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
        let mut guard = self.state.write().unwrap();
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
        }
        state.executor.delta_index.purge(&removed);

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

        let mut seen: HashSet<DocId> = HashSet::new();
        let mut batch: Vec<Prepared> = Vec::new();
        let mut aborted = false;

        let _ = self.identity.lock().unwrap().begin_batch();
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
                    let mut identity = self.identity.lock().unwrap();
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
        let _ = self.identity.lock().unwrap().commit_batch();

        // Only a complete walk tells us what is gone.
        if !aborted {
            let stale: HashSet<DocId> = {
                let state = self.state.read().unwrap();
                state.docs().keys().filter(|id| !seen.contains(id)).copied().collect()
            };
            if !stale.is_empty() {
                log::info!("reconcile: removing {} documents no longer on disk or in scope", stale.len());
            }
            self.commit(Vec::new(), stale);
            self.state.write().unwrap().executor.path_trie.rebuild_prefix_cache(20);
            self.warming.store(false, Ordering::Relaxed);
        }
    }

    /// Extract and index the content of one document. Returns false once the
    /// memory budget is exhausted.
    fn index_one_content(&self, tokenizer: &Tokenizer, doc_id: DocId, path: &str) -> bool {
        if self.state.read().unwrap().executor.delta_index.is_over_budget() {
            if !self.budget_exhausted.swap(true, Ordering::Relaxed) {
                log::warn!("index memory budget reached; remaining file contents are not indexed");
            }
            return false;
        }

        let extracted = self.extractor.lock().unwrap().extract(path);
        let (postings, token_count, content_hash) = match extracted {
            Ok(result) if !result.text.is_empty() => {
                let tokens = tokenizer.tokenize(&result.text);
                let token_count = tokens.len() as u32;
                // Positions are not kept for content: they would dominate memory
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

        let mut state = self.state.write().unwrap();
        // The file may have been removed or replaced while we were reading it
        let still_current = state.docs().get(&doc_id).is_some_and(|doc| doc.path == path && !doc.content_indexed);
        if still_current {
            if postings.is_empty() {
                state.executor.delta_index.mark_content_indexed(doc_id);
            } else {
                state.executor.delta_index.append_postings(doc_id, postings, token_count, content_hash);
            }
            self.dirty.fetch_add(1, Ordering::Relaxed);
        }
        true
    }

    /// Extract content for every document still waiting for it.
    /// `pump` is called between files so live changes keep flowing.
    pub fn index_content(&self, mut pump: impl FnMut()) {
        if !self.config().index_content {
            return;
        }
        let queue: Vec<(DocId, String)> = {
            let state = self.state.read().unwrap();
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
        self.budget_exhausted.store(false, Ordering::Relaxed);
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
            if index % 64 == 63 {
                self.state.write().unwrap().executor.refresh_stats();
                pump();
            }
        }
        self.state.write().unwrap().executor.refresh_stats();
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
                    let state = self.state.read().unwrap();
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
                    let is_new = !self.state.read().unwrap().path_ids.contains_key(path_str);
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
            self.state.write().unwrap().executor.refresh_stats();
        }
    }

    fn log_change(&self, path: &str, metadata: Option<&fs::Metadata>) {
        let guard = self.wal.lock().unwrap();
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

    /// Write the whole index to a new segment, point the manifest at it, and
    /// truncate the WAL it supersedes. Tombstoned data is not carried over,
    /// so this doubles as compaction.
    pub fn snapshot(&self) -> Result<()> {
        // Everything logged so far is applied: logging and applying happen
        // together on the worker thread, which is also the thread snapshotting.
        if let Some(wal) = self.wal.lock().unwrap().as_ref() {
            wal.flush()?;
        }
        let snapshot_seq = self.last_seq.load(Ordering::Relaxed);
        let generation = self.generation.load(Ordering::Relaxed) + 1;
        let name = format!("segment_{generation:06}.seg");

        let (builder, doc_count, signals) = {
            let state = self.state.read().unwrap();
            (
                SegmentBuilder::from_delta(&state.executor.delta_index),
                state.executor.delta_index.live_doc_count() as u64,
                serde_json::to_vec(&state.signals)?,
            )
        };
        builder.finalize(self.data_dir.join(&name))?;

        let manifest = Manifest {
            segment: Some(name.clone()),
            generation,
            snapshot_seq,
            doc_count,
            written_at_secs: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        };
        let tmp = self.data_dir.join("manifest.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&manifest)?)?;
        fs::rename(&tmp, self.data_dir.join("manifest.json"))?;
        self.generation.store(generation, Ordering::Relaxed);

        // The manifest now points at the new segment; older ones and the WAL
        // entries it covers are no longer needed.
        for entry in fs::read_dir(&self.data_dir)?.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "seg") && entry.file_name().to_string_lossy() != name {
                let _ = fs::remove_file(path);
            }
        }
        {
            let mut wal = self.wal.lock().unwrap();
            if wal.is_some() {
                *wal = None;
                fs::File::create(self.wal_path())?;
                *wal = Some(self.open_wal()?);
            }
        }
        fs::write(self.data_dir.join("signals.json"), signals)?;

        self.dirty.store(0, Ordering::Relaxed);
        log::info!("snapshot {name}: {doc_count} documents");
        Ok(())
    }

    // ─── Background worker ────────────────────────────────────────────────────

    /// Run the engine in the background: load, reconcile, extract content,
    /// then follow filesystem events until `shutdown`.
    pub fn start(self: &Arc<Self>) {
        let mut worker = self.worker.lock().unwrap();
        if worker.is_some() {
            return;
        }
        let engine = Arc::clone(self);
        *worker = std::thread::Builder::new()
            .name("localsearch-indexer".to_string())
            .spawn(move || engine.run())
            .map_err(|e| log::error!("could not start indexer thread: {e}"))
            .ok();
    }

    fn run(&self) {
        // Indexing must never compete with the user's foreground I/O
        set_io_policy(IoPolicy::Throttle);

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

        while !self.stop.load(Ordering::Relaxed) && !self.reconfigured.load(Ordering::Relaxed) {
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
                self.state.write().unwrap().executor.path_trie.rebuild_prefix_cache(20);
                last_snapshot = Instant::now();
            }
        }
    }

    /// Replace the configuration for this session (persisting it is the
    /// caller's choice, see `EngineConfig::save`). The worker re-reconciles
    /// against the new roots and exclusions; documents that fell out of scope
    /// are removed.
    pub fn reconfigure(&self, config: EngineConfig) -> Result<()> {
        let config = config.normalized();
        {
            let mut state = self.state.write().unwrap();
            state.executor.spotlight_fallback.set_roots(config.roots.clone());
            state.executor.delta_index.set_memory_budget(config.memory_budget_mb * 1024 * 1024);
        }
        let changed = {
            let mut current = self.config.write().unwrap();
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
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
        self.save_if_dirty();
        *self.wal.lock().unwrap() = None;
        if let Err(e) = fs::write(self.data_dir.join(".clean_shutdown"), b"") {
            log::error!("could not write clean-shutdown marker: {e}");
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
        *self.last_query.lock().unwrap() = query.to_string();

        let query_lc = query.to_lowercase();
        let query_tokens = tokenize_query(query);
        // "report.pdf" or "src/main.rs" name a file; fuzzy and content matches
        // on the fragments would only add noise.
        let literal = query_lc.contains('.') || query_lc.contains('/');

        let state = self.state.read().unwrap();
        let mut scores: HashMap<DocId, (f32, ResultSource)> = HashMap::new();

        if !literal {
            if let Ok(parsed) = Query::parse(query) {
                for result in state.executor.execute(parsed, None).unwrap_or_default() {
                    scores.insert(result.doc_id, (result.score * INDEX_SCORE_WEIGHT, result.source));
                }
            }
        }

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
            scores.entry(doc_id).or_insert((0.0, ResultSource::LocalIndex)).0 += score;
        }

        let mut hits: Vec<SearchHit> = scores
            .into_iter()
            .filter_map(|(doc_id, (score, source))| {
                let doc = state.docs().get(&doc_id)?;
                // Files opened from results before rank higher next time
                let clicks = state.signals.hot_paths.get(&doc.path).copied().unwrap_or(0);
                Some(SearchHit {
                    doc_id: doc_id.0,
                    path: doc.path.clone(),
                    score: score + CLICK_WEIGHT * (1.0 + clicks as f32).ln(),
                    source,
                })
            })
            .collect();

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

        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
        });
        hits.truncate(MAX_RESULTS);

        if let Ok(metrics) = self.metrics.try_lock() {
            if let Some(collector) = metrics.as_ref() {
                let _ = collector.record_query(started.elapsed(), hits.len());
            }
        }
        hits
    }

    /// The user opened `path` from the results of the most recent query.
    pub fn record_click(&self, path: &str) {
        let query = self.last_query.lock().unwrap().to_lowercase();
        let mut state = self.state.write().unwrap();
        state.executor.ranker.record_click(PathBuf::from(path));
        state.signals.record_click(&query, path);
        self.dirty.fetch_add(1, Ordering::Relaxed);
    }

    // ─── Health ───────────────────────────────────────────────────────────────

    pub fn stats(&self) -> EngineStats {
        let state = self.state.read().unwrap();
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
                    .filter(|path| path.extension().is_some_and(|ext| ext == "seg"))
                    .collect()
            })
            .unwrap_or_default();
        let last_snapshot_age_secs = segments
            .iter()
            .filter_map(|path| fs::metadata(path).ok()?.modified().ok()?.elapsed().ok())
            .map(|age| age.as_secs())
            .min()
            .unwrap_or(0);

        let metrics = self.metrics.lock().unwrap();
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
            index_bytes: delta.approx_bytes() as u64,
            over_budget: delta.is_over_budget(),
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
