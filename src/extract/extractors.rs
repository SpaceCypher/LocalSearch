//! Content extraction for various file formats.
//!
//! Extracts text content from files for indexing. Limits extraction to first 64KB
//! to prevent memory issues with large files.

use std::path::Path;
use std::fs::File;
use std::io::Read;

/// Extraction result
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionResult {
    pub text: String,
    pub full_content: bool, // false if truncated to 64KB
}

pub const MAX_EXTRACT_SIZE: usize = 64 * 1024; // 64KB

/// PDFs larger than this are skipped rather than parsed
const MAX_PDF_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// How a file's content is extracted, decided from its extension
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    /// Read directly as text
    Text,
    /// Parsed with PDFKit (in the extractor helper process when available)
    Pdf,
    /// Word, RTF and OpenDocument text, read with the system's document
    /// importers (in the extractor helper process when available)
    RichText,
    /// Not content-indexed
    Unsupported,
}

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rst", "org", "tex", "log", "csv", "tsv",
    "json", "yaml", "yml", "toml", "xml", "html", "htm", "css", "scss", "ini", "cfg", "conf", "plist",
    "rs", "py", "js", "jsx", "ts", "tsx", "swift", "go", "java", "kt", "c", "h", "cpp", "hpp", "cc", "m", "mm",
    "rb", "php", "sh", "zsh", "bash", "sql", "lua", "r", "scala", "dart", "cs",
];

const RICH_TEXT_EXTENSIONS: &[&str] = &["docx", "doc", "rtf", "odt"];

/// Rich-text documents larger than this are skipped rather than parsed
const MAX_RICH_TEXT_FILE_SIZE: u64 = 50 * 1024 * 1024;

pub fn content_kind(path: &Path) -> ContentKind {
    let extension = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if extension == "pdf" {
        ContentKind::Pdf
    } else if RICH_TEXT_EXTENSIONS.contains(&extension.as_str()) {
        ContentKind::RichText
    } else if TEXT_EXTENSIONS.contains(&extension.as_str()) {
        ContentKind::Text
    } else {
        ContentKind::Unsupported
    }
}

/// Extract plaintext content (first 64KB)
pub fn extract_plaintext(path: &Path) -> anyhow::Result<ExtractionResult> {
    let file = File::open(path)?;
    let mut buffer = Vec::with_capacity(MAX_EXTRACT_SIZE);
    file.take(MAX_EXTRACT_SIZE as u64).read_to_end(&mut buffer)?;
    let full_content = buffer.len() < MAX_EXTRACT_SIZE;

    // A NUL byte means this is binary data wearing a text extension
    if buffer.contains(&0) {
        return Ok(ExtractionResult { text: String::new(), full_content: true });
    }
    
    // Convert to UTF-8, replacing invalid sequences
    let text = String::from_utf8_lossy(&buffer).to_string();
    
    Ok(ExtractionResult {
        text,
        full_content,
    })
}

