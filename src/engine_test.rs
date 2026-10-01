use super::*;
use tempfile::TempDir;

/// A data directory and one root, both temporary. Paths are canonical because
/// FSEvents reports `/private/var/...`, not `/var/...`.
struct Fixture {
    _data: TempDir,
    _root: TempDir,
    data_dir: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        Self {
            data_dir: fs::canonicalize(data.path()).unwrap(),
            root: fs::canonicalize(root.path()).unwrap(),
            _data: data,
            _root: root,
        }
    }

    fn config(&self) -> EngineConfig {
        EngineConfig { roots: vec![self.root.clone()], ..EngineConfig::default() }
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    /// An engine driven synchronously: loaded, reconciled, content indexed.
    fn indexed_engine(&self) -> Arc<Engine> {
        let engine = Engine::open(&self.data_dir, self.config()).unwrap();
        engine.load().unwrap();
        engine.reconcile();
        engine.index_content(|| {});
        engine
    }
}

fn paths(hits: &[SearchHit]) -> Vec<&str> {
    hits.iter().map(|hit| hit.path.rsplit('/').next().unwrap()).collect()
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn test_finds_files_by_name() {
    let fx = Fixture::new();
    fx.write("quarterly_report.pdf", "");
    fx.write("budget.xlsx", "");
    fx.write("projects/MeetingNotes.txt", "");
    let engine = fx.indexed_engine();

    assert_eq!(paths(&engine.search("quarterly"))[0], "quarterly_report.pdf");
    assert_eq!(paths(&engine.search("budget.xlsx")), ["budget.xlsx"]);
    // camelCase split, typo tolerance
    assert_eq!(paths(&engine.search("meeting"))[0], "MeetingNotes.txt");
    assert_eq!(paths(&engine.search("quartely"))[0], "quarterly_report.pdf");
    // the directory itself is a document too
    assert!(paths(&engine.search("projects")).contains(&"projects"));
}

#[test]
fn test_finds_files_by_content() {
    let fx = Fixture::new();
    fx.write("a.md", "notes on the zebrafish genome");
    fx.write("b.md", "grocery list: apples, bread");
    fx.write("zebrafish.md", "unrelated words");
    let engine = fx.indexed_engine();

    let hits = engine.search("zebrafish");
    // The file named after the term outranks the file that only mentions it
    assert_eq!(paths(&hits), ["zebrafish.md", "a.md"]);
    assert_eq!(paths(&engine.search("grocery")), ["b.md"]);
}

#[cfg(target_os = "macos")]
#[test]
fn test_finds_pdf_by_content() {
    let fx = Fixture::new();
    let pdf = crate::extract::extractors::tests::sample_pdf();
    fs::write(fx.root.join("scan0001.pdf"), pdf).unwrap();
    let engine = fx.indexed_engine();

    assert_eq!(paths(&engine.search("zebrafish")), ["scan0001.pdf"]);
}

#[test]
fn test_content_indexing_can_be_disabled() {
    let fx = Fixture::new();
    fx.write("a.md", "notes on the zebrafish genome");
    let config = EngineConfig { index_content: false, ..fx.config() };
    let engine = Engine::open(&fx.data_dir, config).unwrap();
    engine.load().unwrap();
    engine.reconcile();
    engine.index_content(|| {});

    assert!(engine.search("zebrafish").is_empty());
    assert_eq!(paths(&engine.search("a.md")), ["a.md"]);
}

#[test]
fn test_exclusions_hidden_files_and_depth() {
    let fx = Fixture::new();
    fx.write("keep/findme.txt", "");
    fx.write("node_modules/findme.txt", "");
    fx.write(".secret/findme.txt", "");
    fx.write("Skipped/findme.txt", "");
    fx.write("a/b/c/findme.txt", "");
    let config = EngineConfig {
        excludes: vec!["node_modules".into(), "skipped".into()],
        max_depth: 3,
        ..fx.config()
    };
    let engine = Engine::open(&fx.data_dir, config).unwrap();
    engine.load().unwrap();
    engine.reconcile();

    let hits = engine.search("findme");
    assert_eq!(hits.len(), 1, "{:?}", hits);
    assert!(hits[0].path.ends_with("keep/findme.txt"));
}

#[test]
fn test_index_survives_restart_without_rescanning() {
    let fx = Fixture::new();
    fx.write("persisted.md", "content about marmalade");
    {
        let engine = fx.indexed_engine();
        engine.shutdown();
    }
    assert!(fx.data_dir.join(".clean_shutdown").exists());

    // Reopen and only load: no walk of the filesystem.
    let engine = Engine::open(&fx.data_dir, fx.config()).unwrap();
    let (loaded, replayed) = engine.load().unwrap();
    assert_eq!((loaded, replayed), (1, 0));
    assert_eq!(paths(&engine.search("persisted")), ["persisted.md"]);
    assert_eq!(paths(&engine.search("marmalade")), ["persisted.md"]);
    // DocIds are stable across restarts
    engine.reconcile();
    assert_eq!(engine.doc_count(), 1);
}

#[test]
fn test_wal_replays_changes_made_after_last_snapshot() {
    let fx = Fixture::new();
    fx.write("old.txt", "");
    let doomed = fx.write("doomed.txt", "");
    {
        let engine = fx.indexed_engine();
        engine.snapshot().unwrap();

        // Changes after the snapshot reach the WAL, then the process "crashes":
        // no shutdown, no second snapshot.
        let added = fx.write("added_later.txt", "");
        fs::remove_file(&doomed).unwrap();
        engine.apply_changes(vec![added, doomed.clone()]);
        *engine.wal.lock().unwrap() = None; // flushes, as the 250ms timer would have
    }

    let engine = Engine::open(&fx.data_dir, fx.config()).unwrap();
    let (loaded, replayed) = engine.load().unwrap();
    assert_eq!((loaded, replayed), (2, 2));
    assert_eq!(paths(&engine.search("added")), ["added_later.txt"]);
    assert!(engine.search("doomed").is_empty());
    assert_eq!(paths(&engine.search("old")), ["old.txt"]);
}

/// Path of one file of the current on-disk base
fn base_file(data_dir: &Path, extension: &str) -> PathBuf {
    fs::read_dir(data_dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == extension))
        .unwrap_or_else(|| panic!("no .{extension} file in the data directory"))
}

