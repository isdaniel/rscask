use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::hint::HintRecord;
use crate::record::Record;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogPointer {
    pub file_id: u64,
    pub offset: u64,
    pub len: u32,
}

#[derive(Debug)]
pub struct CompactionResult {
    pub pointers: Vec<LogPointer>,
}

/// Minimal information needed for KeyDir recovery.
///
/// Returned by [`LogStore::scan_keys`] so that implementations can
/// use hint files instead of reading full record values from disk.
#[derive(Debug, Clone)]
pub struct ScanEntry {
    pub key: Vec<u8>,
    pub pointer: LogPointer,
    pub timestamp: u64,
    pub tombstone: bool,
}

pub trait LogStore {
    fn append(&mut self, record: &Record) -> Result<LogPointer>;
    fn read(&self, pointer: &LogPointer) -> Result<Record>;
    fn scan(&self) -> Result<Vec<(LogPointer, Record)>>;
    /// Scan all segments and return the minimal metadata needed to rebuild
    /// the in-memory KeyDir.
    ///
    /// The default implementation delegates to [`scan`](LogStore::scan),
    /// reading full records. `FileLogStore` overrides this to read hint
    /// files when available, skipping value bytes entirely.
    fn scan_keys(&self) -> Result<Vec<ScanEntry>> {
        self.scan().map(|entries| {
            entries
                .into_iter()
                .map(|(pointer, record)| ScanEntry {
                    key: record.key,
                    pointer,
                    timestamp: record.timestamp,
                    tombstone: record.tombstone,
                })
                .collect()
        })
    }
    fn sync(&mut self) -> Result<()>;
    fn compact(&mut self, records: &[Record]) -> Result<CompactionResult>;
}

pub struct FileLogStore {
    dir: PathBuf,
    max_segment_size: u64,
    active_id: u64,
    active_size: u64,
    active_file: File,
}

impl FileLogStore {
    pub fn open(path: impl AsRef<Path>, max_segment_size: u64) -> Result<Self> {
        let dir = path.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let mut ids = list_segment_ids(&dir)?;
        if ids.is_empty() {
            let new_id = 1;
            let _ = create_segment(&dir, new_id)?;
            ids.push(new_id);
        }
        ids.sort_unstable();
        let active_id = *ids.last().unwrap();
        let mut active_file = open_segment(&dir, active_id)?;
        let active_size = active_file.metadata()?.len();
        active_file.seek(SeekFrom::End(0))?;
        Ok(Self {
            dir,
            max_segment_size,
            active_id,
            active_size,
            active_file,
        })
    }

    fn rotate(&mut self) -> Result<()> {
        let new_id = self.active_id + 1;
        self.active_file.sync_data()?;
        self.active_file = open_segment(&self.dir, new_id)?;
        self.active_id = new_id;
        self.active_size = 0;
        Ok(())
    }
}

impl LogStore for FileLogStore {
    fn append(&mut self, record: &Record) -> Result<LogPointer> {
        let bytes = record.encode();
        let record_len = bytes.len() as u64;
        if self.active_size > 0 && self.active_size + record_len > self.max_segment_size {
            self.rotate()?;
        }
        let offset = self.active_size;
        self.active_file.seek(SeekFrom::End(0))?;
        self.active_file.write_all(&bytes)?;
        self.active_size += record_len;
        Ok(LogPointer {
            file_id: self.active_id,
            offset,
            len: record_len as u32,
        })
    }

