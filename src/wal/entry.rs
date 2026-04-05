use xxhash_rust::xxh3::xxh3_64;

// ─── EventType ────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    Created          = 1,
    Modified         = 2,
    Deleted          = 3,
    Renamed          = 4,
    PermissionChanged = 5,
    MetadataChanged  = 6,
    Checkpoint       = 255,
}

impl TryFrom<u8> for EventType {
    type Error = anyhow::Error;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            1 => Ok(EventType::Created),
            2 => Ok(EventType::Modified),
            3 => Ok(EventType::Deleted),
            4 => Ok(EventType::Renamed),
            5 => Ok(EventType::PermissionChanged),
            6 => Ok(EventType::MetadataChanged),
            255 => Ok(EventType::Checkpoint),
            _ => Err(anyhow::anyhow!("Unknown EventType: {}", v)),
        }
    }
}

// ─── WalEntry ─────────────────────────────────────────────────────────────────

/// A single write-ahead log entry.
///
/// Binary layout (little-endian):
///   [8] seq          u64
///   [8] timestamp_us u64
///   [1] event_type   u8
///   [1] flags        u8
///   [8] doc_id       u64
///   [8] inode        u64
///   [8] volume_uuid  u64
///   [8] mtime_ns     u64
///   [8] size         u64
///   [2] mode         u16
///   [4] uid          u32
///   [4] gid          u32
///   [2] path_len     u16
///   [N] path         utf-8 bytes
///   [8] checksum     xxh3_64 of all preceding bytes
///
/// Total fixed overhead: 70 bytes + path_len + 8 (checksum)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalEntry {
    pub seq:          u64,
    pub timestamp_us: u64,
    pub event_type:   EventType,
    pub flags:        u8,
    pub doc_id:       u64,
    pub inode:        u64,
    pub volume_uuid:  u64,
    pub mtime_ns:     u64,
    pub size:         u64,
    pub mode:         u16,
    pub uid:          u32,
    pub gid:          u32,
    pub path:         String,
}

impl WalEntry {
    /// Serialize to bytes (including trailing xxh3 checksum).
    pub fn serialize(&self, buf: &mut Vec<u8>) {
        let start = buf.len();

        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.timestamp_us.to_le_bytes());
        buf.push(self.event_type as u8);
        buf.push(self.flags);
        buf.extend_from_slice(&self.doc_id.to_le_bytes());
        buf.extend_from_slice(&self.inode.to_le_bytes());
        buf.extend_from_slice(&self.volume_uuid.to_le_bytes());
        buf.extend_from_slice(&self.mtime_ns.to_le_bytes());
        buf.extend_from_slice(&self.size.to_le_bytes());
        buf.extend_from_slice(&self.mode.to_le_bytes());
        buf.extend_from_slice(&self.uid.to_le_bytes());
        buf.extend_from_slice(&self.gid.to_le_bytes());

        let path_bytes = self.path.as_bytes();
        let path_len = path_bytes.len() as u16;
        buf.extend_from_slice(&path_len.to_le_bytes());
        buf.extend_from_slice(path_bytes);

