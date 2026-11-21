use crate::env::RandomAccessFile;
use std::cmp::min;

/// A read-only file held entirely in memory.
///
/// It exists so tests and benchmarks can exercise the table code without the
/// disk. The bytes never change after construction, so it can be read from many
/// threads at once without a lock. It follows the [`RandomAccessFile`] contract:
/// a read at or past the end returns `Ok(0)`, and a read that reaches the end
/// returns only the bytes that exist.
pub struct MemRandomAccessFile {
    data: Vec<u8>,
}

impl MemRandomAccessFile {
    /// Creates a file whose contents are `data`.
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }
}

impl RandomAccessFile for MemRandomAccessFile {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> crate::Result<usize> {
        if offset >= self.data.len() as u64 {
            return Ok(0);
        }
        let size = min(buf.len() as u64, self.data.len() as u64 - offset) as usize;
        buf[..size].copy_from_slice(&self.data[offset as usize..offset as usize + size]);
        Ok(size)
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> crate::Result<()> {
        let mut done = 0usize;
        while done < buf.len() {
            match self.read_at(offset + done as u64, &mut buf[done..])? {
                0 => return Err(crate::error::Error::corruption("unexpected end of file")),
                n => done += n
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    const TEN: &[u8] = b"0123456789";

    fn file(data: &[u8]) -> MemRandomAccessFile {
        MemRandomAccessFile::new(data.to_vec())
    }

    /// Reads `n` bytes at `offset` into a buffer filled with `#`, returning how
    /// many were read and the whole buffer (so untouched bytes are visible).
    fn read(f: &dyn RandomAccessFile, offset: u64, n: usize) -> (usize, Vec<u8>) {
        let mut buf = vec![b'#'; n];
        let got = f.read_at(offset, &mut buf).unwrap();
        (got, buf)
    }

    #[test]
    fn reads_from_the_start() {
        assert_eq!(read(&file(TEN), 0, 4), (4, b"0123".to_vec()));
    }

    #[test]
    fn reads_from_the_middle() {
        assert_eq!(read(&file(TEN), 3, 4), (4, b"3456".to_vec()));
    }

    #[test]
    fn reads_exactly_to_the_end() {
        assert_eq!(read(&file(TEN), 6, 4), (4, b"6789".to_vec()));
    }

    #[test]
    fn reads_the_whole_file_in_one_call() {
        assert_eq!(read(&file(TEN), 0, 10), (10, TEN.to_vec()));
    }

    #[test]
    fn a_read_running_past_the_end_is_short_and_leaves_the_rest_of_the_buffer_alone() {
        assert_eq!(read(&file(TEN), 6, 8), (4, b"6789####".to_vec()));
        assert_eq!(read(&file(TEN), 9, 3), (1, b"9##".to_vec()));
    }

    #[test]
    fn a_buffer_larger_than_the_file_gets_the_whole_file() {
        let (n, buf) = read(&file(TEN), 0, 25);
        assert_eq!(n, 10);
        assert_eq!(&buf[..10], TEN);
        assert!(buf[10..].iter().all(|&b| b == b'#'));
    }

    #[test]
    fn reading_at_the_end_returns_zero() {
        assert_eq!(read(&file(TEN), 10, 4), (0, b"####".to_vec()));
    }

    #[test]
    fn reading_past_the_end_returns_zero() {
        assert_eq!(read(&file(TEN), 11, 4).0, 0);
        assert_eq!(read(&file(TEN), 1_000_000, 4).0, 0);
    }

    #[test]
    fn a_huge_offset_returns_zero_instead_of_overflowing() {
        assert_eq!(read(&file(TEN), u64::MAX, 4).0, 0);
        assert_eq!(read(&file(TEN), u64::MAX - 1, 4).0, 0);
        assert_eq!(read(&file(TEN), u32::MAX as u64 + 1, 4).0, 0);
    }

    #[test]
    fn a_zero_length_buffer_reads_nothing() {
        assert_eq!(read(&file(TEN), 3, 0).0, 0);
        assert_eq!(read(&file(TEN), 10, 0).0, 0);
        assert_eq!(read(&file(TEN), 99, 0).0, 0);
    }

    #[test]
    fn an_empty_file_always_reads_zero() {
        let f = file(b"");
        assert_eq!(read(&f, 0, 4).0, 0);
        assert_eq!(read(&f, 5, 4).0, 0);
        assert_eq!(read(&f, 0, 0).0, 0);
    }

    #[test]
    fn a_single_byte_file() {
        let f = file(b"x");
        assert_eq!(read(&f, 0, 1), (1, b"x".to_vec()));
        assert_eq!(read(&f, 0, 4), (1, b"x###".to_vec()));
        assert_eq!(read(&f, 1, 1).0, 0);
    }

    #[test]
    fn repeated_reads_do_not_change_each_other() {
        let f = file(TEN);
        assert_eq!(read(&f, 5, 3), read(&f, 5, 3));
        assert_eq!(read(&f, 0, 2), (2, b"01".to_vec()));
        assert_eq!(read(&f, 5, 3), (3, b"567".to_vec()));
    }

    #[test]
    fn every_offset_and_length_matches_slicing_the_data() {
        let data: Vec<u8> = (0..40u8).collect();
        let f = file(&data);
        for offset in 0..=45u64 {
            for len in 0..=45usize {
                let (n, buf) = read(&f, offset, len);
                let start = (offset as usize).min(data.len());
                let end = (start + len).min(data.len());
                let expected = &data[start..end];
                assert_eq!(n, expected.len(), "offset {offset}, len {len}");
                assert_eq!(&buf[..n], expected, "offset {offset}, len {len}");
                assert!(
                    buf[n..].iter().all(|&b| b == b'#'),
                    "offset {offset}, len {len}"
                );
            }
        }
    }

    #[test]
    fn a_large_file_reads_back_correctly() {
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let f = file(&data);
        let mut buf = vec![0u8; 100_000];
        for offset in [0u64, 1, 65_535, 1_234_567, 2_900_000] {
            let n = f.read_at(offset, &mut buf).unwrap();
            let start = offset as usize;
            assert_eq!(&buf[..n], &data[start..start + n]);
        }
    }

    #[test]
    fn works_as_a_trait_object_behind_an_arc() {
        let f: Arc<dyn RandomAccessFile> = Arc::new(file(TEN));
        assert_eq!(read(f.as_ref(), 2, 3), (3, b"234".to_vec()));
    }

    #[test]
    fn many_threads_can_read_the_same_file_at_once() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 253) as u8).collect();
        let f: Arc<dyn RandomAccessFile> = Arc::new(MemRandomAccessFile::new(data.clone()));
        let handles: Vec<_> = (0..8u64)
            .map(|t| {
                let f = Arc::clone(&f);
                let data = data.clone();
                thread::spawn(move || {
                    let mut buf = [0u8; 97];
                    for i in 0..2_000u64 {
                        let offset = (t * 7919 + i * 31) % 100_000;
                        let n = f.read_at(offset, &mut buf).unwrap();
                        let start = offset as usize;
                        assert_eq!(&buf[..n], &data[start..start + n]);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }

    // ---- read_exact_at ----------------------------------------------------------

    /// Fills a `#` buffer of `n` bytes with `read_exact_at`, returning the result
    /// and the buffer.
    fn read_exact(f: &dyn RandomAccessFile, offset: u64, n: usize) -> (crate::Result<()>, Vec<u8>) {
        let mut buf = vec![b'#'; n];
        let r = f.read_exact_at(offset, &mut buf);
        (r, buf)
    }

    #[test]
    fn read_exact_fills_the_buffer_from_the_start() {
        let (r, buf) = read_exact(&file(TEN), 0, 4);
        r.unwrap();
        assert_eq!(buf, b"0123");
    }

    #[test]
    fn read_exact_fills_the_buffer_from_the_middle() {
        let (r, buf) = read_exact(&file(TEN), 3, 4);
        r.unwrap();
        assert_eq!(buf, b"3456");
    }

    #[test]
    fn read_exact_succeeds_when_the_buffer_ends_exactly_at_the_end_of_the_file() {
        let (r, buf) = read_exact(&file(TEN), 6, 4);
        r.unwrap();
        assert_eq!(buf, b"6789");
        let (r, buf) = read_exact(&file(TEN), 9, 1);
        r.unwrap();
        assert_eq!(buf, b"9");
    }

    #[test]
    fn read_exact_reads_the_whole_file() {
        let (r, buf) = read_exact(&file(TEN), 0, 10);
        r.unwrap();
        assert_eq!(buf, TEN);
    }

    #[test]
    fn read_exact_fails_with_corruption_when_the_file_is_too_short() {
        for (offset, n) in [(6u64, 5usize), (0, 11), (9, 2), (0, 1000)] {
            let (r, _) = read_exact(&file(TEN), offset, n);
            let e = r.expect_err(&format!("offset {offset}, len {n}"));
            assert!(e.is_corruption(), "offset {offset}, len {n}: {e}");
        }
    }

    #[test]
    fn read_exact_fails_when_starting_at_or_past_the_end() {
        for offset in [10u64, 11, 1_000_000, u64::MAX - 1, u64::MAX] {
            let (r, _) = read_exact(&file(TEN), offset, 4);
            assert!(r.expect_err(&format!("offset {offset}")).is_corruption());
        }
    }

    #[test]
    fn read_exact_with_a_zero_length_buffer_succeeds_anywhere() {
        for offset in [0u64, 5, 10, 11, 1_000_000, u64::MAX] {
            let (r, buf) = read_exact(&file(TEN), offset, 0);
            r.unwrap_or_else(|e| panic!("offset {offset}: {e}"));
            assert!(buf.is_empty());
        }
        read_exact(&file(b""), 0, 0).0.unwrap();
    }

    #[test]
    fn read_exact_on_an_empty_file_fails_unless_the_buffer_is_empty() {
        let (r, _) = read_exact(&file(b""), 0, 1);
        assert!(r.unwrap_err().is_corruption());
        read_exact(&file(b""), 0, 0).0.unwrap();
    }

    #[test]
    fn read_exact_on_a_one_byte_file() {
        let f = file(b"x");
        let (r, buf) = read_exact(&f, 0, 1);
        r.unwrap();
        assert_eq!(buf, b"x");
        assert!(read_exact(&f, 0, 2).0.is_err());
        assert!(read_exact(&f, 1, 1).0.is_err());
    }

    #[test]
    fn read_exact_agrees_with_slicing_for_every_offset_and_length() {
        let data: Vec<u8> = (0..40u8).collect();
        let f = file(&data);
        for offset in 0..=45u64 {
            for len in 0..=45usize {
                let (r, buf) = read_exact(&f, offset, len);
                let fits = len == 0 || offset as usize + len <= data.len();
                assert_eq!(r.is_ok(), fits, "offset {offset}, len {len}");
                if fits && len > 0 {
                    let start = offset as usize;
                    assert_eq!(buf, &data[start..start + len], "offset {offset}, len {len}");
                }
            }
        }
    }

    #[test]
    fn read_exact_does_not_touch_the_buffer_beyond_what_was_asked() {
        let mut buf = *b"##########";
        file(TEN).read_exact_at(2, &mut buf[..4]).unwrap();
        assert_eq!(&buf, b"2345######");
    }

    #[test]
    fn read_exact_repeated_calls_are_independent() {
        let f = file(TEN);
        assert!(read_exact(&f, 8, 5).0.is_err());
        let (r, buf) = read_exact(&f, 8, 2);
        r.unwrap();
        assert_eq!(buf, b"89");
    }

    #[test]
    fn read_exact_works_on_a_large_buffer() {
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let f = file(&data);
        let (r, buf) = read_exact(&f, 1_000, 2_000_000);
        r.unwrap();
        assert_eq!(buf, &data[1_000..2_001_000]);
        assert!(read_exact(&f, 1_000, 2_999_000 + 1).0.is_err());
        read_exact(&f, 0, 3_000_000).0.unwrap();
    }

    #[test]
    fn read_exact_works_as_a_trait_object_behind_an_arc() {
        let f: Arc<dyn RandomAccessFile> = Arc::new(file(TEN));
        let mut buf = [0u8; 3];
        f.read_exact_at(4, &mut buf).unwrap();
        assert_eq!(&buf, b"456");
    }

    #[test]
    fn many_threads_can_read_exact_from_the_same_file_at_once() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 253) as u8).collect();
        let f: Arc<dyn RandomAccessFile> = Arc::new(MemRandomAccessFile::new(data.clone()));
        let handles: Vec<_> = (0..8u64)
            .map(|t| {
                let f = Arc::clone(&f);
                let data = data.clone();
                thread::spawn(move || {
                    let mut buf = [0u8; 97];
                    for i in 0..2_000u64 {
                        let offset = (t * 7919 + i * 31) % 100_000;
                        let start = offset as usize;
                        let r = f.read_exact_at(offset, &mut buf);
                        if start + buf.len() <= data.len() {
                            r.unwrap();
                            assert_eq!(&buf[..], &data[start..start + buf.len()]);
                        } else {
                            assert!(r.unwrap_err().is_corruption());
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn is_send_and_sync() {
        fn assert_traits<T: Send + Sync>() {}
        assert_traits::<MemRandomAccessFile>();
    }
}
