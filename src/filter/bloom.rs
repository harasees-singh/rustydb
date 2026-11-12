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
