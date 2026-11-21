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

use crate::comparator::Comparator;
use std::cmp::Ordering;
use std::sync::Arc;

/// Builds one block in memory from keys added in increasing order.
///
/// The usual cycle is [`add`](Self::add) for every entry, then
/// [`finish`](Self::finish) to get the bytes, then [`reset`](Self::reset) to
/// reuse the builder (and its buffers) for the next block.
pub struct BlockBuilder {
    /// A full key is stored every this many entries. At least 1.
    restart_interval: usize,
    /// Comparator to assert the keys are in sorted order
    cmp: Arc<dyn Comparator>,
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
    pub fn new(restart_interval: usize, cmp: Arc<dyn Comparator>) -> Self {
        assert!(restart_interval >= 1, "restart_interval must be at least 1");
        BlockBuilder {
            restart_interval,
            cmp,
            buffer: Vec::new(),
            restarts: vec![0],
            counter: 0,
            last_key: Vec::new(),
            finished: false,
        }
    }

    /// Appends an entry.
    ///
    /// Keys must be added in strictly increasing order, using the ordering the
    /// reader will use. The builder does not check this in release builds.
    ///
    /// # Panics
    /// If called after [`finish`](Self::finish) without a [`reset`](Self::reset).
    pub fn add(&mut self, key: &[u8], value: &[u8]) {
        assert!(!self.finished);
        debug_assert!(
            self.buffer.is_empty()
                || self.cmp.compare(key, self.last_key.as_slice()) == Ordering::Greater,
            "keys must be added in increasing order"
        );

        let shared_count;
        if self.counter != self.restart_interval {
            shared_count = self
                .last_key
                .iter()
                .zip(key.iter())
                .take_while(|(a, b)| a == b)
                .count()
        } else {
            shared_count = 0;
            self.counter = 0;
            self.restarts.push(self.buffer.len() as u32);
        }

        let diff = key.len() - shared_count;

        crate::util::coding::put_varint32(&mut self.buffer, shared_count as u32);
        crate::util::coding::put_varint32(&mut self.buffer, diff as u32);
        crate::util::coding::put_varint32(&mut self.buffer, value.len() as u32);

        self.buffer.extend_from_slice(&key[shared_count..]);
        self.buffer.extend_from_slice(value);

        self.last_key.truncate(shared_count);
        self.last_key.extend_from_slice(&key[shared_count..]);

        self.counter += 1;
    }

    /// Completes the block by appending the restart array and its length, and
    /// returns the block's bytes.
    ///
    /// The returned slice stays valid until the next call to
    /// [`reset`](Self::reset) or [`add`](Self::add). An empty builder produces a
    /// valid empty block (a single restart point and no entries).
    pub fn finish(&mut self) -> &[u8] {
        // append restarts array and write the count of restarts array in little endian
        assert!(!self.finished);

        self.restarts
            .iter()
            .for_each(|c| crate::util::coding::put_fixed32(&mut self.buffer, *c));

        crate::util::coding::put_fixed32(&mut self.buffer, self.restarts.len() as u32);

        self.finished = true;

        self.buffer.as_slice()
    }

