use crate::comparator::bytewise::BytewiseComparator;
use crate::compression::CompressionType;
use crate::env::WritableFile;
use crate::error::{Error, Result};
use crate::table::block::block_builder::BlockBuilder;
use crate::table::filter_block::builder::FilterBlockBuilder;
use crate::table::filter_block::filter_meta_key;
use crate::table::format::{BLOCK_TRAILER_SIZE, BlockHandle, MAX_BLOCK_SIZE, Footer};
use crate::table::options::TableOptions;
use std::cmp::Ordering;
use std::io::Write;
use std::sync::Arc;

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
            return Err(Error::invalid_argument("restart_interval cannot be 0"));
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
        if self.num_entries > 0
            && self.opts.comparator.compare(key, self.last_key.as_slice()) != Ordering::Greater
        {
            return Err(Error::invalid_argument("key must be greater than last key"));
        }

        self.add_entry_to_index_block(key, self.pending_index_entry);

        self.data_block.add(key, value);

        if let Some(filter) = self.filter.as_mut() {
            filter.add_key(key)
        }

        self.num_entries += 1;

        self.last_key.clear();
        self.last_key.extend_from_slice(key);

        if self.data_block.current_size_estimate() >= self.opts.block_size {
            return self.flush_data_block();
        }

        Ok(())
    }

    fn add_entry_to_index_block(&mut self, key: &[u8], index_handle: Option<BlockHandle>) {
        if let Some(index_entry) = self.pending_index_entry.take() {
            let mut v: Vec<u8> = Vec::new();

            index_entry.encode_to(&mut v);
            self.opts
                .comparator
                .find_shortest_separator(&mut self.last_key, key);

            self.index_block.add(self.last_key.as_slice(), v.as_slice());
        }
    }

    /// Number of entries added so far.
    pub fn num_entries(&self) -> u64 {
        self.num_entries
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
    pub fn finish(mut self) -> Result<u64> {
        let mut meta_index = BlockBuilder::new(1, Arc::new(BytewiseComparator));

        if let Some(mut filter) = self.filter.take() {
            let mut value: Vec<u8> = Vec::new();
            let bytes = filter.finish();

            let handle = self.write_raw(bytes, CompressionType::None)?;
            handle.encode_to(&mut value);

            meta_index.add(
                filter_meta_key(self.opts.filter_policy.take().unwrap().as_ref()).as_bytes(),
                value.as_slice(),
            );
        }

        let metaindex_handle = self.write_raw(meta_index.finish(), CompressionType::None)?;

        self.last_key.push(1);
        self.add_entry_to_index_block(
            self.last_key.to_owned().as_slice(),
            self.pending_index_entry,
        );

        let mut index_block_builder = std::mem::replace(&mut self.index_block, BlockBuilder::new(0, Arc::new(BytewiseComparator)));
        let index_handle = self.write_raw(
            index_block_builder.finish(),
            CompressionType::None,
        )?;

        // write footer and close the file
        let footer: Footer = Footer {
            metaindex_handle,
            index_handle,
        };

        let mut v: Vec<u8> = Vec::new();
        footer.encode_to(&mut v);
        self.file.append(&v)?;

        self.offset += v.len() as u64;

        self.file.sync()?;
        self.file.close()?;

        Ok(self.offset)
    }

    fn write_raw(&mut self, block: &[u8], compression: CompressionType) -> Result<BlockHandle> {
        match crate::table::format::write_block(self.file.as_mut(), self.offset, block, compression)
        {
            Ok(h) => {
                self.offset += h.size + BLOCK_TRAILER_SIZE as u64;
                Ok(h)
            }
            Err(e) => Err(self.fail(e)),
        }
    }

    /// Gives up. Nothing more is written, and the file is dropped, so the caller
    /// should delete it.
    pub fn abandon(self) {
        drop(self)
    }

    /// Writes the data block being built, if it has entries, and records its
    /// index entry as pending.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] if the block is larger than
    /// [`MAX_BLOCK_SIZE`], because `read_block` would refuse to read it back.
    /// [`Error::Io`] if the write fails.
    fn flush_data_block(&mut self) -> Result<()> {
        if self.data_block.is_empty() {
            return Ok(());
        }

        if self.data_block.current_size_estimate() > MAX_BLOCK_SIZE as usize {
            return Err(self.fail(Error::invalid_argument(
                "block size greater than max block size",
            )));
        }

        let handle = match (crate::table::format::write_block(
            self.file.as_mut(),
            self.offset,
            self.data_block.finish(),
            self.opts.compression,
        )) {
            Ok(handle) => handle,
            Err(e) => return Err(self.fail(e)),
        };

        self.offset += handle.size + BLOCK_TRAILER_SIZE as u64;

        self.data_block.reset();

        self.pending_index_entry = Some(handle);

        if let Some(filter) = self.filter.as_mut() {
            filter.start_block(self.offset);
        }

        Ok(())
    }

    /// Remembers `e` as the builder's status and returns it.
    fn fail(&mut self, e: Error) -> Error {
        if self.status.is_ok() {
            self.status = Err(e.clone());
        }
        e
    }
}
