use crate::comparator::bytewise::BytewiseComparator;
use crate::compression::CompressionType;
use crate::env::WritableFile;
use crate::error::{Error, Result};
use crate::table::block::block_builder::BlockBuilder;
use crate::table::filter_block::builder::FilterBlockBuilder;
use crate::table::filter_block::filter_meta_key;
use crate::table::format::{
    BLOCK_TRAILER_SIZE, BlockHandle, FOOTER_ENCODED_LENGTH, Footer, MAX_BLOCK_SIZE,
};
use crate::table::options::TableOptions;
use std::cmp::Ordering;
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
/// # Compression
/// Only data blocks use `opts.compression`. The filter, the meta-index and the
/// index are written with [`CompressionType::None`]:
/// - the filter is close to random bytes, so it does not compress;
/// - the meta-index holds one small entry, so nothing can be saved;
/// - the index is about 0.7% of the file and its entries (short separators and
///   varint handles) compress poorly, so the saving would be a fraction of
///   that, while opening the table would have to decompress it first.
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
    /// Scratch space for encoding a `BlockHandle` as an index entry's value. It
    /// is cleared and reused, so adding an index entry does not allocate once
    /// the buffer has grown (a handle is at most 20 bytes).
    handle_buf: Vec<u8>,
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
            handle_buf: Vec::with_capacity(20),
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
        // return early
        self.status.clone()?;

        // if the next key is not greater than the last key
        if self.num_entries > 0
            && self.opts.comparator.compare(key, self.last_key.as_slice()) != Ordering::Greater
        {
            return Err(Error::invalid_argument("key must be greater than last key"));
        }

        self.add_pending_index_entry(key);

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

    /// Number of entries added so far.
    pub fn num_entries(&self) -> u64 {
        self.num_entries
    }

    /// Bytes written to the file so far, not counting the data block that is
    /// still being built.
    pub fn file_size(&self) -> u64 {
        self.offset
    }

    /// Writes the filter, the meta-index, the index and the footer, then syncs
    /// and closes the file. Returns the final file size.
    ///
    /// A table with no entries is valid: it has no data blocks and an empty
    /// index.
    ///
    /// # Errors
    /// The earlier error, or [`Error::Io`]. The table is complete only if this
    /// returned `Ok`. The builder is consumed, so an error here needs no
    /// poisoning: the caller should discard the file.
    pub fn finish(mut self) -> Result<u64> {
        // return early
        self.status.clone()?;
        self.flush_data_block()?;

        let filter_handle = self.write_filter_block()?;
        let metaindex_handle = self.write_meta_index_block(filter_handle)?;
        let index_handle = self.write_index_block()?;
        self.write_footer(Footer {
            metaindex_handle,
            index_handle,
        })?;

        self.file.sync()?;
        self.file.close()?;

        Ok(self.offset)
    }

    /// Gives up. Nothing more is written, and the file is dropped, so the caller
    /// should delete it.
    pub fn abandon(self) {
        drop(self)
    }

    /// Adds the index entry for the data block written last, if there is one
    /// waiting. `next_key` is the first key of the block that follows, and the
    /// entry's key is `last_key` shortened to lie between the two blocks.
    fn add_pending_index_entry(&mut self, next_key: &[u8]) {
        if let Some(handle) = self.pending_index_entry.take() {
            self.handle_buf.clear();
            handle.encode_to(&mut self.handle_buf);

            self.opts
                .comparator
                .find_shortest_separator(&mut self.last_key, next_key);

            self.index_block
                .add(self.last_key.as_slice(), self.handle_buf.as_slice());
        }
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

        let handle = match write_block_and_advance(
            self.file.as_mut(),
            &mut self.offset,
            self.data_block.finish(),
            self.opts.compression,
        ) {
            Ok(handle) => handle,
            Err(e) => return Err(self.fail(e)),
        };

        self.data_block.reset();

        self.pending_index_entry = Some(handle);

        if let Some(filter) = self.filter.as_mut() {
            filter.start_block(self.offset);
        }

        Ok(())
    }

    /// Writes the filter block, if there is a filter, and returns its handle.
    /// The filter is never compressed, because it is close to random bytes.
    fn write_filter_block(&mut self) -> Result<Option<BlockHandle>> {
        // `take()` moves the filter builder out of `self`, so the bytes it
        // returns borrow the local `filter` and not `self`.
        let Some(mut filter) = self.filter.take() else {
            return Ok(None);
        };

        let bytes = filter.finish();
        let handle = write_block_and_advance(
            self.file.as_mut(),
            &mut self.offset,
            bytes,
            CompressionType::None,
        )?;
        Ok(Some(handle))
    }

    /// Writes the meta-index block and returns its handle. It has one entry,
    /// `filter.<policy name>` -> the filter block's handle, or none if there is
    /// no filter. The footer needs a valid handle either way.
    ///
    /// The block is never compressed: it is far too small to gain anything.
    fn write_meta_index_block(&mut self, filter_handle: Option<BlockHandle>) -> Result<BlockHandle> {
        // The keys are names, not user keys, so they use bytewise order and not
        // the table's comparator. The reader must use bytewise order here too.
        let mut meta_index = BlockBuilder::new(1, Arc::new(BytewiseComparator));

        if let (Some(policy), Some(handle)) = (self.opts.filter_policy.as_ref(), filter_handle) {
            self.handle_buf.clear();
            handle.encode_to(&mut self.handle_buf);
            meta_index.add(
                filter_meta_key(policy.as_ref()).as_bytes(),
                self.handle_buf.as_slice(),
            );
        }

        write_block_and_advance(
            self.file.as_mut(),
            &mut self.offset,
            meta_index.finish(),
            CompressionType::None,
        )
    }

    /// Adds the entry for the last data block, then writes the index block and
    /// returns its handle.
    ///
    /// The last block has no next key to stay below, so its entry uses
    /// `find_short_successor`, which only needs the result to be `>=` the key.
    /// The block is not compressed: it is a small part of the file whose
    /// entries compress poorly, and opening the table would have to decompress
    /// it first.
    fn write_index_block(&mut self) -> Result<BlockHandle> {
        if let Some(handle) = self.pending_index_entry.take() {
            self.opts
                .comparator
                .find_short_successor(&mut self.last_key);

            self.handle_buf.clear();
            handle.encode_to(&mut self.handle_buf);
            self.index_block
                .add(self.last_key.as_slice(), self.handle_buf.as_slice());
        }

        write_block_and_advance(
            self.file.as_mut(),
            &mut self.offset,
            self.index_block.finish(),
            CompressionType::None,
        )
    }

    /// Appends the footer, which must be the last bytes of the file.
    fn write_footer(&mut self, footer: Footer) -> Result<()> {
        let mut encoded: Vec<u8> = Vec::with_capacity(FOOTER_ENCODED_LENGTH);
        footer.encode_to(&mut encoded);

        self.file.append(&encoded)?;
        self.offset += encoded.len() as u64;
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

/// Appends `block` and its trailer at `*offset`, moves `*offset` past them, and
/// returns the block's handle.
///
/// It takes the file and the offset as separate arguments, and not `&mut self`,
/// so that a caller can pass bytes borrowed from another field of the builder
/// (the data block or the index block) in the same call.
fn write_block_and_advance(
    file: &mut dyn WritableFile,
    offset: &mut u64,
    block: &[u8],
    compression: CompressionType,
) -> Result<BlockHandle> {
    let handle = crate::table::format::write_block(file, *offset, block, compression)?;
    *offset += handle.size + BLOCK_TRAILER_SIZE as u64;
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comparator::Comparator;
    use crate::env::file::mem_random_access_file::MemRandomAccessFile;
    use crate::env::file::mem_writable_file::MemWritableFile;
    use crate::filter::bloom::BloomFilterPolicy;
    use crate::iterator::traits::Cursor;
    use crate::table::block::block::Block;
    use crate::table::filter_block::reader::FilterBlockReader;
    use crate::table::format::read_block;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    type Entry = (Vec<u8>, Vec<u8>);

    // ---- building ------------------------------------------------------------

    /// `n` entries with increasing keys: `key-000000` => `value-0`, ...
    fn entries(n: usize) -> Vec<Entry> {
        (0..n)
            .map(|i| (format!("key-{i:06}").into_bytes(), format!("value-{i}").into_bytes()))
            .collect()
    }

    fn opts(block_size: usize, filter: bool) -> TableOptions {
        let mut o = TableOptions::default();
        o.block_size = block_size;
        if !filter {
            o.filter_policy = None;
        }
        o
    }

    fn bytewise() -> Arc<dyn Comparator> {
        Arc::new(BytewiseComparator)
    }

    /// A builder over an in-memory file, plus a handle on the file's bytes.
    fn builder(o: TableOptions) -> (TableBuilder, Arc<Mutex<Vec<u8>>>) {
        let data = Arc::new(Mutex::new(Vec::new()));
        let file = Box::new(MemWritableFile::new(Arc::clone(&data)));
        (TableBuilder::new(o, file).unwrap(), data)
    }

    /// Builds a whole table and returns its bytes and the size `finish` reported.
    fn build(o: TableOptions, es: &[Entry]) -> (Vec<u8>, u64) {
        let (mut tb, data) = builder(o);
        for (k, v) in es {
            tb.add(k, v).unwrap();
        }
        let size = tb.finish().unwrap();
        let file = data.lock().unwrap().clone();
        (file, size)
    }

    // ---- reading it back (no code from the reader side, only the format) ------

    struct ParsedTable {
        file: Vec<u8>,
        footer: Footer,
        /// (separator, handle) of each data block, in index order
        index: Vec<(Vec<u8>, BlockHandle)>,
        /// the entries of each data block
        blocks: Vec<Vec<Entry>>,
        /// the meta-index entries
        meta: Vec<Entry>,
    }

    impl ParsedTable {
        fn all_entries(&self) -> Vec<Entry> {
            self.blocks.concat()
        }

        /// The byte after a block, which is the type byte of its trailer.
        fn trailer_type(&self, h: BlockHandle) -> u8 {
            self.file[(h.offset + h.size) as usize]
        }

        fn raw_block(&self, h: BlockHandle) -> Vec<u8> {
            let raf = MemRandomAccessFile::new(self.file.clone());
            read_block(&raf, h, true).unwrap()
        }

        /// Handle of the filter block, if the meta-index has one.
        fn filter_handle(&self) -> Option<BlockHandle> {
            self.meta.first().map(|(_, value)| {
                let mut v: &[u8] = value;
                let h = BlockHandle::decode_from(&mut v).unwrap();
                assert!(v.is_empty(), "meta-index value has bytes after the handle");
                h
            })
        }

        fn end_of_index(&self) -> u64 {
            self.footer.index_handle.offset + self.footer.index_handle.size + BLOCK_TRAILER_SIZE as u64
        }
    }

    fn read_entries(raf: &MemRandomAccessFile, h: BlockHandle, cmp: &Arc<dyn Comparator>) -> Vec<Entry> {
        let block = Arc::new(Block::new(read_block(raf, h, true).unwrap()).unwrap());
        let mut it = block.iter(Arc::clone(cmp));
        it.seek_to_first();
        let mut out = Vec::new();
        while it.valid() {
            out.push((it.key().to_vec(), it.value().to_vec()));
            it.next();
        }
        it.status().unwrap();
        out
    }

    fn open(file: Vec<u8>, cmp: Arc<dyn Comparator>) -> ParsedTable {
        let raf = MemRandomAccessFile::new(file.clone());
        let footer = Footer::read_from(&raf, file.len() as u64).unwrap();
        let mut index = Vec::new();
        let mut blocks = Vec::new();
        for (sep, value) in read_entries(&raf, footer.index_handle, &cmp) {
            let mut v: &[u8] = &value;
            let handle = BlockHandle::decode_from(&mut v).unwrap();
            assert!(v.is_empty(), "index value has bytes after the handle");
            blocks.push(read_entries(&raf, handle, &cmp));
            index.push((sep, handle));
        }
        // the meta-index always uses bytewise order, whatever the table's comparator
        let meta = read_entries(&raf, footer.metaindex_handle, &bytewise());
        ParsedTable { file, footer, index, blocks, meta }
    }

    /// What every table must satisfy for a seek on the index to find the right
    /// block: contiguous blocks in order, and each separator between its block
    /// and the next, under the table's comparator.
    fn assert_index_invariants(t: &ParsedTable, cmp: &dyn Comparator) {
        assert_eq!(t.index.len(), t.blocks.len());
        let mut expected_offset = 0u64;
        for (i, ((sep, handle), block)) in t.index.iter().zip(&t.blocks).enumerate() {
            assert!(!block.is_empty(), "data block {i} is empty");
            assert_eq!(handle.offset, expected_offset, "block {i} does not follow the previous one");
            expected_offset = handle.offset + handle.size + BLOCK_TRAILER_SIZE as u64;

            let last = &block.last().unwrap().0;
            assert_ne!(cmp.compare(sep, last), Ordering::Less, "separator {i} is below its block's last key");
            if let Some(next) = t.blocks.get(i + 1) {
                assert_eq!(cmp.compare(last, &next[0].0), Ordering::Less, "blocks {i} and {} overlap", i + 1);
                assert_eq!(
                    cmp.compare(sep, &next[0].0),
                    Ordering::Less,
                    "separator {i} is not below the next block's first key"
                );
            }
            if i > 0 {
                assert_eq!(cmp.compare(&t.index[i - 1].0, sep), Ordering::Less, "index keys do not increase at {i}");
            }
        }
    }

    /// Number of restart points in a raw block: its last 4 bytes.
    fn restart_count(raw: &[u8]) -> usize {
        u32::from_le_bytes(raw[raw.len() - 4..].try_into().unwrap()) as usize
    }

    // ---- an empty table, one entry, many entries ---------------------------

    #[test]
    fn an_empty_table_is_valid() {
        for filter in [true, false] {
            let (file, size) = build(opts(4096, filter), &[]);
            assert_eq!(size, file.len() as u64);
            let t = open(file, bytewise());
            assert!(t.index.is_empty(), "no data blocks, so no index entries");
            assert_eq!(t.meta.len(), usize::from(filter));
            assert_eq!(t.end_of_index() + FOOTER_ENCODED_LENGTH as u64, t.file.len() as u64);
        }
    }

    #[test]
    fn one_entry_round_trips() {
        for filter in [true, false] {
            let es = entries(1);
            let (file, size) = build(opts(4096, filter), &es);
            assert_eq!(size, file.len() as u64);
            let t = open(file, bytewise());
            assert_eq!(t.all_entries(), es);
            assert_eq!(t.index.len(), 1);
            assert_invariants_for_bytewise(&t);
        }
    }

    #[test]
    fn many_entries_over_many_blocks_round_trip() {
        for filter in [true, false] {
            let es = entries(2000);
            let (file, size) = build(opts(256, filter), &es);
            assert_eq!(size, file.len() as u64);
            let t = open(file, bytewise());
            assert!(t.index.len() > 20, "expected many blocks, got {}", t.index.len());
            assert_eq!(t.all_entries(), es);
            assert_invariants_for_bytewise(&t);
        }
    }

    fn assert_invariants_for_bytewise(t: &ParsedTable) {
        assert_index_invariants(t, &BytewiseComparator);
    }

    #[test]
    fn the_whole_file_is_blocks_in_order_and_then_the_footer() {
        let (file, _) = build(opts(256, true), &entries(500));
        let t = open(file, bytewise());

        let after_data = t.index.last().map(|(_, h)| h.offset + h.size + 5).unwrap();
        let filter = t.filter_handle().expect("a filter was configured");
        assert_eq!(filter.offset, after_data, "the filter block follows the last data block");
        let after_filter = filter.offset + filter.size + 5;
        assert_eq!(t.footer.metaindex_handle.offset, after_filter);
        let after_meta = t.footer.metaindex_handle.offset + t.footer.metaindex_handle.size + 5;
        assert_eq!(t.footer.index_handle.offset, after_meta);
        assert_eq!(t.end_of_index() + 40, t.file.len() as u64, "the footer is the last 40 bytes");
    }

    // ---- block size ----------------------------------------------------------

    #[test]
    fn a_block_is_flushed_once_it_reaches_block_size() {
        let (file, _) = build(opts(256, false), &entries(1000));
        let t = open(file, bytewise());
        let (last, rest) = t.index.split_last().unwrap();
        for (i, (_, h)) in rest.iter().enumerate() {
            assert!(h.size >= 256, "block {i} was flushed early: {} bytes", h.size);
            assert!(h.size < 256 + 40, "block {i} grew too far past block_size: {} bytes", h.size);
        }
        assert!(last.1.size > 0);
    }

    #[test]
    fn block_size_one_puts_every_entry_in_its_own_block() {
        let es = entries(5);
        let (file, _) = build(opts(1, true), &es);
        let t = open(file, bytewise());
        assert_eq!(t.blocks.len(), 5);
        assert!(t.blocks.iter().all(|b| b.len() == 1));
        assert_eq!(t.all_entries(), es);
        assert_invariants_for_bytewise(&t);
    }

    #[test]
    fn an_entry_larger_than_block_size_gets_a_block_that_ends_with_it() {
        let mut es = entries(10);
        es[4].1 = vec![b'x'; 1000];
        let (file, _) = build(opts(64, true), &es);
        let t = open(file, bytewise());
        assert_eq!(t.all_entries(), es);
        let block = t.blocks.iter().find(|b| b.iter().any(|e| e.1.len() == 1000)).unwrap();
        assert_eq!(block.last().unwrap().1.len(), 1000, "the big entry should end its block");
        assert_invariants_for_bytewise(&t);
    }

    #[test]
    fn restart_interval_reaches_the_data_blocks_and_the_index_restarts_every_entry() {
        let mut o = opts(512, false);
        o.restart_interval = 4;
        let es = entries(300);
        let (file, _) = build(o, &es);
        let t = open(file, bytewise());
        assert!(t.index.len() > 3);
        for ((_, h), block) in t.index.iter().zip(&t.blocks) {
            assert_eq!(restart_count(&t.raw_block(*h)), block.len().div_ceil(4));
        }
        // the index block uses interval 1, whatever the options say
        assert_eq!(restart_count(&t.raw_block(t.footer.index_handle)), t.index.len());
    }

    #[test]
    fn a_zero_restart_interval_is_rejected() {
        let mut o = opts(4096, true);
        o.restart_interval = 0;
        let file = Box::new(MemWritableFile::new(Arc::new(Mutex::new(Vec::new()))));
        let err = TableBuilder::new(o, file).err().expect("restart_interval 0 must be rejected");
        assert!(err.is_invalid_argument());
    }

    // ---- the index ----------------------------------------------------------

    #[test]
    fn index_keys_are_shortened_separators_and_a_successor() {
        // block_size 1 gives one block per key, so every gap gets a separator
        let es: Vec<Entry> = [&b"apple"[..], b"cherry", b"zebra"]
            .iter()
            .map(|k| (k.to_vec(), b"v".to_vec()))
            .collect();
        let (file, _) = build(opts(1, false), &es);
        let t = open(file, bytewise());
        let keys: Vec<&[u8]> = t.index.iter().map(|(k, _)| k.as_slice()).collect();
        // apple|cherry -> "b", cherry|zebra -> "d", and the last block gets the
        // successor of "zebra". This pins BytewiseComparator's shortening.
        assert_eq!(keys, vec![&b"b"[..], b"d", b"{"]);
        assert_invariants_for_bytewise(&t);
    }

    #[test]
    fn index_handles_decode_with_nothing_left_over_even_for_large_offsets() {
        // a long run of blocks pushes offsets into multi-byte varints
        let (file, _) = build(opts(128, false), &entries(5000));
        let t = open(file, bytewise()); // `open` asserts no trailing bytes in every handle
        assert!(t.index.last().unwrap().1.offset > 1 << 14);
        assert_eq!(t.all_entries(), entries(5000));
    }

    // ---- the filter ---------------------------------------------------------

    fn filter_reader(t: &ParsedTable) -> FilterBlockReader {
        let h = t.filter_handle().expect("the table has a filter");
        FilterBlockReader::new(Arc::new(BloomFilterPolicy), t.raw_block(h))
    }

    #[test]
    fn every_key_is_found_by_the_filter_at_its_own_blocks_offset() {
        let (file, _) = build(opts(512, true), &entries(2000));
        let t = open(file, bytewise());
        let r = filter_reader(&t);
        let slots: std::collections::BTreeSet<u64> = t.index.iter().map(|(_, h)| h.offset >> 11).collect();
        assert!(slots.len() > 10, "the test needs a table that spans many filter slots");
        for ((_, h), block) in t.index.iter().zip(&t.blocks) {
            for (key, _) in block {
                assert!(r.key_may_match(h.offset, key), "filter lost {:?}", String::from_utf8_lossy(key));
            }
        }
    }

    #[test]
    fn the_filter_rejects_most_absent_keys() {
        let (file, _) = build(opts(512, true), &entries(2000));
        let t = open(file, bytewise());
        let r = filter_reader(&t);
        let mut false_positives = 0;
        for i in 0..2000usize {
            let (_, h) = &t.index[i % t.index.len()];
            if r.key_may_match(h.offset, format!("absent-{i:06}").as_bytes()) {
                false_positives += 1;
            }
        }
        assert!(false_positives <= 40, "{false_positives} of 2000 absent keys matched");
    }

    #[test]
    fn the_meta_index_names_the_policy_and_points_at_the_filter() {
        let (file, _) = build(opts(512, true), &entries(100));
        let t = open(file, bytewise());
        assert_eq!(t.meta.len(), 1);
        assert_eq!(t.meta[0].0, b"filter.BLOOM_FILTER_POLICY");
        let h = t.filter_handle().unwrap();
        assert!(h.size > 5);
        assert_eq!(t.raw_block(h).last().copied(), Some(11), "the filter block ends with base_lg");
    }

    #[test]
    fn without_a_filter_policy_the_meta_index_is_empty() {
        let (file, _) = build(opts(512, false), &entries(100));
        let t = open(file, bytewise());
        assert!(t.meta.is_empty());
        assert!(t.filter_handle().is_none());
    }

    #[test]
    fn index_meta_index_and_filter_blocks_are_stored_uncompressed() {
        let (file, _) = build(opts(256, true), &entries(500));
        let t = open(file, bytewise());
        assert_eq!(t.trailer_type(t.footer.index_handle), 0);
        assert_eq!(t.trailer_type(t.footer.metaindex_handle), 0);
        assert_eq!(t.trailer_type(t.filter_handle().unwrap()), 0);
    }

    // ---- adding keys ----------------------------------------------------------

    #[test]
    fn a_key_that_is_not_greater_is_rejected_and_changes_nothing() {
        let (mut tb, data) = builder(opts(4096, true));
        tb.add(b"b", b"1").unwrap();
        let before = data.lock().unwrap().len();

        assert!(tb.add(b"a", b"x").unwrap_err().is_invalid_argument(), "smaller key");
        assert!(tb.add(b"b", b"x").unwrap_err().is_invalid_argument(), "duplicate key");
        assert_eq!(tb.num_entries(), 1);
        assert_eq!(data.lock().unwrap().len(), before);

        // the builder is still usable, and the rejected keys are not in the table
        tb.add(b"c", b"2").unwrap();
        tb.finish().unwrap();
        let t = open(data.lock().unwrap().clone(), bytewise());
        assert_eq!(t.all_entries(), vec![(b"b".to_vec(), b"1".to_vec()), (b"c".to_vec(), b"2".to_vec())]);
    }

    #[test]
    fn an_empty_key_is_a_valid_first_key() {
        let (mut tb, data) = builder(opts(4096, true));
        tb.add(b"", b"zero").unwrap();
        tb.add(b"a", b"one").unwrap();
        assert!(tb.add(b"", b"again").unwrap_err().is_invalid_argument());
        tb.finish().unwrap();
        let t = open(data.lock().unwrap().clone(), bytewise());
        assert_eq!(t.all_entries(), vec![(vec![], b"zero".to_vec()), (b"a".to_vec(), b"one".to_vec())]);
        assert!(filter_reader(&t).key_may_match(0, b""));
    }

    #[test]
    fn empty_values_and_binary_keys_round_trip() {
        let es: Vec<Entry> = vec![
            (vec![0x00], vec![]),
            (vec![0x00, 0x00], vec![0xFF; 3]),
            (vec![0x01], vec![]),
            (vec![0x7F, 0x00, 0xFF], b"x".to_vec()),
            (vec![0xFF, 0xFF], vec![0; 100]),
        ];
        let (file, _) = build(opts(16, true), &es);
        let t = open(file, bytewise());
        assert_eq!(t.all_entries(), es);
        assert_invariants_for_bytewise(&t);
    }

    #[test]
    fn num_entries_counts_accepted_entries() {
        let (mut tb, _) = builder(opts(64, true));
        assert_eq!(tb.num_entries(), 0);
        for (i, (k, v)) in entries(100).iter().enumerate() {
            tb.add(k, v).unwrap();
            assert_eq!(tb.num_entries(), i as u64 + 1);
        }
    }

    #[test]
    fn file_size_counts_exactly_the_bytes_written_so_far() {
        let (mut tb, data) = builder(opts(128, true));
        assert_eq!(tb.file_size(), 0);
        let mut previous = 0;
        for (k, v) in entries(500) {
            tb.add(&k, &v).unwrap();
            let size = tb.file_size();
            assert!(size >= previous, "file_size went backwards");
            assert_eq!(size, data.lock().unwrap().len() as u64);
            previous = size;
        }
        assert!(previous > 0, "some blocks should have been flushed");
        let total = tb.finish().unwrap();
        assert!(total > previous, "the filter, index and footer come after the data blocks");
        assert_eq!(total, data.lock().unwrap().len() as u64);
    }

    // ---- other orderings ----------------------------------------------------

    /// Reverses bytewise order. It leaves the separator methods at their
    /// defaults, so it also checks that the builder never assumes bytewise order.
    struct ReverseCmp;
    impl Comparator for ReverseCmp {
        fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
            b.cmp(a)
        }
        fn name(&self) -> &'static str {
            "test.reverse"
        }
    }

    /// Bytewise order, but it insists on the documented precondition of
    /// `find_shortest_separator` (`start < limit`).
    struct StrictCmp;
    impl Comparator for StrictCmp {
        fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
            a.cmp(b)
        }
        fn name(&self) -> &'static str {
            "test.strict"
        }
        fn find_shortest_separator(&self, start: &mut Vec<u8>, limit: &[u8]) {
            assert!(start.as_slice() < limit, "find_shortest_separator called with start >= limit");
        }
    }

    #[test]
    fn a_reverse_ordered_table_has_a_valid_index() {
        let mut o = opts(128, false);
        o.comparator = Arc::new(ReverseCmp);
        let es: Vec<Entry> = (0..300)
            .map(|i| (format!("key-{:06}", 999_999 - i).into_bytes(), b"v".to_vec()))
            .collect();
        let (file, _) = build(o, &es);
        let t = open(file, Arc::new(ReverseCmp));
        assert_eq!(t.all_entries(), es);
        assert!(t.index.len() > 5);
        // the last entry too: a bytewise "key + 1" would be below the last key here
        assert_index_invariants(&t, &ReverseCmp);
    }

    #[test]
    fn the_order_check_uses_the_tables_comparator() {
        let mut o = opts(4096, false);
        o.comparator = Arc::new(ReverseCmp);
        let (mut tb, _) = builder(o);
        tb.add(b"b", b"1").unwrap();
        tb.add(b"a", b"2").unwrap(); // greater under the reverse order
        assert!(tb.add(b"c", b"3").unwrap_err().is_invalid_argument());
    }

    #[test]
    fn a_comparator_that_checks_the_separator_precondition_is_never_misused() {
        let mut o = opts(128, true);
        o.comparator = Arc::new(StrictCmp);
        let es = entries(300);
        let (file, _) = build(o, &es);
        let t = open(file, Arc::new(StrictCmp));
        assert_eq!(t.all_entries(), es);
        assert_index_invariants(&t, &StrictCmp);
    }

    // ---- failures -------------------------------------------------------------

    /// A file whose `fail_at`th `append` fails. Earlier appends are kept.
    struct FailAt {
        data: Arc<Mutex<Vec<u8>>>,
        appends: Arc<AtomicUsize>,
        fail_at: usize,
    }

    impl WritableFile for FailAt {
        fn append(&mut self, d: &[u8]) -> Result<()> {
            let n = self.appends.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            if n == self.fail_at {
                return Err(Error::io("append", std::io::Error::new(std::io::ErrorKind::Other, "injected failure")));
            }
            self.data.lock().unwrap().extend_from_slice(d);
            Ok(())
        }
        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
        fn sync(&mut self) -> Result<()> {
            Ok(())
        }
        fn close(self: Box<Self>) -> Result<()> {
            Ok(())
        }
    }

    /// Builds a table whose `fail_at`th append fails. Returns the result, the
    /// bytes that reached the file, and how many appends were attempted.
    fn build_with_failure(fail_at: usize, es: &[Entry]) -> (Result<u64>, Vec<u8>, usize) {
        let data = Arc::new(Mutex::new(Vec::new()));
        let appends = Arc::new(AtomicUsize::new(0));
        let file = FailAt { data: Arc::clone(&data), appends: Arc::clone(&appends), fail_at };
        let mut tb = TableBuilder::new(opts(128, true), Box::new(file)).unwrap();

        let mut first_error = None;
        for (k, v) in es {
            if let Err(e) = tb.add(k, v) {
                first_error = Some(e);
                break;
            }
        }
        let result = match first_error {
            Some(e) => {
                // a poisoned builder must refuse to finish, with the same kind of error
                let finish_error = tb.finish().expect_err("finish must fail after a failed add");
                assert_eq!(finish_error.is_io_error(), e.is_io_error());
                Err(e)
            }
            None => tb.finish(),
        };
        let bytes = data.lock().unwrap().clone();
        (result, bytes, appends.load(AtomicOrdering::SeqCst))
    }

    fn is_a_table(bytes: &[u8]) -> bool {
        Footer::read_from(&MemRandomAccessFile::new(bytes.to_vec()), bytes.len() as u64).is_ok()
    }

    #[test]
    fn a_write_error_poisons_the_builder() {
        let data = Arc::new(Mutex::new(Vec::new()));
        let file = FailAt { data: Arc::clone(&data), appends: Arc::new(AtomicUsize::new(0)), fail_at: 1 };
        let mut o = opts(8, true);
        o.block_size = 8;
        let mut tb = TableBuilder::new(o, Box::new(file)).unwrap();

        assert!(tb.add(b"aaaa", b"vvvvvvvv").unwrap_err().is_io_error(), "the flush fails");
        assert!(tb.add(b"bbbb", b"vvvvvvvv").unwrap_err().is_io_error(), "later calls return it too");
        assert!(tb.finish().unwrap_err().is_io_error());
        assert!(!is_a_table(&data.lock().unwrap()));
    }

    #[test]
    fn a_write_error_at_any_point_fails_the_build_and_leaves_no_valid_table() {
        let es = entries(60);
        let (ok, bytes, total_appends) = build_with_failure(usize::MAX, &es);
        assert_eq!(ok.unwrap(), bytes.len() as u64);
        assert!(is_a_table(&bytes));
        assert!(total_appends > 10, "expected data blocks, filter, meta-index, index and footer");

        for fail_at in 1..=total_appends {
            let (result, bytes, _) = build_with_failure(fail_at, &es);
            let err = result.expect_err(&format!("append {fail_at} failed but the build succeeded"));
            assert!(err.is_io_error(), "append {fail_at}");
            assert!(!is_a_table(&bytes), "append {fail_at} failed but the file looks like a table");
        }
    }

    #[test]
    fn abandon_writes_no_footer() {
        let (mut tb, data) = builder(opts(64, true));
        for (k, v) in entries(30) {
            tb.add(&k, &v).unwrap();
        }
        let written = data.lock().unwrap().len();
        assert!(written > 0, "some data blocks were already flushed");
        tb.abandon();
        assert_eq!(data.lock().unwrap().len(), written, "abandon writes nothing more");
        assert!(!is_a_table(&data.lock().unwrap()));
    }
}