fn flip_middle_byte(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xFF;
    fs::write(path, bytes).unwrap();
}

#[test]
fn test_damaged_document_table_falls_back_to_rescan() {
    let fx = Fixture::new();
    fx.write("survivor.txt", "");
    {
        let engine = fx.indexed_engine();
        engine.shutdown();
    }
    flip_middle_byte(&base_file(&fx.data_dir, "docs"));

    // Caught even after a clean shutdown: the table is always checked
    let engine = Engine::open(&fx.data_dir, fx.config()).unwrap();
    assert_eq!(engine.load().unwrap().0, 0);
    engine.reconcile();
    assert_eq!(paths(&engine.search("survivor")), ["survivor.txt"]);
}

#[test]
fn test_damaged_postings_are_caught_after_a_crash() {
    let fx = Fixture::new();
    for i in 0..50 {
        fx.write(&format!("note{i}.md"), "shared words about walruses and their tusks");
    }
    {
        let engine = fx.indexed_engine();
        engine.shutdown();
    }
    flip_middle_byte(&base_file(&fx.data_dir, "post"));
    // No clean-shutdown marker: this launch follows a crash, so files are verified
    fs::remove_file(fx.data_dir.join(".clean_shutdown")).unwrap();

    let engine = Engine::open(&fx.data_dir, fx.config()).unwrap();
    assert_eq!(engine.load().unwrap().0, 0, "a base that fails its checksum must not be used");
    engine.reconcile();
    engine.index_content(|| {});
    assert_eq!(engine.search("walruses").len(), 50);
}

#[test]
fn test_damaged_postings_never_panic_a_query() {
    let fx = Fixture::new();
    for i in 0..50 {
        fx.write(&format!("note{i}.md"), "shared words about walruses and their tusks");
    }
    {
        let engine = fx.indexed_engine();
        engine.shutdown();
    }
    // Scribble over the whole postings file, and leave the clean-shutdown
    // marker so the engine has no reason to verify it
    let postings = base_file(&fx.data_dir, "post");
    let len = fs::read(&postings).unwrap().len();
    fs::write(&postings, vec![0xFFu8; len]).unwrap();

    let engine = Engine::open(&fx.data_dir, fx.config()).unwrap();
    engine.load().unwrap();
    // Content matches are lost, but nothing crashes and names still work
    let _ = engine.search("walruses");
    let _ = engine.search("tusks shared words");
    assert_eq!(paths(&engine.search("note7.md")), ["note7.md"]);
}

