use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

const FIXED_HEADER_LEN: usize = 4 + 8 + 1 + 4 + 4;
const HEADER_WITHOUT_CRC_LEN: usize = FIXED_HEADER_LEN - 4;
const TOMBSTONE_FLAG: u8 = 0x01;

#[derive(Debug, Clone)]
pub struct Record {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub timestamp: u64,
    pub tombstone: bool,
}

impl Record {
    pub fn new_put(key: Vec<u8>, value: Vec<u8>) -> Self {
        Self {
            key,
            value,
            timestamp: now_millis(),
            tombstone: false,
        }
    }

    pub fn new_delete(key: Vec<u8>) -> Self {
        Self {
            key,
            value: Vec::new(),
            timestamp: now_millis(),
            tombstone: true,
        }
    }

    pub fn encoded_len(&self) -> usize {
        FIXED_HEADER_LEN + self.key.len() + self.value.len()
    }

    pub fn encode(&self) -> Vec<u8> {
        let key_len = self.key.len() as u32;
        let value_len = self.value.len() as u32;
        let mut payload =
            Vec::with_capacity(HEADER_WITHOUT_CRC_LEN + self.key.len() + self.value.len());
        payload.extend_from_slice(&self.timestamp.to_be_bytes());
        payload.push(if self.tombstone { TOMBSTONE_FLAG } else { 0 });
        payload.extend_from_slice(&key_len.to_be_bytes());
        payload.extend_from_slice(&value_len.to_be_bytes());
        payload.extend_from_slice(&self.key);
        payload.extend_from_slice(&self.value);

        let crc = crc32fast::hash(&payload);
        let mut out = Vec::with_capacity(4 + payload.len());
        out.extend_from_slice(&crc.to_be_bytes());
        out.extend_from_slice(&payload);
        out
    }

    pub fn decode_from_reader<R: Read>(reader: &mut R) -> Result<Option<(Record, usize)>> {
        let mut crc_buf = [0u8; 4];
        if !read_exact_or_eof(reader, &mut crc_buf)? {
            return Ok(None);
        }
        let stored_crc = u32::from_be_bytes(crc_buf);

        let mut header_buf = [0u8; HEADER_WITHOUT_CRC_LEN];
        if !read_exact_or_eof(reader, &mut header_buf)? {
            return Ok(None);
        }

        let timestamp = u64::from_be_bytes(header_buf[0..8].try_into().unwrap());
        let flags = header_buf[8];
        let key_len = u32::from_be_bytes(header_buf[9..13].try_into().unwrap()) as usize;
        let value_len = u32::from_be_bytes(header_buf[13..17].try_into().unwrap()) as usize;

        let mut key = vec![0u8; key_len];
        if !read_exact_or_eof(reader, &mut key)? {
            return Ok(None);
        }
        let mut value = vec![0u8; value_len];
        if !read_exact_or_eof(reader, &mut value)? {
            return Ok(None);
        }

        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&header_buf);
        hasher.update(&key);
        hasher.update(&value);
        let computed = hasher.finalize();

        if computed != stored_crc {
            return Err(Error::Corrupt("crc mismatch".to_string()));
        }

        let record = Record {
            key,
            value,
            timestamp,
            tombstone: flags & TOMBSTONE_FLAG != 0,
        };

        let total_len = FIXED_HEADER_LEN + key_len + value_len;
        Ok(Some((record, total_len)))
    }

    pub fn decode_from_bytes(bytes: &[u8]) -> Result<Record> {
        let mut cursor = std::io::Cursor::new(bytes);
        match Self::decode_from_reader(&mut cursor)? {
            Some((record, _)) => Ok(record),
            None => Err(Error::Corrupt("empty record".to_string())),
        }
    }
}

fn read_exact_or_eof<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<bool> {
    match reader.read_exact(buf) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(err) => Err(err.into()),
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
