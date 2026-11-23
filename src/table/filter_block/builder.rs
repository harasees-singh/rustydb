use crate::filter::traits::FilterPolicy;
use std::sync::Arc;

/// Builds the filter block of one table.
///
/// The table file is cut into ranges of `2^FILTER_BASE_LG` bytes of file
/// offset. One filter is built per range, from the keys of the data blocks that
/// start in it. A range in which no block starts gets an empty filter. A data
/// block is therefore checked against the filter at slot
/// `block_offset >> FILTER_BASE_LG`.
///
/// # Call order
/// ```ignore
/// for each data block {
///     builder.start_block(block_offset);   // once, before its keys
///     for each key in the block {
///         builder.add_key(key);
///     }
/// }
/// let block_bytes = builder.finish();      // once, at the end
/// ```
/// Block offsets given to [`start_block`](Self::start_block) must not decrease.
/// The table builder is the only caller, and it already writes blocks in file
/// order.
///
/// # Layout of the finished block
/// ```text
/// [filter 0] [filter 1] ... [filter n-1]          the filters back to back
/// [fixed32 start of filter 0] ... [fixed32 start of filter n-1]
/// [fixed32 where the array above starts]          "array_offset"
/// [FILTER_BASE_LG: 1 byte]
/// ```
/// Integers are little-endian. The starts are positions inside this block, not
/// in the file. Filter `i` is the bytes from `start[i]` to `start[i + 1]`, and
/// for the last filter to `array_offset`. An empty filter has equal start and
/// end. `FilterBlockReader` decodes this layout.
///
/// # Memory
/// Only the keys of the range being filled are held. The finished filters stay
/// in memory until [`finish`](Self::finish), because the table writes the
/// filter block once, after the last data block.
pub struct FilterBlockBuilder {
    /// Builds each filter, and defines their byte format.
    policy: Arc<dyn FilterPolicy>,
    /// Keys of the range being filled, concatenated. One buffer means adding a
    /// key does not allocate.
    keys: Vec<u8>,
    /// Where each pending key starts inside `keys`. A key runs to the next
    /// start, and the last one to `keys.len()`.
    key_starts: Vec<u32>,
    /// The finished filters, back to back.
    result: Vec<u8>,
    /// Where each finished filter starts inside `result`. Its length is the
    /// number of slots closed so far.
    filter_offsets: Vec<u32>,
}

impl FilterBlockBuilder {
    /// Creates a builder that has no keys and no filters.
    ///
    /// The `policy` is used for every filter of the table, so the reader must
    /// be given a policy with the same [`name`](FilterPolicy::name).
    pub fn new(policy: Arc<dyn FilterPolicy>) -> FilterBlockBuilder {
        todo!()
    }

    /// Tells the builder that a data block starting at file offset
    /// `block_offset` is about to be written.
    ///
    /// Closes every slot before `block_offset >> FILTER_BASE_LG`, so afterwards
    /// the number of closed filters equals that value. The keys added since the
    /// last call go into the first of them, and each further slot is an empty
    /// filter. Calling it again with the same offset, or with an offset in the
    /// same slot, closes nothing.
    ///
    /// Call it before the first [`add_key`](Self::add_key) of the block, and
    /// even for the first block (offset 0).
    pub fn start_block(&mut self, block_offset: u64) {
        todo!()
    }

    /// Adds `key` to the filter for the current slot.
    ///
    /// The bytes are copied, so `key` need not outlive the call. Duplicate keys
    /// are allowed. The key is the one that a lookup will later give to
    /// `FilterBlockReader::key_may_match`, so pass it in the same form.
    pub fn add_key(&mut self, key: &[u8]) {
        todo!()
    }

    /// Closes the last slot, appends the offset array, the array's start and
    /// `FILTER_BASE_LG`, and returns the whole filter block.
    ///
    /// The result is valid until the builder is next used. Call it once, after
    /// the last key. With no keys at all it returns just `array_offset = 0` and
    /// the `FILTER_BASE_LG` byte, which a reader treats as a block with no
    /// filters.
    pub fn finish(&mut self) -> &[u8] {
        todo!()
    }

    /// Closes the current slot.
    ///
    /// Always records the current length of `result` in `filter_offsets`. If no
    /// keys are pending it stops there, and the empty filter is what makes a
    /// slot report "definitely not present". Otherwise it asks the policy to
    /// append one filter for the pending keys to `result`, then clears `keys`
    /// and `key_starts` for the next slot.
    fn generate_filter(&mut self) {
        todo!()
    }
}
