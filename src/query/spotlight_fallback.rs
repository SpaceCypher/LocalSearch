use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use xxhash_rust::xxh3::xxh3_64;
use crate::index::delta::DocId;
use crate::query::executor::SearchResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultSource {
    LocalIndex,
    Spotlight,
}

const MAX_SPOTLIGHT_RESULTS: usize = 20;

/// Asks Spotlight (`mdfind`) for filename matches when the local index has
/// nothing to offer, e.g. while it is still being built.
///
/// Disabled by default so that index-only callers (and unit tests) never
/// spawn a process; the engine enables it and sets the search roots.
pub struct SpotlightFallback {
    enabled: bool,
    roots: Vec<PathBuf>,
}

impl SpotlightFallback {
    pub fn new() -> Self {
        Self { enabled: false, roots: Vec::new() }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Restrict results to these directories (empty = whole machine)
    pub fn set_roots(&mut self, roots: Vec<PathBuf>) {
        self.roots = roots;
    }

    /// Filename search through Spotlight, capped at `MAX_SPOTLIGHT_RESULTS`.
    pub fn query(&self, query_str: &str) -> Vec<SearchResult> {
        let query_str = query_str.trim();
        if !self.enabled || query_str.is_empty() || !cfg!(target_os = "macos") {
            return Vec::new();
        }

        let mut command = Command::new("/usr/bin/mdfind");
        for root in &self.roots {
            command.arg("-onlyin").arg(root);
        }
        command.arg("-name").arg(query_str);

        let Ok(mut child) = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return Vec::new();
        };

        let mut results = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.is_empty() {
                    continue;
                }
                results.push(SearchResult {
                    // Spotlight hits have no DocId of ours; derive a stable one from the path
                    doc_id: DocId(xxh3_64(line.as_bytes())),
                    path: line,
                    score: 0.5,
                    source: ResultSource::Spotlight,
                });
                if results.len() >= MAX_SPOTLIGHT_RESULTS {
                    break;
                }
            }
        }
        // We may have stopped reading early; don't leave mdfind running.
        let _ = child.kill();
        let _ = child.wait();

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disabled_fallback_returns_nothing() {
        let fallback = SpotlightFallback::new();
        assert!(fallback.query("anything").is_empty());
    }

    #[test]
    fn test_empty_query_returns_nothing() {
        let mut fallback = SpotlightFallback::new();
        fallback.set_enabled(true);
        assert!(fallback.query("   ").is_empty());
    }
}
