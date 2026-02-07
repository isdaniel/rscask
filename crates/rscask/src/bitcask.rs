use std::path::Path;

use crate::error::{Error, Result};
use crate::keydir::{HashMapKeyDir, KeyDir, KeyDirEntry};
use crate::record::Record;
use crate::storage::{CompactionResult, FileLogStore, LogStore};

/// Database statistics.
#[derive(Debug, Clone)]
pub struct Stats {
    pub num_keys: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub max_segment_size: u64,
    pub sync_on_write: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_segment_size: 1024 * 1024,
            sync_on_write: true,
        }
    }
}

pub struct Bitcask<L: LogStore, K: KeyDir> {
    log: L,
    keydir: K,
    options: Options,
}

impl Bitcask<FileLogStore, HashMapKeyDir> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_options(path, Options::default())
    }

    pub fn open_with_options(path: impl AsRef<Path>, options: Options) -> Result<Self> {
        let log = FileLogStore::open(path, options.max_segment_size)?;
        let keydir = HashMapKeyDir::new();
        let mut store = Bitcask {
            log,
            keydir,
            options,
        };
        store.rebuild_keydir()?;
        Ok(store)
    }
}

impl<L: LogStore, K: KeyDir> Bitcask<L, K> {
    pub fn with_store(log: L, keydir: K) -> Result<Self> {
        let mut store = Bitcask {
            log,
            keydir,
            options: Options::default(),
        };
        store.rebuild_keydir()?;
        Ok(store)
    }

