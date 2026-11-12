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
            let hash = crate::util::hash::hash(key);
            // divide the hash into 2 32 bit parts and use for the second hashing
            let hash1: u32 = (hash >> 32) as u32;
            let hash2: u32 = (hash.rotate_left(32) >> 32) as u32;

            // calculate the second hash iteratively and set the corresponding bits in the bloom filter
            for i in 0..K as u32 {
                let bit = (hash1.wrapping_add(hash2.wrapping_mul(i)) % ((sz * 8) as u32)) as usize;
                let byte = pt + bit / 8;
                dst[byte] |= 1 << (bit % 8);
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

        let hash = crate::util::hash::hash(key);
        let hash1: u32 = (hash >> 32) as u32;
        let hash2: u32 = (hash.rotate_left(32) >> 32) as u32;

        let k: u8 = *filter.last().unwrap();

        for i in 0..k as u32 {
            let bit = (hash1.wrapping_add(hash2.wrapping_mul(i)) % ((sz * 8) as u32)) as usize;
            let byte = bit / 8;
            if filter[byte] & (1 << (bit % 8)) == 0 {
                return false;
            }
        }

        true
    }
}