    /// Returns the builder to its empty state, keeping its allocations for the
    /// next block.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.last_key.clear();
        self.counter = 0;
        self.finished = false;
        self.restarts.clear();
        self.restarts.push(0);
    }

    /// The size in bytes the block would have if [`finish`](Self::finish) were
    /// called now: the entries so far plus the restart array and its length.
    ///
    /// The table builder uses this to decide when a block is full.
    pub fn current_size_estimate(&self) -> usize {
        self.buffer.len() + 4 * self.restarts.len() + 4
    }

    /// Whether no entry has been added since creation or the last
    /// [`reset`](Self::reset).
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comparator::bytewise::BytewiseComparator;
    use crate::iterator::traits::Cursor;
    use crate::table::block::block::Block;
    use std::collections::BTreeSet;

    type Entries = Vec<(Vec<u8>, Vec<u8>)>;

    /// Orders keys backwards, to prove the builder uses the comparator it is given.
    struct Reverse;

    impl Comparator for Reverse {
        fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
            b.cmp(a)
        }
        fn name(&self) -> &'static str {
            "test.Reverse"
        }
    }

    fn builder(restart_interval: usize) -> BlockBuilder {
        BlockBuilder::new(restart_interval, Arc::new(BytewiseComparator))
    }

    fn entry(key: &str, value: &str) -> (Vec<u8>, Vec<u8>) {
        (key.as_bytes().to_vec(), value.as_bytes().to_vec())
    }

    /// Adds every entry and returns the finished block's bytes.
    fn build_with(b: &mut BlockBuilder, entries: &Entries) -> Vec<u8> {
        for (k, v) in entries {
            b.add(k, v);
        }
        b.finish().to_vec()
    }

    fn build(entries: &Entries, restart_interval: usize) -> Vec<u8> {
        build_with(&mut builder(restart_interval), entries)
    }

    /// Reads a finished block back through `Block` and `BlockIter`.
    fn read_back(bytes: Vec<u8>) -> Entries {
        let block =
            Arc::new(Block::new(bytes).expect("the builder's output must be a valid block"));
        let mut it = block.iter(Arc::new(BytewiseComparator));
        let mut out = Vec::new();
        it.seek_to_first();
        while it.valid() {
            out.push((it.key().to_vec(), it.value().to_vec()));
            it.next();
        }
        assert!(it.status().is_ok());
        out
    }

    /// `(entries_end, restart offsets)` parsed from a finished block's trailer.
    fn trailer(bytes: &[u8]) -> (usize, Vec<usize>) {
        let le = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let count = le(bytes.len() - 4);
        let entries_end = bytes.len() - 4 - 4 * count;
        let restarts = (0..count).map(|i| le(entries_end + 4 * i)).collect();
        (entries_end, restarts)
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    /// `n` distinct sorted entries with many shared prefixes; one value in
    /// five is long enough to need a two-byte length.
    fn random_entries(rng: &mut Rng, n: usize) -> Entries {
        let mut keys = BTreeSet::new();
        while keys.len() < n {
            let len = (rng.next() % 8) as usize;
            keys.insert(
                (0..len)
                    .map(|_| b'a' + (rng.next() % 3) as u8)
                    .collect::<Vec<u8>>(),
            );
        }
        keys.into_iter()
            .map(|k| {
                let len = if rng.next() % 5 == 0 {
                    (rng.next() % 300) as usize
                } else {
                    (rng.next() % 6) as usize
                };
                (k, (0..len).map(|_| rng.next() as u8).collect())
            })
            .collect()
    }

    fn example_entries() -> Entries {
        vec![
            entry("apple", "1"),
            entry("applesauce", "22"),
            entry("apply", "333"),
            entry("banana", "4"),
        ]
    }

    // ---- the format -----------------------------------------------------------

    #[test]
    fn produces_exactly_the_documented_example_block() {
        let mut expected = Vec::new();
        expected.extend_from_slice(&[0, 5, 1, b'a', b'p', b'p', b'l', b'e', b'1']);
        expected.extend_from_slice(&[5, 5, 2, b's', b'a', b'u', b'c', b'e', b'2', b'2']);
        expected.extend_from_slice(&[0, 5, 3, b'a', b'p', b'p', b'l', b'y', b'3', b'3', b'3']);
        expected.extend_from_slice(&[0, 6, 1, b'b', b'a', b'n', b'a', b'n', b'a', b'4']);
        expected.extend_from_slice(&[0, 0, 0, 0]); // restart[0] = 0
        expected.extend_from_slice(&[19, 0, 0, 0]); // restart[1] = 19
        expected.extend_from_slice(&[2, 0, 0, 0]); // two restart points
        assert_eq!(build(&example_entries(), 2), expected);
        assert_eq!(expected.len(), 52);
    }

    #[test]
    fn an_empty_builder_produces_the_one_restart_empty_block() {
        let mut b = builder(16);
        assert!(b.is_empty());
        assert_eq!(b.current_size_estimate(), 8);
        let bytes = b.finish().to_vec();
        assert_eq!(bytes, [0, 0, 0, 0, 1, 0, 0, 0]);
        assert!(read_back(bytes).is_empty());
    }

    #[test]
    fn the_first_entry_is_always_a_restart_point_at_offset_zero() {
        for interval in [1, 2, 16] {
            let bytes = build(&example_entries(), interval);
            let (_, restarts) = trailer(&bytes);
            assert_eq!(restarts[0], 0, "interval {interval}");
        }
    }

    #[test]
    fn a_restart_point_is_recorded_every_interval_entries() {
        let mut rng = Rng(5);
        for n in [1usize, 2, 15, 16, 17, 32, 33, 100] {
            for interval in [1usize, 2, 3, 16] {
                let entries = random_entries(&mut rng, n);
                let bytes = build(&entries, interval);
                let (entries_end, restarts) = trailer(&bytes);
                assert_eq!(
                    restarts.len(),
                    n.div_ceil(interval),
                    "n {n}, interval {interval}"
                );
                assert!(restarts.windows(2).all(|w| w[0] < w[1]), "increasing");
                for &offset in &restarts {
                    assert!(offset < entries_end);
                    assert_eq!(bytes[offset], 0, "a restart entry stores its whole key");
                }
            }
        }
    }

    #[test]
    fn interval_one_stores_every_key_in_full() {
        // every varint here fits in one byte, so an entry is 3 + key + value
        let entries = example_entries();
        let expected: usize = entries
            .iter()
            .map(|(k, v)| 3 + k.len() + v.len())
            .sum::<usize>()
            + 4 * entries.len()
            + 4;
        assert_eq!(build(&entries, 1).len(), expected);
    }

    #[test]
    fn prefix_compression_shrinks_the_block() {
        let entries: Entries = (0..200)
            .map(|i| {
                (
                    format!("a-long-common-prefix-{i:05}").into_bytes(),
                    b"v".to_vec(),
                )
            })
            .collect();
        let plain = build(&entries, 1).len();
        let compressed = build(&entries, 16).len();
        assert!(compressed < plain / 2, "{compressed} vs {plain}");
    }

    // ---- size estimate and emptiness --------------------------------------------

    #[test]
    fn the_size_estimate_equals_the_finished_size_at_every_step() {
        let entries = random_entries(&mut Rng(9), 60);
        for interval in [1, 3, 16] {
            let mut b = builder(interval);
            for k in 0..entries.len() {
                b.add(&entries[k].0, &entries[k].1);
                let expected = build(&entries[..=k].to_vec(), interval).len();
                assert_eq!(
                    b.current_size_estimate(),
                    expected,
                    "interval {interval}, {k} entries"
                );
            }
        }
    }

    #[test]
    fn is_empty_tracks_entries_and_reset() {
        let mut b = builder(16);
        assert!(b.is_empty());
        b.add(b"a", b"1");
        assert!(!b.is_empty());
        b.finish();
        b.reset();
        assert!(b.is_empty());
    }

    // ---- reading it back --------------------------------------------------------

    #[test]
    fn round_trips_through_block_and_block_iter() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for n in [0, 1, 2, 3, 16, 17, 150] {
            for interval in [1, 2, 3, 16, 100] {
                let entries = random_entries(&mut rng, n);
                assert_eq!(
                    read_back(build(&entries, interval)),
                    entries,
                    "n {n}, interval {interval}"
                );
            }
        }
    }

    #[test]
    fn round_trips_large_keys_values_and_a_big_block() {
        let long = |c: char| format!("{}{c}", "k".repeat(300)).into_bytes();
        let entries = vec![
            (Vec::new(), b"empty key".to_vec()),
            (long('a'), vec![1; 200]),
            (long('b'), vec![2; 70_000]),
            (b"z".to_vec(), Vec::new()),
        ];
        for interval in [1, 16] {
            assert_eq!(read_back(build(&entries, interval)), entries);
        }
        let big: Entries = (0..10_000)
            .map(|i| {
                (
                    format!("key{i:06}").into_bytes(),
                    format!("value{i}").into_bytes(),
                )
            })
            .collect();
        assert_eq!(read_back(build(&big, 16)), big);
    }

    #[test]
    fn a_key_that_extends_the_previous_one_shares_all_of_it() {
        let entries = vec![entry("abc", "1"), entry("abcd", "2"), entry("abcde", "3")];
        assert_eq!(read_back(build(&entries, 16)), entries);
        // second entry: shared 3, non_shared 1
        let bytes = build(&entries, 16);
        assert_eq!(&bytes[..8], &[0, 3, 1, b'a', b'b', b'c', b'1', 3]);
    }

    #[test]
    fn seeks_work_on_what_the_builder_wrote() {
        let entries = random_entries(&mut Rng(31), 80);
        let block = Arc::new(Block::new(build(&entries, 4)).unwrap());
        let mut it = block.iter(Arc::new(BytewiseComparator));
        for (key, value) in &entries {
            it.seek(key);
            assert_eq!((it.key(), it.value()), (key.as_slice(), value.as_slice()));
        }
        it.seek_to_last();
        assert_eq!(it.key(), entries.last().unwrap().0.as_slice());
    }

    // ---- reuse ------------------------------------------------------------------

    #[test]
    fn reset_makes_the_builder_behave_like_a_new_one() {
        let mut rng = Rng(17);
        for interval in [1, 2, 16] {
            let mut reused = builder(interval);
            for round in 0..5 {
                let entries = random_entries(&mut rng, 1 + round * 9);
                let bytes = build_with(&mut reused, &entries);
                assert_eq!(
                    bytes,
                    build(&entries, interval),
                    "round {round}, interval {interval}"
                );
                assert_eq!(read_back(bytes), entries);
                reused.reset();
                assert!(reused.is_empty());
                assert_eq!(reused.current_size_estimate(), 8);
            }
        }
    }

    #[test]
    fn a_second_block_after_reset_starts_with_a_restart_at_zero() {
        let mut b = builder(2);
        build_with(
            &mut b,
            &vec![entry("a", "1"), entry("b", "2"), entry("c", "3")],
        );
        b.reset();
        let bytes = build_with(
            &mut b,
            &vec![entry("x", "9"), entry("y", "8"), entry("z", "7")],
        );
        let (_, restarts) = trailer(&bytes);
        assert_eq!(restarts.len(), 2);
        assert_eq!(restarts[0], 0);
    }

    #[test]
    fn a_reset_builder_does_not_remember_the_previous_last_key() {
        // "a" is smaller than the earlier block's last key, which is fine after a reset
        let mut b = builder(16);
        build_with(&mut b, &vec![entry("zzz", "1")]);
        b.reset();
        b.add(b"a", b"2");
        assert_eq!(read_back(b.finish().to_vec()), vec![entry("a", "2")]);
    }

    // ---- misuse -----------------------------------------------------------------

    #[test]
    #[should_panic(expected = "restart_interval")]
    fn a_zero_restart_interval_is_rejected() {
        builder(0);
    }

    #[test]
    #[should_panic]
    fn finish_twice_panics() {
        let mut b = builder(16);
        b.add(b"a", b"1");
        b.finish();
        b.finish();
    }

    #[test]
    #[should_panic]
    fn add_after_finish_panics() {
        let mut b = builder(16);
        b.add(b"a", b"1");
        b.finish();
        b.add(b"b", b"2");
    }

    #[test]
    fn add_works_again_after_finish_and_reset() {
        let mut b = builder(16);
        b.add(b"a", b"1");
        b.finish();
        b.reset();
        b.add(b"b", b"2");
        assert_eq!(read_back(b.finish().to_vec()), vec![entry("b", "2")]);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "increasing")]
    fn keys_out_of_order_are_caught_in_debug_builds() {
        let mut b = builder(16);
        b.add(b"b", b"1");
        b.add(b"a", b"2");
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "increasing")]
    fn a_repeated_key_is_caught_in_debug_builds() {
        let mut b = builder(16);
        b.add(b"a", b"1");
        b.add(b"a", b"2");
    }

    #[test]
    fn an_empty_first_key_is_allowed() {
        let mut b = builder(16);
        b.add(b"", b"v");
        b.add(b"a", b"w");
        assert_eq!(
            read_back(b.finish().to_vec()),
            vec![entry("", "v"), entry("a", "w")]
        );
    }

    #[test]
    fn the_order_check_uses_the_given_comparator() {
        let mut b = BlockBuilder::new(16, Arc::new(Reverse));
        b.add(b"c", b"1"); // descending is "increasing" for Reverse
        b.add(b"b", b"2");
        b.add(b"a", b"3");
        let expected = vec![entry("c", "1"), entry("b", "2"), entry("a", "3")];
        assert_eq!(read_back(b.finish().to_vec()), expected);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "increasing")]
    fn the_given_comparator_rejects_ascending_keys_when_it_orders_descending() {
        let mut b = BlockBuilder::new(16, Arc::new(Reverse));
        b.add(b"a", b"1");
        b.add(b"b", b"2");
    }
}
