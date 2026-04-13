use std::fmt;
#[cfg(target_os = "macos")]
use std::process::Command;

/// Represents the physical storage medium of the drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageType {
    /// Solid State Drive — high random I/O performance.
    SSD,
    /// Hard Disk Drive — slow random I/O, better as cold storage.
    HDD,
    /// Unknown or network storage.
    Unknown,
}

impl fmt::Display for StorageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageType::SSD => write!(f, "SSD"),
            StorageType::HDD => write!(f, "HDD"),
            StorageType::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Detects the storage type for the system drive.
pub fn detect_storage_type() -> StorageType {
    #[cfg(target_os = "macos")]
    {
        detect_storage_type_macos()
    }
    #[cfg(not(target_os = "macos"))]
    {
        StorageType::Unknown
    }
}

#[cfg(target_os = "macos")]
fn detect_storage_type_macos() -> StorageType {
    // Use diskutil text output to avoid a hard IOKit dependency in this crate.
    let output = Command::new("diskutil")
        .arg("info")
        .arg("/")
        .output();

    let Ok(output) = output else {
        return StorageType::Unknown;
    };

    if !output.status.success() {
        return StorageType::Unknown;
    }

    let text = String::from_utf8_lossy(&output.stdout).to_lowercase();

    if text.contains("solid state: yes") || text.contains("rotation rate: 0") {
        return StorageType::SSD;
    }

    if text.contains("solid state: no") || text.contains("rotation rate:") {
        return StorageType::HDD;
    }

    StorageType::Unknown
}

/// Returns the optimal batch size for background indexing/compaction based on storage type.
/// SSDs benefit from larger batches (higher throughput), while HDDs prefer smaller
/// batches to minimize seek contention.
pub fn get_compaction_batch_size(storage: StorageType) -> usize {
    match storage {
        StorageType::SSD => 4 * 1024 * 1024, // 4MB
        StorageType::HDD => 1024 * 1024,     // 1MB
        StorageType::Unknown => 2 * 1024 * 1024,
    }
}

/// Recommended read chunk when prefetching cold segment bytes.
pub fn cold_segment_read_chunk(storage: StorageType) -> usize {
    match storage {
        StorageType::SSD => 256 * 1024,
        StorageType::HDD => 64 * 1024,
        StorageType::Unknown => 128 * 1024,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_type_batch_sizes() {
        assert_eq!(get_compaction_batch_size(StorageType::SSD), 4 * 1024 * 1024);
        assert_eq!(get_compaction_batch_size(StorageType::HDD), 1 * 1024 * 1024);
        assert_eq!(cold_segment_read_chunk(StorageType::SSD), 256 * 1024);
        assert_eq!(cold_segment_read_chunk(StorageType::HDD), 64 * 1024);
    }

    #[test]
    fn test_detection_on_current_os() {
        let st = detect_storage_type();
        #[cfg(target_os = "macos")]
        assert!(matches!(st, StorageType::SSD | StorageType::HDD | StorageType::Unknown));
        
        #[cfg(not(target_os = "macos"))]
        assert_eq!(st, StorageType::Unknown);
    }
}
