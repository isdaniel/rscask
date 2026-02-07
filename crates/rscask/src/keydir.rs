use std::collections::HashMap;

use crate::storage::LogPointer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyDirEntry {
    pub pointer: LogPointer,
    pub timestamp: u64,
}

pub trait KeyDir {
    fn insert(&mut self, key: Vec<u8>, entry: KeyDirEntry);
    fn get(&self, key: &[u8]) -> Option<KeyDirEntry>;
    fn remove(&mut self, key: &[u8]) -> bool;
    fn entries(&self) -> Vec<(Vec<u8>, KeyDirEntry)>;
    fn keys(&self) -> Vec<Vec<u8>> {
        self.entries().into_iter().map(|(k, _)| k).collect()
    }
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn clear(&mut self);
}

pub struct HashMapKeyDir {
    map: HashMap<Vec<u8>, KeyDirEntry>,
}

impl Default for HashMapKeyDir {
    fn default() -> Self {
        Self::new()
    }
}

impl HashMapKeyDir {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

impl KeyDir for HashMapKeyDir {
    fn insert(&mut self, key: Vec<u8>, entry: KeyDirEntry) {
        self.map.insert(key, entry);
    }

    fn get(&self, key: &[u8]) -> Option<KeyDirEntry> {
        self.map.get(key).copied()
    }

    fn remove(&mut self, key: &[u8]) -> bool {
        self.map.remove(key).is_some()
    }

    fn entries(&self) -> Vec<(Vec<u8>, KeyDirEntry)> {
        self.map
            .iter()
            .map(|(key, entry)| (key.clone(), *entry))
            .collect()
    }

    fn keys(&self) -> Vec<Vec<u8>> {
        self.map.keys().cloned().collect()
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn clear(&mut self) {
        self.map.clear();
    }
}
