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

    #[test]
    fn is_send_and_sync() {
        fn assert_traits<T: Send + Sync>() {}
        assert_traits::<MemRandomAccessFile>();
    }
}
