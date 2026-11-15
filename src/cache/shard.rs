//! One shard of the cache: the single-threaded core that does the real work.
//!
//! Three structures are kept in step: a `HashMap` from key to `EntryId`, a slab
//! of entries (key and `Arc<V>`) addressed by `EntryId`, and one
//! `EvictionPolicy`, which tracks only ids and charges. `insert`, `get`,
//! `erase`, `set_capacity` and `prune` update all three, and every id the
//! policy reports as evicted is removed from the table and the slab.
//!
//! Every value the shard removes is handed back to the caller instead of being
//! dropped here, so that no `Drop` runs while the shard's lock is held. The
//! shard sits behind one `Mutex` in `ShardedCache`.

use crate::cache::policy::{EntryId, EvictionPolicy};
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

struct Entry<K, V> {
    key: K,
    value: Arc<V>,
}

pub(crate) struct Shard<K, V, P> {
    table: HashMap<K, EntryId>,
    slab: Vec<Option<Entry<K, V>>>,
    free: Vec<u32>,
    policy: P,
    evicted: Vec<EntryId>,
}

impl<K: Hash + Eq + Clone, V, P: EvictionPolicy> Shard<K, V, P> {
    pub(crate) fn new(capacity: usize) -> Self {
        Shard {
            table: HashMap::new(),
            slab: Vec::new(),
            free: Vec::new(),
            policy: P::with_capacity(capacity),
            evicted: Vec::new(),
        }
    }

    /// Inserts `value` under `key` and returns a shared reference to it.
    ///
    /// Values removed to make room (the entry this one replaces, and anything
    /// the policy evicts, which can include the new entry itself if it is
    /// rejected) are appended to `dropped`. The caller must drop them after
    /// releasing the shard's lock, so no `Drop` runs while it is held.
    pub(crate) fn insert(
        &mut self,
        hash: u64,
        key: K,
        value: V,
        charge: usize,
        dropped: &mut Vec<Arc<V>>,
    ) -> Arc<V> {
        let value = Arc::new(value);

        // Replace an existing entry for this key, if there is one.
        if let Some(old) = self.table.get(&key).copied() {
            self.policy.remove(old);
            if let Some(entry) = self.remove_slot(old) {
                dropped.push(entry.value);
            }
        }

        // Put the new entry in a free slot, or grow the slab.
        let entry = Some(Entry {
            key: key.clone(),
            value: Arc::clone(&value),
        });
        let idx = match self.free.pop() {
            Some(i) => {
                self.slab[i as usize] = entry;
                i as usize
            }
            None => {
                self.slab.push(entry);
                self.slab.len() - 1
            }
        };
        let id = EntryId::new(idx as u32);
        self.table.insert(key, id);

        // Let the policy make room, then remove whatever it evicted.
        let mut evicted = std::mem::take(&mut self.evicted);
        let admitted = self.policy.insert(id, hash, charge, &mut evicted);
        // Contract: a rejected id is reported through `evicted` as well, so the
        // loop below removes it. A policy that returned `false` without doing so
        // would leave an untracked entry behind.
        debug_assert_eq!(
            admitted,
            !evicted.contains(&id),
            "policy rejected the entry without reporting it as evicted"
        );
        for evict in evicted.drain(..) {
            if let Some(entry) = self.remove_slot(evict) {
                dropped.push(entry.value);
            }
        }
        self.evicted = evicted; // drained, so this keeps the buffer's capacity for next time

        value
    }
    pub(crate) fn get(&mut self, hash: u64, key: &K) -> Option<Arc<V>> {
        let entry_id = self.table.get(key)?;
        self.policy.access(*entry_id, hash);
        let entry: &Entry<K, V> = self.slab[entry_id.index()].as_ref()?;

        Some(Arc::clone(&entry.value))
    }
    pub(crate) fn erase(&mut self, _hash: u64, key: &K) -> Option<Arc<V>> {
        let entry_id = self.table.get(key)?;
        self.policy.remove(*entry_id);

        self.remove_slot(*entry_id).map(|e| e.value)
    }
    pub(crate) fn prune(&mut self) -> Vec<Arc<V>> {
        self.table.clear();
        self.free.clear();
        self.policy.clear();
        self.evicted.clear();

        self.slab.drain(..).flatten().map(|e| e.value).collect() // let the caller drop the individual values after unlocking to avoid deadlocks
    }

