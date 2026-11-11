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

    fn encode_varint32(v: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        put_varint32(&mut buf, v);
        buf
    }

    fn encode_varint64(v: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        put_varint64(&mut buf, v);
        buf
    }

    // ---- Fixed-width -------------------------------------------------------

    // Port of Coding.EncodingOutput: little-endian, appended after existing data.
    #[test]
    fn fixed_width_is_little_endian_and_appends() {
        let mut buf = b"AB".to_vec();
        put_fixed32(&mut buf, 0x04030201);
        put_fixed32(&mut buf, 0);
        put_fixed32(&mut buf, u32::MAX);
        assert_eq!(buf, [b'A', b'B', 1, 2, 3, 4, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);

        let mut buf = b"AB".to_vec();
        put_fixed64(&mut buf, 0x0807060504030201);
        put_fixed64(&mut buf, u64::MAX);
        assert_eq!(buf[..10], [b'A', b'B', 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(buf[10..], [0xFF; 8]);
    }

    // Port of Coding.Fixed32.
    #[test]
    fn fixed32_roundtrip() {
        let mut buf = Vec::new();
        for v in 0..100_000u32 {
            put_fixed32(&mut buf, v);
        }
        for (v, chunk) in (0..100_000u32).zip(buf.chunks_exact(4)) {
            assert_eq!(u32::from_le_bytes(chunk.try_into().unwrap()), v);
        }
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
        for (want, chunk) in expected.iter().zip(buf.chunks_exact(8)) {
            assert_eq!(u64::from_le_bytes(chunk.try_into().unwrap()), *want);
        }
    }

    // ---- put_varint32 / put_varint64 ---------------------------------------

    // Hand-written byte sequences at every point where the length changes.
    // These must match LevelDB byte for byte.
    #[test]
    fn varint32_known_encodings() {
        let cases: &[(u32, &[u8])] = &[
            (0, &[0x00]),
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
            assert_eq!(varint_length(value as u64), expected.len(), "length of {value}");
        }
    }

    #[test]
    fn varint64_known_encodings() {
        let cases: &[(u64, &[u8])] = &[
            (0, &[0x00]),
            (300, &[0xAC, 0x02]),
            (1 << 32, &[0x80, 0x80, 0x80, 0x80, 0x10]),
            (1 << 63, &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
            (u64::MAX, &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]),
        ];
        for &(value, expected) in cases {
            assert_eq!(encode_varint64(value), expected, "value {value}");
            assert_eq!(varint_length(value), expected.len(), "length of {value}");
        }
    }

    #[test]
    fn varints_append_after_existing_data() {
        let mut buf = b"AB".to_vec();
        put_varint32(&mut buf, 300);
        put_varint64(&mut buf, 300);
        assert_eq!(buf, [b'A', b'B', 0xAC, 0x02, 0xAC, 0x02]);
    }

    // The unrolled 32-bit encoder and the looped 64-bit encoder must agree.
    #[test]
    fn varint32_and_varint64_agree_for_u32_values() {
        let mut values: Vec<u32> = (0..1000).collect();
        for shift in 0..32 {
            let p = 1u32 << shift;
            values.extend([p - 1, p, p + 1]);
        }
        values.push(u32::MAX);
        for v in values {
            assert_eq!(encode_varint64(v as u64), encode_varint32(v), "value {v}");
        }
    }

    // ---- varint_length -----------------------------------------------------

    // The length grows by one byte at every power of 128.
    #[test]
    fn varint_length_boundaries() {
        assert_eq!(varint_length(0), 1);
        assert_eq!(varint_length(u64::MAX), 10);
        for k in 1..=9usize {
            let boundary = 1u64 << (7 * k);
            assert_eq!(varint_length(boundary - 1), k, "just below 2^{}", 7 * k);
            assert_eq!(varint_length(boundary), k + 1, "at 2^{}", 7 * k);
        }
    }

    // ---- get_varint32 / get_varint64 ---------------------------------------

    // Port of Coding.Varint32.
    #[test]
    fn get_varint32_roundtrip() {
        let values: Vec<u32> = (0..32u32 * 32).map(|i| (i / 32) << (i % 32)).collect();
        let mut buf = Vec::new();
        for &v in &values {
            put_varint32(&mut buf, v);
        }

        let mut input: &[u8] = &buf;
        for &expected in &values {
            let before = input.len();
            let mut actual = 0;
            assert!(get_varint32(&mut input, &mut actual), "value {expected}");
            assert_eq!(actual, expected);
            assert_eq!(before - input.len(), varint_length(expected as u64));
        }
        assert!(input.is_empty());
    }

    // Port of Coding.Varint64.
    #[test]
    fn get_varint64_roundtrip() {
        let mut values = vec![0u64, 100, u64::MAX, u64::MAX - 1];
        for k in 0..64 {
            let power = 1u64 << k;
            values.extend([power, power - 1, power + 1]);
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
            assert_eq!(before - input.len(), varint_length(expected));
        }
        assert!(input.is_empty());
    }

    // Hand-written input (not produced by our own encoder): only the varint is
    // consumed, and whatever follows stays in the slice.
    #[test]
    fn get_varint_stops_at_end_of_varint() {
        let data = [0xAC, 0x02, 0x05, 0xFF];

        let mut input: &[u8] = &data;
        let (mut v32, mut v64) = (0u32, 0u64);
        assert!(get_varint32(&mut input, &mut v32));
        assert_eq!(v32, 300);
        assert_eq!(input, [0x05, 0xFF]);
        assert!(get_varint32(&mut input, &mut v32));
        assert_eq!(v32, 5);
        assert_eq!(input, [0xFF]);

        let mut input: &[u8] = &data;
        assert!(get_varint64(&mut input, &mut v64));
        assert_eq!(v64, 300);
        assert_eq!(input, [0x05, 0xFF]);
    }

    // Covers empty input, a cut-off varint, and the upstream Varint32Overflow
    // / Varint64Overflow inputs. Failure must not touch input or output.
    #[test]
    fn get_varint_rejects_malformed_input() {
        let overflow32: &[u8] = &[0x81, 0x82, 0x83, 0x84, 0x85, 0x11];
        let overflow64: &[u8] = &[
            0x81, 0x82, 0x83, 0x84, 0x85, 0x81, 0x82, 0x83, 0x84, 0x85, 0x11,
        ];
        let cases: [&[u8]; 5] = [&[], &[0x80], &[0xFF, 0xFF, 0xFF], overflow32, overflow64];

        for bytes in cases {
            let mut input = bytes;
            let mut v32 = 7u32;
            assert!(!get_varint32(&mut input, &mut v32), "{bytes:?}");
            assert_eq!((input, v32), (bytes, 7), "{bytes:?}: must be unchanged");
        }
        for bytes in [&[][..], &[0x80], &[0xFF, 0xFF, 0xFF], overflow64] {
            let mut input = bytes;
            let mut v64 = 7u64;
            assert!(!get_varint64(&mut input, &mut v64), "{bytes:?}");
            assert_eq!((input, v64), (bytes, 7), "{bytes:?}: must be unchanged");
        }
    }

    // Port of Coding.Varint32Truncation / Varint64Truncation: every proper
    // prefix of a valid encoding fails, and the whole encoding succeeds.
    #[test]
    fn get_varint_truncation_fails() {
        let big32 = (1u32 << 31) + 100;
        let enc32 = encode_varint32(big32);
        for len in 0..enc32.len() {
            let mut input = &enc32[..len];
            assert!(!get_varint32(&mut input, &mut 0), "32-bit prefix {len}");
        }
        let (mut input, mut v) = (&enc32[..], 0);
        assert!(get_varint32(&mut input, &mut v));
        assert_eq!(v, big32);

        let big64 = (1u64 << 63) + 100;
        let enc64 = encode_varint64(big64);
        for len in 0..enc64.len() {
            let mut input = &enc64[..len];
            assert!(!get_varint64(&mut input, &mut 0), "64-bit prefix {len}");
        }
        let (mut input, mut v) = (&enc64[..], 0);
        assert!(get_varint64(&mut input, &mut v));
        assert_eq!(v, big64);
    }

    // Matches LevelDB: bits of the last byte above the type's width are
    // dropped rather than rejected. This pins the C++ behaviour.
    #[test]
    fn get_varint_last_byte_high_bits_are_dropped_like_leveldb() {
        let mut input: &[u8] = &[0x80, 0x80, 0x80, 0x80, 0x7F];
        let mut v32 = 0u32;
        assert!(get_varint32(&mut input, &mut v32));
        assert_eq!(v32, 0xF000_0000); // 0x7F << 28 keeps only 4 bits

        let mut input: &[u8] = &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x7F];
        let mut v64 = 0u64;
        assert!(get_varint64(&mut input, &mut v64));
        assert_eq!(v64, 1u64 << 63); // 0x7F << 63 keeps only 1 bit
    }

    // ---- length-prefixed slices --------------------------------------------

    #[test]
    fn put_length_prefixed_slice_encoding() {
        let mut buf = b"AB".to_vec();
        put_length_prefixed_slice(&mut buf, b"hello");
        put_length_prefixed_slice(&mut buf, b"");
        assert_eq!(buf, [b'A', b'B', 5, b'h', b'e', b'l', b'l', b'o', 0]);

        // The length is a varint, so 128 bytes need a two-byte prefix.
        let mut buf = Vec::new();
        put_length_prefixed_slice(&mut buf, &[b'x'; 128]);
        assert_eq!(buf[..2], [0x80, 0x01]);
        assert_eq!(buf.len(), 2 + 128);
    }

    // Port of Coding.Strings.
    #[test]
    fn get_length_prefixed_slice_roundtrip_strings() {
        let long = vec![b'x'; 200];
        let mut buf = Vec::new();
        for s in [&b""[..], b"foo", b"bar", &long] {
            put_length_prefixed_slice(&mut buf, s);
        }

        let mut input: &[u8] = &buf;
        for expected in [&b""[..], b"foo", b"bar", &long] {
            let mut v: &[u8] = &[];
            assert!(get_length_prefixed_slice(&mut input, &mut v));
            assert_eq!(v, expected);
        }
        assert!(input.is_empty());
    }

    // The length must fit in what remains: exactly enough works, one short fails.
    #[test]
    fn get_length_prefixed_slice_length_must_fit_remaining_data() {
        let mut input: &[u8] = &[3, b'a', b'b', b'c'];
        let mut v: &[u8] = &[];
        assert!(get_length_prefixed_slice(&mut input, &mut v));
        assert_eq!(v, b"abc");

        let mut input: &[u8] = &[4, b'a', b'b', b'c'];
        let mut v: &[u8] = b"sentinel";
        assert!(!get_length_prefixed_slice(&mut input, &mut v));
        assert_eq!(v, b"sentinel", "result must not change on failure");
    }

    #[test]
    fn get_length_prefixed_slice_rejects_malformed_input() {
        let mut huge = Vec::new();
        put_varint32(&mut huge, u32::MAX); // claims ~4 GiB, only 3 bytes follow
        huge.extend_from_slice(b"abc");

        let cases: [&[u8]; 3] = [&[], &[0x80], &huge];
        for bytes in cases {
            let mut input = bytes;
            let mut v: &[u8] = b"sentinel";
            assert!(!get_length_prefixed_slice(&mut input, &mut v), "{bytes:?}");
            assert_eq!(v, b"sentinel");
        }
    }
}