    fn read(&self, pointer: &LogPointer) -> Result<Record> {
        let path = segment_path(&self.dir, pointer.file_id);
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(pointer.offset))?;
        let mut buf = vec![0u8; pointer.len as usize];
        file.read_exact(&mut buf)?;
        Record::decode_from_bytes(&buf)
    }

    fn scan(&self) -> Result<Vec<(LogPointer, Record)>> {
        let ids = list_segment_ids(&self.dir)?;
        let mut records = Vec::new();
        for id in ids {
            let path = segment_path(&self.dir, id);
            let file = File::open(path)?;
            let mut reader = BufReader::new(file);
            let mut offset = 0u64;
            loop {
                let current_offset = offset;
                match Record::decode_from_reader(&mut reader)? {
                    Some((record, len)) => {
                        records.push((
                            LogPointer {
                                file_id: id,
                                offset: current_offset,
                                len: len as u32,
                            },
                            record,
                        ));
                        offset += len as u64;
                    }
                    None => break,
                }
            }
        }
        Ok(records)
    }

    fn sync(&mut self) -> Result<()> {
        self.active_file.sync_data()?;
        Ok(())
    }

    /// Scan segments using hint files when available, falling back to full
    /// data-file scans for segments without hints (e.g. the active file).
    fn scan_keys(&self) -> Result<Vec<ScanEntry>> {
        let ids = list_segment_ids(&self.dir)?;
        let mut entries = Vec::new();
        for id in ids {
            let hp = hint_path(&self.dir, id);
            if hp.exists() {
                // Fast path: collect hint entries into a temp vec so that
                // a corrupt/partial hint file can be discarded cleanly.
                let mut hint_entries = Vec::new();
                if scan_hint_file(&hp, &mut hint_entries).is_ok() {
                    entries.extend(hint_entries);
                    continue;
                }
                // Hint file corrupt — fall through to data-file scan.
            }
            // Slow path: scan the full data file.
            scan_data_segment(&self.dir, id, &mut entries)?;
        }
        Ok(entries)
    }

    fn compact(&mut self, records: &[Record]) -> Result<CompactionResult> {
        let compacted_id = self.active_id + 1;
        let next_active_id = compacted_id + 1;

        // 1. Write all live records into the compacted segment.
        let mut file = open_segment(&self.dir, compacted_id)?;
        let mut offset = 0u64;
        let mut pointers = Vec::with_capacity(records.len());
        for record in records {
            let bytes = record.encode();
            file.write_all(&bytes)?;
            pointers.push(LogPointer {
                file_id: compacted_id,
                offset,
                len: bytes.len() as u32,
            });
            offset += bytes.len() as u64;
        }
        file.sync_data()?;

        // 2. Write the corresponding hint file.
        write_hint_file(&self.dir, compacted_id, records, &pointers)?;

        // 3. Remove old data files and any stale hint files.
        let ids = list_segment_ids(&self.dir)?;
        for id in ids {
            if id != compacted_id {
                fs::remove_file(segment_path(&self.dir, id))?;
                // Hint file may not exist — ignore errors.
                let _ = fs::remove_file(hint_path(&self.dir, id));
            }
        }

        // 4. Create a fresh, empty active segment so new writes don't
        //    mix with the immutable compacted segment.
        let active_file = open_segment(&self.dir, next_active_id)?;
        self.active_id = next_active_id;
        self.active_size = 0;
        self.active_file = active_file;

        Ok(CompactionResult { pointers })
    }
}

fn segment_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("data_{id}.log"))
}

fn list_segment_ids(dir: &Path) -> Result<Vec<u64>> {
    let mut ids = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(id) = parse_segment_id(&name) {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

fn parse_segment_id(name: &str) -> Option<u64> {
    if let Some(stripped) = name.strip_prefix("data_")
        && let Some(id_part) = stripped.strip_suffix(".log")
    {
        return id_part.parse().ok();
    }
    None
}

fn open_segment(dir: &Path, id: u64) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(segment_path(dir, id))
        .map_err(Into::into)
}

fn create_segment(dir: &Path, id: u64) -> Result<File> {
    open_segment(dir, id)
}

fn hint_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("data_{id}.hint"))
}

/// Write a hint file alongside a compacted data segment.
///
/// Each hint record mirrors the key→pointer mapping but omits the value
/// bytes, so recovery can rebuild the KeyDir without reading full records.
fn write_hint_file(
    dir: &Path,
    file_id: u64,
    records: &[Record],
    pointers: &[LogPointer],
) -> Result<()> {
    let path = hint_path(dir, file_id);
    let mut file = File::create(path)?;
    for (record, pointer) in records.iter().zip(pointers.iter()) {
        let hint = HintRecord::new(record.key.clone(), record.timestamp, *pointer);
        file.write_all(&hint.encode())?;
    }
    file.sync_data()?;
    Ok(())
}

/// Read all entries from a hint file into `out`.
fn scan_hint_file(path: &Path, out: &mut Vec<ScanEntry>) -> Result<()> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    while let Some(hr) = HintRecord::decode_from_reader(&mut reader)? {
        out.push(ScanEntry {
            key: hr.key,
            pointer: hr.pointer,
            timestamp: hr.timestamp,
            tombstone: false, // hint files only contain live records
        });
    }
    Ok(())
}

/// Scan a data segment the traditional (slow) way, reading every record.
fn scan_data_segment(dir: &Path, id: u64, out: &mut Vec<ScanEntry>) -> Result<()> {
    let path = segment_path(dir, id);
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut offset = 0u64;
    while let Some((record, len)) = Record::decode_from_reader(&mut reader)? {
        out.push(ScanEntry {
            key: record.key,
            pointer: LogPointer {
                file_id: id,
                offset,
                len: len as u32,
            },
            timestamp: record.timestamp,
            tombstone: record.tombstone,
        });
        offset += len as u64;
    }
    Ok(())
}