    pub(crate) fn total_charge(&self) -> usize {
        self.policy.charge()
    }
    pub(crate) fn capacity(&self) -> usize {
        self.policy.capacity()
    }
    pub(crate) fn set_capacity(&mut self, capacity: usize, dropped: &mut Vec<Arc<V>>) {
        self.policy.set_capacity(capacity, &mut self.evicted);
        let mut evicted = std::mem::take(&mut self.evicted);

        evicted
            .drain(..)
            .flat_map(|e| self.remove_slot(e))
            .for_each(|e| dropped.push(e.value));

        self.evicted = evicted;
    }

    /// Frees slot `id`: slab, free list and table. Does NOT notify the policy:
    /// callers that remove on their own (erase, overwrite) must call
    /// `policy.remove(id)`, while eviction already did it for them.
    fn remove_slot(&mut self, id: EntryId) -> Option<Entry<K, V>> {
        let entry = self.slab[id.index()].take()?;
        self.free.push(id.index() as u32);
        let removed = self.table.remove(&entry.key);
        debug_assert_eq!(removed, Some(id), "table and slab disagree");

        Some(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::policy::policy_impl::lru::LRUEvictionPolicy;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type TestShard = Shard<u32, u32, LRUEvictionPolicy>;

    /// Inserts and returns the new value and everything the shard handed back.
    fn put(
        s: &mut TestShard,
        key: u32,
        value: u32,
        charge: usize,
    ) -> (Arc<u32>, Vec<u32>, Vec<Arc<u32>>) {
        let mut dropped = Vec::new();
        let v = s.insert(key as u64, key, value, charge, &mut dropped);
        check(s);
        let values = dropped.iter().map(|v| **v).collect();
        (v, values, dropped)
    }

    /// Values handed back by an insert, in order.
    fn evicted(s: &mut TestShard, key: u32, value: u32, charge: usize) -> Vec<u32> {
        put(s, key, value, charge).1
    }

    fn get(s: &mut TestShard, key: u32) -> Option<u32> {
        s.get(key as u64, &key).map(|v| *v)
    }

    fn erase(s: &mut TestShard, key: u32) -> Option<u32> {
        let r = s.erase(key as u64, &key).map(|v| *v);
        check(s);
        r
    }

    /// Reads a value without counting as an access.
    fn peek(s: &TestShard, key: u32) -> Option<u32> {
        let id = s.table.get(&key)?;
        s.slab[id.index()].as_ref().map(|e| *e.value)
    }

    /// Checks that the table, slab, free list and policy agree.
    fn check(s: &TestShard) {
        let live: Vec<usize> = (0..s.slab.len()).filter(|&i| s.slab[i].is_some()).collect();
        assert_eq!(s.table.len(), live.len(), "table and slab sizes differ");
        for (key, id) in &s.table {
            let entry = s.slab[id.index()]
                .as_ref()
                .expect("table points at an empty slot");
            assert_eq!(&entry.key, key);
        }

        let mut free = s.free.clone();
        free.sort();
        free.dedup();
        assert_eq!(free.len(), s.free.len(), "duplicate slot in the free list");
        assert!(
            free.iter().all(|&i| s.slab[i as usize].is_none()),
            "free slot is occupied"
        );
        assert_eq!(
            free.len() + live.len(),
            s.slab.len(),
            "a slot is neither live nor free"
        );

        assert!(s.evicted.is_empty(), "scratch buffer was not drained");
        assert!(s.total_charge() <= s.capacity());
    }

    #[test]
    fn starts_empty() {
        let s = TestShard::new(10);
        assert_eq!(s.total_charge(), 0);
        assert_eq!(s.capacity(), 10);
        check(&s);
    }

    #[test]
    fn get_returns_what_insert_stored() {
        let mut s = TestShard::new(10);
        let (v, dropped, _) = put(&mut s, 1, 100, 1);
        assert!(dropped.is_empty());
        let got = s.get(1, &1).unwrap();
        assert!(Arc::ptr_eq(&v, &got), "insert and get share one allocation");
        assert_eq!(*got, 100);
        assert_eq!(get(&mut s, 2), None);
        assert_eq!(s.total_charge(), 1);
    }

    #[test]
    fn inserting_an_existing_key_replaces_it() {
        let mut s = TestShard::new(10);
        put(&mut s, 1, 100, 2);
        let (_, dropped, _) = put(&mut s, 1, 200, 3);
        assert_eq!(dropped, [100], "the old value is handed back");
        assert_eq!(get(&mut s, 1), Some(200));
        assert_eq!(s.total_charge(), 3, "only the new charge counts");
        assert_eq!(s.table.len(), 1);
    }

    #[test]
    fn replacing_with_an_oversized_value_removes_the_old_one_too() {
        let mut s = TestShard::new(5);
        put(&mut s, 1, 100, 2);
        let (v, dropped, raw) = put(&mut s, 1, 200, 6);
        assert_eq!(dropped, [100, 200]);
        assert!(Arc::ptr_eq(&v, &raw[1]));
        assert_eq!(get(&mut s, 1), None);
        assert_eq!(s.total_charge(), 0);
    }

    #[test]
    fn evicts_the_least_recently_used_entry() {
        let mut s = TestShard::new(3);
        for k in 0..3 {
            assert!(evicted(&mut s, k, k * 10, 1).is_empty());
        }
        assert_eq!(evicted(&mut s, 3, 30, 1), [0]);
        assert_eq!(evicted(&mut s, 4, 40, 1), [10]);
        assert_eq!(get(&mut s, 0), None);
        assert_eq!(get(&mut s, 1), None);
        assert_eq!(get(&mut s, 2), Some(20));
        assert_eq!(s.total_charge(), 3);
    }

    #[test]
    fn get_makes_an_entry_recently_used() {
        let mut s = TestShard::new(3);
        for k in 0..3 {
            put(&mut s, k, k * 10, 1);
        }
        assert_eq!(get(&mut s, 0), Some(0));
        assert_eq!(evicted(&mut s, 3, 30, 1), [10], "key 1 is now the oldest");
        assert_eq!(peek(&s, 0), Some(0));
    }

    #[test]
    fn a_missing_get_changes_nothing() {
        let mut s = TestShard::new(2);
        put(&mut s, 0, 0, 1);
        put(&mut s, 1, 10, 1);
        assert_eq!(get(&mut s, 9), None);
        assert_eq!(evicted(&mut s, 2, 20, 1), [0]);
    }

    #[test]
    fn one_large_insert_evicts_several_entries() {
        let mut s = TestShard::new(5);
        for k in 0..5 {
            put(&mut s, k, k * 10, 1);
        }
        assert_eq!(evicted(&mut s, 9, 90, 3), [0, 10, 20]);
        assert_eq!(s.total_charge(), 5);
    }

    #[test]
    fn a_rejected_insert_still_returns_the_value_and_is_not_stored() {
        let mut s = TestShard::new(5);
        let (v, dropped, raw) = put(&mut s, 1, 7, 6);
        assert_eq!(*v, 7);
        assert_eq!(dropped, [7]);
        assert!(Arc::ptr_eq(&v, &raw[0]));
        assert_eq!(peek(&s, 1), None);
        assert_eq!(s.total_charge(), 0);
        assert_eq!(s.table.len(), 0);
    }

    #[test]
    fn a_rejected_insert_does_not_disturb_existing_entries() {
        let mut s = TestShard::new(5);
        put(&mut s, 1, 10, 2);
        put(&mut s, 2, 20, 2);
        assert_eq!(evicted(&mut s, 3, 30, 9), [30]);
        assert_eq!(peek(&s, 1), Some(10));
        assert_eq!(peek(&s, 2), Some(20));
        assert_eq!(s.total_charge(), 4);
    }

    #[test]
    fn erase_removes_and_returns_the_value() {
        let mut s = TestShard::new(10);
        put(&mut s, 1, 10, 3);
        put(&mut s, 2, 20, 4);
        assert_eq!(erase(&mut s, 1), Some(10));
        assert_eq!(peek(&s, 1), None);
        assert_eq!(s.total_charge(), 4);
        assert_eq!(erase(&mut s, 1), None, "second erase finds nothing");
        assert_eq!(erase(&mut s, 7), None);
    }

    #[test]
    fn erase_frees_room_so_the_next_insert_evicts_nothing() {
        let mut s = TestShard::new(2);
        put(&mut s, 1, 10, 1);
        put(&mut s, 2, 20, 1);
        erase(&mut s, 1);
        assert!(evicted(&mut s, 3, 30, 1).is_empty());
        assert_eq!(peek(&s, 2), Some(20));
    }

    #[test]
    fn freed_slots_are_reused_so_the_slab_does_not_grow() {
        let mut s = TestShard::new(10);
        for i in 0..100 {
            put(&mut s, 1, i, 1);
            erase(&mut s, 1);
        }
        assert_eq!(s.slab.len(), 1);

        let mut s = TestShard::new(3);
        for k in 0..50 {
            put(&mut s, k, k, 1); // each insert past 3 evicts one
        }
        assert!(s.slab.len() <= 4, "slab grew to {}", s.slab.len());
    }

    #[test]
    fn a_value_held_by_the_caller_outlives_its_eviction() {
        let mut s = TestShard::new(1);
        let (a, _, _) = put(&mut s, 1, 11, 1);
        let (_b, dropped, raw) = put(&mut s, 2, 22, 1);
        assert_eq!(dropped, [11]);
        assert!(Arc::ptr_eq(&a, &raw[0]));
        drop(raw);
        assert_eq!(*a, 11, "still readable after eviction");
        assert_eq!(Arc::strong_count(&a), 1, "the shard no longer holds it");
        assert_eq!(s.total_charge(), 1, "evicted entries stop counting");
    }

    #[test]
    fn prune_hands_back_everything_and_the_shard_is_reusable() {
        let mut s = TestShard::new(10);
        for k in 0..4 {
            put(&mut s, k, k * 10, 2);
        }
        let mut values: Vec<u32> = s.prune().iter().map(|v| **v).collect();
        values.sort();
        assert_eq!(values, [0, 10, 20, 30]);
        check(&s);
        assert_eq!(s.total_charge(), 0);
        assert_eq!(s.capacity(), 10, "prune keeps the capacity");
        assert!(s.table.is_empty() && s.slab.is_empty() && s.free.is_empty());

        assert!(evicted(&mut s, 5, 50, 1).is_empty());
        assert_eq!(get(&mut s, 5), Some(50));
        assert_eq!(s.slab.len(), 1);
    }

    #[test]
    fn prune_on_an_empty_shard_returns_nothing() {
        let mut s = TestShard::new(10);
        assert!(s.prune().is_empty());
        check(&s);
    }

    #[test]
    fn prune_skips_slots_that_were_freed() {
        let mut s = TestShard::new(10);
        for k in 0..3 {
            put(&mut s, k, k, 1);
        }
        erase(&mut s, 1);
        assert_eq!(s.prune().len(), 2);
    }

    #[test]
    fn shrinking_the_capacity_hands_back_the_least_recently_used() {
        let mut s = TestShard::new(5);
        for k in 0..5 {
            put(&mut s, k, k * 10, 1);
        }
        get(&mut s, 0);
        let mut dropped = Vec::new();
        s.set_capacity(2, &mut dropped);
        check(&s);
        let values: Vec<u32> = dropped.iter().map(|v| **v).collect();
        assert_eq!(values, [10, 20, 30]);
        assert_eq!(s.capacity(), 2);
        assert_eq!(s.total_charge(), 2);
        assert_eq!(peek(&s, 0), Some(0));
        assert_eq!(peek(&s, 4), Some(40));
    }

    #[test]
    fn growing_the_capacity_keeps_everything_and_makes_room() {
        let mut s = TestShard::new(2);
        put(&mut s, 0, 0, 1);
        put(&mut s, 1, 10, 1);
        let mut dropped = Vec::new();
        s.set_capacity(4, &mut dropped);
        assert!(dropped.is_empty());
        assert!(evicted(&mut s, 2, 20, 1).is_empty());
        assert!(evicted(&mut s, 3, 30, 1).is_empty());
        assert_eq!(s.total_charge(), 4);
    }

    #[test]
    fn capacity_zero_empties_the_shard_and_rejects_new_entries() {
        let mut s = TestShard::new(3);
        for k in 0..3 {
            put(&mut s, k, k, 1);
        }
        let mut dropped = Vec::new();
        s.set_capacity(0, &mut dropped);
        check(&s);
        assert_eq!(dropped.len(), 3);
        assert!(s.table.is_empty());
        assert_eq!(evicted(&mut s, 9, 9, 1), [9]);
    }

    /// A value that counts how many times it has been dropped.
    struct Counted(Arc<AtomicUsize>);

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn no_value_is_dropped_inside_the_shard() {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut s: Shard<u32, Counted, LRUEvictionPolicy> = Shard::new(2);
        let mut dropped = Vec::new();

        // Fill, then overflow: the eviction must hand the value back, not drop it.
        for k in 0..2 {
            drop(s.insert(k as u64, k, Counted(drops.clone()), 1, &mut dropped));
        }
        drop(s.insert(2, 2, Counted(drops.clone()), 1, &mut dropped));
        assert_eq!(dropped.len(), 1);
        assert_eq!(
            drops.load(Ordering::SeqCst),
            0,
            "evicted value dropped under the lock"
        );
        dropped.clear();
        assert_eq!(drops.load(Ordering::SeqCst), 1);

        // Replacement.
        drop(s.insert(2, 2, Counted(drops.clone()), 1, &mut dropped));
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        dropped.clear();
        assert_eq!(drops.load(Ordering::SeqCst), 2);

        // Rejection (the new value itself comes back).
        drop(s.insert(7, 7, Counted(drops.clone()), 9, &mut dropped));
        assert_eq!(drops.load(Ordering::SeqCst), 2);
        dropped.clear();
        assert_eq!(drops.load(Ordering::SeqCst), 3);

        // Erase and shrink.
        let erased = s.erase(1, &1);
        assert_eq!(drops.load(Ordering::SeqCst), 3);
        drop(erased);
        assert_eq!(drops.load(Ordering::SeqCst), 4);
        s.set_capacity(0, &mut dropped);
        assert_eq!(drops.load(Ordering::SeqCst), 4);
        dropped.clear();
        assert_eq!(drops.load(Ordering::SeqCst), 5);

        // Prune.
        for k in 0..2 {
            s.set_capacity(5, &mut dropped);
            drop(s.insert(k as u64, k, Counted(drops.clone()), 1, &mut dropped));
        }
        let pruned = s.prune();
        assert_eq!(drops.load(Ordering::SeqCst), 5);
        drop(pruned);
        assert_eq!(drops.load(Ordering::SeqCst), 7);
    }

    #[test]
    fn matches_a_simple_lru_model_under_random_operations() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        let mut capacity = 12usize;
        let mut s = TestShard::new(capacity);
        // (key, value, charge), front = most recently used.
        let mut model: VecDeque<(u32, u32, usize)> = VecDeque::new();
        let total = |m: &VecDeque<(u32, u32, usize)>| m.iter().map(|e| e.2).sum::<usize>();

        for step in 0..50_000u32 {
            let key = (next() % 20) as u32;
            match next() % 10 {
                0..=4 => {
                    let charge = (next() % 5) as usize;
                    let got = evicted(&mut s, key, step, charge);

                    let mut expected = Vec::new();
                    if let Some(pos) = model.iter().position(|e| e.0 == key) {
                        expected.push(model.remove(pos).unwrap().1);
                    }
                    if charge > capacity {
                        expected.push(step);
                    } else {
                        let mut sum = total(&model);
                        while sum + charge > capacity {
                            let victim = model.pop_back().unwrap();
                            sum -= victim.2;
                            expected.push(victim.1);
                        }
                        model.push_front((key, step, charge));
                    }
                    assert_eq!(got, expected, "insert, step {step}");
                }
                5..=7 => {
                    let expected = model.iter().position(|e| e.0 == key).map(|pos| {
                        let entry = model.remove(pos).unwrap();
                        model.push_front(entry);
                        entry.1
                    });
                    assert_eq!(get(&mut s, key), expected, "get, step {step}");
                    check(&s);
                }
                8 => {
                    let expected = model
                        .iter()
                        .position(|e| e.0 == key)
                        .map(|pos| model.remove(pos).unwrap().1);
                    assert_eq!(erase(&mut s, key), expected, "erase, step {step}");
                }
                _ => {
                    if next() % 10 != 0 {
                        continue;
                    }
                    capacity = (next() % 16) as usize;
                    let mut dropped = Vec::new();
                    s.set_capacity(capacity, &mut dropped);
                    check(&s);

                    let mut expected = Vec::new();
                    while total(&model) > capacity {
                        expected.push(model.pop_back().unwrap().1);
                    }
                    let got: Vec<u32> = dropped.iter().map(|v| **v).collect();
                    assert_eq!(got, expected, "set_capacity, step {step}");
                }
            }

            assert_eq!(s.total_charge(), total(&model), "charge, step {step}");
            for k in 0..20 {
                let expected = model.iter().find(|e| e.0 == k).map(|e| e.1);
                assert_eq!(peek(&s, k), expected, "key {k}, step {step}");
            }
        }
    }
}
