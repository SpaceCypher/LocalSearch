use std::path::Path;
use rusqlite::{Connection, params, OptionalExtension};
use anyhow::Result;

// ─── DocId ────────────────────────────────────────────────────────────────────

/// Stable document identifier, never reused.
///
/// DocIds are monotonically increasing 64-bit integers allocated from a
/// persistent counter. Once allocated, a DocId is never reused, even after
/// the file is deleted. This ensures stable references across the index.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocId(pub u64);

// ─── FileIdentity ─────────────────────────────────────────────────────────────

/// Filesystem identity tuple (not stable across renames).
///
/// This tuple uniquely identifies a file on disk at a point in time.
/// The `generation` field detects inode reuse: when a file is deleted and
/// a new file reuses the same inode, the generation counter increments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    pub volume_uuid: u128,
    pub inode:       u64,
    pub generation:  u32,
    pub device_id:   u32,
}

// ─── IdentityDb ───────────────────────────────────────────────────────────────

/// SQLite-backed mapping from FileIdentity → DocId.
///
/// # Schema
/// ```sql
/// CREATE TABLE identity_map (
///     volume_uuid BLOB NOT NULL,
///     inode INTEGER NOT NULL,
///     generation INTEGER NOT NULL,
///     device_id INTEGER NOT NULL,
///     doc_id INTEGER PRIMARY KEY,
///     created_at INTEGER NOT NULL
/// );
/// CREATE UNIQUE INDEX idx_identity ON identity_map(volume_uuid, inode, generation, device_id);
///
/// CREATE TABLE metadata (
///     key TEXT PRIMARY KEY,
///     value INTEGER NOT NULL
/// );
/// ```
///
/// # DocId Allocation
/// DocIds are allocated from a monotonically increasing counter stored in
/// the `metadata` table under key `"next_doc_id"`. The counter is incremented
/// atomically within a transaction.
pub struct IdentityDb {
    conn: Connection,
}

impl IdentityDb {
    /// Create or open an identity database at the given path.
    pub fn new(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        
        // Create schema if not exists
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS identity_map (
                volume_uuid BLOB NOT NULL,
                inode INTEGER NOT NULL,
                generation INTEGER NOT NULL,
                device_id INTEGER NOT NULL,
                doc_id INTEGER PRIMARY KEY,
                created_at INTEGER NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_identity 
                ON identity_map(volume_uuid, inode, generation, device_id);
            
            CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY,
                value INTEGER NOT NULL
            );
            
            INSERT OR IGNORE INTO metadata (key, value) VALUES ('next_doc_id', 1);
            "#
        )?;
        
