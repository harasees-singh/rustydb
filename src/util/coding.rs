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

pub(crate) fn get_varint32(input: &mut &[u8], value: &mut u32) -> bool {
    const B: u8 = 128;
    let mut shift = 0u32;
    let mut result = 0u32;
    let bytes: &[u8] = input;

    for (i, &byte) in bytes.iter().take(5).enumerate() {
        result |= ((byte & (B - 1u8)) as u32) << shift;
        shift += 7;
        if byte & B == 0 {
            *input = &bytes[i + 1..];   // advance past what we consumed
            *value = result;
            return true;
        }
    }
    false
}

pub(crate) fn get_varint64(input: &mut &[u8], value: &mut u64) -> bool {
    const B: u8 = 128;
    let mut shift = 0u64;
    let mut result = 0u64;
    let bytes: &[u8] = input;

    for (i, &byte) in bytes.iter().take(10).enumerate() {
        result |= ((byte & (B - 1u8)) as u64) << shift;
        shift += 7;
        if byte & B == 0 {
            *input = &bytes[i + 1..];   // advance past what we consumed
            *value = result;
            return true;
        }
    }
    false
}

pub(crate) fn get_length_prefixed_slice<'a>(input: &mut &'a[u8], result: &mut &'a[u8]) -> bool {
    let len : &mut u32 = &mut 0;
    if !get_varint32(input, len) {
        return false;
    }
    if input.len() < *len as usize {
        return false;
    }
    let (head, tail) = input.split_at(*len as usize);
    *result = head;
    *input = tail;
    true
}

