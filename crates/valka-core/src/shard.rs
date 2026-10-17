//! Storage sharding. Every task belongs to exactly one shard; a shard is the unit of
//! single-writer ownership, of snapshotting, and of moving work between nodes.
//!
//! The shard count is chosen when a cluster is created (`cluster.json`), a power of two in
//! `MIN_SHARDS..=MAX_SHARDS`. The shard id is embedded in the task id (UUIDv7, low 16 bits
//! of `rand_b`) so any node can route a request by id without a lookup.

use uuid::Uuid;
use xxhash_rust::xxh3::xxh3_64;

/// Default shard count for a new cluster.
pub const DEFAULT_NUM_SHARDS: u32 = 4096;
/// Smallest and largest shard counts a cluster may be created with.
pub const MIN_SHARDS: u32 = 16;
pub const MAX_SHARDS: u32 = 65_536;

/// Storage shard identifier in `0..num_shards`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ShardId(pub u16);

impl std::fmt::Display for ShardId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:05}", self.0)
    }
}

impl ShardId {
    /// Every shard of a cluster with `num_shards` shards.
    pub fn all(num_shards: u32) -> impl Iterator<Item = ShardId> {
        (0..num_shards).map(|s| ShardId(s as u16))
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// `n` is a usable shard count: a power of two in `MIN_SHARDS..=MAX_SHARDS`.
pub fn valid_shard_count(n: u32) -> bool {
    n.is_power_of_two() && (MIN_SHARDS..=MAX_SHARDS).contains(&n)
}

/// Pick the shard for a task. With a routing key, related tasks co-locate; without one the
/// task is spread pseudo-randomly using the fresh task uuid.
pub fn shard_for(
    queue_name: &str,
    routing_key: Option<&str>,
    task_uuid: &Uuid,
    num_shards: u32,
) -> ShardId {
    let mut buf = Vec::with_capacity(queue_name.len() + 1 + 36);
    buf.extend_from_slice(queue_name.as_bytes());
    buf.push(0);
    match routing_key {
        Some(k) => buf.extend_from_slice(k.as_bytes()),
        None => buf.extend_from_slice(task_uuid.as_bytes()),
    }
    ShardId((xxh3_64(&buf) & (num_shards as u64 - 1)) as u16)
}

/// Generate a new UUIDv7 task id carrying its shard in the low 16 bits.
pub fn new_task_uuid(
    queue_name: &str,
    routing_key: Option<&str>,
    num_shards: u32,
) -> (Uuid, ShardId) {
    let base = Uuid::now_v7();
    let shard = shard_for(queue_name, routing_key, &base, num_shards);
    (embed_shard(base, shard), shard)
}

/// A fresh UUIDv7 that routes to `shard` (run ids, signal ids).
pub fn new_id_in_shard(shard: ShardId) -> Uuid {
    embed_shard(Uuid::now_v7(), shard)
}

/// Overwrite the low 16 bits of a uuid with `shard`. Version and variant bits live in
/// bytes 6 and 8 and are untouched.
pub fn embed_shard(id: Uuid, shard: ShardId) -> Uuid {
    let mut b = *id.as_bytes();
    let nb = shard.0.to_be_bytes();
    b[14] = nb[0];
    b[15] = nb[1];
    Uuid::from_bytes(b)
}

/// Read the shard embedded in a task, run or signal id. `None` if the id is not a uuid.
/// Callers check the result against the cluster's shard count.
pub fn shard_of_task_id(task_id: &str) -> Option<ShardId> {
    let id = Uuid::parse_str(task_id).ok()?;
    let b = id.as_bytes();
    Some(ShardId(u16::from_be_bytes([b[14], b[15]])))
}

/// Default number of partitions a queue is created with.
pub const DEFAULT_QUEUE_PARTITIONS: u32 = 16;

/// The shard partition `p` of `queue` lives on. All of a partition's tasks share one
/// shard, so one owner and one matching queue serve it.
pub fn partition_shard(queue_name: &str, partition: u32, num_shards: u32) -> ShardId {
    let mut buf = Vec::with_capacity(queue_name.len() + 5);
    buf.extend_from_slice(queue_name.as_bytes());
    buf.push(0);
    buf.extend_from_slice(&partition.to_be_bytes());
    ShardId((xxh3_64(&buf) & (num_shards as u64 - 1)) as u16)
}

/// Jump consistent hash (Lamping & Veach): a bucket in `0..buckets` for `key`. When
/// `buckets` grows by one, only about `1/buckets` of the keys move.
pub fn jump_hash(key: &str, buckets: u32) -> u32 {
    let mut k = xxh3_64(key.as_bytes());
    let (mut b, mut j): (i64, i64) = (-1, 0);
    let n = buckets.max(1) as i64;
    while j < n {
        b = j;
        k = k.wrapping_mul(2_862_933_555_777_941_757).wrapping_add(1);
        j = (((b + 1) as f64) * ((1u64 << 31) as f64 / (((k >> 33) + 1) as f64))) as i64;
    }
    b as u32
}

/// Two hex digits of a hash of `rest`. Prefixed to high-rate key families so a burst of
/// writes (a takeover storm) spreads over the bucket's partitions instead of one prefix.
pub fn hash_prefix(rest: &str) -> String {
    format!("{:02x}", xxh3_64(rest.as_bytes()) as u8)
}

/// Rendezvous (highest random weight) score of `node` for `key`. The node with the highest
/// score owns the key; adding or removing a node moves only about 1/N of the keys.
pub fn rendezvous_score(key: &[u8], node: &str) -> u64 {
    let mut buf = Vec::with_capacity(key.len() + node.len() + 1);
    buf.extend_from_slice(key);
    buf.push(0xff);
    buf.extend_from_slice(node.as_bytes());
    xxh3_64(&buf)
}

/// The node with the highest rendezvous score for `key`, ties broken by node id.
pub fn rendezvous_pick<'a>(
    key: &[u8],
    nodes: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    nodes.into_iter().max_by(|a, b| {
        rendezvous_score(key, a)
            .cmp(&rendezvous_score(key, b))
            .then_with(|| a.cmp(b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_shard_round_trips() {
        for n in [16, 256, 4096, 65_536] {
            for _ in 0..500 {
                let (id, shard) = new_task_uuid("q", None, n);
                assert!((shard.0 as u32) < n);
                assert_eq!(shard_of_task_id(&id.to_string()), Some(shard));
                assert_eq!(id.get_version_num(), 7);
                assert_eq!(id.get_variant(), uuid::Variant::RFC4122);
            }
        }
    }

    #[test]
    fn routing_key_pins_shard() {
        let (a, sa) = new_task_uuid("q", Some("customer-1"), 4096);
        let (b, sb) = new_task_uuid("q", Some("customer-1"), 4096);
        assert_ne!(a, b);
        assert_eq!(sa, sb);
    }

    #[test]
    fn ids_stay_time_sortable() {
        let (a, _) = new_task_uuid("q", None, 4096);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let (b, _) = new_task_uuid("q", None, 4096);
        assert!(a.to_string() < b.to_string());
    }

    #[test]
    fn shard_counts() {
        assert!(valid_shard_count(4096));
        assert!(valid_shard_count(16));
        assert!(valid_shard_count(65_536));
        assert!(!valid_shard_count(8));
        assert!(!valid_shard_count(1000));
        assert!(!valid_shard_count(131_072));
    }

    #[test]
    fn jump_hash_is_stable_and_moves_little() {
        let mut moved = 0;
        for i in 0..10_000 {
            let k = format!("key-{i}");
            let a = jump_hash(&k, 16);
            assert!(a < 16);
            assert_eq!(a, jump_hash(&k, 16));
            let b = jump_hash(&k, 17);
            if a != b {
                assert_eq!(b, 16, "keys only move to the new bucket");
                moved += 1;
            }
        }
        assert!((400..800).contains(&moved), "about 1/17 move, got {moved}");
        assert_eq!(jump_hash("x", 1), 0);
    }

    #[test]
    fn partitions_map_into_the_shard_space() {
        for p in 0..16 {
            assert!(partition_shard("q", p, 256).0 < 256);
        }
        assert_eq!(partition_shard("q", 3, 4096), partition_shard("q", 3, 4096));
    }

    #[test]
    fn rendezvous_moves_few_keys_when_a_node_joins() {
        let three = ["a", "b", "c"];
        let four = ["a", "b", "c", "d"];
        let mut moved = 0;
        for s in 0..4096u32 {
            let k = s.to_be_bytes();
            let before = rendezvous_pick(&k, three).unwrap();
            let after = rendezvous_pick(&k, four).unwrap();
            if before != after {
                assert_eq!(after, "d", "keys only move to the new node");
                moved += 1;
            }
        }
        assert!((800..1250).contains(&moved), "about 1/4 move, got {moved}");
    }
}
