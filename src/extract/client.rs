use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;
use xxhash_rust::xxh3::xxh3_64;
use anyhow::Result;

use crate::extract::extractors::{self, ContentKind};

pub struct ExtractionResult {
    pub text: String,
    pub full_content: bool,
    pub content_hash: u64,
}

/// How long the helper gets per file before it is killed and restarted
const HELPER_TIMEOUT: Duration = Duration::from_secs(15);

/// Name of the helper binary built from the `extractor-xpc` crate
pub const HELPER_BINARY: &str = "localsearch-extractor";

struct Helper {
    child: Child,
    stdin: ChildStdin,
    /// One message per response, sent by a reader thread so we can time out
    responses: Receiver<Option<(String, bool)>>,
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Extracts file content for indexing.
///
/// Plain text is read in-process. Formats that need a system parser (PDF via
/// PDFKit) are sent to the `localsearch-extractor` helper process, so a parser
/// crash or hang costs one file rather than the app. If the helper binary
/// cannot be found, those formats are parsed in-process instead.
pub struct ExtractionClient {
    helper_path: Option<PathBuf>,
    helper: Option<Helper>,
}

impl ExtractionClient {
    pub fn new() -> Self {
        let helper_path = locate_helper();
        if helper_path.is_none() {
            log::warn!("{HELPER_BINARY} not found; PDFs will be parsed in-process");
        }
        Self { helper_path, helper: None }
    }

    /// Use a specific helper binary (or none, forcing in-process extraction)
    pub fn with_helper(helper_path: Option<PathBuf>) -> Self {
        Self { helper_path, helper: None }
    }

    pub fn uses_helper(&self) -> bool {
        self.helper_path.is_some()
    }

    /// Calculates XXH3 hash of the first 64KB of a file.
    pub fn calculate_content_hash<P: AsRef<Path>>(path: P) -> Result<u64> {
        let file = File::open(path)?;
        let mut buffer = Vec::with_capacity(65536);
        file.take(65536).read_to_end(&mut buffer)?;
        Ok(xxh3_64(&buffer))
    }

    /// Extracts the indexable text of a file. Unsupported, unreadable-as-text
    /// and unparseable files yield empty text rather than an error.
    pub fn extract<P: AsRef<Path>>(&mut self, path: P) -> Result<ExtractionResult> {
        let path_ref = path.as_ref();
        let content_hash = Self::calculate_content_hash(path_ref)?;

        let (text, full_content) = match extractors::content_kind(path_ref) {
            ContentKind::Unsupported => (String::new(), true),
            ContentKind::Text => {
                let result = extractors::extract_plaintext(path_ref)?;
                (result.text, result.full_content)
            }
            ContentKind::Pdf => {
                if self.helper_path.is_some() {
                    self.extract_via_helper(path_ref).unwrap_or_default()
                } else {
                    let result = extractors::extract_pdf(path_ref)?;
                    (result.text, result.full_content)
                }
            }
        };

        Ok(ExtractionResult { text, full_content, content_hash })
    }

    fn spawn_helper(&mut self) -> Option<&mut Helper> {
        if self.helper.is_none() {
            let mut child = Command::new(self.helper_path.as_ref()?)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| log::warn!("could not start extractor helper: {e}"))
                .ok()?;
            let stdin = child.stdin.take()?;
            let stdout = child.stdout.take()?;

            let (tx, responses) = mpsc::channel();
            std::thread::Builder::new()
                .name("extractor-reader".to_string())
                .spawn(move || {
                    let mut reader = BufReader::new(stdout);
                    while let Some(response) = read_response(&mut reader) {
                        if tx.send(response).is_err() {
                            break;
                        }
                    }
                })
                .ok()?;

            self.helper = Some(Helper { child, stdin, responses });
        }
        self.helper.as_mut()
    }

    /// Returns None if the helper crashed, hung, or reported failure; the
    /// helper is then discarded and respawned for the next file.
    fn extract_via_helper(&mut self, path: &Path) -> Option<(String, bool)> {
        let path_str = path.to_str()?;
        if path_str.contains('\n') {
            return None;
        }

        let helper = self.spawn_helper()?;
        let sent = writeln!(helper.stdin, "{path_str}").and_then(|_| helper.stdin.flush());
        let response = match sent {
            Ok(()) => helper.responses.recv_timeout(HELPER_TIMEOUT).ok(),
            Err(_) => None,
        };

        match response {
            Some(result) => result,
            None => {
                log::warn!("extractor helper failed on {path_str}; restarting it");
                self.helper = None;
                None
            }
        }
    }
}

