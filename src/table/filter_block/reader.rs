use crate::filter::traits::FilterPolicy;
use crate::table::filter_block::FILTER_BASE_LG;
use std::iter::Filter;
use std::sync::Arc;

/// Reads the filter block that `FilterBlockBuilder` wrote, and answers whether a
/// key may be in a given data block.
///
/// The block is untrusted input from disk. Construction never fails, and no
/// method panics on damaged bytes. Anything that cannot be understood means
/// "no information", and the answer is `true` (the key may be present), so a
/// damaged filter costs a disk read and never hides a key.
///
/// # Mapping a block to its filter
/// `slot = block_offset >> base_lg`, where `block_offset` is the data block's
/// `BlockHandle::offset`. `base_lg` is read from the last byte of the block, not
/// taken from `FILTER_BASE_LG`, so a table written with another value still
/// reads correctly.
///
/// See [`FilterBlockBuilder`](super::builder::FilterBlockBuilder) for the layout.
pub struct FilterBlockReader {
    policy: Arc<dyn FilterPolicy>,
    /// The whole filter block, including the offset array and the 5-byte tail.
    data: Vec<u8>,
    /// Where the array of filter starts begins inside `data`.
    array_offset: usize,
    /// Number of filters. Zero for a block that is empty or damaged.
    num_filters: usize,
    /// Each slot covers `2^base_lg` bytes of file offset. Always below 64.
    base_lg: u8,
}

impl FilterBlockReader {
    /// Parses `contents`, the bytes returned by `read_block` for the filter block.
    ///
    /// Never fails. If `contents` is shorter than 5 bytes, if `array_offset` points
    /// past the array's possible end, if the array is not a whole number of
    /// `u32`s, or if `base_lg >= 64`, the reader is built with `num_filters = 0`
    /// and answers `true` to everything.
    pub fn new(policy: Arc<dyn FilterPolicy>, contents: Vec<u8>) -> FilterBlockReader {
        if contents.len() < 5 {
            return empty_filter(policy);
        }
        let base_lg = *contents.last().unwrap();

        if base_lg >= 64 {
            return empty_filter(policy);
        }

        let len = contents.len();

        let array_offset =
            u32::from_le_bytes(contents[len - 5..len - 1].try_into().unwrap()) as usize;

        if array_offset >= len - 5 {
            return empty_filter(policy);
        }

        let total_offset_bytes = len - 5 - array_offset;
        if total_offset_bytes % 4 != 0 {
            return empty_filter(policy);
        }

        let total_offsets = total_offset_bytes / 4;

        FilterBlockReader {
            policy,
            data: contents,
            array_offset,
            num_filters: total_offsets,
            base_lg,
        }
    }

    /// Returns `false` only if `key` is definitely not in the data block that
    /// starts at file offset `block_offset`, and `true` if it might be.
    ///
    /// | Situation | Answer |
    /// |---|---|
    /// | the slot's filter is empty (no keys were added in that range) | `false` |
    /// | the slot's filter is valid | `policy.key_may_match(key, filter)` |
    /// | slot out of range, bad offsets, or the reader is empty | `true` |
    ///
    /// The empty filter is answered here and never passed to the policy: the
    /// builder wrote it on purpose, so "no keys" is certain. A filter that is
    /// damaged is a different case and answers `true`.
    pub fn key_may_match(&self, block_offset: u64, key: &[u8]) -> bool {
        let slot = block_offset >> self.base_lg;
        let Some(start) = self.offset_at(slot as usize) else {
            return true;
        };
        let Some(end) = self.offset_at((slot + 1) as usize) else {
            return true;
        };

        self.policy.key_may_match(key, self.data[start..end].as_ref())
    }

    /// The `i`th word of the offset array, or `None` if `i` is out of range.
    ///
    /// Word `num_filters` is `array_offset` itself, which sits right after the
    /// array. It is what ends the last filter, so no special case is needed.
    fn offset_at(&self, i: usize) -> Option<usize> {
        if i > self.num_filters {
            return None;
        }
        // every step is checked: `i` comes from a block offset read from disk
        let pos = i.checked_mul(4)?.checked_add(self.array_offset)?;
        let bytes: [u8; 4] = self.data.get(pos..pos.checked_add(4)?)?.try_into().ok()?;
        Some(u32::from_le_bytes(bytes) as usize)
    }
}

fn empty_filter(policy: Arc<dyn FilterPolicy>) -> FilterBlockReader {
    FilterBlockReader {
        policy,
        data: Vec::new(),
        array_offset: 0,
        num_filters: 0,
        base_lg: FILTER_BASE_LG,
    }
}
