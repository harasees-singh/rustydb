//! Writes a block: the sorted key/value pairs of one part of an SSTable.
//!
//! # Format
//! ```text
//! entry   = shared:varint32  non_shared:varint32  value_len:varint32
//!           key_delta[non_shared]  value[value_len]
//! block   = entry*  restart_offset[0..n]:fixed32  n:fixed32
//! ```
//! * **Prefix compression.** `shared` is the number of leading bytes the key has
//!   in common with the previous key, and only the remaining `non_shared` bytes
//!   are stored. A reader rebuilds a key with
//!   `key.truncate(shared); key.extend(key_delta)` on a buffer that still holds
//!   the previous key.
//! * **Restart points.** Every `restart_interval` entries the full key is stored
//!   (`shared = 0`), and that entry's byte offset is recorded in the restart
//!   array. Decoding can start at any restart point, which lets a reader
//!   binary-search the restarts and then scan forward.
//! * Integers in the trailer are little-endian. Keys and values are opaque
//!   bytes.
//!
//! The block carries no checksum or compression flag: the table format appends
//! those when it writes the block out. See [`Block`](super::block::Block) for
//! reading.
//!
//! The builder writes data blocks (restart interval 16 by default), the index
//! block (interval 1, so its keys are never compressed) and the meta-index
//! block.

/// Builds one block in memory from keys added in increasing order.
///
/// The usual cycle is [`add`](Self::add) for every entry, then
/// [`finish`](Self::finish) to get the bytes, then [`reset`](Self::reset) to
/// reuse the builder (and its buffers) for the next block.
pub struct BlockBuilder {
    /// A full key is stored every this many entries. At least 1.
    restart_interval: usize,
    /// The encoded entries so far, and after `finish` the whole block.
    buffer: Vec<u8>,
    /// Byte offsets of the restart points. Starts as `[0]`.
    restarts: Vec<u32>,
    /// Entries added since the last restart point.
    counter: usize,
    /// The most recently added key, to compute `shared` for the next one.
    last_key: Vec<u8>,
    /// Whether `finish` has been called since the last `reset`.
    finished: bool,
}

impl BlockBuilder {
    /// Creates an empty builder that stores a full key every `restart_interval`
    /// entries.
    ///
    /// An interval of 1 disables prefix compression. Larger intervals make the
    /// block smaller and seeks scan further.
    ///
    /// # Panics
    /// If `restart_interval` is 0.
    pub fn new(restart_interval: usize) -> Self {
        todo!()
    }

    /// Appends an entry.
    ///
    /// Keys must be added in strictly increasing order, using the ordering the
    /// reader will use. The builder does not check this in release builds.
    ///
    /// # Panics
    /// If called after [`finish`](Self::finish) without a [`reset`](Self::reset).
    pub fn add(&mut self, key: &[u8], value: &[u8]) {
        todo!()
    }

    /// Completes the block by appending the restart array and its length, and
    /// returns the block's bytes.
    ///
    /// The returned slice stays valid until the next call to
    /// [`reset`](Self::reset) or [`add`](Self::add). An empty builder produces a
    /// valid empty block (a single restart point and no entries).
    pub fn finish(&mut self) -> &[u8] {
        todo!()
    }

    /// Returns the builder to its empty state, keeping its allocations for the
    /// next block.
    pub fn reset(&mut self) {
        todo!()
    }

    /// The size in bytes the block would have if [`finish`](Self::finish) were
    /// called now: the entries so far plus the restart array and its length.
    ///
    /// The table builder uses this to decide when a block is full.
    pub fn current_size_estimate(&self) -> usize {
        todo!()
    }

    /// Whether no entry has been added since creation or the last
    /// [`reset`](Self::reset).
    pub fn is_empty(&self) -> bool {
        todo!()
    }
}