pub(crate) fn varint_length(mut v: u64) -> usize {
    const B: u8 = 128;
    let mut len = 1u8;
    while v >= B as u64 {
        v >>= 7;
        len += 1;
    }
    len as usize
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
            assert_eq!(varint_length(value as u64) as usize, expected.len(), "length of {value}");
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
            assert_eq!(consumed, varint_length(expected as u64) as usize);
            pos += consumed;
        }
        assert_eq!(pos, buf.len());
    }

    // ---- Varint64 ----------------------------------------------------------

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
            assert_eq!(varint_length(value) as usize, expected.len(), "length of {value}");
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

        let mut input: &[u8] = &buf;
        for &expected in &values {
            let before = input.len();
            let mut actual = 0;
            assert!(get_varint64(&mut input, &mut actual), "value {expected}");
            assert_eq!(actual, expected);
            assert_eq!(before - input.len(), varint_length(expected) as usize);
        }
        assert!(input.is_empty());
    }

    // ---- get_varint32 ------------------------------------------------------

    #[test]
    fn get_varint32_known_values() {
        let cases: &[(u32, &[u8])] = &[
            (0, &[0x00]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (300, &[0xAC, 0x02]),
            (16_384, &[0x80, 0x80, 0x01]),
            (u32::MAX, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
        ];
        for &(expected, bytes) in cases {
            let mut input = bytes;
            let mut value = 0;
            assert!(get_varint32(&mut input, &mut value), "{bytes:?}");
            assert_eq!(value, expected, "{bytes:?}");
            assert!(input.is_empty(), "{bytes:?}: whole varint must be consumed");
        }
    }

    #[test]
    fn get_varint32_advances_input_and_keeps_trailing_data() {
        let data = [0xAC, 0x02, 0x05, 0xFF];
        let mut input: &[u8] = &data;
        let mut value = 0;

        assert!(get_varint32(&mut input, &mut value));
        assert_eq!(value, 300);
        assert_eq!(input, [0x05, 0xFF]);

        assert!(get_varint32(&mut input, &mut value));
        assert_eq!(value, 5);
        assert_eq!(input, [0xFF]);
    }

    #[test]
    fn get_varint32_empty_input_fails() {
        let mut input: &[u8] = &[];
        let mut value = 7;
        assert!(!get_varint32(&mut input, &mut value));
        assert_eq!(value, 7);
    }

    #[test]
    fn get_varint32_failure_leaves_input_and_value_unchanged() {
        // 0xFF promises another byte, but there is none.
        let data = [0xFF];
        let mut input: &[u8] = &data;
        let mut value = 7;
        assert!(!get_varint32(&mut input, &mut value));
        assert_eq!(input, [0xFF]);
        assert_eq!(value, 7);
    }

    // Port of Coding.Varint32Overflow: five continuation bytes, then a sixth.
    #[test]
    fn get_varint32_overflow_fails() {
        let data = [0x81, 0x82, 0x83, 0x84, 0x85, 0x11];
        let mut input: &[u8] = &data;
        let mut value = 0;
        assert!(!get_varint32(&mut input, &mut value));
        assert_eq!(input, data, "input must be unchanged on failure");
    }

    // Port of Coding.Varint32Truncation: every proper prefix fails, and the
    // full encoding succeeds.
    #[test]
    fn get_varint32_truncation_fails() {
        let large_value = (1u32 << 31) + 100;
        let encoded = encode_varint32(large_value);
        let mut value = 0;

        for len in 0..encoded.len() {
            let mut input = &encoded[..len];
            assert!(!get_varint32(&mut input, &mut value), "prefix length {len}");
        }

        let mut input: &[u8] = &encoded;
        assert!(get_varint32(&mut input, &mut value));
        assert_eq!(value, large_value);
        assert!(input.is_empty());
    }

    // Port of Coding.Varint32: decode many concatenated values in order.
    #[test]
    fn get_varint32_roundtrip_many() {
        let values: Vec<u32> = (0..32u32 * 32).map(|i| (i / 32) << (i % 32)).collect();

        let mut buf = Vec::new();
        for &v in &values {
            put_varint32(&mut buf, v);
        }

        let mut input: &[u8] = &buf;
        for &expected in &values {
            let before = input.len();
            let mut value = 0;
            assert!(get_varint32(&mut input, &mut value));
            assert_eq!(value, expected);
            assert_eq!(before - input.len(), varint_length(expected as u64) as usize);
        }
        assert!(input.is_empty());
    }

    // Matches LevelDB: bits of the 5th byte above bit 31 are dropped, not
    // rejected. 0x7F << 28 keeps only its low 4 bits.
    #[test]
    fn get_varint32_fifth_byte_high_bits_are_dropped_like_leveldb() {
        let mut input: &[u8] = &[0x80, 0x80, 0x80, 0x80, 0x7F];
        let mut value = 0;
        assert!(get_varint32(&mut input, &mut value));
        assert_eq!(value, 0xF000_0000);
        assert!(input.is_empty());
    }

    // ---- get_varint64 ------------------------------------------------------

    #[test]
    fn get_varint64_known_values() {
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
        for &(expected, bytes) in cases {
            let mut input = bytes;
            let mut value = 0;
            assert!(get_varint64(&mut input, &mut value), "{bytes:?}");
            assert_eq!(value, expected, "{bytes:?}");
            assert!(input.is_empty(), "{bytes:?}: whole varint must be consumed");
        }
    }

    // Values that need more than 32 bits must not be truncated: this is the
    // case that distinguishes a correct u64 decoder from a u32 one.
    #[test]
    fn get_varint64_values_above_u32_range() {
        for v in [1u64 << 32, (1 << 32) + 1, 1 << 40, 1 << 62, u64::MAX - 1] {
            let encoded = encode_varint64(v);
            let mut input: &[u8] = &encoded;
            let mut value = 0;
            assert!(get_varint64(&mut input, &mut value));
            assert_eq!(value, v);
        }
    }

    #[test]
    fn get_varint64_advances_input_and_keeps_trailing_data() {
        let data = [0xAC, 0x02, 0x05, 0xFF];
        let mut input: &[u8] = &data;
        let mut value = 0;

        assert!(get_varint64(&mut input, &mut value));
        assert_eq!(value, 300);
        assert_eq!(input, [0x05, 0xFF]);

        assert!(get_varint64(&mut input, &mut value));
        assert_eq!(value, 5);
        assert_eq!(input, [0xFF]);
    }

    #[test]
    fn get_varint64_empty_input_fails() {
        let mut input: &[u8] = &[];
        let mut value = 7;
        assert!(!get_varint64(&mut input, &mut value));
        assert_eq!(value, 7);
    }

    #[test]
    fn get_varint64_failure_leaves_input_and_value_unchanged() {
        let data = [0xFF];
        let mut input: &[u8] = &data;
        let mut value = 7;
        assert!(!get_varint64(&mut input, &mut value));
        assert_eq!(input, [0xFF]);
        assert_eq!(value, 7);
    }

    // Port of Coding.Varint64Overflow: ten continuation bytes, then an 11th.
    #[test]
    fn get_varint64_overflow_fails() {
        let data = [
            0x81, 0x82, 0x83, 0x84, 0x85, 0x81, 0x82, 0x83, 0x84, 0x85, 0x11,
        ];
        let mut input: &[u8] = &data;
        let mut value = 0;
        assert!(!get_varint64(&mut input, &mut value));
        assert_eq!(input, data, "input must be unchanged on failure");
    }

    // Port of Coding.Varint64Truncation.
    #[test]
    fn get_varint64_truncation_fails() {
        let large_value = (1u64 << 63) + 100;
        let encoded = encode_varint64(large_value);
        let mut value = 0;

        for len in 0..encoded.len() {
            let mut input = &encoded[..len];
            assert!(!get_varint64(&mut input, &mut value), "prefix length {len}");
        }

        let mut input: &[u8] = &encoded;
        assert!(get_varint64(&mut input, &mut value));
        assert_eq!(value, large_value);
        assert!(input.is_empty());
    }

    // Matches LevelDB: bits of the 10th byte above bit 63 are dropped, not
    // rejected. 0x7F << 63 keeps only its lowest bit.
    #[test]
    fn get_varint64_tenth_byte_high_bits_are_dropped_like_leveldb() {
        let mut input: &[u8] = &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x7F];
        let mut value = 0;
        assert!(get_varint64(&mut input, &mut value));
        assert_eq!(value, 1u64 << 63);
        assert!(input.is_empty());
    }

    // A varint32 encoding must decode identically with either decoder.
    #[test]
    fn get_varint64_agrees_with_get_varint32_for_u32_values() {
        for v in (0..50_000u32).chain([u32::MAX - 1, u32::MAX]) {
            let encoded = encode_varint32(v);

            let mut in32: &[u8] = &encoded;
            let mut v32 = 0u32;
            assert!(get_varint32(&mut in32, &mut v32));

            let mut in64: &[u8] = &encoded;
            let mut v64 = 0u64;
            assert!(get_varint64(&mut in64, &mut v64));

            assert_eq!(v64, v32 as u64, "value {v}");
            assert_eq!(in64.len(), in32.len(), "value {v}");
        }
    }

    // ---- varint_length -----------------------------------------------------

    // The length grows by one byte at every power of 128 (2^7, 2^14, ...).
    // The value just below each boundary must still use the shorter length.
    #[test]
    fn varint_length_boundaries() {
        assert_eq!(varint_length(0), 1);
        assert_eq!(varint_length(u64::MAX), 10);
        for k in 1..=9u32 {
            let boundary = 1u64 << (7 * k);
            assert_eq!(varint_length(boundary - 1) as u32, k, "just below 2^{}", 7 * k);
            assert_eq!(varint_length(boundary) as u32, k + 1, "at 2^{}", 7 * k);
        }
    }

    // It must agree with the number of bytes the encoder actually writes.
    #[test]
    fn varint_length_matches_encoded_length() {
        for v in 0..200_000u64 {
            assert_eq!(varint_length(v) as usize, encode_varint64(v).len(), "value {v}");
        }
        for shift in 0..64 {
            let p = 1u64 << shift;
            for v in [p - 1, p, p.wrapping_add(1), u64::MAX] {
                assert_eq!(varint_length(v) as usize, encode_varint64(v).len(), "value {v}");
            }
        }
    }

    // ---- put_length_prefixed_slice -----------------------------------------

    fn encode_slice(value: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        put_length_prefixed_slice(&mut buf, value);
        buf
    }

    #[test]
    fn put_length_prefixed_slice_exact_bytes() {
        assert_eq!(encode_slice(b"hello"), [5, b'h', b'e', b'l', b'l', b'o']);
    }

    #[test]
    fn put_length_prefixed_slice_empty_is_a_single_zero_byte() {
        assert_eq!(encode_slice(b""), [0]);
    }

    // The length is a varint, so 127 bytes need a 1-byte prefix and 128 need 2.
    #[test]
    fn put_length_prefixed_slice_prefix_width_changes_at_128() {
        let enc127 = encode_slice(&[b'x'; 127]);
        assert_eq!(enc127.len(), 1 + 127);
        assert_eq!(enc127[0], 0x7F);

        let enc128 = encode_slice(&[b'x'; 128]);
        assert_eq!(enc128.len(), 2 + 128);
        assert_eq!(enc128[..2], [0x80, 0x01]);

        let enc200 = encode_slice(&[b'x'; 200]);
        assert_eq!(enc200[..2], [0xC8, 0x01]);
        assert!(enc200[2..].iter().all(|&b| b == b'x'));
    }

    #[test]
    fn put_length_prefixed_slice_appends_to_existing_data() {
        let mut buf = b"AB".to_vec();
        put_length_prefixed_slice(&mut buf, b"foo");
        assert_eq!(buf, [b'A', b'B', 3, b'f', b'o', b'o']);
    }

    // The slice contents are copied verbatim: zero bytes and bytes with the
    // top bit set must not be treated specially.
    #[test]
    fn put_length_prefixed_slice_copies_arbitrary_bytes() {
        let data = [0x00, 0xFF, 0x80, 0x7F, 0x00];
        assert_eq!(encode_slice(&data), [5, 0x00, 0xFF, 0x80, 0x7F, 0x00]);
    }

    #[test]
    fn put_length_prefixed_slice_consecutive_values() {
        let mut buf = Vec::new();
        put_length_prefixed_slice(&mut buf, b"foo");
        put_length_prefixed_slice(&mut buf, b"");
        put_length_prefixed_slice(&mut buf, b"bar");
        assert_eq!(buf, [3, b'f', b'o', b'o', 0, 3, b'b', b'a', b'r']);
    }

    // ---- get_length_prefixed_slice -----------------------------------------

    // Port of Coding.Strings.
    #[test]
    fn get_length_prefixed_slice_roundtrip_strings() {
        let long = vec![b'x'; 200];
        let mut buf = Vec::new();
        put_length_prefixed_slice(&mut buf, b"");
        put_length_prefixed_slice(&mut buf, b"foo");
        put_length_prefixed_slice(&mut buf, b"bar");
        put_length_prefixed_slice(&mut buf, &long);

        let mut input: &[u8] = &buf;
        let mut v: &[u8] = &[];

        assert!(get_length_prefixed_slice(&mut input, &mut v));
        assert_eq!(v, b"");
        assert!(get_length_prefixed_slice(&mut input, &mut v));
        assert_eq!(v, b"foo");
        assert!(get_length_prefixed_slice(&mut input, &mut v));
        assert_eq!(v, b"bar");
        assert!(get_length_prefixed_slice(&mut input, &mut v));
        assert_eq!(v, &long[..]);
        assert!(input.is_empty());
    }

    #[test]
    fn get_length_prefixed_slice_keeps_trailing_data() {
        let data = [0x02, b'a', b'b', 0x09, 0x08];
        let mut input: &[u8] = &data;
        let mut v: &[u8] = &[];
        assert!(get_length_prefixed_slice(&mut input, &mut v));
        assert_eq!(v, b"ab");
        assert_eq!(input, [0x09, 0x08]);
    }

    // A zero-length string consumes only its one-byte prefix.
    #[test]
    fn get_length_prefixed_slice_empty_string_then_more_data() {
        let data = [0x00, 0x01, b'z'];
        let mut input: &[u8] = &data;
        let mut v: &[u8] = b"sentinel";
        assert!(get_length_prefixed_slice(&mut input, &mut v));
        assert!(v.is_empty());
        assert_eq!(input, [0x01, b'z']);
    }

    // Length exactly equal to the bytes remaining succeeds; one more fails.
    #[test]
    fn get_length_prefixed_slice_exact_fit_and_one_too_short() {
        let exact = [0x03, b'a', b'b', b'c'];
        let mut input: &[u8] = &exact;
        let mut v: &[u8] = &[];
        assert!(get_length_prefixed_slice(&mut input, &mut v));
        assert_eq!(v, b"abc");
        assert!(input.is_empty());

        let short = [0x04, b'a', b'b', b'c'];
        let mut input: &[u8] = &short;
        let mut v: &[u8] = b"sentinel";
        assert!(!get_length_prefixed_slice(&mut input, &mut v));
        assert_eq!(v, b"sentinel", "result must not change on failure");
    }

    #[test]
    fn get_length_prefixed_slice_empty_input_fails() {
        let mut input: &[u8] = &[];
        let mut v: &[u8] = b"sentinel";
        assert!(!get_length_prefixed_slice(&mut input, &mut v));
        assert_eq!(v, b"sentinel");
    }

    // The length prefix itself is cut off (continuation bit, no next byte).
    #[test]
    fn get_length_prefixed_slice_truncated_length_fails() {
        let data = [0x80];
        let mut input: &[u8] = &data;
        let mut v: &[u8] = b"sentinel";
        assert!(!get_length_prefixed_slice(&mut input, &mut v));
        assert_eq!(v, b"sentinel");
    }

    // A corrupt length claiming ~4 GiB must fail cleanly, not panic or try to
    // allocate.
    #[test]
    fn get_length_prefixed_slice_huge_length_fails() {
        let mut buf = Vec::new();
        put_varint32(&mut buf, u32::MAX);
        buf.extend_from_slice(b"abc");
        let mut input: &[u8] = &buf;
        let mut v: &[u8] = &[];
        assert!(!get_length_prefixed_slice(&mut input, &mut v));
    }

    // Every proper prefix of a valid encoding must be rejected, like the
    // varint truncation tests.
    #[test]
    fn get_length_prefixed_slice_truncation_fails() {
        let encoded = encode_slice(&[b'q'; 200]);
        for len in 0..encoded.len() {
            let mut input = &encoded[..len];
            let mut v: &[u8] = &[];
            assert!(!get_length_prefixed_slice(&mut input, &mut v), "prefix length {len}");
        }
        let mut input: &[u8] = &encoded;
        let mut v: &[u8] = &[];
        assert!(get_length_prefixed_slice(&mut input, &mut v));
        assert_eq!(v.len(), 200);
    }

    // The result must point into the original buffer (no copy), and stay
    // valid after `input` has moved on.
    #[test]
    fn get_length_prefixed_slice_result_borrows_from_buffer() {
        let buf = encode_slice(b"hello");
        let mut input: &[u8] = &buf;
        let mut v: &[u8] = &[];
        assert!(get_length_prefixed_slice(&mut input, &mut v));
        assert!(std::ptr::eq(v.as_ptr(), buf[1..].as_ptr()));
        assert!(input.is_empty());
        assert_eq!(v, b"hello");
    }

    // Data containing zero bytes and bytes >= 0x80 comes back unchanged.
    #[test]
    fn get_length_prefixed_slice_arbitrary_bytes_roundtrip() {
        let data = [0x00, 0xFF, 0x80, 0x7F, 0x00];
        let buf = encode_slice(&data);
        let mut input: &[u8] = &buf;
        let mut v: &[u8] = &[];
        assert!(get_length_prefixed_slice(&mut input, &mut v));
        assert_eq!(v, data);
    }
}
