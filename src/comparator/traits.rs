use std::cmp::Ordering;

/// Defines the total order over keys used by the whole database.
///
/// Implementations are shared across threads (writer, compaction, readers),
/// so they must be `Send + Sync` and should be stateless or immutable.
pub trait Comparator: Send + Sync {
    /// Three-way comparison of two keys.
    ///
    /// Must be a consistent total order: antisymmetric, transitive, and the
    /// same result for the same inputs every time. Breaking this silently
    /// corrupts lookups and compactions.
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering;

    /// Name of this ordering.
    ///
    /// It is persisted with the database, and opening an existing database
    /// with a comparator of a different name is refused. Change the name
    /// whenever the ordering changes in an incompatible way, and never change
    /// it otherwise.
    fn name(&self) -> &'static str;

    /// Shrinks `start` to a shorter key that still separates it from `limit`.
    ///
    /// Called with `start < limit`, where `start` is the last key of one block
    /// and `limit` the first key of the next. On return `start` must satisfy
    /// `original_start <= start < limit` under [`compare`](Self::compare).
    ///
    /// This is only an optimisation that shrinks index blocks. Leaving `start`
    /// unchanged is always valid, and the default does exactly that. The
    /// result need not be the shortest possible.
    fn find_shortest_separator(&self, start: &mut Vec<u8>, limit: &[u8]) {}

    /// Shrinks `key` to a shorter key that is `>=` the original.
    ///
    /// Used for the last block of a table, which has no next key to stay
    /// below. On return `key` must be `>=` its original value under
    /// [`compare`](Self::compare).
    ///
    /// Like [`find_shortest_separator`](Self::find_shortest_separator) this is
    /// optional, and leaving `key` unchanged is always valid.
    fn find_short_successor(&self, key: &mut Vec<u8>) {}
}
