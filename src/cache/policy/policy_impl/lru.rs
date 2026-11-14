use crate::cache::policy::{EntryId, EvictionPolicy};

/// Least-recently-used eviction.
pub struct LRUEvictionPolicy;

impl EvictionPolicy for LRUEvictionPolicy {
    fn with_capacity(capacity: usize) -> Self
    where
        Self: Sized,
    {
        todo!()
    }

    fn insert(&mut self, id: EntryId, hash: u64, charge: usize, evicted: &mut Vec<EntryId>) {
        todo!()
    }

    fn access(&mut self, id: EntryId, hash: u64) {
        todo!()
    }

    fn remove(&mut self, id: EntryId) {
        todo!()
    }

    fn clear(&mut self) {
        todo!()
    }

    fn set_capacity(&mut self, capacity: usize, evicted: &mut Vec<EntryId>) {
        todo!()
    }

    fn charge(&self) -> usize {
        todo!()
    }

    fn capacity(&self) -> usize {
        todo!()
    }
}