#[test]
fn test_reconcile_picks_up_offline_changes() {
    let fx = Fixture::new();
    let edited = fx.write("edited.md", "first draft about walruses");
    let removed = fx.write("removed.md", "");
    {
        let engine = fx.indexed_engine();
        engine.shutdown();
    }

    // While the app is not running:
    fs::write(&edited, "second draft about narwhals, considerably longer").unwrap();
    fs::remove_file(&removed).unwrap();
    fx.write("created.md", "");

    let engine = fx.indexed_engine();
    assert_eq!(engine.doc_count(), 2);
    assert!(engine.search("removed").is_empty());
    assert_eq!(paths(&engine.search("created")), ["created.md"]);
    assert_eq!(paths(&engine.search("narwhals")), ["edited.md"]);
    assert!(engine.search("walruses").is_empty(), "stale content must be purged");
}

#[test]
fn test_apply_changes_handles_modify_rename_and_directory_removal() {
    let fx = Fixture::new();
    let file = fx.write("draft.md", "about otters");
    fx.write("folder/inner.md", "");
    let engine = fx.indexed_engine();

    fs::write(&file, "now about badgers instead of the other thing").unwrap();
    engine.apply_changes(vec![file.clone()]);
    assert_eq!(paths(&engine.search("badgers")), ["draft.md"]);
    assert!(engine.search("otters").is_empty());

    let renamed = fx.root.join("final.md");
    fs::rename(&file, &renamed).unwrap();
    engine.apply_changes(vec![file, renamed]);
    assert!(engine.search("draft").is_empty());
    assert_eq!(paths(&engine.search("final")), ["final.md"]);
    assert_eq!(paths(&engine.search("badgers")), ["final.md"]);

    fs::remove_dir_all(fx.root.join("folder")).unwrap();
    engine.apply_changes(vec![fx.root.join("folder")]);
    assert!(engine.search("inner").is_empty());
    assert_eq!(engine.doc_count(), 1);
}

#[test]
fn test_live_updates_from_filesystem_events() {
    let fx = Fixture::new();
    fx.write("existing.txt", "");
    let engine = Engine::open(&fx.data_dir, fx.config()).unwrap();
    engine.start();
    wait_until("the engine is ready", || engine.status().phase == Phase::Ready);
    assert_eq!(paths(&engine.search("existing")), ["existing.txt"]);

    let created = fx.write("appeared_while_running.txt", "mentions a capybara");
    wait_until("the new file is searchable by name", || !engine.search("appeared").is_empty());
    wait_until("the new file is searchable by content", || !engine.search("capybara").is_empty());

    fs::remove_file(&created).unwrap();
    wait_until("the deleted file is gone", || engine.search("appeared").is_empty());

    engine.shutdown();
}

#[test]
fn test_reconfigure_changes_scope_of_running_engine() {
    let fx = Fixture::new();
    fx.write("one/alpha.txt", "");
    fx.write("two/bravo.txt", "");
    let config = EngineConfig { roots: vec![fx.root.join("one")], ..fx.config() };
    let engine = Engine::open(&fx.data_dir, config).unwrap();
    engine.start();
    wait_until("the engine is ready", || engine.status().phase == Phase::Ready);
    assert!(engine.search("bravo").is_empty());

    let config = EngineConfig { roots: vec![fx.root.join("two")], ..fx.config() };
    engine.reconfigure(config).unwrap();
    wait_until("the new root is indexed", || !engine.search("bravo").is_empty());
    wait_until("the old root is dropped", || engine.search("alpha").is_empty());

    engine.shutdown();
}

#[test]
fn test_clicks_raise_rank_and_persist() {
    let fx = Fixture::new();
    fx.write("report_a.txt", "");
    let favourite = fx.write("report_b.txt", "");
    let favourite = favourite.to_str().unwrap();
    {
        let engine = fx.indexed_engine();
        assert_eq!(paths(&engine.search("report"))[0], "report_a.txt");

        engine.record_click(favourite);
        let hits = engine.search("report");
        assert_eq!(paths(&hits)[0], "report_b.txt", "{hits:?}");
        engine.shutdown();
    }

    let engine = Engine::open(&fx.data_dir, fx.config()).unwrap();
    engine.load().unwrap();
    assert_eq!(paths(&engine.search("report"))[0], "report_b.txt");
}

