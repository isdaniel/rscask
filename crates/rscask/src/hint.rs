use std::io::Read;

use crate::error::{Error, Result};
use crate::storage::LogPointer;

/// Bytes in the hint header *after* the 4-byte CRC prefix.
///
/// Layout: `timestamp(8) + key_size(4) + file_id(8) + offset(8) + len(4) = 32`
const HINT_HEADER_WITHOUT_CRC: usize = 8 + 4 + 8 + 8 + 4;

/// A single entry in a hint file.
///
/// Hint files store the same key→pointer mapping as the full data records
/// but **without the value bytes**, making them much smaller and faster to
/// read during KeyDir recovery.
///
/// ## On-disk format
///
/// ```text
/// [crc:4][timestamp:8][key_size:4][file_id:8][offset:8][len:4][key:variable]
/// ```
///
/// Hint files are produced exclusively during merge / compaction and only
/// contain **live** (non-tombstone) records.
#[derive(Debug, Clone)]
pub struct HintRecord {
    pub key: Vec<u8>,
    pub timestamp: u64,
    pub pointer: LogPointer,
}

impl HintRecord {
    /// Create a new hint record.
    pub fn new(key: Vec<u8>, timestamp: u64, pointer: LogPointer) -> Self {
        Self {
            key,
            timestamp,
            pointer,
        }
    }

    /// Serialise the hint record to bytes with a CRC-32 prefix.
    pub fn encode(&self) -> Vec<u8> {
        let key_len = self.key.len() as u32;
        let mut payload = Vec::with_capacity(HINT_HEADER_WITHOUT_CRC + self.key.len());
        payload.extend_from_slice(&self.timestamp.to_be_bytes());
        payload.extend_from_slice(&key_len.to_be_bytes());
        payload.extend_from_slice(&self.pointer.file_id.to_be_bytes());
        payload.extend_from_slice(&self.pointer.offset.to_be_bytes());
        payload.extend_from_slice(&self.pointer.len.to_be_bytes());
        payload.extend_from_slice(&self.key);

        let crc = crc32fast::hash(&payload);
        let mut out = Vec::with_capacity(4 + payload.len());
        out.extend_from_slice(&crc.to_be_bytes());
        out.extend_from_slice(&payload);
        out
    }

    /// Decode one hint record from a reader.
    ///
    /// Returns `Ok(None)` on clean EOF, `Err` on corruption (CRC mismatch).
    pub fn decode_from_reader<R: Read>(reader: &mut R) -> Result<Option<HintRecord>> {
        let mut crc_buf = [0u8; 4];
        if !read_exact_or_eof(reader, &mut crc_buf)? {
            return Ok(None);
        }
        let stored_crc = u32::from_be_bytes(crc_buf);

        let mut header = [0u8; HINT_HEADER_WITHOUT_CRC];
        if !read_exact_or_eof(reader, &mut header)? {
            return Ok(None);
        }

        let timestamp = u64::from_be_bytes(header[0..8].try_into().unwrap());
        let key_len = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;
        let file_id = u64::from_be_bytes(header[12..20].try_into().unwrap());
        let offset = u64::from_be_bytes(header[20..28].try_into().unwrap());
        let len = u32::from_be_bytes(header[28..32].try_into().unwrap());

        let mut key = vec![0u8; key_len];
        if !read_exact_or_eof(reader, &mut key)? {
            return Ok(None);
        }

        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&header);
        hasher.update(&key);
        let computed = hasher.finalize();

        if computed != stored_crc {
            return Err(Error::Corrupt("hint crc mismatch".to_string()));
        }

        Ok(Some(HintRecord {
            key,
            timestamp,
            pointer: LogPointer {
                file_id,
                offset,
                len,
            },
        }))
    }
}

fn read_exact_or_eof<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<bool> {
    match reader.read_exact(buf) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_single() {
        let hr = HintRecord::new(
            b"hello".to_vec(),
            123456,
            LogPointer {
                file_id: 7,
                offset: 42,
                len: 99,
            },
        );
        let bytes = hr.encode();
        let mut cursor = Cursor::new(&bytes);
        let decoded = HintRecord::decode_from_reader(&mut cursor)
            .unwrap()
            .unwrap();
        assert_eq!(decoded.key, b"hello");
        assert_eq!(decoded.timestamp, 123456);
        assert_eq!(decoded.pointer.file_id, 7);
        assert_eq!(decoded.pointer.offset, 42);
        assert_eq!(decoded.pointer.len, 99);
    }

    #[test]
    fn roundtrip_multiple() {
        let records: Vec<HintRecord> = (0..5)
            .map(|i| {
                HintRecord::new(
                    format!("key-{i}").into_bytes(),
                    i * 1000,
                    LogPointer {
                        file_id: 1,
                        offset: i * 100,
                        len: 50,
                    },
                )
            })
            .collect();

        let mut buf = Vec::new();
        for r in &records {
            buf.extend_from_slice(&r.encode());
        }

        let mut cursor = Cursor::new(&buf);
        for expected in &records {
            let decoded = HintRecord::decode_from_reader(&mut cursor)
                .unwrap()
                .unwrap();
            assert_eq!(decoded.key, expected.key);
            assert_eq!(decoded.timestamp, expected.timestamp);
            assert_eq!(decoded.pointer, expected.pointer);
        }
        // EOF
        assert!(HintRecord::decode_from_reader(&mut cursor)
            .unwrap()
            .is_none());
    }

    #[test]
    fn empty_reader_returns_none() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        assert!(HintRecord::decode_from_reader(&mut cursor)
            .unwrap()
            .is_none());
    }

    #[test]
    fn corrupt_crc_is_detected() {
        let hr = HintRecord::new(
            b"key".to_vec(),
            1,
            LogPointer {
                file_id: 1,
                offset: 0,
                len: 10,
            },
        );
        let mut bytes = hr.encode();
        // Flip a byte in the payload (after CRC)
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let mut cursor = Cursor::new(&bytes);
        let result = HintRecord::decode_from_reader(&mut cursor);
        assert!(result.is_err());
    }
}
