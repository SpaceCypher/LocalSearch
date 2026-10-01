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

#[test]
fn test_corrupt_snapshot_falls_back_to_rescan() {
    let fx = Fixture::new();
    fx.write("survivor.txt", "");
    {
        let engine = fx.indexed_engine();
        engine.shutdown();
    }
    let segment = fs::read_dir(&fx.data_dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "seg"))
        .unwrap();
    let mut bytes = fs::read(&segment).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xFF;
    fs::write(&segment, bytes).unwrap();

    let engine = Engine::open(&fx.data_dir, fx.config()).unwrap();
    assert_eq!(engine.load().unwrap().0, 0);
    engine.reconcile();
    assert_eq!(paths(&engine.search("survivor")), ["survivor.txt"]);
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
fn test_memory_budget_stops_content_indexing() {
    let fx = Fixture::new();
    for i in 0..40 {
        let words: String = (0..400).map(|w| format!("word{i}x{w} ")).collect();
        fx.write(&format!("doc{i}.txt"), &words);
    }
    let engine = fx.indexed_engine();
    assert!(!engine.status().budget_exhausted);

    // Same corpus, but pretend the index is already at its ceiling
    let fx2 = Fixture::new();
    for i in 0..40 {
        fx2.write(&format!("doc{i}.txt"), "some words here");
    }
    let engine = Engine::open(&fx2.data_dir, fx2.config()).unwrap();
    engine.load().unwrap();
    engine.reconcile();
    engine.state.write().unwrap().executor.delta_index.set_memory_budget(1);
    engine.index_content(|| {});

    assert!(engine.status().budget_exhausted);
    // Names are still searchable; content is not
    assert_eq!(paths(&engine.search("doc7"))[0], "doc7.txt");
    assert!(engine.search("words").is_empty());
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
fn test_old_format_snapshot_is_rebuilt_not_trusted() {
    let fx = Fixture::new();
    fx.write("kept.txt", "");
    {
        let engine = fx.indexed_engine();
        engine.shutdown();
    }
    // Rewrite the header as an older format version
    let segment = fs::read_dir(&fx.data_dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "seg"))
        .unwrap();
    let mut bytes = fs::read(&segment).unwrap();
    bytes[..8].copy_from_slice(b"LSSEG002");
    fs::write(&segment, bytes).unwrap();

    let engine = Engine::open(&fx.data_dir, fx.config()).unwrap();
    assert_eq!(engine.load().unwrap().0, 0);
    engine.reconcile();
    assert_eq!(paths(&engine.search("kept")), ["kept.txt"]);
}

#[cfg(target_os = "macos")]
#[test]
fn test_finds_word_document_by_content() {
    let fx = Fixture::new();
    // Make a real .docx with the system converter
    let source = fx.data_dir.join("source.txt");
    fs::write(&source, "Quarterly notes about the capybara enclosure.").unwrap();
    let docx = fx.root.join("minutes.docx");
    let converted = std::process::Command::new("/usr/bin/textutil")
        .args(["-convert", "docx", "-output"])
        .arg(&docx)
        .arg(&source)
        .status()
        .expect("textutil runs");
    assert!(converted.success() && docx.exists());
    let engine = fx.indexed_engine();

    assert_eq!(paths(&engine.search("capybara")), ["minutes.docx"]);
    let snippet = engine.snippet(docx.to_str().unwrap(), "capybara").unwrap();
    assert_eq!(marked(&snippet), "Quarterly notes about the [capybara] enclosure.");
}
