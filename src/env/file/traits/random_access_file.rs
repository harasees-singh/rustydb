pub trait RandomAccessFile: Send + Sync {
    /// Reads up to `buf.len()` bytes starting at `offset`; returns how many were read.
    /// Fewer than requested means the end of the file was reached.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> crate::error::Result<usize>;
}
