//! `ShardedCache<K, V, P>`: implements `Cache` on top of several shards.
//!
//! Plan: hash the key once with `util::hash`, pick a shard from the top bits of
//! the hash (16 shards, as in LevelDB) and lock only that shard. The total
//! capacity is split across shards, rounded up so the shards add up to at
//! least the requested capacity. `new_id` is an `AtomicU64` counter.

use std::hash::Hash;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex};
use crate::cache::policy::EvictionPolicy;
use crate::cache::shard::Shard;

const SHARD_BITS: u32 = 4;
const NUM_SHARDS: usize = 1 << SHARD_BITS;
pub struct ShardedCache <K, V, P> {
    shards: Vec<Mutex<Shard<K, V, P>>>,
    capacity: AtomicUsize,
    last_id: AtomicU64,
}

impl<K: Eq + Hash + Clone + Send, V: Send + Sync, P: EvictionPolicy> ShardedCache<K, V, P> {
    pub fn new(capacity: usize) -> Self {
        ShardedCache {
            shards: (0..=NUM_SHARDS).map(|_| Mutex::new(Shard::new((capacity + NUM_SHARDS - 1)/NUM_SHARDS))).collect(),
            capacity: AtomicUsize::new(capacity),
            last_id: AtomicU64::new(0),
        }
    }

    fn shard_for(&self, hash: u64) -> &Mutex<Shard<K, V, P>> {
        &self.shards[(hash >> (64 - SHARD_BITS)) as usize]
    }

    fn hash_key(&self, key: &K) -> u64 {
        crate::util::hash::hash_key(key)
    }

    fn insert(&self, key: K, value: V, charge: usize) -> Arc<V> {
        let mut dropped: Vec<Arc<V>> = Vec::new();
        let hash = self.hash_key(&key);
        let v = self.shard_for(hash).lock().unwrap().insert(hash, key, value, charge, &mut dropped);
        // lock is released because we don't have any reference to it so we can drop safely
        drop(dropped); // this line is not really needed, added for the visual appeal
        v
    }

    fn get(&self, key: &K) -> Option<Arc<V>> {
        let hash = self.hash_key(&key);
        self.shard_for(hash).lock().unwrap().get(hash, key)
    }

    fn erase(&self, key: &K) -> Option<Arc<V>> {
        let hash = self.hash_key(&key);
        self.shard_for(hash).lock().unwrap().erase(hash, key)
    }

    fn prune(&self) {
        let mut dropped: Vec<Arc<V>> = Vec::new();
        self.shards.iter()
            .flat_map(|shard| shard.lock().unwrap().prune())
            .for_each(|v| dropped.push(v));
    }
}