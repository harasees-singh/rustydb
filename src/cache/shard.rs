//! One shard of the cache: the single-threaded core that does the real work.
//!
//! Plan: a hash table from key to `EntryId` keyed by the precomputed hash, a
//! slab of entries holding key, `Arc<V>` and charge, and one `EvictionPolicy`.
//! `insert`, `get`, `erase` and `prune` update all three together, and every id
//! the policy reports as evicted is removed from the table and the slab. The
//! whole shard sits behind one `Mutex` in `ShardedCache`.