    pub fn put(&mut self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Result<()> {
        let key = key.as_ref().to_vec();
        let value = value.as_ref().to_vec();
        let record = Record::new_put(key.clone(), value);
        let pointer = self.log.append(&record)?;
        if self.options.sync_on_write {
            self.log.sync()?;
        }
        self.keydir.insert(
            key,
            KeyDirEntry {
                pointer,
                timestamp: record.timestamp,
            },
        );
        Ok(())
    }

    pub fn get(&self, key: impl AsRef<[u8]>) -> Result<Option<Vec<u8>>> {
        let key_ref = key.as_ref();
        let entry = match self.keydir.get(key_ref) {
            Some(entry) => entry,
            None => return Ok(None),
        };
        let record = self.log.read(&entry.pointer)?;
        if record.key != key_ref {
            return Err(Error::Corrupt("key mismatch".to_string()));
        }
        if record.tombstone {
            return Ok(None);
        }
        Ok(Some(record.value))
    }

    pub fn delete(&mut self, key: impl AsRef<[u8]>) -> Result<bool> {
        let key = key.as_ref().to_vec();
        let record = Record::new_delete(key.clone());
        self.log.append(&record)?;
        if self.options.sync_on_write {
            self.log.sync()?;
        }
        Ok(self.keydir.remove(&key))
    }

    pub fn merge(&mut self) -> Result<()> {
        let entries = self.keydir.entries();
        let mut records = Vec::with_capacity(entries.len());
        for (key, entry) in entries {
            let record = self.log.read(&entry.pointer)?;
            if record.key != key {
                return Err(Error::Corrupt("key mismatch".to_string()));
            }
            if !record.tombstone {
                records.push(record);
            }
        }
        let CompactionResult { pointers } = self.log.compact(&records)?;
        if pointers.len() != records.len() {
            return Err(Error::Corrupt("compaction mismatch".to_string()));
        }
        self.keydir.clear();
        for (record, pointer) in records.into_iter().zip(pointers.into_iter()) {
            self.keydir.insert(
                record.key,
                KeyDirEntry {
                    pointer,
                    timestamp: record.timestamp,
                },
            );
        }
        Ok(())
    }

    pub fn contains_key(&self, key: impl AsRef<[u8]>) -> bool {
        self.keydir.get(key.as_ref()).is_some()
    }

    pub fn len(&self) -> usize {
        self.keydir.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keydir.is_empty()
    }

    /// Return a snapshot of all keys currently in the store.
    pub fn keys(&self) -> Vec<Vec<u8>> {
        self.keydir.keys()
    }

    /// Return database statistics.
    pub fn stats(&self) -> Stats {
        Stats {
            num_keys: self.len(),
        }
    }

    /// Rebuild the in-memory KeyDir from on-disk data.
    ///
    /// For segments that have hint files (produced by merge), only the
    /// compact hint metadata is read — values are skipped entirely.
    /// Segments without hints (the active file) are scanned record by
    /// record as a fallback.
    fn rebuild_keydir(&mut self) -> Result<()> {
        self.keydir.clear();
        for entry in self.log.scan_keys()? {
            if entry.tombstone {
                self.keydir.remove(&entry.key);
            } else {
                self.keydir.insert(
                    entry.key,
                    KeyDirEntry {
                        pointer: entry.pointer,
                        timestamp: entry.timestamp,
                    },
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn count_segments(path: &Path) -> usize {
        fs::read_dir(path)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                name.starts_with("data_") && name.ends_with(".log")
            })
            .count()
    }

    fn count_hint_files(path: &Path) -> usize {
        fs::read_dir(path)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                name.starts_with("data_") && name.ends_with(".hint")
            })
            .count()
    }

    #[test]
    fn put_get_roundtrip() {
        let dir = TempDir::new().unwrap();
        let mut db = Bitcask::open(dir.path()).unwrap();
        db.put("alpha", "one").unwrap();
        assert_eq!(db.get("alpha").unwrap(), Some(b"one".to_vec()));
        assert!(db.contains_key("alpha"));
        assert_eq!(db.len(), 1);
    }

    #[test]
    fn update_overwrites_value() {
        let dir = TempDir::new().unwrap();
        let mut db = Bitcask::open(dir.path()).unwrap();
        db.put("key", "v1").unwrap();
        db.put("key", "v2").unwrap();
        assert_eq!(db.get("key").unwrap(), Some(b"v2".to_vec()));
    }

    #[test]
    fn delete_removes_key() {
        let dir = TempDir::new().unwrap();
        let mut db = Bitcask::open(dir.path()).unwrap();
        db.put("temp", "value").unwrap();
        assert!(db.delete("temp").unwrap());
        assert!(!db.contains_key("temp"));
        assert_eq!(db.get("temp").unwrap(), None);
        assert!(!db.delete("missing").unwrap());
    }

    #[test]
    fn merge_compacts_segments() {
        let dir = TempDir::new().unwrap();
        let options = Options {
            max_segment_size: 64,
            ..Options::default()
        };
        let mut db = Bitcask::open_with_options(dir.path(), options).unwrap();
        db.put("a", "1").unwrap();
        db.put("a", "2").unwrap();
        db.put("b", "3").unwrap();
        db.delete("b").unwrap();
        db.merge().unwrap();
        assert_eq!(db.get("a").unwrap(), Some(b"2".to_vec()));
        assert_eq!(db.get("b").unwrap(), None);
        // After merge: one compacted segment + one fresh active segment.
        assert_eq!(count_segments(dir.path()), 2);
    }

    #[test]
    fn merge_produces_hint_file() {
        let dir = TempDir::new().unwrap();
        let mut db = Bitcask::open(dir.path()).unwrap();
        db.put("a", "1").unwrap();
        db.put("b", "2").unwrap();
        assert_eq!(count_hint_files(dir.path()), 0);
        db.merge().unwrap();
        assert_eq!(count_hint_files(dir.path()), 1);
    }

    #[test]
    fn recovery_uses_hint_files() {
        let dir = TempDir::new().unwrap();
        {
            let mut db = Bitcask::open(dir.path()).unwrap();
            db.put("x", "1").unwrap();
            db.put("y", "2").unwrap();
            db.merge().unwrap();
            // Write more after merge — these go into the active file
            // which has no hint file.
            db.put("z", "3").unwrap();
        }
        // Reopen: segment with hint + active segment without hint.
        let db = Bitcask::open(dir.path()).unwrap();
        assert_eq!(db.get("x").unwrap(), Some(b"1".to_vec()));
        assert_eq!(db.get("y").unwrap(), Some(b"2".to_vec()));
        assert_eq!(db.get("z").unwrap(), Some(b"3".to_vec()));
        assert_eq!(db.len(), 3);
    }

    #[test]
    fn double_merge_keeps_hint_files_consistent() {
        let dir = TempDir::new().unwrap();
        let mut db = Bitcask::open(dir.path()).unwrap();
        db.put("a", "1").unwrap();
        db.merge().unwrap();
        db.put("b", "2").unwrap();
        db.merge().unwrap();
        assert_eq!(count_hint_files(dir.path()), 1);
        assert_eq!(db.get("a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(db.get("b").unwrap(), Some(b"2".to_vec()));
    }

    #[test]
    fn is_empty_and_keys() {
        let dir = TempDir::new().unwrap();
        let mut db = Bitcask::open(dir.path()).unwrap();
        assert!(db.is_empty());
        db.put("a", "1").unwrap();
        db.put("b", "2").unwrap();
        assert!(!db.is_empty());
        let mut keys = db.keys();
        keys.sort();
        assert_eq!(keys, vec![b"a".to_vec(), b"b".to_vec()]);
    }

    #[test]
    fn stats_reports_key_count() {
        let dir = TempDir::new().unwrap();
        let mut db = Bitcask::open(dir.path()).unwrap();
        assert_eq!(db.stats().num_keys, 0);
        db.put("a", "1").unwrap();
        assert_eq!(db.stats().num_keys, 1);
        db.put("b", "2").unwrap();
        assert_eq!(db.stats().num_keys, 2);
        db.delete("a").unwrap();
        assert_eq!(db.stats().num_keys, 1);
    }
}