#[test]
fn test_content_has_no_ceiling_and_the_delta_is_merged_as_it_fills() {
    let fx = Fixture::new();
    for i in 0..200 {
        let words: String = (0..300).map(|w| format!("word{i}x{w} ")).collect();
        fx.write(&format!("doc{i}.txt"), &format!("{words} everywhere"));
    }
    // A one-byte allowance for the in-memory delta forces a merge every few files
    let config = EngineConfig { memory_budget_mb: 0, ..fx.config() };
    let engine = Engine::open(&fx.data_dir, config).unwrap();
    engine.load().unwrap();
    engine.state.write().unwrap().executor.delta_index.set_memory_budget(1);
    engine.reconcile();
    engine.index_content(|| {});

    // Every file's content is searchable, including the last ones indexed
    assert!(!engine.status().budget_exhausted);
    assert_eq!(engine.search("everywhere").len(), 100, "capped at the result limit, i.e. far more than a budget would have allowed");
    for i in [0, 57, 123, 199] {
        assert_eq!(paths(&engine.search(&format!("word{i}x299"))), [format!("doc{i}.txt")]);
    }

    // Many merges happened, the delta stayed small, and one generation is on disk
    let stats = engine.stats();
    assert!(engine.generation.load(Ordering::Relaxed) >= 10);
    assert!(stats.delta_bytes < 200_000, "delta holds {} bytes", stats.delta_bytes);
    assert_eq!(stats.segment_count, 1);
    assert!(stats.index_bytes > 0);
    // (These synthetic words are so regular that the dictionary compresses
    // tens of thousands of them into a few hundred bytes; count terms instead.)
    let on_disk_terms = engine.state.read().unwrap().executor.base.as_ref().unwrap().term_count();
    assert!(on_disk_terms > 50_000, "{on_disk_terms} terms on disk");
}

#[test]
fn test_stats_reflect_the_real_index() {
    let fx = Fixture::new();
    fx.write("a.md", "alpha");
    fx.write("b.md", "beta");
    let gone = fx.write("c.png", "");
    let engine = fx.indexed_engine();
    engine.snapshot().unwrap();
    engine.search("alpha");
    engine.search("nothing-matches-this");

    let stats = engine.stats();
    assert_eq!(stats.doc_count, 3);
    assert_eq!(stats.segment_count, 1);
    assert_eq!(stats.content_indexed_fraction, 1.0);
    assert_eq!(stats.wal_lag_events, 0);
    assert_eq!(stats.phantom_rate, 0.0);
    assert_eq!(stats.zero_result_rate, 0.5);
    assert!(stats.index_bytes > 0);

    // A file deleted behind the index's back shows up as a phantom,
    // and an unsnapshotted change as WAL lag.
    fs::remove_file(&gone).unwrap();
    assert!((engine.stats().phantom_rate - 1.0 / 3.0).abs() < 1e-6);
    engine.apply_changes(vec![gone]);
    *engine.wal.lock().unwrap() = None;
    let stats = engine.stats();
    assert_eq!((stats.doc_count, stats.wal_lag_events, stats.phantom_rate), (2, 1, 0.0));
}

#[test]
fn test_config_normalization() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.root.join("nested")).unwrap();
    let config = EngineConfig {
        roots: vec![fx.root.join("nested"), fx.root.clone(), fx.root.clone()],
        excludes: vec![" Node_Modules ".into(), "".into()],
        max_depth: 0,
        ..EngineConfig::default()
    }
    .normalized();

    assert_eq!(config.roots, vec![fx.root.clone()], "nested and duplicate roots collapse");
    assert_eq!(config.excludes, vec!["node_modules".to_string()]);
    assert_eq!(config.max_depth, 1);

    config.save(&fx.data_dir).unwrap();
    let text = fs::read_to_string(fx.data_dir.join("config.json")).unwrap();
    assert_eq!(serde_json::from_str::<EngineConfig>(&text).unwrap(), config);
    // Partial JSON takes defaults for the rest
    let partial: EngineConfig = serde_json::from_str(r#"{"max_depth": 4}"#).unwrap();
    assert!(partial.index_content && partial.max_depth == 4);
}

fn marked(snippet: &str) -> String {
    snippet.replace(SNIPPET_MARK_START, "[").replace(SNIPPET_MARK_END, "]")
}

#[test]
fn test_snippet_shows_the_matching_line() {
    let fx = Fixture::new();
    let notes = fx.write(
        "notes.md",
        "# Planning\n\nFirst item is unrelated.\nWe agreed the zebrafish tanks\tneed  cleaning weekly.\nLast line.\n",
    );
    let engine = fx.indexed_engine();

    let snippet = engine.snippet(notes.to_str().unwrap(), "zebrafish").unwrap();
    assert_eq!(marked(&snippet), "We agreed the [zebrafish] tanks need cleaning weekly.");
}