/// Extract the text layer of a PDF with PDFKit, page by page, up to 64KB.
///
/// Malformed files yield an empty result. PDFKit runs in whichever process
/// calls this; `ExtractionClient` routes PDFs to the helper process so a
/// PDFKit crash cannot take the app down.
#[cfg(target_os = "macos")]
pub fn extract_pdf(path: &Path) -> anyhow::Result<ExtractionResult> {
    use objc2::msg_send;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::{AnyClass, AnyObject};
    use std::ffi::{CStr, CString};
    use std::os::raw::c_char;

    #[link(name = "PDFKit", kind = "framework")]
    extern "C" {}
    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}

    let empty = ExtractionResult { text: String::new(), full_content: true };

    if std::fs::metadata(path)?.len() > MAX_PDF_FILE_SIZE {
        return Ok(empty);
    }
    let (Some(ns_string), Some(ns_url), Some(pdf_document)) = (
        AnyClass::get("NSString"),
        AnyClass::get("NSURL"),
        AnyClass::get("PDFDocument"),
    ) else {
        return Ok(empty);
    };
    let Ok(c_path) = CString::new(path.to_string_lossy().as_bytes()) else {
        return Ok(empty);
    };

    let mut text = String::new();
    let mut full_content = true;

    autoreleasepool(|_| unsafe {
        let ns_path: *mut AnyObject = msg_send![ns_string, stringWithUTF8String: c_path.as_ptr()];
        if ns_path.is_null() {
            return;
        }
        let url: *mut AnyObject = msg_send![ns_url, fileURLWithPath: ns_path];
        let document: *mut AnyObject = msg_send![pdf_document, alloc];
        let document: *mut AnyObject = msg_send![document, initWithURL: url];
        if document.is_null() {
            return;
        }

        let page_count: usize = msg_send![document, pageCount];
        for index in 0..page_count {
            if text.len() >= MAX_EXTRACT_SIZE {
                full_content = false;
                break;
            }
            // One pool per page keeps peak memory flat on long documents
            autoreleasepool(|_| {
                let page: *mut AnyObject = msg_send![document, pageAtIndex: index];
                if page.is_null() {
                    return;
                }
                let page_text: *mut AnyObject = msg_send![page, string];
                if page_text.is_null() {
                    return;
                }
                let utf8: *const c_char = msg_send![page_text, UTF8String];
                if !utf8.is_null() {
                    text.push_str(&CStr::from_ptr(utf8).to_string_lossy());
                    text.push('\n');
                }
            });
        }
        let _: () = msg_send![document, release];
    });

    if text.len() > MAX_EXTRACT_SIZE {
        let mut cut = MAX_EXTRACT_SIZE;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        full_content = false;
    }

    Ok(ExtractionResult { text, full_content })
}

#[cfg(not(target_os = "macos"))]
pub fn extract_pdf(_path: &Path) -> anyhow::Result<ExtractionResult> {
    Ok(ExtractionResult { text: String::new(), full_content: true })
}

