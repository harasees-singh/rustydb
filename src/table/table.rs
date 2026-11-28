use crate::env::RandomAccessFile;
use crate::error::Result;
use crate::table::block::block::{Block, BlockIter};
use crate::table::filter_block::reader::FilterBlockReader;
use crate::table::format::BlockHandle;
use crate::table::options::TableOptions;
use std::sync::Arc;

/// An open, immutable table (SSTable): the reading side of
/// [`TableBuilder`](super::table_builder::TableBuilder).
///
/// Opening a table reads only the small parts that every lookup needs: the
/// footer, the index block and, if a filter policy is configured, the filter
/// block. Data blocks are read one at a time, when a lookup needs them.
///
/// # Looking up a key
/// ```text
/// get(key)
///   1. index.seek(key)                 first data block whose separator >= key
///        none                          -> Ok(None): key is above every block
///   2. filter.key_may_match(handle.offset, key)
///        false                         -> Ok(None): no disk read at all
///   3. read the data block, seek(key)
///        key found                     -> Ok(Some(value))
///        otherwise                     -> Ok(None)
/// ```
/// The filter is checked after the index, because it is addressed by the block's
/// offset (see [`FilterBlockReader`]). A missing key therefore costs one search
/// of the in-memory index and one filter probe, and usually no I/O.
///
/// # What it needs from the caller
/// - **The comparator.** It must be the one the table was written with. The
///   table file does not record the comparator's name (the database's manifest
///   does), so a table opened with another ordering is not detected here, and
///   lookups on it return wrong answers without an error.
/// - **The file size.** A [`RandomAccessFile`] has no length, and the footer is
///   found from the end of the file.
/// - **The filter policy, if any.** A filter is used only if the options name a
///   policy and the table has a filter written under the same name. A table
///   written with a filter and opened without a policy (or with another one)
///   simply skips the filter.
///
/// # Threads
/// A `Table` is immutable once opened and every method takes `&self`, so it is
/// meant to be shared as `Arc<Table>` by any number of readers. Reads use
/// offset-based I/O, so they do not interfere.
///
/// # Not here yet
/// A block cache (data blocks are re-read and re-parsed on every lookup), and an
/// iterator over the whole table. The iterator is built from `index_iter` and
/// `read_data_block`, which are the two pieces it needs.
pub struct Table {
    opts: TableOptions,
    file: Arc<dyn RandomAccessFile>,
    /// The index block: one entry per data block, mapping a separator key to the
    /// block's [`BlockHandle`]. Kept in memory for the life of the table.
    index_block: Arc<Block>,
    /// `None` if the options have no filter policy, or the table has no filter
    /// for it, or the filter block could not be read.
    filter: Option<FilterBlockReader>,
}

impl Table {
    /// Opens the table stored in the first `file_size` bytes of `file`.
    ///
    /// ```text
    /// 1. read the footer                      (the last 40 bytes)
    /// 2. read the index block                 -> Block::new
    /// 3. if a filter policy is configured:
    ///      read the meta-index block, look up "filter.<policy name>",
    ///      read the filter block it points to -> FilterBlockReader::new
    /// ```
    /// Checksums are verified while reading if `opts.verify_checksums` is set.
    ///
    /// The index is required, and a damaged one fails the open. The filter is
    /// an optimisation, so a problem in step 3 (a damaged or unreadable
    /// meta-index or filter block, an undecodable handle) does not fail the
    /// open: the table is opened with no filter, and lookups are just slower.
    /// This matches LevelDB, which ignores errors in the meta blocks.
    ///
    /// # Errors
    /// - [`Error::Corruption`](crate::Error::Corruption) if the file is too
    ///   short to hold a footer, the footer's magic number is wrong, or the
    ///   index block fails its checksum or does not parse.
    /// - [`Error::Io`](crate::Error::Io) if reading the footer or the index
    ///   fails.
    pub fn open(opts: TableOptions, file: Arc<dyn RandomAccessFile>, file_size: u64) -> Result<Table> {
        todo!()
    }

    /// Returns the value stored under exactly `key`, or `None` if the table has
    /// no such key.
    ///
    /// At most one data block is read, and none if the index or the filter shows
    /// that the key cannot be there. The value is copied out of the block. A
    /// borrowed value needs the block to be kept alive, which comes with the
    /// block cache.
    ///
    /// # Errors
    /// - [`Error::Corruption`](crate::Error::Corruption) if the data block fails
    ///   its checksum or does not parse, or the index holds a handle that cannot
    ///   be decoded.
    /// - [`Error::Io`](crate::Error::Io) if reading the block fails.
    ///
    /// A key that is simply absent is `Ok(None)`, not an error.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        todo!()
    }

    /// Reads and parses the data block that `handle` points at.
    ///
    /// This is the one place a data block enters memory, so the block cache will
    /// go here. `get` and the table iterator both use it.
    ///
    /// # Errors
    /// [`Error::Corruption`](crate::Error::Corruption) or
    /// [`Error::Io`](crate::Error::Io), as for [`read_block`](crate::table::format::read_block),
    /// and `Corruption` if the bytes do not form a valid block.
    pub(crate) fn read_data_block(&self, handle: BlockHandle) -> Result<Arc<Block>> {
        todo!()
    }

    /// A cursor over the index block: each key is a separator, each value an
    /// encoded [`BlockHandle`]. It is the outer level of the table iterator.
    pub(crate) fn index_iter(&self) -> BlockIter {
        todo!()
    }

    /// Whether the table has a filter that lookups will consult. For tests and
    /// diagnostics.
    pub(crate) fn has_filter(&self) -> bool {
        todo!()
    }

    /// The meta-index step of [`open`](Self::open), as a function of its own
    /// because it is best effort: every failure becomes `None`.
    ///
    /// The meta-index is read with bytewise order and not the table's
    /// comparator, because its keys are names (`filter.<policy name>`), the way
    /// `TableBuilder` wrote it.
    fn read_filter(
        opts: &TableOptions,
        file: &dyn RandomAccessFile,
        metaindex_handle: BlockHandle,
    ) -> Option<FilterBlockReader> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table is shared between threads as `Arc<Table>`, so it must be `Send`
    /// and `Sync`. This fails to compile if a field ever stops being so.
    #[allow(dead_code)]
    fn table_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Table>();
    }
}