#[test]
fn test_snippet_matches_stems_prefixes_and_every_query_word() {
    let fx = Fixture::new();
    let file = fx.write("a.txt", "The reports were cleaned up before the cleanup meeting.");
    let engine = fx.indexed_engine();
    let path = file.to_str().unwrap();

    // "report" ~ "reports" (stem), "clean" ~ "cleaned" (stem) and "cleanup" (prefix)
    let snippet = engine.snippet(path, "report clean kind:txt -draft").unwrap();
    assert_eq!(marked(&snippet), "The [reports] were [cleaned] up before the [cleanup] meeting.");
}

#[test]
fn test_snippet_trims_long_lines_around_the_match() {
    let fx = Fixture::new();
    let long_line = format!("{} needle {}", "lorem ipsum ".repeat(40), "dolor sit ".repeat(40));
    let file = fx.write("long.txt", &long_line);
    let engine = fx.indexed_engine();

    let snippet = engine.snippet(file.to_str().unwrap(), "needle").unwrap();
    let shown = marked(&snippet);
    assert!(shown.starts_with('…') && shown.ends_with('…'), "{shown}");
    assert!(shown.contains("[needle]"));
    assert!(shown.chars().count() <= SNIPPET_CHARS + 4, "{} chars", shown.chars().count());
    // Starts on a word boundary, not mid-word
    assert!(shown.starts_with("…lorem") || shown.starts_with("…ipsum"), "{shown}");
}

#[test]
fn test_snippet_is_none_without_a_content_match() {
    let fx = Fixture::new();
    let file = fx.write("zebrafish.md", "nothing relevant in here");
    let image = fx.write("zebrafish.png", "zebrafish");
    let engine = fx.indexed_engine();

    // Matched on its name only
    assert_eq!(engine.snippet(file.to_str().unwrap(), "zebrafish"), None);
    // Not a content-indexed type
    assert_eq!(engine.snippet(image.to_str().unwrap(), "zebrafish"), None);
    // Not in the index at all: the engine does not read arbitrary files
    let outside = fx.data_dir.join("outside.md");
    fs::write(&outside, "zebrafish").unwrap();
    assert_eq!(engine.snippet(outside.to_str().unwrap(), "zebrafish"), None);
    // Nothing but filters in the query
    assert_eq!(engine.snippet(file.to_str().unwrap(), "kind:md"), None);
}

#[test]
fn test_snippet_handles_non_ascii_text() {
    let fx = Fixture::new();
    let file = fx.write("menu.txt", "Entrée du jour — crème brûlée 🍮 avec café");
    let engine = fx.indexed_engine();

    let snippet = engine.snippet(file.to_str().unwrap(), "café").unwrap();
    assert_eq!(marked(&snippet), "Entrée du jour — crème brûlée 🍮 avec [café]");
}

// ─── Production hardening ─────────────────────────────────────────────────────

#[test]
fn test_typos_return_close_matches_not_everything() {
    let fx = Fixture::new();
    fx.write("report.txt", "");
    // Names one or two edits away from the typo "reprot" but unrelated to it
    for name in ["repo", "reboot", "depot", "repost", "retort", "remote", "resort", "rebook"] {
        fx.write(&format!("{name}.txt"), "");
    }
    let engine = fx.indexed_engine();

    let hits = engine.search("reprot");
    assert_eq!(paths(&hits)[0], "report.txt");
    // The transposition is found; the pile of two-edit neighbours is not dumped on the user
    assert!(hits.len() <= 4, "{:?}", paths(&hits));
}

#[test]
fn test_approximate_matches_do_not_dilute_real_ones() {
    let fx = Fixture::new();
    for i in 0..6 {
        fx.write(&format!("report_{i}.txt"), "");
    }
    fx.write("repost.txt", "");
    fx.write("deport.txt", "");
    let engine = fx.indexed_engine();

    let hits = engine.search("report");
    assert_eq!(hits.len(), 6, "{:?}", paths(&hits));
    assert!(paths(&hits).iter().all(|name| name.starts_with("report_")));
}

#[test]
fn test_data_directory_is_private() {
    let fx = Fixture::new();
    let _engine = fx.indexed_engine();
    let mode = fs::metadata(&fx.data_dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "index must not be readable by other users");
}

