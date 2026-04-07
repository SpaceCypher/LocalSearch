// Index migration and versioning system

use std::path::{Path, PathBuf};
use anyhow::Result;

#[derive(Debug, Clone, PartialEq)]
pub enum MigrationPlan {
    None,
    Additive,
    FullReindex { index_dir: PathBuf },
}

/// Check compatibility between index version and reader version
pub fn check_compatibility(index_version: u32, reader_version: u32) -> Result<MigrationPlan> {
    if index_version == reader_version {
        Ok(MigrationPlan::None)
    } else if index_version > reader_version {
        // Index is newer but only has additive changes
        Ok(MigrationPlan::Additive)
    } else {
        // Index is older, needs full reindex
        Ok(MigrationPlan::FullReindex {
            index_dir: PathBuf::from("index"),
        })
    }
}

/// Execute migration plan with backup logic
pub fn execute_migration(plan: MigrationPlan) -> Result<()> {
    match plan {
        MigrationPlan::None => Ok(()),
        MigrationPlan::Additive => Ok(()),
        MigrationPlan::FullReindex { index_dir } => {
            // Create backup before reindexing
            let backup_dir = index_dir.parent()
                .unwrap_or_else(|| Path::new("."))
                .join("index.v0.backup");
            
            if index_dir.exists() {
                std::fs::rename(&index_dir, &backup_dir)?;
            }
            
            // Create new index directory
            std::fs::create_dir_all(&index_dir)?;
            
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migration_none_when_versions_match() {
        let plan = check_compatibility(1, 1).unwrap();
        assert_eq!(plan, MigrationPlan::None);
    }

    #[test]
    fn test_migration_additive_is_transparent() {
        // Reader version 1 opens index version 2 (additive only) — should succeed
        let plan = check_compatibility(2, 1).unwrap();
        assert_eq!(plan, MigrationPlan::Additive);
    }

    #[test]
    fn test_migration_breaking_triggers_backup() {
        // Simulate Tier 3 migration
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("index");
        std::fs::create_dir(&index_dir).unwrap();
        // Create fake old-format segment
        std::fs::write(index_dir.join("seg_0.lssg"), b"old").unwrap();

        let result = execute_migration(MigrationPlan::FullReindex { index_dir: index_dir.clone() });
        assert!(result.is_ok());
        // Backup should exist
        assert!(dir.path().join("index.v0.backup").exists());
    }
}
