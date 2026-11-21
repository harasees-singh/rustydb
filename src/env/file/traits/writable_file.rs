use crate::error::Result;

/// A file that is written once, sequentially, from start to finish.
///
/// `TableBuilder` appends each finished block, then the filter, the index and
/// the footer. The write-ahead log and the manifest use the same trait.
///
/// # Ownership and threads
/// The methods take `&mut self`, so only one thread can write at a time, and
/// the compiler enforces it. The trait is [`Send`] but not [`Sync`]: the file
/// can be moved to another thread (a background compaction owns its output
/// file), but it cannot be shared. To let several threads append to one file,
/// wrap it in a `Mutex`.
///
/// # Buffered, flushed, durable
/// Bytes go through three stages, and each method moves them one stage further:
///
/// | stage | reached by | survives |
/// |---|---|---|
/// | in the file object's buffer | [`append`](Self::append) | nothing |
/// | handed to the operating system | [`flush`](Self::flush) | a crash of this process |
/// | written to the disk | [`sync`](Self::sync) | a power loss or OS crash |
///
/// # Finishing
/// Always call [`close`](Self::close) and check its result. Dropping a file
/// without closing it cannot report an error, and the last buffered bytes may
/// still fail to reach the disk (for example when it is full). A table must not
/// be treated as complete unless `close` succeeded.
///
/// `close` takes the file by value (`self: Box<Self>`), so a closed file cannot
/// be used again: the compiler rejects it, and implementations need no
/// "already closed" state. The consequence is that `close` can only be called
/// on an owned `Box<dyn WritableFile>`, not on a `&mut` borrow.
pub trait WritableFile: Send {
    /// Writes `data` at the end of the file.
    ///
    /// The bytes may only be buffered when this returns. Call
    /// [`flush`](Self::flush) or [`sync`](Self::sync) to push them further.
    ///
    /// # Errors
    /// [`Error::Io`](crate::Error::Io) if the write fails. After an error, the
    /// contents of the file are unspecified and the file should be discarded.
    fn append(&mut self, data: &[u8]) -> Result<()>;

    /// Hands everything buffered so far to the operating system.
    ///
    /// This is cheap compared with [`sync`](Self::sync), but it does not make
    /// the data durable: it survives a crash of this process, not of the
    /// machine.
    fn flush(&mut self) -> Result<()>;

    /// Forces everything written so far onto the disk (`fsync`).
    ///
    /// This is slow, and it is what makes data durable across a power loss. A
    /// table file is synced once when it is finished, and the log may be synced
    /// after each write depending on the write options.
    fn sync(&mut self) -> Result<()>;

    /// Flushes any buffered data, releases the file and consumes it.
    ///
    /// The file is moved into this call, so it cannot be used afterwards. A
    /// failure here means data may have been lost, so the result must not be
    /// ignored. A file that is simply dropped skips this step, and any error
    /// from the final flush is lost.
    ///
    /// This does not call [`sync`](Self::sync). Call that first if the data
    /// must be durable.
    ///
    /// # Errors
    /// [`Error::Io`](crate::Error::Io) if the final flush or the release of the
    /// file fails.
    fn close(self: Box<Self>) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time check that the trait can be used as a trait object, since the
    /// table builder will hold a `Box<dyn WritableFile>`.
    #[allow(dead_code)]
    fn writable_file_is_object_safe(_: &dyn WritableFile) {}

    /// `close` must be callable through the box the table builder owns.
    #[allow(dead_code)]
    fn a_boxed_file_can_be_closed(f: Box<dyn WritableFile>) -> Result<()> {
        f.close()
    }

    /// The trait is `Send`, so a boxed file can move to another thread.
    #[allow(dead_code)]
    fn a_boxed_file_is_send(f: Box<dyn WritableFile>) {
        fn assert_send<T: Send>(_: &T) {}
        assert_send(&f);
    }
}
