//! Segment encoding.
//!
//! ```text
//! header (32 bytes, little-endian):
//!   magic u32 = 0x564B4C57 | version u16 = 1 | flags u16 (bit0 = zstd body)
//!   epoch u32 | seq u64 | node_hash u64 | record_count u32
//! body (zstd if flag set):
//!   repeat: len u32 | crc32 u32 | payload[len] (JSON Envelope)
//! ```

use bytes::{Buf, BufMut, Bytes, BytesMut};
use xxhash_rust::xxh3::xxh3_64;

use crate::error::WalError;
use crate::lsn::Lsn;
use crate::record::Envelope;

pub const MAGIC: u32 = 0x564B_4C57;
pub const VERSION: u16 = 1;
pub const FLAG_ZSTD: u16 = 1;
pub const HEADER_LEN: usize = 32;
const ZSTD_LEVEL: i32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentHeader {
    pub version: u16,
    pub flags: u16,
    pub lsn: Lsn,
    pub node_hash: u64,
    pub record_count: u32,
}

pub fn node_hash(node_id: &str) -> u64 {
    xxh3_64(node_id.as_bytes())
}

/// Encode a batch of envelopes into one segment.
pub fn encode(
    lsn: Lsn,
    node_id: &str,
    records: &[Envelope],
    compress: bool,
) -> Result<Bytes, WalError> {
    let mut body = BytesMut::with_capacity(records.len() * 256);
    for r in records {
        let payload = serde_json::to_vec(r)?;
        body.put_u32_le(payload.len() as u32);
        body.put_u32_le(crc32fast::hash(&payload));
        body.put_slice(&payload);
    }
    let (body, flags) = if compress {
        (
            Bytes::from(zstd::encode_all(&body[..], ZSTD_LEVEL)?),
            FLAG_ZSTD,
        )
    } else {
        (body.freeze(), 0)
    };

    let mut out = BytesMut::with_capacity(HEADER_LEN + body.len());
    out.put_u32_le(MAGIC);
    out.put_u16_le(VERSION);
    out.put_u16_le(flags);
    out.put_u32_le(lsn.epoch);
    out.put_u64_le(lsn.seq);
    out.put_u64_le(node_hash(node_id));
    out.put_u32_le(records.len() as u32);
    debug_assert_eq!(out.len(), HEADER_LEN);
    out.put_slice(&body);
    Ok(out.freeze())
}

pub fn decode_header(bytes: &[u8]) -> Result<SegmentHeader, WalError> {
    if bytes.len() < HEADER_LEN {
        return Err(WalError::Corrupt("segment shorter than header".into()));
    }
    let mut b = bytes;
    let magic = b.get_u32_le();
    if magic != MAGIC {
        return Err(WalError::Corrupt(format!("bad magic {magic:#x}")));
    }
    let version = b.get_u16_le();
    if version != VERSION {
        return Err(WalError::Corrupt(format!(
            "unsupported segment version {version}"
        )));
    }
    let flags = b.get_u16_le();
    let epoch = b.get_u32_le();
    let seq = b.get_u64_le();
    let node_hash = b.get_u64_le();
    let record_count = b.get_u32_le();
    Ok(SegmentHeader {
        version,
        flags,
        lsn: Lsn::new(epoch, seq),
        node_hash,
        record_count,
    })
}

/// Decode a full segment. A truncated or corrupt tail yields an error; there is no partial
/// recovery because a segment is written atomically (object PUT) or not at all.
pub fn decode(bytes: &[u8]) -> Result<(SegmentHeader, Vec<Envelope>), WalError> {
    let header = decode_header(bytes)?;
    let raw = &bytes[HEADER_LEN..];
    let body: Bytes = if header.flags & FLAG_ZSTD != 0 {
        Bytes::from(zstd::decode_all(raw)?)
    } else {
        Bytes::copy_from_slice(raw)
    };
    let mut b = &body[..];
    let mut out = Vec::with_capacity(header.record_count as usize);
    while b.has_remaining() {
        if b.remaining() < 8 {
            return Err(WalError::Corrupt("truncated record frame".into()));
        }
        let len = b.get_u32_le() as usize;
        let crc = b.get_u32_le();
        if b.remaining() < len {
            return Err(WalError::Corrupt("record length exceeds body".into()));
        }
        let payload = &b[..len];
        if crc32fast::hash(payload) != crc {
            return Err(WalError::Corrupt(format!(
                "crc mismatch at record {}",
                out.len()
            )));
        }
        let env: Envelope = serde_json::from_slice(payload)
            .map_err(|e| WalError::Corrupt(format!("record {} json: {e}", out.len())))?;
        out.push(env);
        b.advance(len);
    }
    if out.len() != header.record_count as usize {
        return Err(WalError::Corrupt(format!(
            "record count mismatch: header {} body {}",
            header.record_count,
            out.len()
        )));
    }
    Ok((header, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::WalRecord;
    use valka_core::ShardId;

    fn sample(n: usize) -> Vec<Envelope> {
        (0..n)
            .map(|i| {
                Envelope::new(
                    ShardId((i % 4096) as u16),
                    WalRecord::TaskPromoted {
                        task_id: format!("task-{i}"),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn round_trip_compressed_and_raw() {
        for compress in [true, false] {
            let recs = sample(50);
            let bytes = encode(Lsn::new(2, 9), "node-a", &recs, compress).unwrap();
            let (h, back) = decode(&bytes).unwrap();
            assert_eq!(h.lsn, Lsn::new(2, 9));
            assert_eq!(h.record_count, 50);
            assert_eq!(h.node_hash, node_hash("node-a"));
            assert_eq!(back, recs);
        }
    }

    #[test]
    fn empty_segment_is_valid() {
        let bytes = encode(Lsn::new(1, 1), "n", &[], true).unwrap();
        let (h, back) = decode(&bytes).unwrap();
        assert_eq!(h.record_count, 0);
        assert!(back.is_empty());
    }

    #[test]
    fn corruption_is_detected() {
        let recs = sample(3);
        let bytes = encode(Lsn::new(1, 1), "n", &recs, false).unwrap();
        let mut bad = bytes.to_vec();
        // flip a byte inside the first payload
        bad[HEADER_LEN + 12] ^= 0xFF;
        assert!(matches!(decode(&bad), Err(WalError::Corrupt(_))));

        let mut short = bytes.to_vec();
        short.truncate(bytes.len() - 3);
        assert!(matches!(decode(&short), Err(WalError::Corrupt(_))));

        let mut magic = bytes.to_vec();
        magic[0] = 0;
        assert!(matches!(decode(&magic), Err(WalError::Corrupt(_))));
    }
}
