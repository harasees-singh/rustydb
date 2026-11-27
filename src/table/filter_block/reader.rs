use crate::filter::traits::FilterPolicy;
use crate::table::filter_block::FILTER_BASE_LG;
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
    /// Number of slots, which is the length of the offset array. Slots with no
    /// keys count too: they are empty filters, but they still have an entry.
    /// Zero for a block that is empty or damaged.
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

        if array_offset > len - 5 {
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
        let Ok(slot) = usize::try_from(slot) else {
            return true;
        };
        if slot >= self.num_filters {
            return true;
        }
        let (Some(start), Some(limit)) = (self.offset_at(slot), self.offset_at(slot + 1)) else {
            return true;
        };
        if start > limit || limit > self.array_offset {
            return true;
        }
        if start == limit {
            return false;
        }
        self.policy.key_may_match(key, &self.data[start..limit])
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


#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::bloom::BloomFilterPolicy;
    use crate::table::filter_block::builder::FilterBlockBuilder;

    // ---- helpers ------------------------------------------------------------

    /// A policy with no false positives: the filter is the keys, each as
    /// `[len][bytes]`. It makes "which filter did the reader pick" observable.
    /// Like any policy it answers `true` for a filter it cannot read.
    struct ExactPolicy;

    impl FilterPolicy for ExactPolicy {
        fn name(&self) -> &'static str {
            "test.exact"
        }

        fn create_filter(&self, keys: &[&[u8]], dst: &mut Vec<u8>) {
            for key in keys {
                dst.push(key.len() as u8);
                dst.extend_from_slice(key);
            }
        }

        fn key_may_match(&self, key: &[u8], filter: &[u8]) -> bool {
            let mut pos = 0;
            while pos < filter.len() {
                let len = filter[pos] as usize;
                let Some(candidate) = filter.get(pos + 1..pos + 1 + len) else {
                    return true; // malformed: no information
                };
                if candidate == key {
                    return true;
                }
                pos += 1 + len;
            }
            false
        }
    }

    /// Fails the test if it is ever asked about an empty filter. The reader must
    /// answer an empty slot itself.
    struct NeverEmptyPolicy;

    impl FilterPolicy for NeverEmptyPolicy {
        fn name(&self) -> &'static str {
            "test.never-empty"
        }
        fn create_filter(&self, _keys: &[&[u8]], _dst: &mut Vec<u8>) {}
        fn key_may_match(&self, _key: &[u8], filter: &[u8]) -> bool {
            assert!(!filter.is_empty(), "the policy was asked about an empty filter");
            true
        }
    }

    fn bloom() -> Arc<dyn FilterPolicy> {
        Arc::new(BloomFilterPolicy)
    }

    fn reader(policy: Arc<dyn FilterPolicy>, block: Vec<u8>) -> FilterBlockReader {
        FilterBlockReader::new(policy, block)
    }

    /// `n` distinct keys: `prefix-0000`, `prefix-0001`, ...
    fn keys(prefix: &str, n: usize) -> Vec<Vec<u8>> {
        (0..n).map(|i| format!("{prefix}-{i:04}").into_bytes()).collect()
    }

    /// Builds a block with the real builder: one `(block_offset, keys)` per data block.
    fn build(policy: Arc<dyn FilterPolicy>, blocks: &[(u64, &[Vec<u8>])]) -> Vec<u8> {
        let mut b = FilterBlockBuilder::new(policy);
        for (offset, ks) in blocks {
            b.start_block(*offset);
            for k in ks.iter() {
                b.add_key(k);
            }
        }
        b.finish().to_vec()
    }

    /// The worked example: blocks at 0, 4105 and 8200 holding 100, 100 and 50
    /// keys. Slots 0, 2 and 4 hold filters, and slots 1 and 3 are empty.
    fn three_blocks(policy: Arc<dyn FilterPolicy>) -> (Vec<u8>, [Vec<Vec<u8>>; 3]) {
        let sets = [keys("a", 100), keys("b", 100), keys("c", 50)];
        let block = build(policy, &[(0, &sets[0]), (4105, &sets[1]), (8200, &sets[2])]);
        (block, sets)
    }

    /// Writes a filter block by hand, without the builder: `filters[i]` is the
    /// filter of slot `i`.
    fn assemble(filters: &[&[u8]], base_lg: u8) -> Vec<u8> {
        let mut out = Vec::new();
        let mut starts = Vec::new();
        for f in filters {
            starts.push(out.len() as u32);
            out.extend_from_slice(f);
        }
        let array_offset = out.len() as u32;
        for s in starts {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out.extend_from_slice(&array_offset.to_le_bytes());
        out.push(base_lg);
        out
    }

    fn exact_filter(keys: &[&[u8]]) -> Vec<u8> {
        let mut dst = Vec::new();
        ExactPolicy.create_filter(keys, &mut dst);
        dst
    }

    fn array_offset_of(block: &[u8]) -> usize {
        let n = block.len();
        u32::from_le_bytes(block[n - 5..n - 1].try_into().unwrap()) as usize
    }

    /// Overwrites word `i` of the offset array.
    fn set_offset(block: &mut [u8], i: usize, value: u32) {
        let at = array_offset_of(block) + 4 * i;
        block[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Block offsets that probe every interesting slot of the worked example.
    const PROBE_OFFSETS: [u64; 10] = [0, 1, 2047, 2048, 4105, 6144, 8200, 10_240, 100_000, u64::MAX];

    // ---- valid blocks -------------------------------------------------------

    #[test]
    fn every_added_key_matches_at_its_own_block_offset() {
        let (block, sets) = three_blocks(bloom());
        let r = reader(bloom(), block);
        for (offset, set) in [0u64, 4105, 8200].iter().zip(sets.iter()) {
            for key in set {
                assert!(
                    r.key_may_match(*offset, key),
                    "missing {:?} at {offset}",
                    String::from_utf8_lossy(key)
                );
            }
        }
    }

    #[test]
    fn any_offset_inside_a_blocks_range_finds_the_same_filter() {
        let (block, sets) = three_blocks(bloom());
        let r = reader(bloom(), block);
        // slot 2 covers file offsets 4096..=6143, and block 1 starts at 4105
        for offset in [4096u64, 4105, 5000, 6143] {
            assert!(r.key_may_match(offset, &sets[1][0]));
        }
    }

    #[test]
    fn slots_with_no_block_answer_false() {
        let (block, _) = three_blocks(bloom());
        let r = reader(bloom(), block);
        for offset in [2048u64, 3000, 4095, 6144, 7000, 8191] {
            assert!(!r.key_may_match(offset, b"a-0001"), "offset {offset}");
            assert!(!r.key_may_match(offset, b"anything"), "offset {offset}");
        }
    }

    #[test]
    fn an_empty_slot_is_answered_without_asking_the_policy() {
        // slot 1 is empty, and NeverEmptyPolicy fails the test if it is consulted
        let block = assemble(&[b"filter0", b"", b"filter2"], 11);
        let r = reader(Arc::new(NeverEmptyPolicy), block);
        assert!(!r.key_may_match(2048, b"k"));
        assert!(r.key_may_match(0, b"k"));
        assert!(r.key_may_match(4096, b"k"));
    }

    #[test]
    fn a_key_is_only_found_in_the_filter_of_its_own_block() {
        let (block, sets) = three_blocks(Arc::new(ExactPolicy));
        let r = reader(Arc::new(ExactPolicy), block);
        // exact policy: block 0's keys are in slot 0 only
        assert!(r.key_may_match(0, &sets[0][5]));
        assert!(!r.key_may_match(4105, &sets[0][5]));
        assert!(!r.key_may_match(8200, &sets[0][5]));
        assert!(r.key_may_match(4105, &sets[1][5]));
        assert!(!r.key_may_match(0, &sets[1][5]));
        assert!(r.key_may_match(8200, &sets[2][49]));
        assert!(!r.key_may_match(4105, &sets[2][49]));
    }

    #[test]
    fn the_last_slot_ends_at_the_array() {
        // the last filter has no next offset, so it runs to array_offset
        let (block, sets) = three_blocks(Arc::new(ExactPolicy));
        let r = reader(Arc::new(ExactPolicy), block);
        for key in &sets[2] {
            assert!(r.key_may_match(8200, key));
        }
    }

    #[test]
    fn blocks_in_the_same_slot_see_each_others_keys() {
        let first = vec![b"one".to_vec()];
        let second = vec![b"two".to_vec()];
        let block = build(Arc::new(ExactPolicy), &[(0, &first), (100, &second)]);
        let r = reader(Arc::new(ExactPolicy), block);
        assert!(r.key_may_match(0, b"two"));
        assert!(r.key_may_match(100, b"one"));
    }

    #[test]
    fn most_absent_keys_are_rejected_by_bloom() {
        let (block, _) = three_blocks(bloom());
        let r = reader(bloom(), block);
        let false_positives = (0..1000)
            .filter(|i| r.key_may_match(0, format!("zz-absent-{i:05}").as_bytes()))
            .count();
        // 15 bits per key gives about 0.1%: allow 1%, and require it is not "always true"
        assert!(false_positives <= 10, "{false_positives} of 1000 absent keys matched");
    }

    // ---- blocks that say nothing --------------------------------------------

    #[test]
    fn a_block_with_no_filters_answers_true_for_everything() {
        let block = build(bloom(), &[]);
        assert_eq!(block, vec![0, 0, 0, 0, 11]);
        let r = reader(bloom(), block);
        for offset in PROBE_OFFSETS {
            assert!(r.key_may_match(offset, b"k"), "offset {offset}");
        }
    }

    #[test]
    fn an_empty_reader_answers_true_for_everything() {
        let r = reader(bloom(), Vec::new());
        for offset in PROBE_OFFSETS {
            assert!(r.key_may_match(offset, b"k"), "offset {offset}");
        }
    }

    #[test]
    fn an_offset_past_the_last_slot_answers_true() {
        let (block, _) = three_blocks(bloom()); // 5 slots: 0..=4
        let r = reader(bloom(), block);
        for offset in [10_240u64, 12_288, 1 << 40, u64::MAX] {
            assert!(r.key_may_match(offset, b"never-added"), "offset {offset}");
        }
    }

    // ---- damaged blocks -----------------------------------------------------

    #[test]
    fn blocks_that_cannot_be_parsed_answer_true() {
        let bad: Vec<(&str, Vec<u8>)> = vec![
            ("empty", vec![]),
            ("1 byte", vec![11]),
            ("4 bytes", vec![0, 0, 0, 11]),
            ("array_offset past the tail", {
                let (mut b, _) = three_blocks(bloom());
                let n = b.len();
                b[n - 5..n - 1].copy_from_slice(&(n as u32).to_le_bytes());
                b
            }),
            ("array_offset u32::MAX", {
                let (mut b, _) = three_blocks(bloom());
                let n = b.len();
                b[n - 5..n - 1].copy_from_slice(&u32::MAX.to_le_bytes());
                b
            }),
            ("array not whole u32s", {
                let (mut b, _) = three_blocks(bloom());
                let n = b.len();
                b.remove(n - 6); // one byte out of the offset array
                b
            }),
            ("base_lg 64", {
                let (mut b, _) = three_blocks(bloom());
                *b.last_mut().unwrap() = 64;
                b
            }),
            ("base_lg 255", {
                let (mut b, _) = three_blocks(bloom());
                *b.last_mut().unwrap() = 255;
                b
            }),
        ];
        for (name, block) in bad {
            let r = reader(bloom(), block);
            // 2048 is an empty slot in the valid block, so `false` here would mean it was parsed
            for offset in [0u64, 2048, 4105, 8200, u64::MAX] {
                assert!(r.key_may_match(offset, b"a-0001"), "{name}, offset {offset}");
            }
        }
    }

    #[test]
    fn an_offset_that_goes_backwards_answers_true() {
        let (mut block, _) = three_blocks(bloom());
        set_offset(&mut block, 0, 400); // start 400 > limit 189
        let r = reader(bloom(), block);
        assert!(r.key_may_match(0, b"never-added"));
    }

    #[test]
    fn a_limit_past_the_filter_bytes_answers_true() {
        let (block, _) = three_blocks(bloom());
        let array_offset = array_offset_of(&block) as u32;
        // 474 is inside the offset array, 9999 is past the end of the block
        for bad in [array_offset + 1, array_offset + 4, 9999, u32::MAX] {
            let mut damaged = block.clone();
            set_offset(&mut damaged, 4, bad); // the start of slot 4 moved past its limit
            let mut damaged2 = block.clone();
            set_offset(&mut damaged2, 3, bad); // the limit of slot 2 moved past the array
            assert!(reader(bloom(), damaged).key_may_match(8200, b"never-added"), "bad {bad}");
            assert!(reader(bloom(), damaged2).key_may_match(4105, b"never-added"), "bad {bad}");
        }
    }

    #[test]
    fn a_damaged_slot_does_not_affect_the_other_slots() {
        let (mut block, sets) = three_blocks(Arc::new(ExactPolicy));
        set_offset(&mut block, 4, 9999);
        let r = reader(Arc::new(ExactPolicy), block);
        // slot 0 is untouched
        assert!(r.key_may_match(0, &sets[0][0]));
        assert!(!r.key_may_match(0, &sets[1][0]));
    }

    #[test]
    fn junk_never_panics() {
        // xorshift64: deterministic pseudo-random bytes
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..2000 {
            let len = (next() % 80) as usize;
            let junk: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let r = reader(bloom(), junk);
            for offset in PROBE_OFFSETS {
                let _ = r.key_may_match(offset, b"k");
            }
        }
    }

    #[test]
    fn every_truncation_of_a_valid_block_is_safe() {
        // A truncated block may parse as a different, smaller block, so the answer
        // is not checked, only that nothing panics. read_block's checksum is what
        // catches truncation in a real file.
        let (block, _) = three_blocks(bloom());
        for n in 0..=block.len() {
            let r = reader(bloom(), block[..n].to_vec());
            for offset in PROBE_OFFSETS {
                let _ = r.key_may_match(offset, b"a-0001");
            }
        }
    }

    #[test]
    fn every_single_byte_change_is_safe() {
        let (block, _) = three_blocks(bloom());
        let mut panicked = Vec::new();
        for pos in 0..block.len() {
            for value in [0u8, 1, 0x7F, 0x80, 0xFF] {
                let mut damaged = block.clone();
                damaged[pos] = value;
                let outcome = std::panic::catch_unwind(|| {
                    let r = reader(bloom(), damaged);
                    for offset in PROBE_OFFSETS {
                        let _ = r.key_may_match(offset, b"a-0001");
                    }
                });
                if outcome.is_err() {
                    panicked.push((pos, value));
                }
            }
        }
        assert!(panicked.is_empty(), "panicked for (position, value): {panicked:?}");
    }

    // ---- base_lg ------------------------------------------------------------

    #[test]
    fn base_lg_is_taken_from_the_block_and_not_from_the_constant() {
        // 1 KiB slots (base_lg 10): offsets 0, 1024, 2048 are slots 0, 1, 2
        let f0 = exact_filter(&[b"zero"]);
        let f2 = exact_filter(&[b"two"]);
        let block = assemble(&[&f0, b"", &f2], 10);
        let r = reader(Arc::new(ExactPolicy), block);
        assert!(r.key_may_match(0, b"zero"));
        assert!(r.key_may_match(1023, b"zero"));
        assert!(!r.key_may_match(1024, b"zero")); // slot 1 is empty
        assert!(!r.key_may_match(1024, b"anything"));
        assert!(r.key_may_match(2048, b"two"));
        assert!(!r.key_may_match(2048, b"zero"));
        assert!(r.key_may_match(3072, b"anything")); // slot 3 is past the end
    }

    #[test]
    fn base_lg_zero_has_one_slot_per_byte_offset() {
        let f0 = exact_filter(&[b"a"]);
        let f1 = exact_filter(&[b"b"]);
        let block = assemble(&[&f0, &f1], 0);
        let r = reader(Arc::new(ExactPolicy), block);
        assert!(r.key_may_match(0, b"a"));
        assert!(!r.key_may_match(0, b"b"));
        assert!(r.key_may_match(1, b"b"));
        assert!(r.key_may_match(2, b"anything")); // past the end
        assert!(r.key_may_match(u64::MAX, b"anything")); // must not overflow
    }

    #[test]
    fn base_lg_63_is_accepted_and_64_is_not() {
        let f0 = exact_filter(&[b"a"]);
        let ok = assemble(&[&f0], 63);
        assert!(!reader(Arc::new(ExactPolicy), ok).key_may_match(0, b"b"));
        let too_big = assemble(&[&f0], 64);
        assert!(reader(Arc::new(ExactPolicy), too_big).key_may_match(0, b"b"));
    }

    #[test]
    fn a_block_read_back_with_another_policy_still_cannot_panic() {
        // a table written with bloom but opened with a different policy: the
        // reader hands the bloom bytes to ExactPolicy, which sees junk
        let (block, _) = three_blocks(bloom());
        let r = reader(Arc::new(ExactPolicy), block);
        for offset in PROBE_OFFSETS {
            let _ = r.key_may_match(offset, b"a-0001");
        }
    }

    // ---- offset_at ----------------------------------------------------------

    #[test]
    fn offset_at_returns_every_word_of_the_array_and_the_array_offset() {
        let (block, _) = three_blocks(bloom());
        let r = reader(bloom(), block);
        assert_eq!(r.num_filters, 5);
        let words: Vec<Option<usize>> = (0..8).map(|i| r.offset_at(i)).collect();
        assert_eq!(
            words,
            vec![Some(0), Some(189), Some(189), Some(378), Some(378), Some(473), None, None]
        );
    }

    #[test]
    fn offset_at_is_none_for_huge_indexes() {
        let (block, _) = three_blocks(bloom());
        let r = reader(bloom(), block);
        assert_eq!(r.offset_at(usize::MAX), None);
        assert_eq!(r.offset_at(usize::MAX / 4 + 1), None);
        assert_eq!(reader(bloom(), Vec::new()).offset_at(0), None);
    }
}
