use std::process::Command;
use crate::index::delta::DocId;
use crate::query::executor::SearchResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultSource {
    LocalIndex,
    Spotlight,
}

pub struct SpotlightFallback {
    max_results: usize,
}

impl SpotlightFallback {
    pub fn new() -> Self {
        Self { max_results: 20 }
    }

    /// Queries Spotlight using `mdfind` and maps paths into fallback search results.
    pub fn query(&self, query_str: &str) -> Vec<SearchResult> {
        if query_str.trim().is_empty() {
            return Vec::new();
        }

        #[cfg(target_os = "macos")]
        {
            let output = Command::new("mdfind").arg(query_str).output();
            if let Ok(output) = output {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    return parse_mdfind_output(&stdout, self.max_results)
                        .into_iter()
                        .enumerate()
                        .map(|(idx, path)| SearchResult {
                            doc_id: DocId(900_000 + idx as u64),
                            path,
                            score: 0.5,
                            source: ResultSource::Spotlight,
                        })
                        .collect();
                }
            }
        }

        Vec::new()
    }
}

fn parse_mdfind_output(output: &str, max_results: usize) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(max_results)
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mdfind_output_limits_results() {
        let raw = "/a\n/b\n/c\n";
        let parsed = parse_mdfind_output(raw, 2);
        assert_eq!(parsed, vec!["/a".to_string(), "/b".to_string()]);
    }

    #[test]
    fn test_spotlight_fallback_marks_source() {
        let fallback = SpotlightFallback::new();
        let results = fallback.query("spotlight");
        assert!(results.iter().all(|r| r.source == ResultSource::Spotlight));
    }
}
