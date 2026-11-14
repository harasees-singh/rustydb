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
