/// A cursor over a sorted sequence of key/value pairs.
///
/// A `Cursor` is positioned either on one entry or on nothing. A new cursor is
/// positioned on nothing: call [`seek_to_first`](Self::seek_to_first),
/// [`seek_to_last`](Self::seek_to_last) or [`seek`](Self::seek) before reading
/// from it.
///
/// Keys and values are byte strings, and entries are ordered by the comparator
/// of the structure behind the cursor (a block, a table, a memtable, ...). The
/// trait is implemented by the block, table, merging and DB iterators.
///
/// ```ignore
/// cursor.seek_to_first();
/// while cursor.valid() {
///     process(cursor.key(), cursor.value());
///     cursor.next();
/// }
/// cursor.status()?; // Ok: reached the end. Err: stopped early because of damage.
/// ```
///
/// # Why not `std::iter::Iterator`
/// `key` and `value` borrow from the cursor, which often rebuilds the key in
/// its own buffer, so they cannot be handed out as owned items. A cursor also
/// needs `seek` and `prev`.
///
/// Because the methods that move the cursor take `&mut self`, a slice returned
/// by [`key`](Self::key) or [`value`](Self::value) cannot be used after the
/// cursor moves. Copy it (`key().to_vec()`) if it must outlive the position.
///
/// # Errors
/// [`valid`](Self::valid) being `false` means either "past the end" or "stopped
/// because of an error". Call [`status`](Self::status) afterwards to tell them
/// apart.
///
/// # Threads
/// A cursor is driven by one thread at a time. Different cursors over the same
/// data can be used from different threads.
pub trait Cursor {
    /// Whether the cursor is positioned on an entry.
    ///
    /// It is `false` for a new cursor, after moving past either end, and after
    /// an error.
    fn valid(&self) -> bool;

    /// Positions the cursor on the first entry, or leaves it invalid if there
    /// are no entries.
    fn seek_to_first(&mut self);

    /// Positions the cursor on the last entry, or leaves it invalid if there are
    /// no entries.
    fn seek_to_last(&mut self);

    /// Positions the cursor on the first entry whose key is greater than or
    /// equal to `target`, or leaves it invalid if there is no such entry.
    ///
    /// `target` is compared with the same ordering as the entries.
    fn seek(&mut self, target: &[u8]);

    /// Moves to the next entry, or becomes invalid after the last one.
    ///
    /// The cursor must be [`valid`](Self::valid).
    fn next(&mut self);

    /// Moves to the previous entry, or becomes invalid before the first one.
    ///
    /// The cursor must be [`valid`](Self::valid). Going backwards can be slower
    /// than going forwards, because most formats only link entries forwards.
    fn prev(&mut self);

    /// The key of the current entry.
    ///
    /// The cursor must be [`valid`](Self::valid); implementations may panic
    /// otherwise. The slice is only usable until the cursor next moves.
    fn key(&self) -> &[u8];

    /// The value of the current entry.
    ///
    /// The cursor must be [`valid`](Self::valid); implementations may panic
    /// otherwise. The slice is only usable until the cursor next moves.
    fn value(&self) -> &[u8];

    /// `Ok(())` if the cursor has not hit an error, or the first error it hit.
    ///
    /// Once an error occurs the cursor becomes invalid and keeps returning that
    /// error (a clone of it), so a scan that ended early because of corruption
    /// can be told apart from one that reached the end.
    fn status(&self) -> crate::Result<()>;
}
