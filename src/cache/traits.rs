use std::sync::Arc;

/// A thread-safe, size-bounded cache from keys to shared values.
///
/// Used for two jobs: the block cache (decoded SSTable blocks, charged by size
/// in bytes) and the table cache (open table files, charged 1 each).
///
/// # Charge and capacity
/// Every entry carries a `charge`, a cost in whatever unit the caller chooses
/// (bytes, entries, ...). The cache keeps the total charge of its entries at or
/// under its capacity, which is expressed in the same unit, by evicting entries
/// when it would otherwise go over.
///
/// # Ownership instead of handles
/// Values are handed out as `Arc<V>`. A reader keeps its value alive for as
/// long as it holds the `Arc`, even if the cache evicts or erases the entry in
/// the meantime, and the value is dropped when the last `Arc` goes away. There
/// is no explicit release step, and `V`'s `Drop` takes the place of a deleter
/// callback. Evicted entries that are still referenced no longer count towards
/// [`total_charge`](Self::total_charge).
///
/// All methods take `&self`, so a cache can be shared across threads as an
/// `Arc<dyn Cache<K, V>>`.
pub trait Cache<K, V>: Send + Sync {
    /// Inserts `value` under `key` with the given `charge`, replacing any
    /// existing entry for that key, and returns a shared reference to it.
    ///
    /// The value is returned even if the cache does not keep it, for example
    /// because `charge` exceeds the whole capacity or the eviction policy
    /// declines to admit it. The caller can always use what it just inserted.
    /// Inserting may evict other entries to stay within capacity.
    fn insert(&self, key: K, value: V, charge: usize) -> Arc<V>;

    /// Returns the value for `key`, if present, and records the access so the
    /// eviction policy sees the entry as recently or frequently used.
    fn get(&self, key: &K) -> Option<Arc<V>>;

    /// Removes the entry for `key` if there is one. Readers that already hold
    /// the value are unaffected.
    fn erase(&self, key: &K);

    /// Removes every entry. Values still held by readers stay alive until
    /// those readers drop them.
    fn prune(&self);

    /// Sum of the charges of the entries currently held by the cache.
    fn total_charge(&self) -> usize;

    /// The configured limit on [`total_charge`](Self::total_charge).
    fn capacity(&self) -> usize;

    /// Changes the capacity, evicting immediately if the cache is now over it.
    fn set_capacity(&self, capacity: usize);

    /// Returns an id that has never been returned by this cache before.
    ///
    /// Several clients can share one cache by putting their own id into their
    /// keys, so that identical offsets or numbers from different clients never
    /// collide. Safe to call from any thread.
    fn new_id(&self) -> u64;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time check that `Cache` can be used as a trait object, since
    /// `Options` will hold an `Arc<dyn Cache<K, V>>`.
    #[allow(dead_code)]
    fn cache_is_object_safe(_: &dyn Cache<u64, String>) {}
}
