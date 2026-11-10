// Little-endian fixed-width and varint encodings, ported from LevelDB's
// util/coding.cc. All `put_*` functions append to `dst`.

pub(crate) fn put_fixed32(dst: &mut Vec<u8>, value: u32) {
    // same as dst.extend_from_slice(&value.to_le_bytes());
    dst.push(value as u8);
    dst.push((value >> 8) as u8);
    dst.push((value >> 16) as u8);
    dst.push((value >> 24) as u8);
}

pub(crate) fn put_fixed64(dst: &mut Vec<u8>, value: u64) {
    dst.push(value as u8);
    dst.push((value >> 8) as u8);
    dst.push((value >> 16) as u8);
    dst.push((value >> 24) as u8);
    dst.push((value >> 32) as u8);
    dst.push((value >> 40) as u8);
    dst.push((value >> 48) as u8);
    dst.push((value >> 56) as u8);
}

pub(crate) fn put_varint32(dst: &mut Vec<u8>, value: u32) {
    // Each byte carries 7 bits of the value; the top bit means "more bytes
    // follow". A u32 needs at most 5 bytes (like 0xFFFFFFFF)
    // unrolled version might be a bit faster compared to the loop in put_varint64
    const B: u8 = 128;
    if value < (1 << 7) {
        dst.push(value as u8);
    } else if value < (1 << 14) {
        dst.push(value as u8 | B);
        dst.push((value >> 7) as u8);
    } else if value < (1 << 21) {
        dst.push(value as u8 | B);
        dst.push((value >> 7) as u8 | B);
        dst.push((value >> 14) as u8);
    } else if value < (1 << 28) {
        dst.push(value as u8 | B);
        dst.push((value >> 7) as u8 | B);
        dst.push((value >> 14) as u8 | B);
        dst.push((value >> 21) as u8);
    } else {
        dst.push(value as u8 | B);
        dst.push((value >> 7) as u8 | B);
        dst.push((value >> 14) as u8 | B);
        dst.push((value >> 21) as u8 | B);
        dst.push((value >> 28) as u8);
    }
}

pub(crate) fn put_varint64(dst: &mut Vec<u8>, mut value: u64) {
    const B: u8 = 128;
    while value >= B as u64 {
        dst.push(value as u8 | B);
        value >>= 7;
    }
    dst.push(value as u8);
}