        Ok(IdentityDb { conn })
    }
    
    /// Get or allocate a DocId for the given file identity.
    ///
    /// If the identity already exists in the database, returns the existing DocId.
    /// Otherwise, allocates a new DocId, inserts the mapping, and returns it.
    ///
    /// # Inode Reuse Detection
    /// If the same (volume_uuid, inode) pair exists with a different generation,
    /// this is treated as a new file (inode was reused) and gets a new DocId.
    pub fn get_or_allocate(
        &mut self,
        volume_uuid: u128,
        inode: u64,
        generation: u32,
        device_id: u32,
    ) -> Result<DocId> {
        let identity = FileIdentity { volume_uuid, inode, generation, device_id };
        
        // Try lookup first
        if let Some(doc_id) = self.lookup(&identity)? {
            return Ok(doc_id);
        }
        
        // Allocate new DocId
        let doc_id = self.allocate_next_id()?;
        self.insert(identity, doc_id)?;
        
        Ok(doc_id)
    }
    
    /// Look up an existing DocId for the given identity.
    fn lookup(&self, identity: &FileIdentity) -> Result<Option<DocId>> {
        let volume_bytes = identity.volume_uuid.to_le_bytes();
        
        let mut stmt = self.conn.prepare_cached(
            "SELECT doc_id FROM identity_map 
             WHERE volume_uuid = ?1 AND inode = ?2 AND generation = ?3 AND device_id = ?4"
        )?;
        
        let doc_id = stmt.query_row(
            params![
                &volume_bytes[..],
                identity.inode as i64,
                identity.generation as i64,
                identity.device_id as i64,
            ],
            |row| row.get::<_, i64>(0)
        ).optional()?;
        
        Ok(doc_id.map(|id| DocId(id as u64)))
    }
    
    /// Allocate the next DocId from the counter.
    fn allocate_next_id(&mut self) -> Result<DocId> {
        let tx = self.conn.transaction()?;
        
        // Read current counter
        let next_id: i64 = tx.query_row(
            "SELECT value FROM metadata WHERE key = 'next_doc_id'",
            [],
            |row| row.get(0)
        )?;
        
        // Increment counter
        tx.execute(
            "UPDATE metadata SET value = ?1 WHERE key = 'next_doc_id'",
            params![next_id + 1]
        )?;
        
        tx.commit()?;
        
        Ok(DocId(next_id as u64))
    }
    
    /// Insert a new identity → DocId mapping.
    fn insert(&mut self, identity: FileIdentity, doc_id: DocId) -> Result<()> {
        let volume_bytes = identity.volume_uuid.to_le_bytes();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        
        self.conn.execute(
            "INSERT INTO identity_map (volume_uuid, inode, generation, device_id, doc_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &volume_bytes[..],
                identity.inode as i64,
                identity.generation as i64,
                identity.device_id as i64,
                doc_id.0 as i64,
                now as i64,
            ]
        )?;
        
        Ok(())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_doc_id_allocation_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = IdentityDb::new(&dir.path().join("identity.db")).unwrap();
        
        let id1 = db.get_or_allocate(0xabc, 1001, 1, 16).unwrap();
        let id2 = db.get_or_allocate(0xabc, 1001, 1, 16).unwrap();
        
        assert_eq!(id1, id2, "Same file identity should return same DocId");
    }
    
    #[test]
    fn test_inode_reuse_gets_new_doc_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = IdentityDb::new(&dir.path().join("identity.db")).unwrap();
        
        // File with generation=1
        let id1 = db.get_or_allocate(0xabc, 1001, 1, 16).unwrap();
        
        // Same inode but generation=2 (inode was reused)
        let id2 = db.get_or_allocate(0xabc, 1001, 2, 16).unwrap();
        
        assert_ne!(id1, id2, "Inode reuse (different generation) should get new DocId");
    }
    
    #[test]
    fn test_doc_ids_are_monotonic() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = IdentityDb::new(&dir.path().join("identity.db")).unwrap();
        
        let id1 = db.get_or_allocate(0x111, 1, 1, 16).unwrap();
        let id2 = db.get_or_allocate(0x222, 2, 1, 16).unwrap();
        let id3 = db.get_or_allocate(0x333, 3, 1, 16).unwrap();
        
        assert!(id2.0 > id1.0, "DocIds should be monotonically increasing");
        assert!(id3.0 > id2.0, "DocIds should be monotonically increasing");
    }
    
    #[test]
    fn test_different_volumes_get_different_doc_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = IdentityDb::new(&dir.path().join("identity.db")).unwrap();
        
        // Same inode, different volumes
        let id1 = db.get_or_allocate(0xaaa, 1001, 1, 16).unwrap();
        let id2 = db.get_or_allocate(0xbbb, 1001, 1, 16).unwrap();
        
        assert_ne!(id1, id2, "Same inode on different volumes should get different DocIds");
    }
    
    #[test]
    fn test_persistence_across_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.db");
        
        let id1 = {
            let mut db = IdentityDb::new(&path).unwrap();
            db.get_or_allocate(0xabc, 1001, 1, 16).unwrap()
        };
        
        // Reopen database
        let id2 = {
            let mut db = IdentityDb::new(&path).unwrap();
            db.get_or_allocate(0xabc, 1001, 1, 16).unwrap()
        };
        
        assert_eq!(id1, id2, "DocId should persist across database reopens");
    }
    
    #[test]
    fn test_counter_persists_across_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.db");
        
        let id1 = {
            let mut db = IdentityDb::new(&path).unwrap();
            db.get_or_allocate(0x111, 1, 1, 16).unwrap()
        };
        
        // Reopen and allocate new DocId
        let id2 = {
            let mut db = IdentityDb::new(&path).unwrap();
            db.get_or_allocate(0x222, 2, 1, 16).unwrap()
        };
        
        assert!(id2.0 > id1.0, "Counter should persist and continue incrementing");
    }
    
    #[test]
    fn test_doc_id_never_zero() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = IdentityDb::new(&dir.path().join("identity.db")).unwrap();
        
        let id = db.get_or_allocate(0x111, 1, 1, 16).unwrap();
        
        assert!(id.0 > 0, "DocId should never be zero (starts at 1)");
    }
}
