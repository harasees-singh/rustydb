//! One shard of the cache: the single-threaded core that does the real work.
//!
//! Plan: a hash table from key to `EntryId` keyed by the precomputed hash, a
//! slab of entries holding key, `Arc<V>` and charge, and one `EvictionPolicy`.
//! `insert`, `get`, `erase` and `prune` update all three together, and every id
//! the policy reports as evicted is removed from the table and the slab. The
//! whole shard sits behind one `Mutex` in `ShardedCache`.

use crate::cache::policy::{EntryId, EvictionPolicy};
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

struct Entry<K, V> {
    key: K,
    value: Arc<V>,
    charge: usize,
    hash: u64, // kept so the table can be rebuilt or rehashed without recomputing it
}

pub(crate) struct Shard<K, V, P> {
    table: HashMap<K, EntryId>,
    slab: Vec<Option<Entry<K, V>>>,
    free: Vec<u32>,
    policy: P,
    evicted: Vec<EntryId>,
}

impl<K: Hash + Eq + Clone + Copy, V, P: EvictionPolicy> Shard<K, V, P> {
    pub(crate) fn new(capacity: usize) -> Self {
        Shard {
            table: HashMap::new(),
            slab: Vec::new(),
            free: Vec::new(),
            policy: P::with_capacity(capacity),
            evicted: Vec::new(),
        }
    }

    pub(crate) fn insert(&mut self, hash: u64, key: K, value: V, charge: usize) -> Arc<V> {
        // first cleanup an existing key
        let v = Arc::new(value);
        let existing = self.table.get(&key).copied();
        if let Some(existing) = existing {
            self.remove_slot(existing);
            self.policy.remove(existing);
        }
        let free = self.free.pop();
        let entry = Option::from(Entry {
            key: key.clone(),
            value: Arc::clone(&v),
            charge,
            hash,
        });
        let mut idx = self.slab.len();
        if let Some(free_idx) = free {
            idx = free_idx as usize;
        } else {
            self.slab.push(None);
        }
        self.slab[idx] = entry;
        let entry_id = EntryId::new(idx as u32);
        self.table.insert(key, entry_id.clone());
        self.policy.insert(entry_id, hash, charge, self.evicted.as_mut());
        let mut evicted = std::mem::take(&mut self.evicted);
        for evict in evicted.drain(..) {
            self.remove_slot(evict);
        }
        v
    }
    pub(crate) fn get(&mut self, hash: u64, key: &K) -> Option<Arc<V>> {
        let entry_id = self.table.get(key)?;
        self.policy.access(*entry_id, hash);
        let entry: &Entry<K, V> = self.slab[entry_id.index()].as_ref()?;
        Some(Arc::clone(&entry.value))
    }
    pub(crate) fn erase(&mut self, hash: u64, key: &K) -> Option<Arc<V>> {
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
    pub(crate) fn set_capacity(&mut self, capacity: usize) {
        todo!()
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
