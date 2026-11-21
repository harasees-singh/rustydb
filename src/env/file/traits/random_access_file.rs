/// A file that can be read at any offset, and from many threads at once.
///
/// This is how the table code reads: the footer, the index and filter blocks
/// when a table is opened, and one data block at a time on a block cache miss.
///
/// # Why reads take an offset
/// A read at an explicit offset (`pread` on Unix) uses no file position that is
/// shared between callers. Two threads can therefore read different parts of
/// the same file at once without a lock, which is what lets one open table be
/// shared as an `Arc<Table>`. That is also why the trait requires [`Sync`] and
/// takes `&self`.
///
/// The file is treated as immutable while it is open: SSTables are written
/// once and never modified in place.
///
/// # Implementing it
/// The real implementation wraps a `std::fs::File`. A `Vec<u8>` version lets
/// tests and benchmarks run without touching the disk.
pub trait RandomAccessFile: Send + Sync {
    /// Reads up to `buf.len()` bytes starting at byte `offset` and returns how
    /// many were read.
    ///
    /// A result smaller than `buf.len()` means the end of the file was reached,
    /// and reading at or past the end returns `Ok(0)`. Implementations must keep
    /// reading until `buf` is full or the file ends, because the operating
    /// system may return fewer bytes than asked for without being at the end
    /// (for example when interrupted). The caller then knows that a short result
    /// is a short file, and it decides whether that is an error: reading a block
    /// reports a truncated file as corruption.
    ///
    /// Safe to call from any number of threads at the same time.
    ///
    /// # Errors
    /// [`Error::Io`](crate::Error::Io) if the underlying read fails.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> crate::error::Result<usize>;

    /// Fills `buf` with the bytes starting at byte `offset`, or fails.
    ///
    /// Unlike [`read_at`](Self::read_at), a file that ends before `buf` is full
    /// is an error and not a short result. It is for callers that know exactly
    /// how many bytes must exist, such as the footer or a block whose size is in
    /// its handle, so they do not each repeat the "was it short?" check.
    ///
    /// On success every byte of `buf` has been written. On error the contents of
    /// `buf` are unspecified (it may be partly filled).
    ///
    /// A zero-length `buf` succeeds without reading, even at or past the end of
    /// the file.
    ///
    /// Safe to call from any number of threads at the same time.
    ///
    /// # Errors
    /// - [`Error::Corruption`](crate::Error::Corruption) if the file ends before
    ///   `buf` is full. A table that is shorter than its own handles say is
    ///   damaged or truncated, which is what callers want to report.
    /// - [`Error::Io`](crate::Error::Io) if the underlying read fails.
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> crate::error::Result<()> {
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

    /// Compile-time check that the trait can be used as a trait object, since a
    /// table will hold an `Arc<dyn RandomAccessFile>`.
    #[allow(dead_code)]
    fn random_access_file_is_object_safe(_: &dyn RandomAccessFile) {}
}
