use crate::comparator::Comparator;

pub struct BytewiseComparator;

const NAME: &str = "BYTEWISE";
impl Comparator for BytewiseComparator {
    fn compare(&self, a: &[u8], b: &[u8]) -> std::cmp::Ordering {
        a.cmp(b)
    }

    fn name(&self) -> &'static str {
        NAME
    }

    fn find_shortest_separator(&self, start: &mut Vec<u8>, limit: &[u8]) {
        let l = std::cmp::min(limit.len(), start.len());
        for i in 0..l {
            if start[i] < limit[i] && (limit[i] - start[i] != 1u8) {
                // ith byte is lesser and difference is > 1
                start[i] += 1;
                start.truncate(i + 1);
                return;
            }
        }
    }

    fn find_short_successor(&self, key: &mut Vec<u8>) {
        if !key.is_empty() && !(*key.first().unwrap() == 0xffu8) {
            *key.get_mut(0).unwrap() += 1;
            key.truncate(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::{Equal, Greater, Less};

    const C: BytewiseComparator = BytewiseComparator;

    fn separator(start: &[u8], limit: &[u8]) -> Vec<u8> {
        let mut s = start.to_vec();
        C.find_shortest_separator(&mut s, limit);
        s
    }

    fn successor(key: &[u8]) -> Vec<u8> {
        let mut k = key.to_vec();
        C.find_short_successor(&mut k);
        k
    }

    /// Small deterministic xorshift so the property tests need no dependency.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        /// Random key of length 0..=max_len. Bytes come from a tiny alphabet
        /// that includes both extremes, so prefixes, adjacent bytes and 0xff
        /// show up often.
        fn key(&mut self, max_len: usize) -> Vec<u8> {
            const ALPHABET: [u8; 5] = [0x00, 0x01, 0x7f, 0xfe, 0xff];
            let len = self.next() as usize % (max_len + 1);
            (0..len)
                .map(|_| ALPHABET[self.next() as usize % ALPHABET.len()])
                .collect()
        }
    }

    #[test]
    fn name_is_fixed() {
        assert_eq!(C.name(), NAME);
    }

    #[test]
    fn compare_orders_bytewise() {
        assert_eq!(C.compare(b"abc", b"abc"), Equal);
        assert_eq!(C.compare(b"abc", b"abd"), Less);
        assert_eq!(C.compare(b"abd", b"abc"), Greater);
        assert_eq!(C.compare(b"", b""), Equal);
    }

    #[test]
    fn compare_treats_a_prefix_as_smaller() {
        assert_eq!(C.compare(b"abc", b"abcd"), Less);
        assert_eq!(C.compare(b"abcd", b"abc"), Greater);
        assert_eq!(C.compare(b"", b"a"), Less);
    }

    #[test]
    fn compare_uses_unsigned_bytes() {
        // 0x80 and above must sort after ASCII, not before as signed bytes would.
        assert_eq!(C.compare(&[0x7f], &[0x80]), Less);
        assert_eq!(C.compare(&[0x00], &[0xff]), Less);
        assert_eq!(C.compare(&[0xff], &[0x00]), Greater);
    }

    #[test]
    fn separator_shortens_at_first_differing_byte() {
        assert_eq!(separator(b"the quick brown fox", b"the who"), b"the r");
        assert_eq!(separator(b"a", b"c"), b"b");
        assert_eq!(separator(&[0x00, 0x05], &[0x02, 0x00]), [0x01]);
    }

    #[test]
    fn separator_leaves_adjacent_bytes_unchanged_when_tail_cannot_shrink() {
        // 'c' + 1 == 'd', so bumping the differing byte would equal limit.
        assert_eq!(separator(b"abc", b"abd"), b"abc");
        assert_eq!(separator(&[0x01], &[0x02]), [0x01]);
    }

    #[test]
    fn separator_leaves_prefix_and_equal_keys_unchanged() {
        assert_eq!(separator(b"abc", b"abcdef"), b"abc");
        assert_eq!(separator(b"abc", b"abc"), b"abc");
        assert_eq!(separator(b"", b"abc"), b"");
        assert_eq!(separator(b"abc", b""), b"abc");
    }

    #[test]
    fn separator_stops_after_one_edit_without_panicking() {
        // Regression: the loop used to keep indexing after truncating.
        assert_eq!(
            separator(b"the quick brown fox", b"the who and more"),
            b"the r"
        );
        // start is longer than limit and the only difference has a gap of 1.
        assert_eq!(separator(b"aaaaaaaaaa", b"aab"), b"aaaaaaaaaa");
    }

    #[test]
    fn successor_increments_first_byte_and_truncates() {
        assert_eq!(successor(b"the quick brown fox"), b"u");
        assert_eq!(successor(&[0x00, 0x05]), [0x01]);
        assert_eq!(successor(&[0xfe, 0x10]), [0xff]);
        assert_eq!(successor(b"a"), b"b");
    }

    #[test]
    fn successor_leaves_empty_and_leading_ff_unchanged() {
        assert_eq!(successor(b""), b"");
        assert_eq!(successor(&[0xff]), [0xff]);
        assert_eq!(successor(&[0xff, 0x10, 0x20]), [0xff, 0x10, 0x20]);
        assert_eq!(successor(&[0xff, 0xff]), [0xff, 0xff]);
    }

    #[test]
    fn separator_property_holds_for_random_pairs() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut shortened = 0;
        let mut checked = 0;
        for _ in 0..200_000 {
            let (mut a, mut b) = (rng.key(8), rng.key(8));
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            if a == b {
                continue;
            }
            let sep = separator(&a, &b);
            assert_ne!(
                C.compare(&a, &sep),
                Greater,
                "sep below start: {a:?} {b:?} -> {sep:?}"
            );
            assert_eq!(
                C.compare(&sep, &b),
                Less,
                "sep not below limit: {a:?} {b:?} -> {sep:?}"
            );
            assert!(
                sep.len() <= a.len(),
                "sep longer than start: {a:?} {b:?} -> {sep:?}"
            );
            checked += 1;
            if sep.len() < a.len() {
                shortened += 1;
            }
        }
        // Guard against a vacuous test: the generator must exercise shortening.
        assert!(
            shortened > checked / 50,
            "only {shortened}/{checked} shortened"
        );
    }

    #[test]
    fn successor_property_holds_for_random_keys() {
        let mut rng = Rng(0xdead_beef_cafe_f00d);
        for _ in 0..100_000 {
            let key = rng.key(8);
            let succ = successor(&key);
            assert_ne!(
                C.compare(&succ, &key),
                Less,
                "succ below key: {key:?} -> {succ:?}"
            );
            assert!(
                succ.len() <= key.len(),
                "succ longer than key: {key:?} -> {succ:?}"
            );
        }
    }
}
