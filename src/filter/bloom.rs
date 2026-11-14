use crate::filter::traits::FilterPolicy;

const K: u8 = 7;
const N: usize = 15;
const NAME: &str = "BLOOM_FILTER_POLICY";
pub struct BloomFilterPolicy {}
impl FilterPolicy for BloomFilterPolicy {
    fn name(&self) -> &'static str {
        NAME
    }

    fn create_filter(&self, keys: &[&[u8]], dst: &mut Vec<u8>) {
        // number of bits for the filter (appended to dst) is N * number of keys for a ~0.1% False Positive error rate with N = 15
        // round up to bytes and take floor with 8 bytes
        let sz = std::cmp::max(((keys.len() * N) + 7) / 8, 8);
        let pt: usize = dst.len();

        dst.append(&mut vec![0; sz]);

        // double-hash the keys and populate the bloom filter
        for key in keys {
            for bit in probes(key, (sz * 8) as u32, K) {
                dst[pt + bit / 8] |= 1 << (bit % 8);
            }
        }
        // append the K byte to the end of the filter so that reader can access K
        dst.push(K);
    }

    fn key_may_match(&self, key: &[u8], filter: &[u8]) -> bool {
        if filter.len() < 2 {
            return true;
        }

        // exclude the last K byte
        let sz = filter.len() - 1;
        let k: u8 = *filter.last().unwrap();
        probes(key, (sz * 8) as u32, k).all(|bit| filter[bit / 8] & (1 << (bit % 8)) != 0)
    }
}

