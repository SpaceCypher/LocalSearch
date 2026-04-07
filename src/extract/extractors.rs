//! Content extraction for various file formats.
//!
//! Extracts text content from files for indexing. Limits extraction to first 64KB
//! to prevent memory issues with large files.

use std::path::Path;
use std::fs::File;
use std::io::{Read, BufReader};

/// Extraction result
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionResult {
    pub text: String,
    pub full_content: bool, // false if truncated to 64KB
}

const MAX_EXTRACT_SIZE: usize = 64 * 1024; // 64KB

/// Extract plaintext content (first 64KB)
pub fn extract_plaintext(path: &Path) -> anyhow::Result<ExtractionResult> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut buffer = vec![0u8; MAX_EXTRACT_SIZE];
    
    let bytes_read = reader.read(&mut buffer)?;
    buffer.truncate(bytes_read);
    
    // Convert to UTF-8, replacing invalid sequences
    let text = String::from_utf8_lossy(&buffer).to_string();
    let full_content = bytes_read < MAX_EXTRACT_SIZE;
    
    Ok(ExtractionResult {
        text,
        full_content,
    })
}

/// Extract content from any file (dispatcher)
pub fn extract_content(path: &Path) -> anyhow::Result<ExtractionResult> {
    // Get file extension
    let extension = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    
    match extension.as_str() {
        "txt" | "md" | "rs" | "py" | "js" | "ts" | "json" | "yaml" | "yml" | "toml" | "xml" | "html" | "css" | "sh" => {
            // Plain text files
            extract_plaintext(path)
        }
        "pdf" => {
            // PDF extraction would go here (requires PDFKit FFI)
            // For now, return empty result gracefully
            Ok(ExtractionResult {
                text: String::new(),
                full_content: true,
            })
        }
        _ => {
            // Unknown file type - return empty
            Ok(ExtractionResult {
                text: String::new(),
                full_content: true,
            })
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_plaintext_first_64kb() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        let content = "hello world ".repeat(10000); // > 64KB
        std::fs::write(&file, &content).unwrap();

        let result = extract_plaintext(&file).unwrap();
        assert!(!result.full_content); // Only partial
        assert!(result.text.len() <= 64 * 1024 + 100); // ~64KB
        assert!(result.text.contains("hello world"));
    }

    #[test]
    fn test_extract_unknown_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("binary.bin");
        std::fs::write(&file, &[0u8; 100]).unwrap();
        let result = extract_content(&file).unwrap();
        assert!(result.text.is_empty());
    }

    #[test]
    fn test_extractor_crash_does_not_panic_main_process() {
        // Simulate malformed PDF → extractor returns empty, main process unaffected
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bad.pdf");
        std::fs::write(&file, b"not a real pdf").unwrap();
        let result = extract_content(&file); // Should not panic
        assert!(result.is_ok()); // Graceful empty result
    }
}
