//! Segment encoding.
//!
//! ```text
//! header (little-endian):
//!   magic u32 = 0x564B4C57 | version u16 = 2 | flags u16 (bit0 zstd body, bit1 SEALED)
//!   inc u32 | seq u64 | record_count u32 | header_len u32
//!   node_len u16 | node id bytes
//!   shard_count u32 | shard u16 × shard_count   (sorted, distinct)
//!   header_crc u32                              (crc32 of every header byte before it)
//! body (zstd if flag set):
//!   repeat: len u32 | crc32 u32 | payload[len] (JSON Envelope)
//! ```
//!
//! A SEALED segment has no records. A splitter writes one at the first gap of a dead
//! incarnation's log: the dead writer's late PUT for that position then fails, and every
//! reader stops there.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use valka_core::ShardId;

use crate::error::WalError;
use crate::lsn::Lsn;
use crate::record::Envelope;

pub const MAGIC: u32 = 0x564B_4C57;
pub const VERSION: u16 = 2;
pub const FLAG_ZSTD: u16 = 1;
pub const FLAG_SEALED: u16 = 2;
/// Bytes before the variable part of the header.
pub const FIXED_HEADER_LEN: usize = 28;
const ZSTD_LEVEL: i32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentHeader {
    pub version: u16,
    pub flags: u16,
    pub lsn: Lsn,
    pub node_id: String,
    pub record_count: u32,
    /// Shards with at least one record in this segment, sorted.
    pub shards: Vec<ShardId>,
    /// Total header length in bytes; the body starts here.
    pub header_len: usize,
}

impl SegmentHeader {
    pub fn sealed(&self) -> bool {
        self.flags & FLAG_SEALED != 0
    }
}

fn header_bytes(lsn: Lsn, node_id: &str, flags: u16, records: &[Envelope]) -> BytesMut {
    let mut shards: Vec<u16> = records.iter().map(|r| r.shard.0).collect();
    shards.sort_unstable();
    shards.dedup();
    let header_len = FIXED_HEADER_LEN + 2 + node_id.len() + 4 + shards.len() * 2 + 4;
    let mut out = BytesMut::with_capacity(header_len);
    out.put_u32_le(MAGIC);
    out.put_u16_le(VERSION);
    out.put_u16_le(flags);
    out.put_u32_le(lsn.inc);
    out.put_u64_le(lsn.seq);
    out.put_u32_le(records.len() as u32);
    out.put_u32_le(header_len as u32);
    out.put_u16_le(node_id.len() as u16);
    out.put_slice(node_id.as_bytes());
    out.put_u32_le(shards.len() as u32);
    for s in &shards {
        out.put_u16_le(*s);
    }
    let crc = crc32fast::hash(&out);
    out.put_u32_le(crc);
    debug_assert_eq!(out.len(), header_len);
    out
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
    let mut out = header_bytes(lsn, node_id, flags, records);
    out.put_slice(&body);
    Ok(out.freeze())
}

/// An empty SEALED segment for position `lsn` of `node_id`'s log.
pub fn encode_sealed(lsn: Lsn, node_id: &str) -> Bytes {
    header_bytes(lsn, node_id, FLAG_SEALED, &[]).freeze()
}

/// Bytes needed to decode the header, given at least the fixed part. Lets a reader fetch
/// just the header with a range GET.
pub fn header_len(prefix: &[u8]) -> Result<usize, WalError> {
    if prefix.len() < FIXED_HEADER_LEN {
        return Err(WalError::Corrupt("segment shorter than header".into()));
    }
    Ok(u32::from_le_bytes(prefix[24..28].try_into().unwrap()) as usize)
}

