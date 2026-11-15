//! The thread-safe [`Cache`]: a fixed set of independent shards, each behind its
//! own `Mutex`.
//!
//! A key is hashed once with `util::hash::hash_key`. The top four bits of that
//! hash pick one of 16 shards, and only that shard is locked, so threads that
//! touch different shards never contend. The same hash is passed on to the
//! shard and its eviction policy, so nothing hashes the key twice.
//!
//! Every method locks at most one shard at a time, and values removed from a
//! shard (evicted, replaced, erased, pruned) are dropped only after its lock is
//! released, so a `V::drop` that is slow or calls back into the cache cannot
//! deadlock.

use crate::cache::Cache;
use crate::cache::policy::EvictionPolicy;
use crate::cache::shard::Shard;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const SHARD_BITS: u32 = 4;
const NUM_SHARDS: usize = 1 << SHARD_BITS;

/// The capacity of each shard for a total of `total`, rounded up so the shards
/// add up to at least `total`. `div_ceil` cannot overflow, unlike
/// `(total + NUM_SHARDS - 1) / NUM_SHARDS`.
fn per_shard(total: usize) -> usize {
    total.div_ceil(NUM_SHARDS)
}

/// A [`Cache`] split into 16 shards, each with its own lock and its own
/// eviction policy `P`.
///
/// The total capacity is divided evenly between the shards, rounded up, so the
/// shards can hold slightly more than the requested total, and
/// [`capacity`](Cache::capacity) reports the total that was asked for. Each
/// shard enforces only its own share, so a skewed set of keys can start
/// evicting in one shard while others still have room.
/// [`total_charge`](Cache::total_charge) adds the shards up one at a time, so
/// under concurrent writes it is a snapshot, not an exact figure.
pub struct ShardedCache<K, V, P> {
    shards: Vec<Mutex<Shard<K, V, P>>>,
    capacity: AtomicUsize,
    last_id: AtomicU64,
}

impl<K: Eq + Hash + Clone + Send, V: Send + Sync, P: EvictionPolicy> ShardedCache<K, V, P> {
    pub fn new(capacity: usize) -> Self {
        ShardedCache {
            shards: (0..NUM_SHARDS)
                .map(|_| Mutex::new(Shard::new(per_shard(capacity))))
                .collect(),
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
}

impl<K: Eq + Hash + Clone + Send, V: Send + Sync, P: EvictionPolicy> Cache<K, V>
    for ShardedCache<K, V, P>
{
    fn insert(&self, key: K, value: V, charge: usize) -> Arc<V> {
        let mut dropped: Vec<Arc<V>> = Vec::new();
        let hash = self.hash_key(&key);
        let v = self
            .shard_for(hash)
            .lock()
            .unwrap()
            .insert(hash, key, value, charge, &mut dropped);
        // lock is released because we don't have any reference to it so we can drop safely
        drop(dropped); // this line is not really needed, added for the visual appeal
        v
    }

    fn get(&self, key: &K) -> Option<Arc<V>> {
        let hash = self.hash_key(key);
        self.shard_for(hash).lock().unwrap().get(hash, key)
    }

    fn erase(&self, key: &K) {
        let hash = self.hash_key(key);
        let _removed = self.shard_for(hash).lock().unwrap().erase(hash, key);
    }

    fn prune(&self) {
        let mut dropped: Vec<Arc<V>> = Vec::new();
        self.shards
            .iter()
            .flat_map(|shard| shard.lock().unwrap().prune())
            .for_each(|v| dropped.push(v));
    }

    fn total_charge(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.lock().unwrap().total_charge())
            .sum()
    }

    fn capacity(&self) -> usize {
        self.capacity.load(Ordering::Relaxed)
    }

    // Callers should serialize calls to this method.
    fn set_capacity(&self, capacity: usize) {
        let capacity_per_shard = per_shard(capacity);
        let mut dropped: Vec<Arc<V>> = Vec::new();
        self.shards.iter().for_each(|shard| {
            shard
                .lock()
                .unwrap()
                .set_capacity(capacity_per_shard, &mut dropped)
        });
        self.capacity.store(capacity, Ordering::Relaxed);
    }

    fn new_id(&self) -> u64 {
        self.last_id.fetch_add(1, Ordering::Relaxed) + 1
    }
}
