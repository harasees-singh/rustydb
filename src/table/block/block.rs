//! Reads a block written by [`BlockBuilder`](super::block_builder::BlockBuilder).
//!
//! [`Block`] holds the validated bytes and [`BlockIter`] is a
//! [`Cursor`](crate::iterator::traits::Cursor) over its entries. See the
//! builder's module docs for the byte format: prefix-compressed entries, a
//! restart point every `restart_interval` entries, then the restart array and
//! its length.
//!
//! # How a read works
//! * **Forward.** Decode one entry at a time, rebuilding each key from the
//!   previous one (`truncate(shared)`, then append the stored bytes).
//! * **Seek.** Binary-search the restart array, whose entries are full keys, for
//!   the last restart whose key is less than the target, then scan forward.
//! * **Backward.** Entries only point forwards, so `prev` goes back to a restart
//!   point before the current entry and scans forward to just before it.
//!
//! Blocks come from disk, so nothing in them is trusted: every length and
//! offset is bounds-checked, and damage is reported as
//! [`Error::Corruption`](crate::Error::Corruption), never as a panic.

use crate::comparator::Comparator;
use std::range::Range;
use std::sync::Arc;

/// An immutable, validated block.
///
/// A `Block` is shared as an `Arc<Block>`: the block cache stores it that way
/// (charged [`size`](Self::size) bytes) and every [`BlockIter`] keeps a clone,
/// so a block stays alive while an iterator is reading it even if the cache
/// evicts it.
pub struct Block {
    /// The whole block: entries, restart array and restart count.
    data: Vec<u8>,
    /// Offset where the restart array starts, which is also where the entries
    /// end.
    restarts_offset: usize,
    /// The number of restart points.
    restarts_count: u32,
}

impl Block {
    /// Takes ownership of the bytes of a block and checks its trailer.
    ///
    /// # Errors
    /// [`Error::Corruption`](crate::Error::Corruption) if the data is too short
    /// to hold the restart count, or the restart array the count describes does
    /// not fit in it. Only the trailer is checked here. Individual entries are
    /// checked when an iterator decodes them.
    pub fn new(data: Vec<u8>) -> crate::Result<Block> {
        todo!()
    }

    /// The size of the block in bytes, used as its cost in the block cache.
    pub fn size(&self) -> usize {
        todo!()
    }

    /// Creates a cursor over the block, which starts out not positioned on any
    /// entry.
    ///
    /// `cmp` must be the comparator the keys were sorted with when the block was
    /// built. A different one makes `seek` return wrong results.
    pub fn iter(self: &Arc<Self>, cmp: Arc<dyn Comparator>) -> BlockIter {
        todo!()
    }
}

/// A cursor over the entries of a [`Block`].
///
/// Create one with [`Block::iter`]. If it meets a malformed entry it becomes
/// invalid and [`status`](crate::iterator::traits::Cursor::status) returns a
/// corruption error.
pub struct BlockIter {
    /// The block being read, kept alive by this reference.
    block: Arc<Block>,
    /// Orders keys for `seek`.
    cmp: Arc<dyn Comparator>,
    /// Offset of the current entry within `block.data`. Equal to
    /// `block.restarts_offset` when the cursor is not positioned on an entry.
    current: usize,
    /// Index of the restart point at or before `current`: where `prev` goes back
    /// to, and where a seek's linear scan begins.
    restart_index: usize,
    /// The current entry's full key, rebuilt from the previous key and the
    /// stored suffix.
    key: Vec<u8>,
    /// Where the current entry's value is within `block.data`. The value is
    /// returned as a slice of the block, not copied.
    value: Range<usize>,
    /// `Ok(())`, or the corruption error that stopped the cursor.
    status: crate::Result<()>,
}

/// Cursor over a block's entries. See
/// [`Cursor`](crate::iterator::traits::Cursor) for the contract of each method.
impl crate::iterator::traits::Cursor for BlockIter {
    fn valid(&self) -> bool {
        todo!()
    }

    fn seek_to_first(&mut self) {
        todo!()
    }

    fn seek_to_last(&mut self) {
        todo!()
    }

    fn seek(&mut self, target: &[u8]) {
        todo!()
    }

    fn next(&mut self) {
        todo!()
    }

    fn prev(&mut self) {
        todo!()
    }

    fn key(&self) -> &[u8] {
        todo!()
    }

    fn value(&self) -> &[u8] {
        todo!()
    }

    fn status(&self) -> crate::Result<()> {
        todo!()
    }
}