pub fn decode_header(bytes: &[u8]) -> Result<SegmentHeader, WalError> {
    let len = header_len(bytes)?;
    if len < FIXED_HEADER_LEN + 10 || bytes.len() < len {
        return Err(WalError::Corrupt("truncated segment header".into()));
    }
    let crc = u32::from_le_bytes(bytes[len - 4..len].try_into().unwrap());
    if crc32fast::hash(&bytes[..len - 4]) != crc {
        return Err(WalError::Corrupt("segment header crc mismatch".into()));
    }
    let mut b = &bytes[..len - 4];
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
    let inc = b.get_u32_le();
    let seq = b.get_u64_le();
    let record_count = b.get_u32_le();
    let _header_len = b.get_u32_le();
    let node_len = b.get_u16_le() as usize;
    if b.remaining() < node_len + 4 {
        return Err(WalError::Corrupt("segment header node id overruns".into()));
    }
    let node_id = String::from_utf8(b[..node_len].to_vec())
        .map_err(|_| WalError::Corrupt("segment node id is not utf-8".into()))?;
    b.advance(node_len);
    let count = b.get_u32_le() as usize;
    if b.remaining() != count * 2 {
        return Err(WalError::Corrupt("segment header shard list length".into()));
    }
    let shards = (0..count).map(|_| ShardId(b.get_u16_le())).collect();
    Ok(SegmentHeader {
        version,
        flags,
        lsn: Lsn::new(inc, seq),
        node_id,
        record_count,
        shards,
        header_len: len,
    })
}

/// Decode a full segment. A truncated or corrupt tail yields an error; there is no partial
/// recovery because a segment is written atomically (object PUT) or not at all.
pub fn decode(bytes: &[u8]) -> Result<(SegmentHeader, Vec<Envelope>), WalError> {
    let header = decode_header(bytes)?;
    let raw = &bytes[header.header_len..];
    if header.sealed() {
        if !raw.is_empty() || header.record_count != 0 {
            return Err(WalError::Corrupt("sealed segment carries records".into()));
        }
        return Ok((header, Vec::new()));
    }
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

    fn sample(n: usize) -> Vec<Envelope> {
        (0..n)
            .map(|i| {
                Envelope::new(
                    ShardId((i % 7) as u16),
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
            assert_eq!(h.node_id, "node-a");
            assert_eq!(h.shards, (0..7).map(ShardId).collect::<Vec<_>>());
            assert!(!h.sealed());
            assert_eq!(back, recs);
            assert_eq!(
                header_len(&bytes[..FIXED_HEADER_LEN]).unwrap(),
                h.header_len
            );
            assert_eq!(decode_header(&bytes[..h.header_len]).unwrap(), h);
        }
    }

    #[test]
    fn empty_and_sealed_segments() {
        let bytes = encode(Lsn::new(1, 1), "n", &[], true).unwrap();
        let (h, back) = decode(&bytes).unwrap();
        assert_eq!(h.record_count, 0);
        assert!(back.is_empty() && h.shards.is_empty() && !h.sealed());

        let seal = encode_sealed(Lsn::new(4, 12), "n");
        let (h, back) = decode(&seal).unwrap();
        assert!(h.sealed());
        assert_eq!(h.lsn, Lsn::new(4, 12));
        assert!(back.is_empty());
    }

    #[test]
    fn corruption_is_detected() {
        let recs = sample(3);
        let bytes = encode(Lsn::new(1, 1), "n", &recs, false).unwrap();
        let hl = decode_header(&bytes).unwrap().header_len;
        let mut bad = bytes.to_vec();
        // flip a byte inside the first payload
        bad[hl + 12] ^= 0xFF;
        assert!(matches!(decode(&bad), Err(WalError::Corrupt(_))));

        let mut short = bytes.to_vec();
        short.truncate(bytes.len() - 3);
        assert!(matches!(decode(&short), Err(WalError::Corrupt(_))));

        let mut magic = bytes.to_vec();
        magic[0] = 0;
        assert!(matches!(decode(&magic), Err(WalError::Corrupt(_))));

        let mut hdr = bytes.to_vec();
        hdr[FIXED_HEADER_LEN + 2] ^= 0x01; // node id byte: header crc catches it
        assert!(matches!(decode(&hdr), Err(WalError::Corrupt(_))));
    }
}
