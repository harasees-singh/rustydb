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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time check that the trait can be used as a trait object, since a
    /// table will hold an `Arc<dyn RandomAccessFile>`.
    #[allow(dead_code)]
    fn random_access_file_is_object_safe(_: &dyn RandomAccessFile) {}
}
