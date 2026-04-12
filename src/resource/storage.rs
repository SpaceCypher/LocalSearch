use std::fmt;

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
        // Modern macOS machines are almost exclusively SSD.
        // A production-grade implementation would use IOKit's kIOPropertyDeviceCharacteristicsKey
        // to check for the 'Rotational' attribute (HDD) vs 'Non-rotational' (SSD).
        StorageType::SSD
    }
    #[cfg(not(target_os = "macos"))]
    {
        StorageType::Unknown
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_type_batch_sizes() {
        assert_eq!(get_compaction_batch_size(StorageType::SSD), 4 * 1024 * 1024);
        assert_eq!(get_compaction_batch_size(StorageType::HDD), 1 * 1024 * 1024);
    }

    #[test]
    fn test_detection_on_current_os() {
        let st = detect_storage_type();
        #[cfg(target_os = "macos")]
        assert_eq!(st, StorageType::SSD);
        
        #[cfg(not(target_os = "macos"))]
        assert_eq!(st, StorageType::Unknown);
    }
}