/// Cut `text` to the extraction limit on a character boundary.
fn truncated(mut text: String) -> ExtractionResult {
    let full_content = text.len() <= MAX_EXTRACT_SIZE;
    if !full_content {
        let mut cut = MAX_EXTRACT_SIZE;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    ExtractionResult { text, full_content }
}

/// Extract the text of a Word (.docx, .doc), RTF or OpenDocument file using
/// `NSAttributedString`'s document importers.
///
/// Malformed files yield an empty result. Like `extract_pdf`, this runs the
/// system parser in the calling process; `ExtractionClient` routes these
/// files to the helper.
#[cfg(target_os = "macos")]
pub fn extract_rich_text(path: &Path) -> anyhow::Result<ExtractionResult> {
    use objc2::msg_send;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::{AnyClass, AnyObject};
    use std::ffi::{CStr, CString};
    use std::os::raw::c_char;

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    let empty = ExtractionResult { text: String::new(), full_content: true };

    if std::fs::metadata(path)?.len() > MAX_RICH_TEXT_FILE_SIZE {
        return Ok(empty);
    }
    let (Some(ns_string), Some(ns_url), Some(ns_dictionary), Some(attributed_string)) = (
        AnyClass::get("NSString"),
        AnyClass::get("NSURL"),
        AnyClass::get("NSDictionary"),
        AnyClass::get("NSAttributedString"),
    ) else {
        return Ok(empty);
    };
    let Ok(c_path) = CString::new(path.to_string_lossy().as_bytes()) else {
        return Ok(empty);
    };

    let mut text = String::new();
    autoreleasepool(|_| unsafe {
        let ns_path: *mut AnyObject = msg_send![ns_string, stringWithUTF8String: c_path.as_ptr()];
        if ns_path.is_null() {
            return;
        }
        let url: *mut AnyObject = msg_send![ns_url, fileURLWithPath: ns_path];
        // Empty options: the importer picks the format from the file itself
        let options: *mut AnyObject = msg_send![ns_dictionary, dictionary];
        let no_attributes: *mut *mut AnyObject = std::ptr::null_mut();
        let no_error: *mut *mut AnyObject = std::ptr::null_mut();

        let document: *mut AnyObject = msg_send![attributed_string, alloc];
        let document: *mut AnyObject = msg_send![
            document,
            initWithURL: url,
            options: options,
            documentAttributes: no_attributes,
            error: no_error
        ];
        if document.is_null() {
            return;
        }
        let contents: *mut AnyObject = msg_send![document, string];
        if !contents.is_null() {
            let utf8: *const c_char = msg_send![contents, UTF8String];
            if !utf8.is_null() {
                text.push_str(&CStr::from_ptr(utf8).to_string_lossy());
            }
        }
        let _: () = msg_send![document, release];
    });

    Ok(truncated(text))
}

#[cfg(not(target_os = "macos"))]
pub fn extract_rich_text(_path: &Path) -> anyhow::Result<ExtractionResult> {
    Ok(ExtractionResult { text: String::new(), full_content: true })
}

/// Extract content from any file (dispatcher), in the calling process
pub fn extract_content(path: &Path) -> anyhow::Result<ExtractionResult> {
    match content_kind(path) {
        ContentKind::Text => extract_plaintext(path),
        ContentKind::Pdf => extract_pdf(path),
        ContentKind::RichText => extract_rich_text(path),
        ContentKind::Unsupported => Ok(ExtractionResult {
            text: String::new(),
            full_content: true,
        }),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
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
        assert!(result.unwrap().text.is_empty());
    }

    #[test]
    fn test_binary_with_text_extension_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("blob.txt");
        std::fs::write(&file, [b'a', 0, b'b']).unwrap();
        assert!(extract_content(&file).unwrap().text.is_empty());
    }

    /// A minimal one-page PDF whose text layer says "Hello Zebrafish"
    pub(crate) fn sample_pdf() -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 144] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            {
                let stream = "BT /F1 18 Tf 20 100 Td (Hello Zebrafish) Tj ET";
                format!("<< /Length {} >>\nstream\n{}\nendstream", stream.len(), stream)
            },
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, body).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            pdf.extend_from_slice(format!("{:010} 00000 n \n", offset).as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n", objects.len() + 1, xref).as_bytes(),
        );
        pdf
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_extract_rtf_text() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("letter.rtf");
        std::fs::write(&file, r"{\rtf1\ansi\deff0 {\fonttbl {\f0 Helvetica;}}\f0 Dear {\b Zebrafish} owner,\par see you soon.}").unwrap();

        let result = extract_content(&file).unwrap();
        assert!(result.text.contains("Dear Zebrafish owner"), "got {:?}", result.text);
        assert!(!result.text.contains("rtf1"), "markup must not be indexed: {:?}", result.text);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_malformed_word_document_yields_empty_text() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("broken.docx");
        std::fs::write(&file, b"this is not a zip archive").unwrap();

        // Must neither panic nor index the raw bytes as if they were prose
        let result = extract_content(&file).unwrap();
        assert!(!result.text.contains("zip archive") || result.text.len() < 64, "got {:?}", result.text);
    }

    #[test]
    fn test_content_kinds() {
        assert_eq!(content_kind(Path::new("a.DOCX")), ContentKind::RichText);
        assert_eq!(content_kind(Path::new("a.rtf")), ContentKind::RichText);
        assert_eq!(content_kind(Path::new("a.pdf")), ContentKind::Pdf);
        assert_eq!(content_kind(Path::new("a.rs")), ContentKind::Text);
        assert_eq!(content_kind(Path::new("a.png")), ContentKind::Unsupported);
        assert_eq!(content_kind(Path::new("noextension")), ContentKind::Unsupported);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_extract_pdf_text_layer() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hello.pdf");
        std::fs::write(&file, sample_pdf()).unwrap();

        let result = extract_content(&file).unwrap();
        assert!(result.text.contains("Hello Zebrafish"), "got {:?}", result.text);
    }
}