/// Wire format, helper → client, one per request:
/// `OK <byte_len> <full:0|1>\n<bytes>` or `ERR\n`.
/// Outer None = stream closed; inner None = helper reported an error.
fn read_response(reader: &mut impl BufRead) -> Option<Option<(String, bool)>> {
    let mut header = String::new();
    if reader.read_line(&mut header).ok()? == 0 {
        return None;
    }
    let mut parts = header.split_whitespace();
    match parts.next()? {
        "OK" => {
            let len: usize = parts.next()?.parse().ok()?;
            let full = parts.next()? == "1";
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).ok()?;
            Some(Some((String::from_utf8_lossy(&body).into_owned(), full)))
        }
        _ => Some(None),
    }
}

/// Serve extraction requests on stdin/stdout until stdin closes.
/// This is the body of the `localsearch-extractor` helper process.
pub fn serve_helper(input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        match extractors::extract_content(Path::new(&line)) {
            Ok(result) => {
                writeln!(output, "OK {} {}", result.text.len(), result.full_content as u8)?;
                output.write_all(result.text.as_bytes())?;
            }
            Err(_) => writeln!(output, "ERR")?,
        }
        output.flush()?;
    }
    Ok(())
}

/// Find the helper: explicit env var, then next to the running executable
/// (app bundle `Contents/MacOS`, or `target/<profile>` during development).
fn locate_helper() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("LOCALSEARCH_EXTRACTOR_PATH") {
        let path = PathBuf::from(explicit);
        return path.is_file().then_some(path);
    }

    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [
        dir.join(HELPER_BINARY),
        dir.join("../Helpers").join(HELPER_BINARY),
        // Test binaries live in target/<profile>/deps
        dir.join("..").join(HELPER_BINARY),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_content_hash_consistency() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        let mut file = File::create(&file_path).unwrap();
        file.write_all(b"hello world").unwrap();

        let hash1 = ExtractionClient::calculate_content_hash(&file_path).unwrap();
        let hash2 = ExtractionClient::calculate_content_hash(&file_path).unwrap();
        assert_eq!(hash1, hash2);

        let mut file = File::create(&file_path).unwrap();
        file.write_all(b"hello rust").unwrap();
        let hash3 = ExtractionClient::calculate_content_hash(&file_path).unwrap();
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_extract_text_in_process() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("notes.md");
        std::fs::write(&file_path, "meeting about zebrafish").unwrap();

        let mut client = ExtractionClient::with_helper(None);
        let result = client.extract(&file_path).unwrap();
        assert_eq!(result.text, "meeting about zebrafish");
        assert!(result.full_content);
    }

    #[test]
    fn test_helper_protocol_roundtrip() {
        let dir = tempdir().unwrap();
        let good = dir.path().join("a.txt");
        std::fs::write(&good, "first\nsecond line").unwrap();
        let missing = dir.path().join("missing.txt");

        let requests = format!("{}\n{}\n", good.display(), missing.display());
        let mut wire = Vec::new();
        serve_helper(requests.as_bytes(), &mut wire).unwrap();

        let mut reader = BufReader::new(wire.as_slice());
        assert_eq!(
            read_response(&mut reader),
            Some(Some(("first\nsecond line".to_string(), true)))
        );
        assert_eq!(read_response(&mut reader), Some(None));
        assert_eq!(read_response(&mut reader), None);
    }

    #[test]
    fn test_dead_helper_yields_empty_text_not_error() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("doc.pdf");
        std::fs::write(&file_path, b"not a real pdf").unwrap();

        // `/usr/bin/true` exits immediately without answering: a crashed helper.
        let mut client = ExtractionClient::with_helper(Some(PathBuf::from("/usr/bin/true")));
        let result = client.extract(&file_path).unwrap();
        assert!(result.text.is_empty());
    }
}
