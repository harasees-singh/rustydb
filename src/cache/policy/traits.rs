use crate::cache::policy::EntryId;

/// Decides which entries a shard should evict to stay within its capacity.
///
/// The shard owns the keys, values and hash table and calls into the policy
/// on every insert, hit and removal. Each shard has its own policy behind its
/// own lock, so implementations need not be thread-safe, only [`Send`].
///
/// # Contract
/// * After any method returns, [`charge`](Self::charge) is at most
///   [`capacity`](Self::capacity).
/// * Ids reported through `evicted` are no longer tracked by the policy, and
///   the shard must remove the matching entries from its own storage.
/// * The shard only passes ids it has inserted and not yet removed or
///   evicted, except for `insert`, which takes a new id.
pub trait EvictionPolicy: Send {
    /// Creates an empty policy with room for `capacity` units of charge.
    fn with_capacity(capacity: usize) -> Self
    where
        Self: Sized;

    /// Starts tracking a new entry and appends any ids that must be evicted to
    /// make room to `evicted`. The vector is only appended to, never cleared.
    ///
    /// `hash` is the key's 64-bit hash, computed once by the cache, for
    /// policies that keep frequency statistics. `evicted` may include `id`
    /// itself when the newcomer is rejected, either because its `charge` is
    /// larger than the whole capacity or because the policy judges existing
    /// entries more valuable.
    fn insert(&mut self, id: EntryId, hash: u64, charge: usize, evicted: &mut Vec<EntryId>);

    /// Records a cache hit on `id`.
    fn access(&mut self, id: EntryId, hash: u64);

    /// Forgets `id` because the shard removed it (erase or overwrite). It is
    /// not reported through `evicted`, because the shard already knows.
    fn remove(&mut self, id: EntryId);

    /// Forgets every entry.
    fn clear(&mut self);

    /// Changes the capacity, appending to `evicted` any ids that no longer fit.
    fn set_capacity(&mut self, capacity: usize, evicted: &mut Vec<EntryId>);

    /// Total charge of the entries currently tracked.
    fn charge(&self) -> usize;

    /// The current capacity.
    fn capacity(&self) -> usize;
}
