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
        for shard in &self.shards {
            let _values = shard.lock().unwrap().prune();
        }
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
        for shard in &self.shards {
            let mut dropped: Vec<Arc<V>> = Vec::new();
            shard
                .lock()
                .unwrap()
                .set_capacity(capacity_per_shard, &mut dropped);
        }
        self.capacity.store(capacity, Ordering::Relaxed);
    }

    fn new_id(&self) -> u64 {
        self.last_id.fetch_add(1, Ordering::Relaxed) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::policy::policy_impl::lru::LRUEvictionPolicy;
    use std::collections::HashSet;
    use std::sync::Weak;
    use std::sync::atomic::AtomicUsize;

    type TestCache<V> = ShardedCache<u32, V, LRUEvictionPolicy>;

    /// The shard a key lands in, computed independently of `shard_for`.
    fn shard_of(key: u32) -> usize {
        (crate::util::hash::hash_key(&key) >> (64 - SHARD_BITS)) as usize
    }

    /// The first `n` keys that land in `shard`.
    fn keys_in_shard(shard: usize, n: usize) -> Vec<u32> {
        (0u32..).filter(|&k| shard_of(k) == shard).take(n).collect()
    }

    /// Charge currently held by each shard.
    fn shard_charges<V: Send + Sync>(cache: &TestCache<V>) -> Vec<usize> {
        cache
            .shards
            .iter()
            .map(|s| s.lock().unwrap().total_charge())
            .collect()
    }

    #[test]
    fn per_shard_rounds_up_and_cannot_overflow() {
        assert_eq!(per_shard(0), 0);
        assert_eq!(per_shard(1), 1);
        assert_eq!(per_shard(16), 1);
        assert_eq!(per_shard(17), 2);
        assert_eq!(per_shard(100), 7);
        assert_eq!(per_shard(usize::MAX), usize::MAX / 16 + 1);
    }

    #[test]
    fn can_be_used_as_a_shared_trait_object() {
        let cache: Arc<dyn Cache<u32, String>> = Arc::new(TestCache::<String>::new(100));
        let other = Arc::clone(&cache);
        std::thread::spawn(move || {
            other.insert(1, "one".to_string(), 1);
        })
        .join()
        .unwrap();
        assert_eq!(cache.get(&1).as_deref().map(String::as_str), Some("one"));
    }

    #[test]
    fn insert_get_and_erase() {
        let cache = TestCache::<u32>::new(1000);
        let v = cache.insert(1, 10, 1);
        let got = cache.get(&1).unwrap();
        assert!(Arc::ptr_eq(&v, &got), "insert and get share one allocation");
        assert_eq!(*got, 10);
        assert!(cache.get(&2).is_none());

        cache.erase(&1);
        assert!(cache.get(&1).is_none());
        assert_eq!(*v, 10, "a reader keeps its value after an erase");
        cache.erase(&1); // erasing a missing key is a no-op
        cache.erase(&99);
    }

    #[test]
    fn inserting_an_existing_key_replaces_it() {
        let cache = TestCache::<u32>::new(1000);
        cache.insert(1, 10, 2);
        cache.insert(1, 20, 3);
        assert_eq!(cache.get(&1).as_deref(), Some(&20));
        assert_eq!(cache.total_charge(), 3, "only the new charge counts");
    }

    #[test]
    fn capacity_reports_the_requested_total_not_the_rounded_one() {
        let cache = TestCache::<u32>::new(100);
        assert_eq!(cache.capacity(), 100);
        let held: usize = cache
            .shards
            .iter()
            .map(|s| s.lock().unwrap().capacity())
            .sum();
        assert_eq!(held, NUM_SHARDS * per_shard(100), "112: shards round up");

        cache.set_capacity(37);
        assert_eq!(cache.capacity(), 37);
        cache.set_capacity(0);
        assert_eq!(cache.capacity(), 0);
    }

    #[test]
    fn keys_spread_over_all_shards() {
        let cache = TestCache::<u32>::new(1_000_000);
        for k in 0..2_000 {
            cache.insert(k, k, 1);
        }
        let charges = shard_charges(&cache);
        assert_eq!(charges.iter().sum::<usize>(), 2_000);
        assert_eq!(cache.total_charge(), 2_000);
        // 125 per shard on average: none empty, none badly overloaded.
        for (i, c) in charges.iter().enumerate() {
            assert!((60..250).contains(c), "shard {i} holds {c}");
        }
    }

    #[test]
    fn a_key_always_lands_in_the_shard_shard_of_predicts() {
        let cache = TestCache::<u32>::new(1_000_000);
        for k in 0..200 {
            cache.insert(k, k, 1);
        }
        for k in 0..200 {
            let id = *cache.shards[shard_of(k)]
                .lock()
                .unwrap()
                .get(0, &k)
                .expect("key is in its predicted shard");
            assert_eq!(id, k);
        }
    }

    #[test]
    fn total_charge_is_bounded_by_the_rounded_up_capacity() {
        let cache = TestCache::<u32>::new(100);
        for k in 0..10_000 {
            cache.insert(k, k, 1);
        }
        // Every shard is full at 7, so the cache holds 112, slightly over 100.
        assert_eq!(cache.total_charge(), NUM_SHARDS * per_shard(100));
        assert_eq!(cache.capacity(), 100);
    }

    #[test]
    fn eviction_is_per_shard() {
        let cache = TestCache::<u32>::new(32); // 2 per shard
        let [a, b, c] = keys_in_shard(3, 3)[..] else {
            unreachable!()
        };
        let d = keys_in_shard(5, 1)[0];

        cache.insert(a, 1, 1);
        cache.insert(b, 2, 1);
        cache.insert(d, 4, 1);
        cache.insert(c, 3, 1); // shard 3 is full: evicts a, leaves shard 5 alone

        assert!(cache.get(&a).is_none());
        assert!(cache.get(&b).is_some());
        assert!(cache.get(&c).is_some());
        assert!(cache.get(&d).is_some());
        assert_eq!(cache.total_charge(), 3);
    }

    #[test]
    fn get_makes_an_entry_recently_used() {
        let cache = TestCache::<u32>::new(32);
        let [a, b, c] = keys_in_shard(7, 3)[..] else {
            unreachable!()
        };
        cache.insert(a, 1, 1);
        cache.insert(b, 2, 1);
        assert!(cache.get(&a).is_some());
        cache.insert(c, 3, 1);
        assert!(cache.get(&a).is_some());
        assert!(cache.get(&b).is_none(), "b was the least recently used");
    }

    #[test]
    fn an_oversized_insert_is_returned_but_not_kept() {
        let cache = TestCache::<u32>::new(32); // 2 per shard
        let v = cache.insert(1, 7, 3);
        assert_eq!(*v, 7);
        assert!(cache.get(&1).is_none());
        assert_eq!(cache.total_charge(), 0);
    }

    #[test]
    fn prune_empties_every_shard_and_the_cache_is_reusable() {
        let cache = TestCache::<u32>::new(1_000_000);
        let held = cache.insert(5, 50, 1);
        for k in 0..500 {
            cache.insert(k, k, 2);
        }
        cache.prune();
        assert_eq!(cache.total_charge(), 0);
        assert!(shard_charges(&cache).iter().all(|&c| c == 0));
        assert!((0..500).all(|k| cache.get(&k).is_none()));
        assert_eq!(*held, 50, "a reader keeps its value after a prune");
        assert_eq!(cache.capacity(), 1_000_000, "prune keeps the capacity");

        cache.insert(1, 1, 1);
        assert_eq!(cache.get(&1).as_deref(), Some(&1));
    }

    #[test]
    fn shrinking_the_capacity_evicts_across_all_shards() {
        let cache = TestCache::<u32>::new(1_600); // 100 per shard
        for k in 0..10_000 {
            cache.insert(k, k, 1);
        }
        assert_eq!(cache.total_charge(), 1_600);

        cache.set_capacity(160); // 10 per shard
        assert_eq!(cache.capacity(), 160);
        assert_eq!(cache.total_charge(), 160);

        cache.set_capacity(1_600);
        assert_eq!(cache.total_charge(), 160, "growing evicts nothing");
        for k in 10_000..20_000 {
            cache.insert(k, k, 1);
        }
        assert_eq!(cache.total_charge(), 1_600, "and makes room again");
    }

    #[test]
    fn capacity_zero_keeps_nothing() {
        let cache = TestCache::<u32>::new(100);
        for k in 0..50 {
            cache.insert(k, k, 1);
        }
        cache.set_capacity(0);
        assert_eq!(cache.total_charge(), 0);
        cache.insert(1, 1, 1);
        assert!(cache.get(&1).is_none());
    }

    #[test]
    fn new_id_counts_up_from_one() {
        let cache = TestCache::<u32>::new(10);
        assert_eq!(cache.new_id(), 1);
        assert_eq!(cache.new_id(), 2);
        assert_eq!(cache.new_id(), 3);
    }

    #[test]
    fn new_id_is_unique_across_threads() {
        let cache = TestCache::<u32>::new(10);
        let ids: Vec<Vec<u64>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| s.spawn(|| (0..1_000).map(|_| cache.new_id()).collect::<Vec<_>>()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let all: HashSet<u64> = ids.iter().flatten().copied().collect();
        assert_eq!(all.len(), 8_000);
        assert_eq!(*all.iter().max().unwrap(), 8_000);
    }

    /// A value that checks, when it is dropped, that no shard lock is held.
    struct Probe {
        cache: Weak<TestCache<Probe>>,
        drops: Arc<AtomicUsize>,
        violations: Arc<AtomicUsize>,
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
            if let Some(cache) = self.cache.upgrade() {
                if cache.shards.iter().any(|s| s.try_lock().is_err()) {
                    self.violations.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    }

    /// A cache of probes plus the counters they report to.
    struct ProbeSetup {
        cache: Arc<TestCache<Probe>>,
        drops: Arc<AtomicUsize>,
        violations: Arc<AtomicUsize>,
    }

    impl ProbeSetup {
        fn new(capacity: usize) -> Self {
            ProbeSetup {
                cache: Arc::new(TestCache::new(capacity)),
                drops: Arc::new(AtomicUsize::new(0)),
                violations: Arc::new(AtomicUsize::new(0)),
            }
        }

        /// Inserts a probe and drops the returned `Arc`, so that the cache
        /// holds the only reference to the value.
        fn put(&self, key: u32, charge: usize) {
            let probe = Probe {
                cache: Arc::downgrade(&self.cache),
                drops: Arc::clone(&self.drops),
                violations: Arc::clone(&self.violations),
            };
            drop(self.cache.insert(key, probe, charge));
        }

        fn drops(&self) -> usize {
            self.drops.load(Ordering::SeqCst)
        }

        fn assert_no_drop_happened_under_a_lock(&self) {
            assert_eq!(self.violations.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn eviction_drops_values_after_the_lock_is_released() {
        let t = ProbeSetup::new(32); // 2 per shard
        let keys = keys_in_shard(2, 3);
        t.put(keys[0], 1);
        t.put(keys[1], 1);
        assert_eq!(t.drops(), 0);
        t.put(keys[2], 1); // evicts keys[0]
        assert_eq!(t.drops(), 1);
        t.assert_no_drop_happened_under_a_lock();
    }

    #[test]
    fn replacement_drops_the_old_value_after_the_lock_is_released() {
        let t = ProbeSetup::new(1_000);
        t.put(1, 1);
        t.put(1, 1);
        assert_eq!(t.drops(), 1);
        t.assert_no_drop_happened_under_a_lock();
    }

    #[test]
    fn a_rejected_insert_drops_after_the_lock_is_released() {
        let t = ProbeSetup::new(32);
        t.put(1, 3); // bigger than the 2 a shard can hold
        assert_eq!(t.drops(), 1);
        t.assert_no_drop_happened_under_a_lock();
    }

    #[test]
    fn erase_drops_the_value_after_the_lock_is_released() {
        let t = ProbeSetup::new(1_000);
        t.put(1, 1);
        t.cache.erase(&1);
        assert_eq!(t.drops(), 1);
        t.assert_no_drop_happened_under_a_lock();
    }

    #[test]
    fn prune_drops_values_after_the_locks_are_released() {
        let t = ProbeSetup::new(1_000_000);
        for k in 0..100 {
            t.put(k, 1);
        }
        t.cache.prune();
        assert_eq!(t.drops(), 100);
        t.assert_no_drop_happened_under_a_lock();
    }

    #[test]
    fn set_capacity_drops_values_after_the_locks_are_released() {
        let t = ProbeSetup::new(1_000_000);
        for k in 0..100 {
            t.put(k, 1);
        }
        t.cache.set_capacity(0);
        assert_eq!(t.drops(), 100);
        t.assert_no_drop_happened_under_a_lock();
    }

    #[test]
    fn concurrent_use_keeps_values_consistent_and_within_capacity() {
        let cache = TestCache::<u64>::new(100);
        std::thread::scope(|s| {
            for t in 0..8u64 {
                let cache = &cache;
                s.spawn(move || {
                    let mut state =
                        0x9E37_79B9_7F4A_7C15u64 ^ (t + 1).wrapping_mul(0xD1B5_4A32_D192_ED03);
                    let mut next = move || {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        state
                    };
                    for i in 0..20_000u64 {
                        let key = (next() % 200) as u32;
                        // A value always encodes the key it was stored under.
                        let value = key as u64 * 1_000_000 + i;
                        match next() % 20 {
                            0..=7 => {
                                let charge = (next() % 3 + 1) as usize;
                                let v = cache.insert(key, value, charge);
                                assert_eq!(*v / 1_000_000, key as u64);
                            }
                            8..=15 => {
                                if let Some(v) = cache.get(&key) {
                                    assert_eq!(*v / 1_000_000, key as u64);
                                }
                            }
                            16..=18 => cache.erase(&key),
                            _ => {
                                if next() % 50 == 0 {
                                    cache.prune();
                                } else {
                                    let _ = cache.total_charge();
                                }
                            }
                        }
                    }
                });
            }
        });

        assert!(cache.total_charge() <= NUM_SHARDS * per_shard(100));
        cache.prune();
        assert_eq!(cache.total_charge(), 0);
    }
}
