use crate::filter::traits::FilterPolicy;
use crate::table::filter_block::FILTER_BASE_LG;
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
        FilterBlockBuilder {
            policy,
            keys: Vec::new(),
            key_starts: Vec::new(),
            result: Vec::new(),
            filter_offsets: Vec::new(),
        }
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
        while block_offset >> FILTER_BASE_LG > self.filter_offsets.len() as u64 {
            self.generate_filter()
        }
    }

    /// Adds `key` to the filter for the current slot.
    ///
    /// The bytes are copied, so `key` need not outlive the call. Duplicate keys
    /// are allowed. The key is the one that a lookup will later give to
    /// `FilterBlockReader::key_may_match`, so pass it in the same form.
    pub fn add_key(&mut self, key: &[u8]) {
        self.key_starts.push(self.keys.len() as u32);
        self.keys.extend_from_slice(key);
    }

    /// Closes the last slot, appends the offset array, the array's start and
    /// `FILTER_BASE_LG`, and returns the whole filter block.
    ///
    /// The result is valid until the builder is next used. Call it once, after
    /// the last key. With no keys at all it returns just `array_offset = 0` and
    /// the `FILTER_BASE_LG` byte, which a reader treats as a block with no
    /// filters.
    pub fn finish(&mut self) -> &[u8] {
        if !self.key_starts.is_empty() {
            self.generate_filter();
        }

        let start_offset = self.result.len();

        for offset in self.filter_offsets.iter() {
            crate::util::coding::put_fixed32(&mut self.result, *offset);
        }
        crate::util::coding::put_fixed32(&mut self.result, start_offset as u32);
        self.result.push(FILTER_BASE_LG);

        self.result.as_slice()
    }

    /// Closes the current slot.
    ///
    /// Always records the current length of `result` in `filter_offsets`. If no
    /// keys are pending it stops there, and the empty filter is what makes a
    /// slot report "definitely not present". Otherwise it asks the policy to
    /// append one filter for the pending keys to `result`, then clears `keys`
    /// and `key_starts` for the next slot.
    fn generate_filter(&mut self) {
        // record start offset of the new filter block
        self.filter_offsets.push(self.result.len() as u32);
        // do not push an empty filter
        if self.key_starts.is_empty() {
            return;
        }

        self.key_starts.push(self.keys.len() as u32);
        let keys: Vec<&[u8]> = (0..self.key_starts.len() - 1)
            .map(|i| {
                self.keys[(self.key_starts[i] as usize)..(self.key_starts[i + 1] as usize)].as_ref()
            })
            .collect();

        self.policy.create_filter(keys.as_slice(), &mut self.result);

        self.keys.clear();
        self.key_starts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::bloom::BloomFilterPolicy;

    /// A policy whose filter is easy to read back: `[count][len][bytes]...`.
    /// It lets the tests see exactly which keys reached `create_filter`.
    struct EncodePolicy;

    impl FilterPolicy for EncodePolicy {
        fn name(&self) -> &'static str {
            "test.encode"
        }

        fn create_filter(&self, keys: &[&[u8]], dst: &mut Vec<u8>) {
            dst.push(keys.len() as u8);
            for key in keys {
                dst.push(key.len() as u8);
                dst.extend_from_slice(key);
            }
        }

        fn key_may_match(&self, _key: &[u8], _filter: &[u8]) -> bool {
            true
        }
    }

    /// Reads back what `EncodePolicy` wrote.
    fn decode(filter: &[u8]) -> Vec<Vec<u8>> {
        let count = filter[0] as usize;
        let mut keys = Vec::new();
        let mut pos = 1;
        for _ in 0..count {
            let len = filter[pos] as usize;
            keys.push(filter[pos + 1..pos + 1 + len].to_vec());
            pos += 1 + len;
        }
        assert_eq!(pos, filter.len(), "bytes left over after the keys");
        keys
    }

    /// A finished filter block taken apart, without using any crate decoding
    /// code, so the tests do not trust the code under test to check itself.
    struct Parsed {
        filters: Vec<Vec<u8>>,
        base_lg: u8,
        array_offset: usize,
    }

    fn parse(block: &[u8]) -> Parsed {
        assert!(block.len() >= 5, "a filter block is at least 5 bytes");
        let n = block.len();
        let base_lg = block[n - 1];
        let array_offset = u32::from_le_bytes(block[n - 5..n - 1].try_into().unwrap()) as usize;
        assert!(array_offset <= n - 5, "array_offset points past the array");
        assert_eq!((n - 5 - array_offset) % 4, 0, "array is not whole u32s");
        let count = (n - 5 - array_offset) / 4;
        let word = |i: usize| -> usize {
            // word(count) is array_offset itself, the end of the last filter
            let at = array_offset + 4 * i;
            u32::from_le_bytes(block[at..at + 4].try_into().unwrap()) as usize
        };
        let filters = (0..count)
            .map(|i| {
                let (start, limit) = (word(i), word(i + 1));
                assert!(
                    start <= limit && limit <= array_offset,
                    "bad offsets at slot {i}"
                );
                block[start..limit].to_vec()
            })
            .collect();
        Parsed {
            filters,
            base_lg,
            array_offset,
        }
    }

    fn bloom_builder() -> FilterBlockBuilder {
        FilterBlockBuilder::new(Arc::new(BloomFilterPolicy))
    }

    fn encode_builder() -> FilterBlockBuilder {
        FilterBlockBuilder::new(Arc::new(EncodePolicy))
    }

    /// `n` distinct keys: `prefix-0000`, `prefix-0001`, ...
    fn keys(prefix: &str, n: usize) -> Vec<Vec<u8>> {
        (0..n)
            .map(|i| format!("{prefix}-{i:04}").into_bytes())
            .collect()
    }

    fn add_all(b: &mut FilterBlockBuilder, keys: &[Vec<u8>]) {
        for key in keys {
            b.add_key(key);
        }
    }

    // ---- the shape of the output ------------------------------------------

    #[test]
    fn empty_builder_finishes_to_five_bytes() {
        let mut b = bloom_builder();
        assert_eq!(b.finish(), &[0, 0, 0, 0, FILTER_BASE_LG]);
        let parsed = parse(&[0, 0, 0, 0, FILTER_BASE_LG]);
        assert!(parsed.filters.is_empty());
    }

    #[test]
    fn one_block_one_key() {
        let mut b = bloom_builder();
        b.start_block(0);
        b.add_key(b"hello");
        let block = b.finish().to_vec();

        let parsed = parse(&block);
        assert_eq!(parsed.base_lg, FILTER_BASE_LG);
        assert_eq!(parsed.filters.len(), 1);
        // 8-byte minimum bit array + the trailing k byte
        assert_eq!(parsed.filters[0].len(), 9);
        assert_eq!(parsed.array_offset, 9);
        assert_eq!(block.len(), 9 + 4 + 4 + 1);
        assert!(BloomFilterPolicy.key_may_match(b"hello", &parsed.filters[0]));
    }

    #[test]
    fn three_block_layout_matches_the_worked_example() {
        let (a, b_keys, c) = (keys("a", 100), keys("b", 100), keys("c", 50));
        let mut b = bloom_builder();
        b.start_block(0);
        add_all(&mut b, &a);
        b.start_block(4105);
        add_all(&mut b, &b_keys);
        b.start_block(8200);
        add_all(&mut b, &c);
        let block = b.finish().to_vec();

        assert_eq!(block.len(), 498);
        assert_eq!(*block.last().unwrap(), 0x0B);

        let parsed = parse(&block);
        assert_eq!(parsed.array_offset, 473);
        let sizes: Vec<usize> = parsed.filters.iter().map(|f| f.len()).collect();
        assert_eq!(sizes, vec![189, 0, 189, 0, 95]);
        // the offset array, byte for byte
        let expected: Vec<u8> = [0u32, 189, 189, 378, 378]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(&block[473..493], expected.as_slice());
        assert_eq!(&block[493..497], &473u32.to_le_bytes());
    }

    #[test]
    fn three_block_example_has_no_false_negatives() {
        let (a, b_keys, c) = (keys("a", 100), keys("b", 100), keys("c", 50));
        let mut b = bloom_builder();
        b.start_block(0);
        add_all(&mut b, &a);
        b.start_block(4105);
        add_all(&mut b, &b_keys);
        b.start_block(8200);
        add_all(&mut b, &c);
        let parsed = parse(&b.finish().to_vec());

        for (slot, ks) in [(0usize, &a), (2, &b_keys), (4, &c)] {
            for key in ks {
                assert!(
                    BloomFilterPolicy.key_may_match(key, &parsed.filters[slot]),
                    "key {:?} missing from slot {slot}",
                    String::from_utf8_lossy(key)
                );
            }
        }
    }

    #[test]
    fn filters_are_back_to_back_and_fill_everything_before_the_array() {
        let mut b = bloom_builder();
        b.start_block(0);
        add_all(&mut b, &keys("a", 10));
        b.start_block(5000);
        add_all(&mut b, &keys("b", 10));
        let block = b.finish().to_vec();
        let parsed = parse(&block);
        let total: usize = parsed.filters.iter().map(|f| f.len()).sum();
        assert_eq!(total, parsed.array_offset);
        let joined: Vec<u8> = parsed.filters.concat();
        assert_eq!(joined.as_slice(), &block[..parsed.array_offset]);
    }

    // ---- start_block --------------------------------------------------------

    #[test]
    fn start_block_with_the_same_offset_adds_no_filter() {
        let mut b = encode_builder();
        b.start_block(4105);
        let closed = b.filter_offsets.len();
        b.start_block(4105);
        b.start_block(4105);
        assert_eq!(b.filter_offsets.len(), closed);
    }

    #[test]
    fn start_block_on_a_builder_with_no_keys_records_empty_slots() {
        // would loop forever if the slot count were compared with the wrong length
        let mut b = encode_builder();
        b.start_block(4105);
        assert_eq!(b.filter_offsets, vec![0, 0]);
        let parsed = parse(&b.finish().to_vec());
        assert_eq!(parsed.filters, vec![Vec::<u8>::new(), Vec::new()]);
    }

    #[test]
    fn start_block_counts_slots_not_bytes() {
        // each filter here is several bytes, so a byte count would stop early
        let mut b = encode_builder();
        b.start_block(0);
        add_all(&mut b, &keys("a", 20));
        b.start_block(10 * 2048);
        assert_eq!(b.filter_offsets.len(), 10);
    }

    #[test]
    fn blocks_in_the_same_slot_share_one_filter() {
        let mut b = encode_builder();
        b.start_block(0);
        b.add_key(b"first");
        b.start_block(100); // same 2 KiB range as offset 0
        b.add_key(b"second");
        let parsed = parse(&b.finish().to_vec());
        assert_eq!(parsed.filters.len(), 1);
        assert_eq!(
            decode(&parsed.filters[0]),
            vec![b"first".to_vec(), b"second".to_vec()]
        );
    }

    #[test]
    fn a_block_starting_exactly_on_a_range_boundary_gets_the_next_slot() {
        let mut b = encode_builder();
        b.start_block(0);
        b.add_key(b"low");
        b.start_block(2048);
        b.add_key(b"high");
        let parsed = parse(&b.finish().to_vec());
        assert_eq!(parsed.filters.len(), 2);
        assert_eq!(decode(&parsed.filters[0]), vec![b"low".to_vec()]);
        assert_eq!(decode(&parsed.filters[1]), vec![b"high".to_vec()]);
    }

    #[test]
    fn a_block_starting_one_byte_before_a_boundary_stays_in_the_old_slot() {
        let mut b = encode_builder();
        b.start_block(0);
        b.add_key(b"low");
        b.start_block(2047);
        b.add_key(b"also-low");
        let parsed = parse(&b.finish().to_vec());
        assert_eq!(parsed.filters.len(), 1);
    }

    // ---- which keys reach the policy ---------------------------------------

    #[test]
    fn each_slot_gets_exactly_its_own_keys() {
        let mut b = encode_builder();
        b.start_block(0);
        b.add_key(b"a");
        b.add_key(b"bb");
        b.start_block(4105); // slot 2, so slot 1 is empty
        b.add_key(b"ccc");
        b.add_key(b"ccc"); // duplicates are allowed
        let parsed = parse(&b.finish().to_vec());

        assert_eq!(parsed.filters.len(), 3);
        assert_eq!(
            decode(&parsed.filters[0]),
            vec![b"a".to_vec(), b"bb".to_vec()]
        );
        assert!(parsed.filters[1].is_empty());
        assert_eq!(
            decode(&parsed.filters[2]),
            vec![b"ccc".to_vec(), b"ccc".to_vec()]
        );
    }

    #[test]
    fn keys_of_different_lengths_are_split_at_the_right_places() {
        let ks: Vec<Vec<u8>> = vec![
            b"x".to_vec(),
            b"yyyyyyyy".to_vec(),
            b"zz".to_vec(),
            b"w".to_vec(),
        ];
        let mut b = encode_builder();
        b.start_block(0);
        add_all(&mut b, &ks);
        let parsed = parse(&b.finish().to_vec());
        assert_eq!(decode(&parsed.filters[0]), ks);
    }

    #[test]
    fn keys_do_not_leak_into_the_next_slot() {
        let mut b = encode_builder();
        b.start_block(0);
        add_all(&mut b, &keys("a", 5));
        b.start_block(2048);
        add_all(&mut b, &keys("b", 3));
        let parsed = parse(&b.finish().to_vec());
        assert_eq!(decode(&parsed.filters[0]).len(), 5);
        assert_eq!(decode(&parsed.filters[1]).len(), 3);
    }

    #[test]
    fn the_configured_policy_is_the_one_used() {
        let mut b = encode_builder();
        b.start_block(0);
        b.add_key(b"k");
        let parsed = parse(&b.finish().to_vec());
        // EncodePolicy wrote [1, 1, b'k']; a bloom filter would be 9 bytes
        assert_eq!(parsed.filters[0], vec![1, 1, b'k']);
    }

    // ---- empty keys ---------------------------------------------------------

    #[test]
    fn an_empty_key_makes_a_real_filter_in_finish() {
        let mut b = encode_builder();
        b.start_block(0);
        b.add_key(b"");
        let parsed = parse(&b.finish().to_vec());
        assert_eq!(parsed.filters.len(), 1);
        assert_eq!(decode(&parsed.filters[0]), vec![Vec::<u8>::new()]);
    }

    #[test]
    fn an_empty_key_makes_a_real_filter_in_start_block() {
        let mut b = encode_builder();
        b.start_block(0);
        b.add_key(b"");
        b.start_block(2048);
        b.add_key(b"x");
        let parsed = parse(&b.finish().to_vec());
        assert_eq!(parsed.filters.len(), 2);
        // the empty key must not be lost, and must not spill into the next slot
        assert_eq!(decode(&parsed.filters[0]), vec![Vec::<u8>::new()]);
        assert_eq!(decode(&parsed.filters[1]), vec![b"x".to_vec()]);
    }

    #[test]
    fn an_empty_key_is_found_by_bloom() {
        let mut b = bloom_builder();
        b.start_block(0);
        b.add_key(b"");
        let parsed = parse(&b.finish().to_vec());
        assert!(BloomFilterPolicy.key_may_match(b"", &parsed.filters[0]));
    }

    // ---- an empty slot is empty, not a tiny filter -------------------------

    #[test]
    fn slots_without_a_block_are_zero_bytes() {
        let mut b = bloom_builder();
        b.start_block(0);
        b.add_key(b"a");
        b.start_block(6 * 2048);
        b.add_key(b"b");
        let parsed = parse(&b.finish().to_vec());
        assert_eq!(parsed.filters.len(), 7);
        for slot in 1..6 {
            assert!(
                parsed.filters[slot].is_empty(),
                "slot {slot} should be empty"
            );
        }
        assert!(!parsed.filters[0].is_empty());
        assert!(!parsed.filters[6].is_empty());
    }
}
