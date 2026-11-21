use crate::env::WritableFile;
use std::sync::{Arc, Mutex};

/// A write-only file that keeps its bytes in memory, for tests and benchmarks.
///
/// The bytes live in a shared `Arc<Mutex<Vec<u8>>>` that the caller keeps a
/// handle to. A builder takes ownership of the file (as a
/// `Box<dyn WritableFile>`), so without the shared handle the written bytes
/// would be unreachable afterward. The mutex is what lets a handle holder read
/// while the file owns a clone of the `Arc`. It does not control who may write:
/// the `&mut self` methods already allow only one writer.
///
/// `flush` and `sync` do nothing, because there is nothing to push to.
///
/// ```ignore
/// let bytes = Arc::new(Mutex::new(Vec::new()));
/// let file = MemWritableFile::new(Arc::clone(&bytes));
/// let mut file: Box<dyn WritableFile> = Box::new(file);
/// file.append(b"abc")?;
/// file.close()?;
/// assert_eq!(*bytes.lock().unwrap(), b"abc");
/// ```
pub struct MemWritableFile {
    data: Arc<Mutex<Vec<u8>>>,
}

impl MemWritableFile {
    /// Creates a file that appends to `data`. Keep another clone of the `Arc`
    /// to read the bytes back.
    pub fn new(data: Arc<Mutex<Vec<u8>>>) -> Self {
        Self { data }
    }
}

impl WritableFile for MemWritableFile {
    fn append(&mut self, data: &[u8]) -> crate::Result<()> {
        self.data.lock().unwrap().extend_from_slice(data);
        Ok(())
    }

    fn flush(&mut self) -> crate::Result<()> {
        Ok(())
    }

    fn sync(&mut self) -> crate::Result<()> {
        Ok(())
    }

    fn close(self: Box<Self>) -> crate::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::RandomAccessFile;
    use crate::env::file::mem_random_access_file::MemRandomAccessFile;
    use std::thread;

    type Shared = Arc<Mutex<Vec<u8>>>;

    fn new_file() -> (MemWritableFile, Shared) {
        let bytes: Shared = Arc::new(Mutex::new(Vec::new()));
        (MemWritableFile::new(Arc::clone(&bytes)), bytes)
    }

    fn contents(bytes: &Shared) -> Vec<u8> {
        bytes.lock().unwrap().clone()
    }

    #[test]
    fn a_new_file_is_empty() {
        let (_file, bytes) = new_file();
        assert!(contents(&bytes).is_empty());
    }

    #[test]
    fn appends_accumulate_in_order() {
        let (mut file, bytes) = new_file();
        file.append(b"hello").unwrap();
        file.append(b", ").unwrap();
        file.append(b"world").unwrap();
        assert_eq!(contents(&bytes), b"hello, world");
    }

    #[test]
    fn each_append_is_visible_immediately() {
        let (mut file, bytes) = new_file();
        file.append(b"ab").unwrap();
        assert_eq!(contents(&bytes), b"ab");
        file.append(b"cd").unwrap();
        assert_eq!(contents(&bytes), b"abcd");
    }

    #[test]
    fn an_empty_append_changes_nothing() {
        let (mut file, bytes) = new_file();
        file.append(b"").unwrap();
        assert!(contents(&bytes).is_empty());
        file.append(b"x").unwrap();
        file.append(b"").unwrap();
        assert_eq!(contents(&bytes), b"x");
    }

    #[test]
    fn appends_keep_arbitrary_bytes() {
        let (mut file, bytes) = new_file();
        let all: Vec<u8> = (0..=255u8).collect();
        file.append(&all).unwrap();
        file.append(&[0, 0, 0]).unwrap();
        let mut expected = all;
        expected.extend_from_slice(&[0, 0, 0]);
        assert_eq!(contents(&bytes), expected);
    }

    #[test]
    fn a_large_append_is_kept_whole() {
        let (mut file, bytes) = new_file();
        let big: Vec<u8> = (0..2_000_000u32).map(|i| (i % 251) as u8).collect();
        file.append(&big).unwrap();
        assert_eq!(contents(&bytes), big);
    }

