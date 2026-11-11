use crc_fast::{checksum, CrcAlgorithm::Crc32Iscsi, Digest};

// using Castagnoli polynomial for CRC32C (Crc32Iscsi)

const M: u32 = 0x3d2afb3c; // random number for masking
pub(crate) fn value(data: &[u8]) -> u32 {
    checksum(Crc32Iscsi, data) as u32
}

pub(crate) fn extend(crc: u32, data: &[u8]) -> u32 {
    // final XOR with 0xffff_ffff flips the bits in starting state as they will be flipped again during finalize()
    let mut digest: Digest = Digest::new_with_init_state(Crc32Iscsi, (crc ^ 0xffff_ffff) as u64);
    digest.update(data);
    digest.finalize() as u32
}

pub(crate) fn mask(crc: u32) -> u32 {
    let t = crc.rotate_left(15);
    t.wrapping_add(M)
}

pub(crate) fn unmask(crc: u32) -> u32 {
    let t = crc.wrapping_sub(M);
    t.rotate_right(15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extend_equals_one_shot() {
        assert_eq!(value(b"hello world"), extend(value(b"hello "), b"world"));
    }

    #[test]
    fn extend_works_at_every_split_point_and_in_many_pieces() {
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let whole = value(&data);
        for split in 0..=data.len() {
            let (a, b) = data.split_at(split);
            assert_eq!(extend(value(a), b), whole, "split at {split}");
        }
        let mut crc = value(&[]);
        for chunk in data.chunks(7) {
            crc = extend(crc, chunk);
        }
        assert_eq!(crc, whole);
    }

    #[test]
    fn extend_with_empty_data_changes_nothing() {
        for crc in [0, 1, value(b"foo"), 0xdead_beef, u32::MAX] {
            assert_eq!(extend(crc, b""), crc);
        }
        assert_eq!(extend(value(b""), b"abc"), value(b"abc"));
    }

    #[test]
    fn mask_roundtrip() {
        let crc = value(b"foo");
        assert_ne!(crc, mask(crc));
        assert_ne!(crc, mask(mask(crc)));
        assert_eq!(crc, unmask(mask(crc)));
        assert_eq!(crc, unmask(unmask(mask(mask(crc)))));
    }

    #[test]
    fn mask_is_reversible_for_edge_and_sampled_values() {
        let edges = [0, 1, 0x8000_0000, 0xffff_ffff, M, M.wrapping_sub(1), M.wrapping_add(1)];
        for crc in edges {
            assert_eq!(unmask(mask(crc)), crc, "crc {crc:#x}");
            assert_eq!(mask(unmask(crc)), crc, "masked {crc:#x}");
        }
        for crc in (0..u32::MAX).step_by(65_537) {
            assert_eq!(unmask(mask(crc)), crc);
        }
    }

    #[test]
    fn detects_a_single_flipped_bit() {
        let mut data = vec![0x5au8; 4096];
        let good = value(&data);
        for byte in [0, 1, 2047, 4095] {
            for bit in 0..8 {
                data[byte] ^= 1 << bit;
                assert_ne!(value(&data), good, "byte {byte} bit {bit}");
                data[byte] ^= 1 << bit;
            }
        }
        assert_eq!(value(&data), good);
    }
}