#[test]
fn test_second_engine_cannot_write_the_same_index() {
    let fx = Fixture::new();
    fx.write("shared.txt", "");
    let owner = fx.indexed_engine();
    owner.snapshot().unwrap();

    let intruder = Engine::open(&fx.data_dir, fx.config()).unwrap();
    let error = intruder.load().unwrap_err().to_string();
    assert!(error.contains("in use by another LocalSearch process"), "{error}");
    assert!(intruder.snapshot().is_err());

    // It can still read what the owner saved...
    assert_eq!(intruder.load_snapshot().unwrap(), 1);
    assert_eq!(paths(&intruder.search("shared")), ["shared.txt"]);
    // ...and shutting it down must not mark the owner's session as cleanly ended
    intruder.shutdown();
    assert!(!fx.data_dir.join(".clean_shutdown").exists());

    // Once the owner lets go, the directory can be taken over
    owner.shutdown();
    let successor = Engine::open(&fx.data_dir, fx.config()).unwrap();
    assert!(successor.load().is_ok());
}

#[test]
fn test_started_engine_falls_back_to_read_only_when_index_is_owned() {
    let fx = Fixture::new();
    fx.write("owned.txt", "");
    let owner = fx.indexed_engine();
    owner.snapshot().unwrap();

    let second = Engine::open(&fx.data_dir, fx.config()).unwrap();
    second.start();
    wait_until("the second engine is serving", || second.status().phase == Phase::Ready);
    assert_eq!(paths(&second.search("owned")), ["owned.txt"]);

    // It is not following the filesystem: that is the owner's job
    fx.write("later.txt", "");
    std::thread::sleep(Duration::from_millis(900));
    assert!(second.search("later").is_empty());
    second.shutdown();
}

#[test]
fn test_unreadable_root_keeps_its_entries() {
    let fx = Fixture::new();
    let kept = fx.root.join("kept");
    let volume = fx.root.join("volume");
    fs::create_dir_all(&kept).unwrap();
    fs::create_dir_all(&volume).unwrap();
    fs::write(kept.join("here.txt"), "").unwrap();
    fs::write(volume.join("offline.txt"), "").unwrap();
    let config = EngineConfig { roots: vec![kept.clone(), volume.clone()], ..fx.config() };
    let engine = Engine::open(&fx.data_dir, config).unwrap();
    engine.load().unwrap();
    engine.reconcile();
    assert_eq!(engine.doc_count(), 2);

    // The "volume" is unplugged: the root disappears entirely
    let parked = fx.root.join("volume-unmounted");
    fs::rename(&volume, &parked).unwrap();
    engine.reconcile();
    assert_eq!(paths(&engine.search("offline")), ["offline.txt"], "an unavailable root is not a deleted one");

    // Plugged back in with the file really gone: now it is removed
    fs::rename(&parked, &volume).unwrap();
    fs::remove_file(volume.join("offline.txt")).unwrap();
    engine.reconcile();
    assert!(engine.search("offline").is_empty());
    assert_eq!(engine.doc_count(), 1);
}

#[test]
fn test_worker_recovers_from_a_panic() {
    let fx = Fixture::new();
    fx.write("resilient.txt", "");
    let engine = Engine::open(&fx.data_dir, fx.config()).unwrap();
    engine.panic_in_next_reconcile.store(true, Ordering::Relaxed);
    engine.start();

    wait_until("the worker has restarted and indexed", || {
        engine.status().phase == Phase::Ready && !engine.search("resilient").is_empty()
    });
    // Queries and saving still work after the panic
    assert!(!engine.panic_in_next_reconcile.load(Ordering::Relaxed));
    engine.shutdown();
    assert!(fx.data_dir.join("manifest.json").exists());
}

#[test]
fn test_queries_survive_a_poisoned_lock() {
    let fx = Fixture::new();
    fx.write("still_here.txt", "");
    let engine = fx.indexed_engine();

    // A thread panics while holding the index lock
    let poisoner = Arc::clone(&engine);
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.state.write().unwrap();
        panic!("poison the lock");
    })
    .join();
    assert!(engine.state.is_poisoned());

    assert_eq!(paths(&engine.search("still")), ["still_here.txt"]);
    assert_eq!(engine.doc_count(), 1);
}