/// Bit positions probed for `key` in a filter of `bits` bits, using `k` probes.
///
/// The writer sets these bits and the reader tests them, so both must use this
/// one function: any difference between the two would cause false negatives.
#[inline]
fn probes(key: &[u8], bits: u32, k: u8) -> impl Iterator<Item = usize> {
    let hash = crate::util::hash::hash(key);
    // split the 64-bit hash into two 32-bit halves for double hashing
    let h1 = (hash >> 32) as u32;
    let h2 = hash as u32;
    (0..k as u32).map(move |i| (h1.wrapping_add(h2.wrapping_mul(i)) % bits) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: BloomFilterPolicy = BloomFilterPolicy {};

    fn make_keys(n: usize, prefix: &str) -> Vec<Vec<u8>> {
        (0..n)
            .map(|i| format!("{prefix}-{i}").into_bytes())
            .collect()
    }

    fn build(keys: &[Vec<u8>]) -> Vec<u8> {
        let refs: Vec<&[u8]> = keys.iter().map(|k| k.as_slice()).collect();
        let mut dst = Vec::new();
        POLICY.create_filter(&refs, &mut dst);
        dst
    }

    #[test]
    fn name_is_fixed() {
        assert_eq!(POLICY.name(), NAME);
    }

    #[test]
    fn has_no_false_negatives_at_any_size() {
        for n in [0, 1, 2, 7, 8, 9, 100, 1_000, 10_000] {
            let keys = make_keys(n, "key");
            let filter = build(&keys);
            for key in &keys {
                assert!(POLICY.key_may_match(key, &filter), "n={n} key={key:?}");
            }
        }
    }

    #[test]
    fn layout_is_bit_array_then_k_byte_with_an_8_byte_floor() {
        for n in [0, 1, 4, 5, 100, 1_000] {
            let filter = build(&make_keys(n, "key"));
            let bytes = std::cmp::max((n * N).div_ceil(8), 8);
            assert_eq!(filter.len(), bytes + 1, "n={n}");
            assert_eq!(*filter.last().unwrap(), K, "n={n}");
        }
    }

    #[test]
    fn output_is_pinned() {
        // The filter is persisted, so its exact bytes must never change by accident.
        let keys = [b"hello".to_vec(), b"world".to_vec()];
        assert_eq!(build(&keys), [72, 146, 36, 170, 42, 0, 0, 0, 7]);
    }

    #[test]
    fn false_positive_rate_is_near_the_design_target() {
        // 15 bits per key with 7 probes should give about 0.1%.
        let filter = build(&make_keys(10_000, "key"));
        let probes = 100_000;
        let hits = make_keys(probes, "absent")
            .iter()
            .filter(|k| POLICY.key_may_match(k, &filter))
            .count();
        assert!(
            hits < probes / 200,
            "false positive rate too high: {hits}/{probes}"
        );
    }

    #[test]
    fn a_filter_for_no_keys_matches_nothing() {
        let filter = build(&[]);
        assert_eq!(filter.len(), 9);
        for key in make_keys(1_000, "absent") {
            assert!(!POLICY.key_may_match(&key, &filter));
        }
        assert!(!POLICY.key_may_match(b"", &filter));
    }

    #[test]
    fn unusable_filters_answer_maybe_without_panicking() {
        assert!(POLICY.key_may_match(b"key", &[]));
        assert!(POLICY.key_may_match(b"key", &[K]));
    }

    #[test]
    fn handles_empty_binary_and_duplicate_keys() {
        let keys = vec![
            vec![],
            vec![0x00],
            vec![0xff, 0xff],
            vec![0x00, 0x01, 0x02],
            vec![0x00],
        ];
        let filter = build(&keys);
        for key in &keys {
            assert!(POLICY.key_may_match(key, &filter), "key={key:?}");
        }
    }

    #[test]
    fn create_filter_appends_and_leaves_existing_bytes_alone() {
        let keys = make_keys(50, "key");
        let refs: Vec<&[u8]> = keys.iter().map(|k| k.as_slice()).collect();
        let mut dst = vec![0xAA, 0xBB, 0xCC];
        POLICY.create_filter(&refs, &mut dst);
        assert_eq!(&dst[..3], &[0xAA, 0xBB, 0xCC]);
        assert_eq!(&dst[3..], build(&keys).as_slice());
    }

    #[test]
    fn filters_written_back_to_back_are_each_readable() {
        let a = make_keys(30, "a");
        let b = make_keys(500, "b");
        let (refs_a, refs_b): (Vec<&[u8]>, Vec<&[u8]>) = (
            a.iter().map(|k| k.as_slice()).collect(),
            b.iter().map(|k| k.as_slice()).collect(),
        );
        let mut dst = Vec::new();
        POLICY.create_filter(&refs_a, &mut dst);
        let split = dst.len();
        POLICY.create_filter(&refs_b, &mut dst);
        let (fa, fb) = dst.split_at(split);
        assert!(a.iter().all(|k| POLICY.key_may_match(k, fa)));
        assert!(b.iter().all(|k| POLICY.key_may_match(k, fb)));
    }

    #[test]
    fn reader_uses_the_k_stored_in_the_filter() {
        let keys = make_keys(100, "key");
        let mut filter = build(&keys);
        // Fewer probes than were written is still a subset, so no key can be lost.
        *filter.last_mut().unwrap() = 1;
        assert!(keys.iter().all(|k| POLICY.key_may_match(k, &filter)));

        // With the bit array cleared, any stored k >= 1 rejects every key.
        let mut empty = vec![0u8; 16];
        empty.push(3);
        assert!(!POLICY.key_may_match(b"anything", &empty));

        // With every bit set, every key matches.
        let mut full = vec![0xffu8; 16];
        full.push(3);
        assert!(POLICY.key_may_match(b"anything", &full));
    }

    #[test]
    fn probe_positions_are_in_range_and_deterministic() {
        for bits in [8u32, 64, 1_000, 80_000] {
            let first: Vec<usize> = probes(b"some key", bits, K).collect();
            let second: Vec<usize> = probes(b"some key", bits, K).collect();
            assert_eq!(first, second);
            assert_eq!(first.len(), K as usize);
            assert!(first.iter().all(|&b| b < bits as usize));
        }
    }
}