        // Checksum covers everything from start up to (not including) the checksum itself
        let checksum = xxh3_64(&buf[start..]);
        buf.extend_from_slice(&checksum.to_le_bytes());
    }

    /// Deserialize from bytes. Does NOT verify checksum — call `verify_checksum` separately.
    pub fn deserialize(buf: &[u8]) -> anyhow::Result<Self> {
        if buf.len() < 78 {
            anyhow::bail!("WAL entry too short: {} bytes", buf.len());
        }

        let seq          = u64::from_le_bytes(buf[0..8].try_into()?);
        let timestamp_us = u64::from_le_bytes(buf[8..16].try_into()?);
        let event_type   = EventType::try_from(buf[16])?;
        let flags        = buf[17];
        let doc_id       = u64::from_le_bytes(buf[18..26].try_into()?);
        let inode        = u64::from_le_bytes(buf[26..34].try_into()?);
        let volume_uuid  = u64::from_le_bytes(buf[34..42].try_into()?);
        let mtime_ns     = u64::from_le_bytes(buf[42..50].try_into()?);
        let size         = u64::from_le_bytes(buf[50..58].try_into()?);
        let mode         = u16::from_le_bytes(buf[58..60].try_into()?);
        let uid          = u32::from_le_bytes(buf[60..64].try_into()?);
        let gid          = u32::from_le_bytes(buf[64..68].try_into()?);
        let path_len     = u16::from_le_bytes(buf[68..70].try_into()?) as usize;

        let path_end = 70 + path_len;
        if buf.len() < path_end + 8 {
            anyhow::bail!("WAL entry truncated at path");
        }

        let path = String::from_utf8(buf[70..path_end].to_vec())
            .map_err(|e| anyhow::anyhow!("WAL path not UTF-8: {}", e))?;

        Ok(WalEntry {
            seq, timestamp_us, event_type, flags,
            doc_id, inode, volume_uuid, mtime_ns, size,
            mode, uid, gid, path,
        })
    }

    /// Verify the xxh3 checksum stored in the last 8 bytes of a serialized entry.
    /// Used for self-consistency checks after a round-trip.
    pub fn verify_checksum(&self) -> bool {
        let mut buf = Vec::new();
        self.serialize(&mut buf);
        Self::verify_checksum_bytes(&buf)
    }

    /// Verify the checksum in a raw byte buffer (before deserialization).
    /// This is the correct check for detecting corruption in the WAL file.
    pub fn verify_checksum_bytes(buf: &[u8]) -> bool {
        if buf.len() < 8 {
            return false;
        }
        let data = &buf[..buf.len() - 8];
        let stored = u64::from_le_bytes(buf[buf.len() - 8..].try_into().unwrap());
        xxh3_64(data) == stored
    }

    /// Returns total serialized byte length (fixed header + path + checksum).
    pub fn serialized_len(&self) -> usize {
        70 + self.path.len() + 8
    }

    #[cfg(test)]
    pub fn new_test() -> Self {
        WalEntry {
            seq:          1,
            timestamp_us: 1_000_000,
            event_type:   EventType::Created,
            flags:        0,
            doc_id:       1,
            inode:        100,
            volume_uuid:  0,
            mtime_ns:     0,
            size:         0,
            mode:         0o644,
            uid:          501,
            gid:          20,
            path:         "/test/file.txt".to_string(),
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wal_entry_roundtrip() {
        let entry = WalEntry {
            seq:          1,
            timestamp_us: 1_000_000,
            event_type:   EventType::Created,
            flags:        0,
            doc_id:       42,
            inode:        12345,
            volume_uuid:  0xdeadbeef,
            mtime_ns:     999,
            size:         1024,
            mode:         0o644,
            uid:          501,
            gid:          20,
            path:         "/Users/test/file.txt".to_string(),
        };

        let mut buf = Vec::new();
        entry.serialize(&mut buf);
        let decoded = WalEntry::deserialize(&buf).unwrap();

        assert_eq!(decoded.seq, 1);
        assert_eq!(decoded.doc_id, 42);
        assert_eq!(decoded.inode, 12345);
        assert_eq!(decoded.volume_uuid, 0xdeadbeef);
        assert_eq!(decoded.path, "/Users/test/file.txt");
        assert_eq!(decoded.event_type, EventType::Created);
        assert!(decoded.verify_checksum());
    }

    #[test]
    fn test_corrupt_entry_detected() {
        let entry = WalEntry::new_test();
        let mut buf = Vec::new();
        entry.serialize(&mut buf);
        // Checksum must pass on clean buffer
        assert!(WalEntry::verify_checksum_bytes(&buf));
        // Flip bits in the middle of the timestamp field
        buf[10] ^= 0xFF;
        // verify_checksum_bytes catches corruption in the raw buffer (WAL reader usage)
        assert!(!WalEntry::verify_checksum_bytes(&buf));
    }

    #[test]
    fn test_event_type_roundtrip() {
        for &(byte, expected) in &[
            (1u8, EventType::Created),
            (2u8, EventType::Modified),
            (3u8, EventType::Deleted),
            (4u8, EventType::Renamed),
            (255u8, EventType::Checkpoint),
        ] {
            assert_eq!(EventType::try_from(byte).unwrap(), expected);
        }
    }

    #[test]
    fn test_serialized_len_accurate() {
        let entry = WalEntry::new_test();
        let mut buf = Vec::new();
        entry.serialize(&mut buf);
        assert_eq!(buf.len(), entry.serialized_len());
    }

    #[test]
    fn test_unicode_path_roundtrip() {
        let mut entry = WalEntry::new_test();
        entry.path = "/Users/tëst/fïlé.txt".to_string();
        let mut buf = Vec::new();
        entry.serialize(&mut buf);
        let decoded = WalEntry::deserialize(&buf).unwrap();
        assert_eq!(decoded.path, "/Users/tëst/fïlé.txt");
        assert!(decoded.verify_checksum());
    }
}