#[test]
fn test_queries_do_not_write_metrics_synchronously() {
    let fx = Fixture::new();
    fx.write("a.txt", "");
    let engine = fx.indexed_engine();
    engine.search("a");
    engine.search("b");

    // Timings are queued by the query and written later, off the query path
    assert_eq!(engine.pending_metrics.lock().unwrap().len(), 2);
    let recorded = engine.metrics.lock().unwrap().as_ref().unwrap().entry_count().unwrap();
    assert_eq!(recorded, 0);

    engine.flush_metrics();
    assert!(engine.pending_metrics.lock().unwrap().is_empty());
    assert_eq!(engine.metrics.lock().unwrap().as_ref().unwrap().entry_count().unwrap(), 2);
}

#[test]
fn test_oversized_query_is_cut_not_crashed() {
    let fx = Fixture::new();
    fx.write("needle.txt", "");
    let engine = fx.indexed_engine();

    let huge = format!("needle {}", "x".repeat(100_000));
    let started = Instant::now();
    let hits = engine.search(&huge);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(paths(&hits)[0], "needle.txt");
}

#[test]
fn test_index_from_an_older_format_is_rebuilt_not_trusted() {
    let fx = Fixture::new();
    fx.write("kept.txt", "");
    {
        let engine = fx.indexed_engine();
        engine.shutdown();
    }
    // What the previous format left behind: a manifest naming a segment file
    // (and no base), plus the segment itself
    fs::write(
        fx.data_dir.join("manifest.json"),
        r#"{"segment": "segment_000014.seg", "generation": 14, "snapshot_seq": 271, "doc_count": 1, "written_at_secs": 0}"#,
    )
    .unwrap();
    fs::write(fx.data_dir.join("segment_000014.seg"), b"LSSEG003 old bytes").unwrap();

    let engine = Engine::open(&fx.data_dir, fx.config()).unwrap();
    assert_eq!(engine.load().unwrap().0, 0);
    engine.reconcile();
    assert_eq!(paths(&engine.search("kept")), ["kept.txt"]);

    // The next save uses the new format, numbered after the old generation,
    // and the leftovers are gone
    engine.snapshot().unwrap();
    assert!(fx.data_dir.join("base_000015.terms").exists());
    assert!(!fx.data_dir.join("segment_000014.seg").exists());
    let leftovers = fs::read_dir(&fx.data_dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("base_")).count();
    assert_eq!(leftovers, 3, "exactly one generation's three files remain");
}

// ─── On-disk base + in-memory delta ───────────────────────────────────────────

#[test]
fn test_after_a_merge_the_index_is_on_disk_not_in_memory() {
    let fx = Fixture::new();
    fx.write("a.md", "notes on the zebrafish genome");
    fx.write("sub/b.md", "grocery list");
    let engine = fx.indexed_engine();
    assert!(engine.stats().delta_bytes > 0, "freshly indexed postings start in the delta");

    engine.snapshot().unwrap();
    let stats = engine.stats();
    assert_eq!(stats.delta_bytes, 0);
    assert!(stats.index_bytes > 0);
    assert!(engine.state.read().unwrap().executor.delta_index.index.is_empty());

    // Answers now come from the mapped files
    assert_eq!(paths(&engine.search("zebrafish")), ["a.md"]);
    assert_eq!(paths(&engine.search("grocery")), ["b.md"]);
    assert_eq!(paths(&engine.search("b.md")), ["b.md"]);
}

#[test]
fn test_edits_and_deletes_supersede_what_is_on_disk() {
    let fx = Fixture::new();
    let edited = fx.write("edited.md", "first draft about walruses");
    let deleted = fx.write("deleted.md", "ephemeral thoughts about walruses");
    fx.write("untouched.md", "steady notes about walruses");
    let engine = fx.indexed_engine();
    engine.snapshot().unwrap();
    assert_eq!(engine.search("walruses").len(), 3);

    fs::write(&edited, "second draft about narwhals, considerably longer").unwrap();
    fs::remove_file(&deleted).unwrap();
    engine.apply_changes(vec![edited.clone(), deleted.clone()]);

    // The base still physically holds the old postings; they must not answer
    assert_eq!(engine.stats().dead_in_base, 2);
    assert_eq!(paths(&engine.search("walruses")), ["untouched.md"]);
    assert_eq!(paths(&engine.search("narwhals")), ["edited.md"]);
    assert!(engine.search("ephemeral").is_empty());
    assert!(engine.search("deleted").is_empty());

    // After the next merge the dead postings are gone for real, and nothing changes for the user
    engine.snapshot().unwrap();
    assert_eq!(engine.stats().dead_in_base, 0);
    assert_eq!(paths(&engine.search("walruses")), ["untouched.md"]);
    assert_eq!(paths(&engine.search("narwhals")), ["edited.md"]);
    assert!(engine.search("ephemeral").is_empty());

    // And it survives a restart
    engine.shutdown();
    let engine = Engine::open(&fx.data_dir, fx.config()).unwrap();
    assert_eq!(engine.load().unwrap(), (2, 0));
    assert_eq!(paths(&engine.search("narwhals")), ["edited.md"]);
    assert_eq!(paths(&engine.search("walruses")), ["untouched.md"]);
}