pub(crate) fn put_length_prefixed_slice(dst: &mut Vec<u8>, value: &[u8]) {
    put_varint32(dst, value.len() as u32);
    dst.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Test-only helpers -------------------------------------------------
    // These are independent of the code under test. Replace the decoders with
    // `get_*` once those are ported.

    fn decode_fixed32(bytes: &[u8]) -> u32 {
        u32::from_le_bytes(bytes[..4].try_into().unwrap())
    }

    fn decode_fixed64(bytes: &[u8]) -> u64 {
        u64::from_le_bytes(bytes[..8].try_into().unwrap())
    }

    /// Returns (value, number of bytes consumed).
    fn decode_varint32(input: &[u8]) -> (u32, usize) {
        let mut result = 0u32;
        for (i, &byte) in input.iter().enumerate() {
            result |= ((byte & 127) as u32) << (7 * i);
            if byte & 128 == 0 {
                return (result, i + 1);
            }
        }
        panic!("truncated varint: {input:?}");
    }

    /// The simple loop form of the encoding, used to cross-check the unrolled
    /// version in `put_varint32`.
    fn reference_varint32(mut v: u32) -> Vec<u8> {
        let mut out = Vec::new();
        while v >= 128 {
            out.push(v as u8 | 128);
            v >>= 7;
        }
        out.push(v as u8);
        out
    }

    fn encode_varint32(v: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        put_varint32(&mut buf, v);
        buf
    }

    // ---- Fixed32 -----------------------------------------------------------

    #[test]
    fn fixed32_appends_little_endian() {
        let mut buf = Vec::new();
        put_fixed32(&mut buf, 0x12345678);
        assert_eq!(buf, [0x78, 0x56, 0x34, 0x12]);
    }

    #[test]
    fn fixed32_appends_to_existing_data() {
        let mut buf = b"AB".to_vec();
        put_fixed32(&mut buf, 52);
        assert_eq!(buf, [b'A', b'B', 52, 0, 0, 0]);
    }

    #[test]
    fn fixed32_always_four_bytes() {
        for v in [0u32, 1, 255, 256, u32::MAX] {
            let mut buf = Vec::new();
            put_fixed32(&mut buf, v);
            assert_eq!(buf.len(), 4, "value {v}");
        }
    }

    #[test]
    fn fixed32_boundaries() {
        let mut buf = Vec::new();
        put_fixed32(&mut buf, 0);
        put_fixed32(&mut buf, u32::MAX);
        assert_eq!(buf, [0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
    }

    // Port of Coding.Fixed32 and the 32-bit half of Coding.EncodingOutput.
    #[test]
    fn fixed32_roundtrip_many() {
        let mut buf = Vec::new();
        for v in 0..100_000u32 {
            put_fixed32(&mut buf, v);
        }
        assert_eq!(buf.len(), 100_000 * 4);
        for (v, chunk) in (0..100_000u32).zip(buf.chunks_exact(4)) {
            assert_eq!(decode_fixed32(chunk), v);
        }
    }

    // ---- Fixed64 -----------------------------------------------------------

    // Port of the 64-bit half of Coding.EncodingOutput.
    #[test]
    fn fixed64_appends_little_endian() {
        let mut buf = Vec::new();
        put_fixed64(&mut buf, 0x0807060504030201);
        assert_eq!(buf, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn fixed64_appends_to_existing_data() {
        let mut buf = b"AB".to_vec();
        put_fixed64(&mut buf, 52);
        assert_eq!(buf, [b'A', b'B', 52, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn fixed64_boundaries() {
        let mut buf = Vec::new();
        put_fixed64(&mut buf, 0);
        put_fixed64(&mut buf, u64::MAX);
        assert_eq!(buf[..8], [0; 8]);
        assert_eq!(buf[8..], [0xFF; 8]);
    }

    // Port of Coding.Fixed64: values around every power of two.
    #[test]
    fn fixed64_roundtrip_near_powers_of_two() {
        let mut buf = Vec::new();
        let mut expected = Vec::new();
        for power in 0..=63 {
            let v = 1u64 << power;
            for x in [v - 1, v, v + 1] {
                put_fixed64(&mut buf, x);
                expected.push(x);
            }
        }
        assert_eq!(buf.len(), expected.len() * 8);
        for (want, chunk) in expected.iter().zip(buf.chunks_exact(8)) {
            assert_eq!(decode_fixed64(chunk), *want);
        }
    }

    // ---- Varint32 ----------------------------------------------------------

    // Known byte sequences at every point where the encoded length changes.
    // These are the values that must match LevelDB byte for byte.
    #[test]
    fn varint32_known_encodings() {
        let cases: &[(u32, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (52, &[0x34]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (300, &[0xAC, 0x02]),
            (16_383, &[0xFF, 0x7F]),
            (16_384, &[0x80, 0x80, 0x01]),
            (2_097_151, &[0xFF, 0xFF, 0x7F]),
            (2_097_152, &[0x80, 0x80, 0x80, 0x01]),
            (268_435_455, &[0xFF, 0xFF, 0xFF, 0x7F]),
            (268_435_456, &[0x80, 0x80, 0x80, 0x80, 0x01]),
            (u32::MAX, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
        ];
        for &(value, expected) in cases {
            assert_eq!(encode_varint32(value), expected, "value {value}");
        }
    }

    #[test]
    fn varint32_appends_to_existing_data() {
        let mut buf = b"AB".to_vec();
        put_varint32(&mut buf, 300);
        assert_eq!(buf, [b'A', b'B', 0xAC, 0x02]);
    }

    #[test]
    fn varint32_consecutive_values_do_not_overlap() {
        let mut buf = Vec::new();
        put_varint32(&mut buf, 1);
        put_varint32(&mut buf, 300);
        put_varint32(&mut buf, u32::MAX);
        assert_eq!(buf, [0x01, 0xAC, 0x02, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    }

    // The unrolled encoder must agree with the simple loop for every value we
    // can reasonably check: a dense low range plus the neighbourhood of every
    // power of two.
    #[test]
    fn varint32_matches_reference_encoding() {
        for v in 0..200_000u32 {
            assert_eq!(encode_varint32(v), reference_varint32(v), "value {v}");
        }
        for shift in 0..32 {
            let p = 1u32 << shift;
            for v in [p - 1, p, p.wrapping_add(1)] {
                assert_eq!(encode_varint32(v), reference_varint32(v), "value {v}");
            }
        }
        assert_eq!(
            encode_varint32(u32::MAX),
            reference_varint32(u32::MAX),
            "u32::MAX"
        );
    }

    // Structural rule of the format: every byte but the last has the top bit
    // set, and the last byte does not. Length is 1..=5.
    #[test]
    fn varint32_continuation_bits_are_well_formed() {
        for shift in 0..32 {
            let p = 1u32 << shift;
            for v in [p - 1, p, p.wrapping_add(1), u32::MAX] {
                let enc = encode_varint32(v);
                assert!((1..=5).contains(&enc.len()), "value {v}: len {}", enc.len());
                let (last, rest) = enc.split_last().unwrap();
                assert_eq!(last & 128, 0, "value {v}: last byte has continuation bit");
                assert!(
                    rest.iter().all(|b| b & 128 != 0),
                    "value {v}: missing continuation bit"
                );
            }
        }
    }

    // Port of Coding.Varint32: many values concatenated, then decoded in order.
    #[test]
    fn varint32_roundtrip_many() {
        let values: Vec<u32> = (0..32u32 * 32).map(|i| (i / 32) << (i % 32)).collect();

        let mut buf = Vec::new();
        for &v in &values {
            put_varint32(&mut buf, v);
        }

        let mut pos = 0;
        for &expected in &values {
            let (actual, consumed) = decode_varint32(&buf[pos..]);
            assert_eq!(actual, expected);
            assert_eq!(consumed, reference_varint32(expected).len());
            pos += consumed;
        }
        assert_eq!(pos, buf.len());
    }

    // ---- Varint64 ----------------------------------------------------------

    /// Returns (value, number of bytes consumed).
    fn decode_varint64(input: &[u8]) -> (u64, usize) {
        let mut result = 0u64;
        for (i, &byte) in input.iter().enumerate() {
            result |= ((byte & 127) as u64) << (7 * i);
            if byte & 128 == 0 {
                return (result, i + 1);
            }
        }
        panic!("truncated varint: {input:?}");
    }

    fn encode_varint64(v: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        put_varint64(&mut buf, v);
        buf
    }

    #[test]
    fn varint64_known_encodings() {
        let cases: &[(u64, &[u8])] = &[
            (0, &[0x00]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (300, &[0xAC, 0x02]),
            (u32::MAX as u64, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
            (1 << 32, &[0x80, 0x80, 0x80, 0x80, 0x10]),
            (1 << 63, &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
            (u64::MAX, &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]),
        ];
        for &(value, expected) in cases {
            assert_eq!(encode_varint64(value), expected, "value {value}");
        }
    }

    #[test]
    fn varint64_appends_to_existing_data() {
        let mut buf = b"AB".to_vec();
        put_varint64(&mut buf, 300);
        assert_eq!(buf, [b'A', b'B', 0xAC, 0x02]);
    }

    // For values that fit in 32 bits, the 32- and 64-bit encoders must agree.
    #[test]
    fn varint64_agrees_with_varint32_for_u32_values() {
        for v in (0..200_000u32).chain([u32::MAX - 1, u32::MAX]) {
            assert_eq!(encode_varint64(v as u64), encode_varint32(v), "value {v}");
        }
        for shift in 0..32 {
            let p = 1u32 << shift;
            for v in [p - 1, p, p.wrapping_add(1)] {
                assert_eq!(encode_varint64(v as u64), encode_varint32(v), "value {v}");
            }
        }
    }

    #[test]
    fn varint64_continuation_bits_are_well_formed() {
        for shift in 0..64 {
            let p = 1u64 << shift;
            for v in [p - 1, p, p.wrapping_add(1), u64::MAX] {
                let enc = encode_varint64(v);
                assert!((1..=10).contains(&enc.len()), "value {v}: len {}", enc.len());
                let (last, rest) = enc.split_last().unwrap();
                assert_eq!(last & 128, 0, "value {v}: last byte has continuation bit");
                assert!(
                    rest.iter().all(|b| b & 128 != 0),
                    "value {v}: missing continuation bit"
                );
            }
        }
    }

    // Port of Coding.Varint64.
    #[test]
    fn varint64_roundtrip_many() {
        let mut values = vec![0u64, 100, u64::MAX, u64::MAX - 1];
        for k in 0..64 {
            let power = 1u64 << k;
            values.extend([power, power - 1, power.wrapping_add(1)]);
        }

        let mut buf = Vec::new();
        for &v in &values {
            put_varint64(&mut buf, v);
        }

        let mut pos = 0;
        for &expected in &values {
            let (actual, consumed) = decode_varint64(&buf[pos..]);
            assert_eq!(actual, expected);
            assert_eq!(consumed, encode_varint64(expected).len());
            pos += consumed;
        }
        assert_eq!(pos, buf.len());
    }
}
