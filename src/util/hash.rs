use xxhash_rust::xxh3::xxh3_64;

// hashing as fast as the memory bus can feed
pub(crate) fn hash(data: &[u8]) -> u64 {
    xxh3_64(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn is_deterministic() {
        let key = b"some key";
        assert_eq!(hash(key), hash(key));
        assert_eq!(hash(key), hash(&key.to_vec()));
    }

    #[test]
    fn pinned_values() {
        // Hash values are persisted (bloom filters), so they must never change.
        // The empty-input value is the published XXH3-64 (seed 0) constant;
        // the others were generated with xxhash-rust 0.8 and pin its output.
        assert_eq!(hash(b""), 0x2d06800538d394c2);
        assert_eq!(hash(b"a"), 0xe6c632b61e964e1f);
        assert_eq!(hash(b"leveldb"), 0x5ced9a40b7d4c5c8);
    }

    #[test]
    fn different_inputs_give_different_hashes() {
        assert_ne!(hash(b""), hash(&[0]));
        assert_ne!(hash(&[0]), hash(&[0, 0]));
        assert_ne!(hash(b"abc"), hash(b"abd"));
        assert_ne!(hash(b"abc"), hash(b"cba"));
    }

    #[test]
    fn all_lengths_across_internal_code_paths_are_distinct() {
        // XXH3 switches algorithm at 0, 1-3, 4-8, 9-16, 17-128, 129-240 and
        // >240 bytes. Hash every length of all-zero data up to 1000 bytes and
        // make sure none collide.
        let data = [0u8; 1000];
        let seen: HashSet<u64> = (0..=data.len()).map(|n| hash(&data[..n])).collect();
        assert_eq!(seen.len(), data.len() + 1);
    }

    #[test]
    fn unaligned_input_hashes_like_an_aligned_copy() {
        let buf: Vec<u8> = (0..=255u8).collect();
        for offset in 0..8 {
            for len in [0, 1, 3, 4, 8, 16, 17, 100, 200] {
                let slice = &buf[offset..offset + len];
                assert_eq!(hash(slice), hash(&slice.to_vec()));
            }
        }
    }

    #[test]
    fn sequential_keys_spread_evenly_over_shards_and_buckets() {
        // The cache picks a shard from the top 4 bits and a bucket from the low
        // bits, using 16-byte (id, offset) style keys.
        const N: u64 = 160_000;
        let mut shards = [0u32; 16];
        let mut low = [0u32; 16];
        for i in 0..N {
            let mut key = [0u8; 16];
            key[..8].copy_from_slice(&1u64.to_le_bytes());
            key[8..].copy_from_slice(&(i * 4096).to_le_bytes());
            let h = hash(&key);
            shards[(h >> 60) as usize] += 1;
            low[(h & 0xf) as usize] += 1;
        }
        let expected = (N / 16) as u32;
        for counts in [&shards, &low] {
            for &c in counts {
                // Within 5% of a perfectly even split.
                assert!(
                    c > expected * 95 / 100 && c < expected * 105 / 100,
                    "{counts:?}"
                );
            }
        }
    }
}
