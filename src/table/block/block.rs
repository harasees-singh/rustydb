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
        // restarts_count is the last 4 bytes written in little endian
        let len = data.len();

        if len < 4 {
            return Err(crate::Error::corruption(
                "data doesn't conform to minimum possible size of 4 bytes",
            ));
        }

        let c = u32::from_le_bytes(data[len - 4..].try_into().unwrap());

        // check if restarts array fits data
        if len < 4 + 4 * c as usize {
            return Err(crate::Error::corruption(
                "data doesn't have enough space for restarts array",
            ));
        }

        Ok(Block {
            data,
            restarts_offset: len - 4 - 4 * c as usize,
            restarts_count: c,
        })
    }

    /// The size of the block in bytes, used as its cost in the block cache.
    pub fn size(&self) -> usize {
        self.data.len()
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
        self.status.is_ok() && self.current < self.block.restarts_offset
    }

    fn seek_to_first(&mut self) {
        self.current = 0;
        self.restart_index = 0;
        self.key.clear();
        self.value.end = 0;

        self.next();
    }

    fn seek_to_last(&mut self) {
        todo!()
    }

    fn seek(&mut self, target: &[u8]) {
        todo!()
    }

    fn next(&mut self) {
        self.current = self.value.end;
        let mut key: Vec<u8> = Vec::new();
        let mut value: Range<usize> = Range::default();

        // read the current entry
        self.read(self.current, self.key.clone(), &mut key, &mut value);

        self.key = key;
        self.value = value;
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

impl BlockIter {
    fn read(&mut self, current: usize, prev_key: Vec<u8>, key: &mut Vec<u8>, value: &mut Range<usize>) -> bool {
        let mut data = &self.block.data[current..];
        let mut common_bytes_for_key = 0u32;
        if !crate::util::coding::get_varint32(&mut data, &mut common_bytes_for_key) {
            return false;
        }
        let mut new_bytes_for_key = 0u32;
        if !crate::util::coding::get_varint32(&mut data, &mut new_bytes_for_key) {
            return false;
        }
        let mut value_bytes = 0u32;
        if !crate::util::coding::get_varint32(&mut data, &mut value_bytes) {
            return false;
        }

        // update restart_index so that prev() can start from this index and avoid looking up the restart_offsets for the closest entry
        if common_bytes_for_key == 0 {
            self.restart_index = current;
        }

        key.extend_from_slice(&prev_key[0..common_bytes_for_key as usize]);
        key.extend_from_slice(&data[0..new_bytes_for_key as usize]);

        value.start = self.block.data.len() - data.len() + new_bytes_for_key as usize;
        value.end = value.start + value_bytes as usize;

        value.end <= self.block.restarts_offset
    }
}