    #[test]
    fn flush_and_sync_succeed_and_do_not_change_the_contents() {
        let (mut file, bytes) = new_file();
        file.flush().unwrap();
        file.sync().unwrap();
        assert!(contents(&bytes).is_empty());
        file.append(b"abc").unwrap();
        file.flush().unwrap();
        file.sync().unwrap();
        file.flush().unwrap();
        assert_eq!(contents(&bytes), b"abc");
    }

    #[test]
    fn writes_after_a_flush_continue_where_they_left_off() {
        let (mut file, bytes) = new_file();
        file.append(b"ab").unwrap();
        file.flush().unwrap();
        file.append(b"cd").unwrap();
        file.sync().unwrap();
        file.append(b"ef").unwrap();
        assert_eq!(contents(&bytes), b"abcdef");
    }

    #[test]
    fn close_succeeds_and_keeps_what_was_written() {
        let (mut file, bytes) = new_file();
        file.append(b"abc").unwrap();
        Box::new(file).close().unwrap();
        assert_eq!(contents(&bytes), b"abc");
    }

    #[test]
    fn closing_an_empty_file_succeeds() {
        let (file, bytes) = new_file();
        Box::new(file).close().unwrap();
        assert!(contents(&bytes).is_empty());
    }

    #[test]
    fn the_bytes_are_reachable_after_the_file_moves_into_a_boxed_trait_object() {
        let (file, bytes) = new_file();
        let mut boxed: Box<dyn WritableFile> = Box::new(file);
        boxed.append(b"table ").unwrap();
        boxed.append(b"bytes").unwrap();
        boxed.sync().unwrap();
        boxed.close().unwrap();
        assert_eq!(contents(&bytes), b"table bytes");
    }

    #[test]
    fn dropping_without_close_keeps_the_bytes_already_written() {
        let (mut file, bytes) = new_file();
        file.append(b"abc").unwrap();
        drop(file);
        assert_eq!(contents(&bytes), b"abc");
    }

    #[test]
    fn a_file_can_move_to_another_thread_and_be_written_there() {
        let (file, bytes) = new_file();
        let mut boxed: Box<dyn WritableFile> = Box::new(file);
        let worker = thread::spawn(move || {
            for i in 0..100u8 {
                boxed.append(&[i]).unwrap();
            }
            boxed.close().unwrap();
        });
        worker.join().unwrap();
        assert_eq!(contents(&bytes), (0..100u8).collect::<Vec<_>>());
    }

    #[test]
    fn two_files_with_different_buffers_do_not_interfere() {
        let (mut a, a_bytes) = new_file();
        let (mut b, b_bytes) = new_file();
        a.append(b"aaa").unwrap();
        b.append(b"bbb").unwrap();
        a.append(b"A").unwrap();
        assert_eq!(contents(&a_bytes), b"aaaA");
        assert_eq!(contents(&b_bytes), b"bbb");
    }

    #[test]
    fn files_sharing_one_buffer_append_to_it_in_call_order() {
        let bytes: Shared = Arc::new(Mutex::new(Vec::new()));
        let mut a = MemWritableFile::new(Arc::clone(&bytes));
        let mut b = MemWritableFile::new(Arc::clone(&bytes));
        a.append(b"1").unwrap();
        b.append(b"2").unwrap();
        a.append(b"3").unwrap();
        assert_eq!(contents(&bytes), b"123");
    }

    #[test]
    fn what_was_written_can_be_read_back_with_mem_random_access_file() {
        let (mut file, bytes) = new_file();
        file.append(b"0123456789").unwrap();
        Box::new(file).close().unwrap();

        let reader = MemRandomAccessFile::new(contents(&bytes));
        let mut buf = [0u8; 4];
        assert_eq!(reader.read_at(3, &mut buf).unwrap(), 4);
        assert_eq!(&buf, b"3456");
        assert_eq!(reader.read_at(8, &mut buf).unwrap(), 2);
        assert_eq!(&buf[..2], b"89");
        assert_eq!(reader.read_at(10, &mut buf).unwrap(), 0);
    }

    #[test]
    fn is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<MemWritableFile>();
    }
}