#[test]
fn test_names_on_disk_and_content_in_memory_answer_together() {
    let fx = Fixture::new();
    let file = fx.write("capybara_notes.md", "observations of the giant rodent");
    let engine = Engine::open(&fx.data_dir, fx.config()).unwrap();
    engine.load().unwrap();
    engine.reconcile();
    // Names are merged to disk before any content is read, as on first launch
    engine.snapshot().unwrap();
    engine.index_content(|| {});

    assert_eq!(paths(&engine.search("capybara")), ["capybara_notes.md"]);
    assert_eq!(paths(&engine.search("rodent")), ["capybara_notes.md"]);

    // Editing it replaces both halves
    fs::write(&file, "now about tapirs and nothing else at all").unwrap();
    engine.apply_changes(vec![file]);
    assert!(engine.search("rodent").is_empty());
    assert_eq!(paths(&engine.search("tapirs")), ["capybara_notes.md"]);
    assert_eq!(paths(&engine.search("capybara")), ["capybara_notes.md"]);
    assert_eq!(engine.doc_count(), 1);
}

#[test]
fn test_rename_after_a_merge_moves_the_document() {
    let fx = Fixture::new();
    let old = fx.write("draft.md", "about otters");
    let engine = fx.indexed_engine();
    engine.snapshot().unwrap();

    let new = fx.root.join("final.md");
    fs::rename(&old, &new).unwrap();
    engine.apply_changes(vec![old, new]);

    assert!(engine.search("draft").is_empty());
    assert_eq!(paths(&engine.search("final")), ["final.md"]);
    assert_eq!(paths(&engine.search("otters")), ["final.md"]);
    engine.snapshot().unwrap();
    assert_eq!(paths(&engine.search("otters")), ["final.md"]);
    assert_eq!(engine.doc_count(), 1);
}

#[test]
fn test_merging_while_files_change_loses_nothing() {
    let fx = Fixture::new();
    for i in 0..30 {
        fx.write(&format!("seed{i}.txt"), "original content");
    }
    let engine = fx.indexed_engine();

    // One thread keeps creating and editing files while this one merges repeatedly
    let writer = {
        let engine = Arc::clone(&engine);
        let root = fx.root.clone();
        std::thread::spawn(move || {
            for i in 0..120 {
                let path = root.join(format!("live{i}.txt"));
                fs::write(&path, format!("fresh token{i} content")).unwrap();
                let seed = root.join(format!("seed{}.txt", i % 30));
                fs::write(&seed, format!("rewritten {i} times over, longer than before")).unwrap();
                engine.apply_changes(vec![path, seed]);
            }
        })
    };
    while !writer.is_finished() {
        engine.snapshot().unwrap();
    }
    writer.join().unwrap();
    engine.snapshot().unwrap();

    // Every file written is findable by name and by its last content
    assert_eq!(engine.doc_count(), 150);
    for i in [0, 59, 119] {
        assert_eq!(paths(&engine.search(&format!("token{i}"))), [format!("live{i}.txt")], "token{i}");
    }
    assert!(engine.search("original").is_empty(), "every seed file was rewritten");
    assert_eq!(engine.search("rewritten").len(), 30);

    // The same holds from disk alone
    engine.shutdown();
    let reopened = Engine::open(&fx.data_dir, fx.config()).unwrap();
    assert_eq!(reopened.load().unwrap(), (150, 0));
    assert_eq!(reopened.search("rewritten").len(), 30);
    assert_eq!(paths(&reopened.search("token119")), ["live119.txt"]);
}

#[test]
fn test_words_whose_stem_changes_when_stemmed_twice_are_found() {
    let fx = Fixture::new();
    fx.write("a.txt", "it was everywhere");
    fx.write("b.txt", "nothing to see");
    let engine = fx.indexed_engine();

    // "everywhere" stems to "everywher", which stems again to something else
    assert_eq!(paths(&engine.search("everywhere")), ["a.txt"]);
}
