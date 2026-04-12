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

    /// Calculates XXH3 hash of the first 64KB of a file.
    pub fn calculate_content_hash<P: AsRef<Path>>(path: P) -> Result<u64> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut buffer = [0u8; 65536]; // 64KB
        let n = reader.read(&mut buffer)?;
        Ok(xxh3_64(&buffer[..n]))
    }

    /// Extracts content from a file, skipping if requested. (Simplified for Task 43)
    pub fn extract<P: AsRef<Path>>(&self, path: P) -> Result<ExtractionResult> {
        let path_ref = path.as_ref();
        let hash = Self::calculate_content_hash(path_ref)?;
        
        // This is a placeholder for the actual extraction logic (from Task 25)
        // In a real implementation, this would call specialized extractors.
        let mut file = File::open(path_ref)?;
        let mut text = String::new();
        let n = file.read_to_string(&mut text)?;
        
        Ok(ExtractionResult {
            text,
            full_content: n < 65536,
            content_hash: hash,
        })
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
}
