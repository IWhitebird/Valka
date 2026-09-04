//! Storage sharding. Every task belongs to exactly one of `NUM_SHARDS` shards; a shard is
//! the unit of single-writer ownership and of snapshotting.
//!
//! The shard id is embedded in the task id (UUIDv7, low 12 bits of `rand_b`) so any node
//! can route a request by id without a lookup.

use uuid::Uuid;
use xxhash_rust::xxh3::xxh3_64;

/// Fixed number of storage shards. Changing this is a data migration.
pub const NUM_SHARDS: u16 = 4096;

/// Storage shard identifier in `0..NUM_SHARDS`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ShardId(pub u16);

impl std::fmt::Display for ShardId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}", self.0)
    }
}

impl ShardId {
    pub fn all() -> impl Iterator<Item = ShardId> {
        (0..NUM_SHARDS).map(ShardId)
    }
}

/// Pick the shard for a task. With a routing key, related tasks co-locate; without one the
/// task is spread pseudo-randomly using the fresh task uuid.
pub fn shard_for(queue_name: &str, routing_key: Option<&str>, task_uuid: &Uuid) -> ShardId {
    let mut buf = Vec::with_capacity(queue_name.len() + 1 + 36);
    buf.extend_from_slice(queue_name.as_bytes());
    buf.push(0);
    match routing_key {
        Some(k) => buf.extend_from_slice(k.as_bytes()),
        None => buf.extend_from_slice(task_uuid.as_bytes()),
    }
    ShardId((xxh3_64(&buf) % NUM_SHARDS as u64) as u16)
}

/// Generate a new UUIDv7 task id carrying `shard` in its low 12 bits.
pub fn new_task_uuid(queue_name: &str, routing_key: Option<&str>) -> (Uuid, ShardId) {
    let base = Uuid::now_v7();
    let shard = shard_for(queue_name, routing_key, &base);
    (embed_shard(base, shard), shard)
}

/// Overwrite the low 12 bits of a uuid with `shard`. Keeps version/variant bits intact.
pub fn embed_shard(id: Uuid, shard: ShardId) -> Uuid {
    let mut b = *id.as_bytes();
    let low = u16::from_be_bytes([b[14], b[15]]);
    let new_low = (low & !0x0FFF) | (shard.0 & 0x0FFF);
    let nb = new_low.to_be_bytes();
    b[14] = nb[0];
    b[15] = nb[1];
    Uuid::from_bytes(b)
}

/// Read the shard embedded in a task id. Returns `None` if the id is not a uuid.
pub fn shard_of_task_id(task_id: &str) -> Option<ShardId> {
    let id = Uuid::parse_str(task_id).ok()?;
    let b = id.as_bytes();
    let low = u16::from_be_bytes([b[14], b[15]]);
    Some(ShardId(low & 0x0FFF))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_shard_round_trips() {
        for _ in 0..1000 {
            let (id, shard) = new_task_uuid("q", None);
            assert_eq!(shard_of_task_id(&id.to_string()), Some(shard));
            assert_eq!(id.get_version_num(), 7);
        }
    }

    #[test]
    fn routing_key_pins_shard() {
        let (a, sa) = new_task_uuid("q", Some("customer-1"));
        let (b, sb) = new_task_uuid("q", Some("customer-1"));
        assert_ne!(a, b);
        assert_eq!(sa, sb);
    }

    #[test]
    fn ids_stay_time_sortable() {
        let (a, _) = new_task_uuid("q", None);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let (b, _) = new_task_uuid("q", None);
        assert!(a.to_string() < b.to_string());
    }
}
