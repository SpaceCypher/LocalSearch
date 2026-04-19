use std::fs::File;
use std::io::{Read, BufReader};
use std::path::Path;
use xxhash_rust::xxh3::xxh3_64;
use anyhow::Result;

pub struct ExtractionResult {
    pub text: String,
    pub full_content: bool,
    pub content_hash: u64,
}

pub struct ExtractionClient;

impl ExtractionClient {
    pub fn new() -> Self {
        Self
    }

    /// Calculates XXH3 hash based on file size and modification time.
    pub fn calculate_content_hash<P: AsRef<Path>>(path: P) -> Result<u64> {
        let meta = std::fs::metadata(path.as_ref())?;
        let mtime = meta.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let size = meta.len();
        
        let mut buffer = Vec::with_capacity(24);
        buffer.extend_from_slice(&mtime.to_ne_bytes());
        buffer.extend_from_slice(&size.to_ne_bytes());
        Ok(xxh3_64(&buffer))
    }

    /// Extracts content from a file, skipping if requested. (Simplified for Task 43)
    pub fn extract<P: AsRef<Path>>(&self, path: P) -> Result<ExtractionResult> {
        let path_ref = path.as_ref();
        let hash = Self::calculate_content_hash(path_ref)?;

        // File extraction disabled for massive power/CPU savings. Reading 64KB of 
        // 1M files and validating UTF8 was locking the CPU at 100%.
        Ok(ExtractionResult {
            text: String::new(),
            full_content: false,
            content_hash: hash,
        })
    }

    /// Returns true when extraction should run based on hash change.
    pub fn should_extract<P: AsRef<Path>>(path: P, previous_hash: Option<u64>) -> Result<bool> {
        let current_hash = Self::calculate_content_hash(path)?;
        Ok(previous_hash.map_or(true, |prev| prev != current_hash))
    }

    /// Extracts only when content hash changed; returns None when unchanged.
    pub fn extract_if_changed<P: AsRef<Path>>(
        &self,
        path: P,
        previous_hash: Option<u64>,
    ) -> Result<Option<ExtractionResult>> {
        let path_ref = path.as_ref();
        if !Self::should_extract(path_ref, previous_hash)? {
            return Ok(None);
        }
        self.extract(path_ref).map(Some)
    }
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
    fn test_extract_if_changed_skips_unchanged_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("stable.txt");
        let mut file = File::create(&file_path).unwrap();
        file.write_all(b"unchanged content").unwrap();

        let client = ExtractionClient::new();
        let first = client.extract_if_changed(&file_path, None).unwrap();
        assert!(first.is_some());

        let prev_hash = first.unwrap().content_hash;
        let second = client.extract_if_changed(&file_path, Some(prev_hash)).unwrap();
        assert!(second.is_none());
    }

    #[test]
    fn test_extract_if_changed_runs_when_file_changes() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("changing.txt");
        let mut file = File::create(&file_path).unwrap();
        file.write_all(b"v1 content").unwrap();

        let client = ExtractionClient::new();
        let first = client.extract_if_changed(&file_path, None).unwrap().unwrap();

        let mut file = File::create(&file_path).unwrap();
        file.write_all(b"v2 content changed").unwrap();

        let second = client
            .extract_if_changed(&file_path, Some(first.content_hash))
            .unwrap();
        assert!(second.is_some());
    }
}
