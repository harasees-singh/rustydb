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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comparator::bytewise::BytewiseComparator;
    use crate::util::coding::put_varint32;
    use std::collections::BTreeSet;
    use std::sync::mpsc;
    use std::time::Duration;

    type Entries = Vec<(Vec<u8>, Vec<u8>)>;

    /// Orders keys backwards, to prove `seek` goes through the comparator.
    struct Reverse;

    impl Comparator for Reverse {
        fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
            b.cmp(a)
        }
        fn name(&self) -> &'static str {
            "test.Reverse"
        }
    }

    fn entry(key: &str, value: &str) -> (Vec<u8>, Vec<u8>) {
        (key.as_bytes().to_vec(), value.as_bytes().to_vec())
    }

    /// Writes the restart array and the restart count after `entries`.
    fn with_trailer(mut entries: Vec<u8>, restarts: &[u32]) -> Vec<u8> {
        for r in restarts {
            entries.extend_from_slice(&r.to_le_bytes());
        }
        entries.extend_from_slice(&(restarts.len() as u32).to_le_bytes());
        entries
    }

    /// Encodes sorted entries in the block format (a stand-in for
    /// `BlockBuilder`, which the tests should switch to once it exists).
    fn encode(entries: &[(Vec<u8>, Vec<u8>)], restart_interval: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut restarts = vec![0u32];
        let mut counter = 0;
        let mut last: &[u8] = &[];
        for (key, value) in entries {
            let shared = if counter < restart_interval {
                last.iter().zip(key).take_while(|(a, b)| a == b).count()
            } else {
                restarts.push(buf.len() as u32);
                counter = 0;
                0
            };
            put_varint32(&mut buf, shared as u32);
            put_varint32(&mut buf, (key.len() - shared) as u32);
            put_varint32(&mut buf, value.len() as u32);
            buf.extend_from_slice(&key[shared..]);
            buf.extend_from_slice(value);
            last = key;
            counter += 1;
        }
        with_trailer(buf, &restarts)
    }

    fn open_with(data: Vec<u8>, cmp: Arc<dyn Comparator>) -> BlockIter {
        Arc::new(Block::new(data).unwrap()).iter(cmp)
    }

    fn open(data: Vec<u8>) -> BlockIter {
        open_with(data, Arc::new(BytewiseComparator))
    }

    fn at(it: &BlockIter) -> (Vec<u8>, Vec<u8>) {
        (it.key().to_vec(), it.value().to_vec())
    }

    fn scan_forward(it: &mut BlockIter) -> Entries {
        let mut out = Vec::new();
        it.seek_to_first();
        while it.valid() {
            out.push(at(it));
            it.next();
        }
        out
    }

    fn scan_backward(it: &mut BlockIter) -> Entries {
        let mut out = Vec::new();
        it.seek_to_last();
        while it.valid() {
            out.push(at(it));
            it.prev();
        }
        out
    }

    /// The first entry that is not less than `target`, by `cmp`.
    fn lower_bound<'a>(
        entries: &'a Entries,
        cmp: &dyn Comparator,
        target: &[u8],
    ) -> Option<&'a (Vec<u8>, Vec<u8>)> {
        entries
            .iter()
            .find(|e| cmp.compare(&e.0, target) != Ordering::Less)
    }

    /// Runs `f` on another thread and fails the test if it hangs or panics.
    fn must_finish(f: impl FnOnce() + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            f();
            let _ = tx.send(());
        });
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(()) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => panic!("hung"),
            Err(mpsc::RecvTimeoutError::Disconnected) => panic!("panicked"),
        }
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

    fn random_key(rng: &mut Rng) -> Vec<u8> {
        let len = (rng.next() % 8) as usize;
        (0..len).map(|_| b'a' + (rng.next() % 3) as u8).collect()
    }

    /// `n` distinct sorted entries; keys share many prefixes and one in five
    /// values is long enough to need a two-byte length.
    fn random_entries(rng: &mut Rng, n: usize) -> Entries {
        let mut keys = BTreeSet::new();
        while keys.len() < n {
            keys.insert(random_key(rng));
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

    /// The example from the builder's docs: four entries, restart interval 2.
    fn example_entries() -> Entries {
        vec![
            entry("apple", "1"),
            entry("applesauce", "22"),
            entry("apply", "333"),
            entry("banana", "4"),
        ]
    }

    fn example_bytes() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&[0, 5, 1, b'a', b'p', b'p', b'l', b'e', b'1']);
        v.extend_from_slice(&[5, 5, 2, b's', b'a', b'u', b'c', b'e', b'2', b'2']);
        v.extend_from_slice(&[0, 5, 3, b'a', b'p', b'p', b'l', b'y', b'3', b'3', b'3']);
        v.extend_from_slice(&[0, 6, 1, b'b', b'a', b'n', b'a', b'n', b'a', b'4']);
        with_trailer(v, &[0, 19])
    }

    // ---- the test encoder ---------------------------------------------------

    #[test]
    fn the_test_encoder_produces_the_documented_example() {
        let bytes = encode(&example_entries(), 2);
        assert_eq!(bytes.len(), 52);
        assert_eq!(bytes, example_bytes());
    }

    // ---- Block::new -----------------------------------------------------------

    #[test]
    fn new_reads_the_trailer_of_the_documented_example() {
        let block = Block::new(example_bytes()).unwrap();
        assert_eq!(block.restarts_count, 2);
        assert_eq!(block.restarts_offset, 40);
        assert_eq!(block.size(), 52);
    }

    #[test]
    fn new_rejects_data_too_short_for_the_restart_count() {
        for len in 0..4 {
            let err = Block::new(vec![0; len]).err().unwrap();
            assert!(err.is_corruption(), "len {len}");
        }
    }

    #[test]
    fn new_accepts_zero_restarts_and_the_builders_empty_block() {
        let block = Block::new(vec![0; 4]).unwrap();
        assert_eq!((block.restarts_offset, block.restarts_count), (0, 0));
        let block = Block::new(with_trailer(Vec::new(), &[0])).unwrap();
        assert_eq!((block.restarts_offset, block.restarts_count), (0, 1));
    }

    #[test]
    fn new_checks_that_the_restart_array_fits() {
        // 4 + 4 * count == len fits exactly, one more does not.
        let exact = Block::new(vec![9, 9, 9, 9, 1, 0, 0, 0]).unwrap();
        assert_eq!((exact.restarts_offset, exact.restarts_count), (0, 1));
        assert!(Block::new(vec![9, 9, 9, 9, 2, 0, 0, 0]).err().unwrap().is_corruption());
        assert!(Block::new(vec![0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]).is_err());
    }

    // ---- BlockIter: basics ----------------------------------------------------

    #[test]
    fn a_new_cursor_is_not_positioned_on_any_entry() {
        let it = open(example_bytes());
        assert!(!it.valid());
        assert!(it.status().is_ok());
    }

    #[test]
    fn the_cursor_keeps_the_block_alive() {
        let block = Arc::new(Block::new(example_bytes()).unwrap());
        let mut it = block.iter(Arc::new(BytewiseComparator));
        drop(block);
        assert_eq!(scan_forward(&mut it), example_entries());
    }

    #[test]
    fn forward_scan_rebuilds_keys_from_the_prefix_compression() {
        let mut it = open(example_bytes());
        assert_eq!(scan_forward(&mut it), example_entries());
        assert!(it.status().is_ok());
    }

    #[test]
    fn seek_to_first_and_last_on_the_example() {
        let mut it = open(example_bytes());
        it.seek_to_first();
        assert_eq!(at(&it), entry("apple", "1"));
        it.seek_to_last();
        assert_eq!(at(&it), entry("banana", "4"));
    }

    #[test]
    fn next_after_the_last_entry_is_a_clean_end() {
        let mut it = open(example_bytes());
        it.seek_to_last();
        it.next();
        assert!(!it.valid());
        assert!(it.status().is_ok(), "reaching the end is not an error");
    }

    #[test]
    fn prev_before_the_first_entry_is_a_clean_end() {
        let mut it = open(example_bytes());
        it.seek_to_first();
        it.prev();
        assert!(!it.valid());
        assert!(it.status().is_ok());
    }

    #[test]
    fn an_empty_block_has_no_entries_and_no_error() {
        let empty = with_trailer(Vec::new(), &[0]);
        let mut it = open(empty);
        it.seek_to_first();
        assert!(!it.valid() && it.status().is_ok());
        it.seek_to_last();
        assert!(!it.valid() && it.status().is_ok());
        it.seek(b"anything");
        assert!(!it.valid() && it.status().is_ok());
    }

    #[test]
    fn a_block_with_one_entry() {
        let entries = vec![entry("only", "one")];
        let mut it = open(encode(&entries, 16));
        assert_eq!(scan_forward(&mut it), entries);
        assert_eq!(scan_backward(&mut it), entries);
        it.seek(b"a");
        assert_eq!(at(&it), entry("only", "one"));
        it.seek(b"z");
        assert!(!it.valid());
    }

    #[test]
    fn long_keys_and_values_use_multi_byte_lengths() {
        let long_key = |c: char| format!("{}{c}", "k".repeat(300));
        let entries = vec![
            (long_key('a').into_bytes(), vec![1; 200]),
            (long_key('b').into_bytes(), vec![2; 70_000]),
            (b"z".to_vec(), Vec::new()),
        ];
        for interval in [1, 16] {
            let mut it = open(encode(&entries, interval));
            assert_eq!(scan_forward(&mut it), entries, "interval {interval}");
            assert_eq!(scan_backward(&mut it).len(), 3);
        }
    }

    #[test]
    fn keys_may_be_empty() {
        let entries = vec![entry("", "first"), entry("a", "second")];
        let mut it = open(encode(&entries, 16));
        assert_eq!(scan_forward(&mut it), entries);
        it.seek(b"");
        assert_eq!(at(&it), entry("", "first"));
    }

    // ---- BlockIter: prev --------------------------------------------------------

    #[test]
    fn prev_walks_back_across_restart_points() {
        for interval in [1, 2, 3, 16] {
            let entries = random_entries(&mut Rng(7), 40);
            let mut it = open(encode(&entries, interval));
            let mut expected = entries.clone();
            expected.reverse();
            assert_eq!(scan_backward(&mut it), expected, "interval {interval}");
            assert!(it.status().is_ok());
        }
    }

    #[test]
    fn prev_from_every_position_goes_to_the_previous_entry() {
        let entries = random_entries(&mut Rng(11), 25);
        for interval in [1, 2, 5] {
            let data = encode(&entries, interval);
            for k in 1..entries.len() {
                let mut it = open(data.clone());
                it.seek_to_first();
                for _ in 0..k {
                    it.next();
                }
                it.prev();
                assert_eq!(at(&it), entries[k - 1], "interval {interval}, k {k}");
            }
        }
    }

    #[test]
    fn next_and_prev_can_be_mixed() {
        let entries = random_entries(&mut Rng(3), 30);
        let mut it = open(encode(&entries, 4));
        it.seek_to_first();
        let mut pos = 0;
        let mut rng = Rng(99);
        for _ in 0..500 {
            if pos == 0 || (pos + 1 < entries.len() && rng.next() % 2 == 0) {
                it.next();
                pos += 1;
            } else {
                it.prev();
                pos -= 1;
            }
            assert_eq!(at(&it), entries[pos]);
        }
    }

    // ---- BlockIter: seek --------------------------------------------------------

    #[test]
    fn seek_finds_the_first_key_not_less_than_the_target() {
        let mut it = open(example_bytes());
        let cases: [(&str, Option<&str>); 8] = [
            ("", Some("apple")),
            ("a", Some("apple")),
            ("apple", Some("apple")),
            ("applea", Some("applesauce")),
            ("applesauce", Some("applesauce")),
            ("applz", Some("banana")),
            ("banana", Some("banana")),
            ("bananb", None),
        ];
        for (target, expected) in cases {
            it.seek(target.as_bytes());
            match expected {
                Some(key) => {
                    assert!(it.valid(), "target {target:?}");
                    assert_eq!(it.key(), key.as_bytes(), "target {target:?}");
                }
                None => assert!(!it.valid(), "target {target:?}"),
            }
            assert!(it.status().is_ok());
        }
    }

    #[test]
    fn seek_works_from_any_previous_position() {
        let entries = random_entries(&mut Rng(5), 60);
        let mut it = open(encode(&entries, 4));
        for target in &entries {
            it.seek_to_last();
            it.seek(&target.0);
            assert_eq!(&at(&it), target, "after seek_to_last");
            it.seek_to_first();
            it.seek(&target.0);
            assert_eq!(&at(&it), target, "after seek_to_first");
            it.seek(&target.0); // seeking the same key twice
            assert_eq!(&at(&it), target);
        }
    }

    #[test]
    fn seek_then_next_and_prev_continue_from_there() {
        let entries = random_entries(&mut Rng(8), 50);
        let mut it = open(encode(&entries, 3));
        for k in 1..entries.len() - 1 {
            it.seek(&entries[k].0);
            it.next();
            assert_eq!(at(&it), entries[k + 1]);
            it.seek(&entries[k].0);
            it.prev();
            assert_eq!(at(&it), entries[k - 1]);
        }
    }

    #[test]
    fn seek_uses_the_comparator_it_was_given() {
        let mut entries = random_entries(&mut Rng(21), 40);
        entries.reverse(); // sorted by Reverse
        let cmp: Arc<dyn Comparator> = Arc::new(Reverse);
        let mut it = open_with(encode(&entries, 4), cmp.clone());
        let mut rng = Rng(77);
        let mut targets: Vec<Vec<u8>> = entries.iter().map(|e| e.0.clone()).collect();
        targets.extend((0..40).map(|_| random_key(&mut rng)));
        targets.push(vec![0xff, 0xff]);
        for t in targets {
            it.seek(&t);
            match lower_bound(&entries, &*cmp, &t) {
                Some(e) => assert_eq!(&at(&it), e, "target {t:?}"),
                None => assert!(!it.valid() && it.status().is_ok(), "target {t:?}"),
            }
        }
    }

    // ---- model: everything against a plain Vec, many shapes ---------------------

    #[test]
    fn matches_a_vec_model_for_many_sizes_and_restart_intervals() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for n in [0, 1, 2, 3, 17, 40, 150] {
            for interval in [1, 2, 3, 16, 100] {
                let entries = random_entries(&mut rng, n);
                let data = encode(&entries, interval);
                let mut it = open(data.clone());
                assert_eq!(scan_forward(&mut it), entries, "n {n}, interval {interval}");
                assert!(it.status().is_ok());

                let mut reversed = entries.clone();
                reversed.reverse();
                assert_eq!(scan_backward(&mut it), reversed, "n {n}, interval {interval}");
                assert!(it.status().is_ok());

                let mut targets: Vec<Vec<u8>> = entries.iter().map(|e| e.0.clone()).collect();
                targets.extend((0..20).map(|_| random_key(&mut rng)));
                targets.push(Vec::new());
                targets.push(vec![0xff; 4]);
                for t in &targets {
                    it.seek(t);
                    match lower_bound(&entries, &BytewiseComparator, t) {
                        Some(e) => assert_eq!(&at(&it), e, "n {n}, iv {interval}, {t:?}"),
                        None => assert!(!it.valid() && it.status().is_ok()),
                    }
                }
            }
        }
    }

    // ---- damaged blocks: errors, never panics or hangs --------------------------

    #[test]
    fn a_value_running_past_the_entries_is_corruption() {
        let mut it = open(with_trailer(vec![0, 1, 9, b'k', b'v'], &[0]));
        it.seek_to_first();
        assert!(!it.valid());
        assert!(it.status().unwrap_err().is_corruption());
    }

    #[test]
    fn a_key_running_past_the_entries_is_corruption() {
        let mut it = open(with_trailer(vec![0, 50, 1, b'k', b'v'], &[0]));
        it.seek_to_first();
        assert!(!it.valid());
        assert!(it.status().unwrap_err().is_corruption());
    }

    #[test]
    fn sharing_more_than_the_previous_key_has_is_corruption() {
        let mut it = open(with_trailer(vec![7, 1, 1, b'k', b'v'], &[0]));
        it.seek_to_first();
        assert!(!it.valid());
        assert!(it.status().unwrap_err().is_corruption());
    }

    #[test]
    fn a_truncated_length_is_corruption() {
        let mut it = open(with_trailer(vec![0, 0x80], &[0]));
        it.seek_to_first();
        assert!(!it.valid());
        assert!(it.status().unwrap_err().is_corruption());
    }

    #[test]
    fn a_bad_entry_after_good_ones_stops_the_scan_with_an_error() {
        let data = with_trailer(vec![0, 1, 1, b'a', b'x', 0, 9, 9, b'b'], &[0]);
        let mut it = open(data);
        it.seek_to_first();
        assert_eq!(at(&it), entry("a", "x"));
        it.next();
        assert!(!it.valid());
        assert!(it.status().unwrap_err().is_corruption());
    }

    #[test]
    fn an_error_is_sticky() {
        let mut it = open(with_trailer(vec![0, 1, 9, b'k', b'v'], &[0]));
        it.seek_to_first();
        assert!(it.status().is_err());
        it.seek_to_first();
        it.seek(b"a");
        it.seek_to_last();
        assert!(!it.valid());
        assert!(it.status().is_err());
    }

    #[test]
    fn seek_on_a_damaged_block_returns_with_an_error() {
        must_finish(|| {
            let data = with_trailer(vec![0, 1, 1, b'a', b'x', 0, 9, 9, b'b'], &[0]);
            let mut it = open(data);
            it.seek(b"z");
            assert!(!it.valid());
            assert!(it.status().unwrap_err().is_corruption());
        });
    }

    #[test]
    fn a_restart_point_outside_the_entries_is_corruption_found_by_seek() {
        // two entries, but the second restart point is far past them
        let data = with_trailer(vec![0, 1, 1, b'a', b'x', 0, 1, 1, b'c', b'y'], &[0, 200]);
        let mut it = open(data);
        it.seek(b"b");
        assert!(!it.valid());
        assert!(it.status().unwrap_err().is_corruption());
    }

    #[test]
    fn a_restart_entry_that_shares_a_prefix_is_corruption_found_by_seek() {
        // the entry at restart point 1 claims shared = 1, which a restart never does
        let data = with_trailer(vec![0, 1, 1, b'a', b'x', 1, 1, 1, b'c', b'y'], &[0, 5]);
        let mut it = open(data);
        it.seek(b"b");
        assert!(!it.valid());
        assert!(it.status().unwrap_err().is_corruption());
    }

    #[test]
    fn a_block_without_restart_points_is_handled_without_panicking() {
        let mut it = open(vec![0, 0, 0, 0]);
        it.seek_to_last();
        assert!(!it.valid());
        it.seek(b"a");
        assert!(!it.valid());
    }

    #[test]
    fn random_damage_never_panics_or_hangs() {
        must_finish(|| {
            let mut rng = Rng(0xD1B5_4A32_D192_ED03);
            for _ in 0..3_000 {
                let n = 1 + (rng.next() % 30) as usize;
                let interval = 1 + (rng.next() % 6) as usize;
                let mut data = encode(&random_entries(&mut rng, n), interval);
                for _ in 0..1 + rng.next() % 3 {
                    let at = (rng.next() as usize) % data.len();
                    data[at] = rng.next() as u8;
                }
                let Ok(block) = Block::new(data) else { continue };
                let mut it = Arc::new(block).iter(Arc::new(BytewiseComparator));
                let target = random_key(&mut rng);

                it.seek(&target);
                let mut steps = 0;
                while it.valid() && steps < 10_000 {
                    it.next();
                    steps += 1;
                }
                it.seek_to_last();
                steps = 0;
                while it.valid() && steps < 10_000 {
                    it.prev();
                    steps += 1;
                }
                it.seek_to_first();
                it.seek(&target);
                if it.valid() {
                    it.prev();
                }
            }
        });
    }
}
