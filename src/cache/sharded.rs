//! `ShardedCache<K, V, P>`: implements `Cache` on top of several shards.
//!
//! Plan: hash the key once with `util::hash`, pick a shard from the top bits of
//! the hash (16 shards, as in LevelDB) and lock only that shard. The total
//! capacity is split across shards, rounded up so the shards add up to at
//! least the requested capacity. `new_id` is an `AtomicU64` counter.
