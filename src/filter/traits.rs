/// A policy for building and querying per-table key filters.
///
/// A filter is a compact summary of the keys in a group of data blocks. It lets
/// a lookup skip reading a block that cannot contain the key, which matters
/// most for keys that are absent from the table.
///
/// The contract is one-sided: a filter may say "maybe" for a key that is not
/// present (a false positive, which only costs an unnecessary read), but it
/// must never say "no" for a key that was used to build it (a false negative,
/// which would make a stored key invisible).
///
/// Implementations are shared across threads (readers, compaction), so they
/// should be immutable after construction.
pub trait FilterPolicy: Send + Sync {
    /// Name of this filter format.
    ///
    /// It is persisted with the table, and a reader only trusts a filter whose
    /// stored name matches the configured policy. Change the name whenever the
    /// filter encoding changes incompatibly, and never change it otherwise.
    fn name(&self) -> &'static str;

    /// Builds a filter covering `keys` and **appends** it to `dst`.
    ///
    /// `keys` borrows the caller's key bytes, so nothing is copied per key. It
    /// may be empty, in which case the appended filter must still be one that
    /// [`key_may_match`](Self::key_may_match) can read. Duplicate keys are
    /// allowed.
    ///
    /// `dst` is appended to, never cleared, because a table writes many
    /// filters one after another into the same buffer. Whatever `dst` already
    /// holds must be left untouched.
    fn create_filter(&self, keys: &[&[u8]], dst: &mut Vec<u8>);

    /// Returns `false` only if `key` is definitely not in the set the `filter`
    /// was built from, and `true` if it might be.
    ///
    /// For every key passed to [`create_filter`](Self::create_filter) this must
    /// return `true` when given the filter that call produced. That is the
    /// no-false-negatives guarantee.
    ///
    /// `filter` is the bytes one call to `create_filter` appended, and may come
    /// from disk, so it can be empty, truncated or corrupt. An unusable filter
    /// must be answered with `true` ("no information") rather than panicking
    /// or returning `false`.
    fn key_may_match(&self, key: &[u8], filter: &[u8]) -> bool;
}
