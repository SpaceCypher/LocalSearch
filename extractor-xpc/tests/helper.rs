//! End-to-end tests of the real helper binary: protocol, parsers, sandbox.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

const HELPER: &str = env!("CARGO_BIN_EXE_localsearch-extractor");

/// Send paths to a fresh helper and return the text extracted for each
/// (None where the helper answered ERR).
fn extract(paths: &[&Path]) -> Vec<Option<String>> {
    let mut child = Command::new(HELPER)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("helper starts");
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    let mut results = Vec::new();
    for path in paths {
        writeln!(stdin, "{}", path.display()).unwrap();
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        let mut parts = header.split_whitespace();
        match parts.next() {
            Some("OK") => {
                let len: usize = parts.next().unwrap().parse().unwrap();
                let mut body = vec![0u8; len];
                reader.read_exact(&mut body).unwrap();
                results.push(Some(String::from_utf8(body).unwrap()));
            }
            _ => results.push(None),
        }
    }
    drop(stdin);
    assert!(child.wait().unwrap().success());
    results
}

/// A minimal one-page PDF whose text layer says "Hello Zebrafish"
fn sample_pdf() -> Vec<u8> {
    let stream = "BT /F1 18 Tf 20 100 Td (Hello Zebrafish) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 144] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{}\nendstream", stream.len(), stream),
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
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n", objects.len() + 1, xref).as_bytes());
    pdf
}

#[test]
fn sandboxed_helper_still_extracts_every_supported_format() {
    let dir = tempfile::tempdir().unwrap();
    let text = dir.path().join("notes.md");
    let pdf = dir.path().join("scan.pdf");
    let rtf = dir.path().join("letter.rtf");
    let broken = dir.path().join("broken.pdf");
    let missing = dir.path().join("missing.txt");
    std::fs::write(&text, "plain zebrafish").unwrap();
    std::fs::write(&pdf, sample_pdf()).unwrap();
    std::fs::write(&rtf, r"{\rtf1\ansi Rich {\b zebrafish} text}").unwrap();
    std::fs::write(&broken, b"not a pdf at all").unwrap();

    let results = extract(&[&text, &pdf, &rtf, &broken, &missing]);

    assert_eq!(results[0].as_deref(), Some("plain zebrafish"));
    assert!(results[1].as_deref().unwrap().contains("Hello Zebrafish"), "{:?}", results[1]);
    assert!(results[2].as_deref().unwrap().contains("Rich zebrafish text"), "{:?}", results[2]);
    // A malformed file is an empty answer, and the helper keeps serving after it
    assert_eq!(results[3].as_deref(), Some(""));
    assert_eq!(results[4], None);
}

#[test]
fn helper_cannot_write_files_or_reach_the_network() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(HELPER)
        .arg("--self-test")
        .arg(dir.path())
        .output()
        .expect("helper starts");
    let report = String::from_utf8_lossy(&output.stdout);

    assert!(report.contains("write: denied"), "{report}");
    assert!(report.contains("network: denied"), "{report}");
    assert!(report.contains("read: allowed"), "{report}");
    assert!(!dir.path().join("escaped.txt").exists());
}
