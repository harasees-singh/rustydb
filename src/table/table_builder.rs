use std::cmp::Ordering;
use std::iter::Filter;
use std::sync::Arc;
use crate::env::WritableFile;
use crate::error::{Error, Result};
use crate::table::block::block_builder::BlockBuilder;
use crate::table::filter_block::builder::FilterBlockBuilder;
use crate::table::format::{BlockHandle, BLOCK_TRAILER_SIZE, MAX_BLOCK_SIZE};
use crate::table::options::TableOptions;

/// Writes one sorted table (SSTable) to a file.
///
/// Keys must be added in strictly increasing order under the comparator in the
/// options. Call [`finish`](Self::finish) to complete the table, or
/// [`abandon`](Self::abandon) to give up. A builder that is simply dropped
/// leaves a file without a footer, which is not a table.
///
/// # File layout it writes
/// ```text
/// [data block][5B trailer] ... [filter block][5B] [meta-index][5B] [index][5B] [footer, 40B]
/// ```
/// The index holds one entry per data block: a key `>=` every key in that block
/// and `<` every key in the next, and the block's [`BlockHandle`] as the value.
///
/// # How it works
/// ```text
/// new      nothing to do for the filter: the first block starts at offset 0,
///          which is slot 0, and `start_block(0)` would close no slot
/// add      1. reject a key that is not greater than the last one (every build,
///             before any state changes)
///          2. if an index entry is pending: shorten `last_key` against `key`,
///             and add (separator -> handle) to the index block
///          3. filter.add_key(key); data_block.add(key, value); last_key = key
///          4. if data_block.current_size_estimate() >= block_size: flush
/// flush    write_block(data_block.finish()) -> handle; data_block.reset()
///          offset += handle.size + BLOCK_TRAILER_SIZE
///          pending index entry = handle; filter.start_block(offset)
/// finish   flush the last data block
///          filter block  -> written with CompressionType::None
///          meta-index    -> one entry "filter.<name>" -> filter handle, or empty
///          index         -> add the last entry using find_short_successor, write
///          footer        -> Footer { metaindex_handle, index_handle }
///          sync, then close
/// ```
/// `filter.start_block` is called in `flush`, after the offset moves, because
/// the next block's keys belong to the filter slot of that block's own start
/// offset. The first block needs no call, since it starts at offset 0.
///
/// # Errors
/// After an I/O error the builder is poisoned: every later call returns that
/// error, and the file should be discarded. A rejected key (out of order) is not
/// an I/O error. It changes nothing, and the caller may continue.
pub struct TableBuilder {
    opts: TableOptions,
    file: Box<dyn WritableFile>,
    /// Where the next block starts in the file. Equals the bytes written so far.
    offset: u64,
    data_block: BlockBuilder,
    /// Built with restart interval 1: every index entry is a restart point, so
    /// each one is found on its own and no prefix compression is used.
    index_block: BlockBuilder,
    /// `None` when the options have no filter policy.
    filter: Option<FilterBlockBuilder>,
    /// The last key added. Compared with the next key, and shortened into a
    /// separator when a data block ends.
    last_key: Vec<u8>,
    /// Handle of a data block that was just written and has no index entry yet.
    /// The entry waits for the next key, so the separator can be chosen from
    /// both sides of the gap between the blocks.
    pending_index_entry: Option<BlockHandle>,
    num_entries: u64,
    /// The first error, kept and returned by every later call.
    status: Result<()>,
}

impl TableBuilder {
    /// Starts a table in `file`, which must be new and empty.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] if `opts.restart_interval` is 0, which
    /// `BlockBuilder` would panic on.
    pub fn new(opts: TableOptions, file: Box<dyn WritableFile>) -> Result<TableBuilder> {
        if opts.restart_interval == 0 {
            return Err(crate::error::Error::invalid_argument("restart_interval cannot be 0"));
        }

        let mut filter: Option<FilterBlockBuilder> = None;

        if let Some(p) = opts.filter_policy.as_ref() {
            let mut f = FilterBlockBuilder::new(Arc::clone(p));
            f.start_block(0);
            filter = Some(f);
        }

        Ok(TableBuilder {
            opts: opts.clone(),
            file,
            offset: 0,
            data_block: BlockBuilder::new(opts.restart_interval, Arc::clone(&opts.comparator)),
            index_block: BlockBuilder::new(1, Arc::clone(&opts.comparator)),
            filter,
            last_key: Vec::new(),
            pending_index_entry: None,
            num_entries: 0,
            status: Ok(()),
        })
    }

    /// Adds an entry.
    ///
    /// # Errors
    /// - [`Error::InvalidArgument`] if `key` is not greater than the previous
    ///   key. Checked in every build, because a table with its keys out of
    ///   order is silently unreadable. Nothing is changed.
    /// - The earlier error, if the builder is poisoned, or [`Error::Io`] from
    ///   writing a block.
    pub fn add(&mut self, key: &[u8], value: &[u8]) -> Result<()> {

        if self.status.is_err() {
            return self.status.clone();
        }

        // if the next key is not greater than the last key
        if self.num_entries > 0 && self.opts.comparator.compare(key, self.last_key.as_slice()) != Ordering::Greater {
            self.status = Err(Error::invalid_argument("key must be greater than last key"));
            return self.status.clone();
        }

        if let Some(index_entry) = self.pending_index_entry.take() {
            let mut v : Vec<u8> = Vec::new();

            index_entry.encode_to(&mut v);
            self.opts.comparator.find_shortest_separator(&mut self.last_key, key);

            self.index_block.add(self.last_key.as_slice(), v.as_slice());
        }

        self.data_block.add(key, value);

        if let Some(filter) = self.filter.as_mut() {
            filter.add_key(key)
        }

        self.num_entries += 1;

        self.last_key.clear();
        self.last_key.extend_from_slice(key);

        if self.data_block.current_size_estimate() >= self.opts.block_size{
            return self.flush_data_block();
        }

        Ok(())
    }

    /// Number of entries added so far.
    pub fn num_entries(&self) -> u64 {
        todo!()
    }

    /// Bytes written to the file so far, not counting the data block that is
    /// still being built.
    pub fn file_size(&self) -> u64 {
        todo!()
    }

    /// Writes the filter, the meta-index, the index and the footer, then syncs
    /// and closes the file. Returns the final file size.
    ///
    /// A table with no entries is valid: it has no data blocks and an empty
    /// index.
    ///
    /// # Errors
    /// The earlier error, or [`Error::Io`]. The table is complete only if this
    /// returned `Ok`.
    pub fn finish(self) -> Result<u64> {
        todo!()
    }

    /// Gives up. Nothing more is written, and the file is dropped, so the caller
    /// should delete it.
    pub fn abandon(self) {
        todo!()
    }

    /// Writes the data block being built, if it has entries, and records its
    /// index entry as pending.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] if the block is larger than
    /// [`MAX_BLOCK_SIZE`], because `read_block` would refuse to read it back.
    /// [`Error::Io`] if the write fails.
    fn flush_data_block(&mut self) -> Result<()> {
        todo!()
    }

    /// Remembers `e` as the builder's status and returns it.
    fn fail(&mut self, e: Error) -> Error {
        todo!()
    }
}
