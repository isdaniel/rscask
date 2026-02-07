use std::fs;
use rscaskd::{Bitcask, Options};
use tempfile::TempDir;

fn count_segments(path: &std::path::Path) -> usize {
    fs::read_dir(path)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.starts_with("data_") && name.ends_with(".log")
        })
        .count()
}

#[test]
fn put_get_persists() {
    let dir = TempDir::new().unwrap();
    {
        let mut db = Bitcask::open(dir.path()).unwrap();
        db.put("alpha", "one").unwrap();
    }
    {
        let db = Bitcask::open(dir.path()).unwrap();
        assert_eq!(db.get("alpha").unwrap(), Some(b"one".to_vec()));
    }
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
    {
        let mut db = Bitcask::open(dir.path()).unwrap();
        db.put("temp", "value").unwrap();
        assert!(db.delete("temp").unwrap());
    }
    {
        let db = Bitcask::open(dir.path()).unwrap();
        assert_eq!(db.get("temp").unwrap(), None);
    }
}

#[test]
fn rotates_and_reopens() {
    let dir = TempDir::new().unwrap();
    let options = Options {
        max_segment_size: 64,
        ..Options::default()
    };
    {
        let mut db = Bitcask::open_with_options(dir.path(), options).unwrap();
        db.put("k1", "value1").unwrap();
        db.put("k2", "value2").unwrap();
        db.put("k3", "value3").unwrap();
    }
    let db = Bitcask::open_with_options(dir.path(), options).unwrap();
    assert_eq!(db.get("k1").unwrap(), Some(b"value1".to_vec()));
    assert_eq!(db.get("k2").unwrap(), Some(b"value2".to_vec()));
    assert_eq!(db.get("k3").unwrap(), Some(b"value3".to_vec()));
}

#[test]
fn merge_compacts_segments() {
    let dir = TempDir::new().unwrap();
    let mut db = Bitcask::open(dir.path()).unwrap();
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
