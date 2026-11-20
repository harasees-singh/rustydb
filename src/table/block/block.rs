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
use crate::iterator::traits::Cursor;
use std::cmp::Ordering;
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
        BlockIter {
            block: Arc::clone(self),
            cmp,
            current: 0,
            restart_index: 0,
            key: Vec::new(),
            value: Range::from(0..0),
            status: Ok(())
        }
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
    /// Index of the last restart point strictly before `current` (0 if none):
    /// where `prev` goes back to, and where a seek's linear scan begins.
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
        if self.block.restarts_count == 0 {
            // No restart points, so nothing to start decoding from.
            self.current = self.block.restarts_offset;
            return;
        }

        // The last entry is in the last restart interval: start there and read
        // forward to the entry that ends where the entries end.
        self.seek_to_restart_point(self.block.restarts_count as usize - 1);
        self.scan_until(self.block.restarts_offset);
    }

    fn seek(&mut self, target: &[u8]) {
        if self.status.is_err() {
            return; // keep the first error
        }
        let count = self.block.restarts_count as usize;
        if count == 0 {
            self.current = self.block.restarts_offset;
            return;
        }

        // Binary search for the last restart point whose key is < target (or 0
        // if there is none): everything before it is too small.
        let (mut left, mut right) = (0, count - 1);
        while left < right {
            let mid = (left + right + 1) / 2;
            let Some(key) = self.restart_key(mid) else {
                self.current = self.block.restarts_offset;
                self.status = Err(crate::Error::corruption(format!(
                    "bad entry at restart point {mid}"
                )));
                return;
            };
            if self.cmp.compare(key, target) == Ordering::Less {
                left = mid;
            } else {
                right = mid - 1;
            }
        }

        // Then scan forward from there to the first key >= target.
        self.seek_to_restart_point(left);
        loop {
            self.next();
            if !self.valid() || self.cmp.compare(&self.key, target) != Ordering::Less {
                break;
            }
        }
    }

    fn next(&mut self) {
        self.current = self.value.end;
        if self.current >= self.block.restarts_offset {
            self.current = self.block.restarts_offset;
            return;
        }
        // read the current entry
        if !self.read(self.current) {
            self.status = Err(crate::Error::corruption(format!(
                "bad entry, unable to read block data at index {}",
                self.current
            )));
        }
    }

    fn prev(&mut self) {
        debug_assert!(self.valid());
        let original = self.current;

        // Go back to a restart point strictly before the current entry.
        while self.restart_point(self.restart_index) >= original {
            if self.restart_index == 0 {
                // No earlier entry: invalid.
                self.current = self.block.restarts_offset;
                self.restart_index = self.block.restarts_count as usize;
                return;
            }
            self.restart_index -= 1;
        }

        // Start there and read forward until the next entry would be the
        // original one.
        self.seek_to_restart_point(self.restart_index);
        self.scan_until(original);
        if self.valid() && self.value.end != original {
            self.status = Err(crate::Error::corruption(
                "bad entry, unable to find the entry before the current one",
            ));
        }
    }

    fn key(&self) -> &[u8] {
        self.key.as_slice()
    }

    fn value(&self) -> &[u8] {
        self.block.data[self.value.clone()].as_ref()
    }

    fn status(&self) -> crate::Result<()> {
        self.status.clone()
    }
}

/// Decodes the three varints that start every entry (`shared`, `non_shared`,
/// `value_len`) and advances `data` past them. Returns `None` if they are
/// truncated or malformed.
#[inline]
fn decode_entry_header(data: &mut &[u8]) -> Option<(u32, u32, u32)> {
    // Fast path: all three varints are one byte, which is almost always the
    // case, so decode them with a single bounds check.
    if let [a, b, c, rest @ ..] = *data
        && (*a | *b | *c) < 128
    {
        *data = rest;
        return Some((*a as u32, *b as u32, *c as u32));
    }
    let (mut shared, mut non_shared, mut value_len) = (0u32, 0u32, 0u32);
    (crate::util::coding::get_varint32(data, &mut shared)
        && crate::util::coding::get_varint32(data, &mut non_shared)
        && crate::util::coding::get_varint32(data, &mut value_len))
    .then_some((shared, non_shared, value_len))
}

impl BlockIter {
    fn read(&mut self, current: usize) -> bool {
        if current >= self.block.restarts_offset || !self.valid() {
            return false;
        }
        let mut data = &self.block.data[current..self.block.restarts_offset];
        let Some((shared, non_shared, value_len)) = decode_entry_header(&mut data) else {
            return false;
        };
        if self.key.len() < shared as usize {
            return false;
        }

        // the key delta and the value must both fit in the entries
        if data.len() < non_shared as usize + value_len as usize {
            return false;
        }

        self.key.truncate(shared as usize);
        self.key.extend_from_slice(&data[0..non_shared as usize]);

        self.value.start = self.block.restarts_offset - data.len() + non_shared as usize;
        self.value.end = self.value.start + value_len as usize;

        // keep restart_index at the last restart point before `current`, so that
        // prev() can start from it without searching the restart array
        while self.restart_index + 1 < self.block.restarts_count as usize
            && self.restart_point(self.restart_index + 1) < current
        {
            self.restart_index += 1;
        }

        true
    }

    /// Positions the cursor just before restart point `i`, so that the next call
    /// to `next()` reads the entry stored there. The key starts empty, which is
    /// right because a restart entry stores its whole key.
    fn seek_to_restart_point(&mut self, i: usize) {
        self.restart_index = i;
        self.key.clear();
        let start = self.restart_point(i);
        self.value = Range::from(start..start);
    }

    /// Reads forward until the current entry ends at or after `limit`, or the
    /// cursor becomes invalid (end of the block, or an error).
    fn scan_until(&mut self, limit: usize) {
        loop {
            self.next();
            if !self.valid() || self.value.end >= limit {
                break;
            }
        }
    }

    /// The full key stored at restart point `i`, as a slice of the block, or
    /// `None` if that entry is malformed.
    ///
    /// A restart entry stores its whole key (`shared == 0`), so the key can be
    /// read in place without touching the cursor's own key buffer.
    fn restart_key(&self, i: usize) -> Option<&[u8]> {
        let offset = self.restart_point(i);
        if offset >= self.block.restarts_offset {
            return None;
        }
        let mut data = &self.block.data[offset..self.block.restarts_offset];
        let (shared, non_shared, value_len) = decode_entry_header(&mut data)?;
        if shared != 0 || data.len() < non_shared as usize + value_len as usize {
            return None;
        }
        Some(&data[..non_shared as usize])
    }

    /// The offset of the entry stored at restart point `i`.
    fn restart_point(&self, i: usize) -> usize {
        let at = self.block.restarts_offset + 4 * i;
        u32::from_le_bytes(self.block.data[at..at + 4].try_into().unwrap()) as usize
    }
}
