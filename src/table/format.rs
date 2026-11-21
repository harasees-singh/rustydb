//! How blocks sit in a table file: pointers to blocks, the footer that is the
//! entry point of a file, and the trailer after every block that carries its
//! checksum.
//!
//! # File layout
//! ```text
//! [data block 0][trailer] ... [data block N][trailer]
//! [filter block][trailer]
//! [meta-index block][trailer]
//! [index block][trailer]
//! [footer]
//! ```
//! The footer is last and has a fixed size, so a reader finds it without
//! knowing anything else about the file. It holds a [`BlockHandle`] for the
//! index block and one for the meta-index block, and everything else is reached
//! from those.
//!
//! # Footer (40 bytes)
//! ```text
//! | metaindex offset | metaindex size | index offset | index size | magic |
//! |      u64 LE      |     u64 LE     |    u64 LE    |   u64 LE   | u64 LE|
//! ```
//! The magic number only says "this is a table file". It is a constant and not a
//! checksum, and the footer has no checksum of its own.
//!
//! # Block trailer (5 bytes)
//! ```text
//! | stored bytes | type: 1 byte | masked crc32c: 4 bytes, LE |
//! ```
//! * The **stored bytes** are the block as written to disk: compressed when the
//!   writer chose to compress it, raw otherwise. A [`BlockHandle`]'s `size`
//!   counts only these.
//! * The **type byte** is a [`CompressionType`] telling the reader how to decode
//!   the stored bytes. It is stored per block because the writer keeps a
//!   compressed block only when it saves enough space, so one table can mix
//!   both forms.
//! * The **checksum** covers the stored bytes and the type byte, so damage to
//!   either is caught (see `trailer_crc`).
//!
//! Integers are little-endian, except inside a [`BlockHandle`] written to an
//! index block, where they are varints.

use crate::compression::CompressionType;
use crate::env::{RandomAccessFile, WritableFile};
use crate::util::crc32c::{extend, mask, value};

/// The last 8 bytes of every table file. A constant that identifies the file
/// type, so opening something that is not a table fails right away instead of
/// decoding garbage.
pub const TABLE_MAGIC_NUMBER: u64 = 0xdb47_7524_8b80_fb57;

/// Fixed footer size: four u64 fields + the magic number.
pub const FOOTER_ENCODED_LENGTH: usize = 40;

/// Bytes after every block: type byte + masked CRC32C.
pub const BLOCK_TRAILER_SIZE: usize = 5;

/// A compressed block is kept only if it saves at least 1/8 of its size.
const MIN_COMPRESSION_SAVING_DIVISOR: usize = 8;

/// A pointer to a block in the file.
///
/// `size` is the length of the block's stored bytes and does not include the
/// [trailer](BLOCK_TRAILER_SIZE), so the next block starts at
/// `offset + size + BLOCK_TRAILER_SIZE`.
///
/// The index block stores one handle per data block, as the value of an entry,
/// in the varint form of [`encode_to`](Self::encode_to). The footer stores its
/// two handles as fixed-width fields instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockHandle {
    /// Byte position of the block in the file.
    pub offset: u64,
    /// Length of the block's stored bytes, without the trailer.
    pub size: u64,
}

impl BlockHandle {
    /// Appends the handle as two varint64s: `offset`, then `size`.
    ///
    /// That is between 2 and 20 bytes, and small values take few bytes, which
    /// matters for the index block where there is one handle per data block.
    pub fn encode_to(&self, dst: &mut Vec<u8>) {
        todo!()
    }

    /// Reads one handle from the front of `src` and advances `src` past it, so a
    /// caller can read what follows.
    ///
    /// # Errors
    /// [`Error::Corruption`](crate::Error::Corruption) if `src` ends in the
    /// middle of a varint or a varint is longer than 10 bytes. `src` is
    /// unspecified after an error.
    pub fn decode_from(src: &mut &[u8]) -> crate::error::Result<BlockHandle> {
        todo!()
    }
}

/// The fixed-size record at the end of a table file, and the first thing read
/// when opening one.
///
/// It points at the two blocks everything else is found through:
/// * the **index block**, which maps a key to the data block that can hold it;
/// * the **meta-index block**, which maps a name (such as the filter's) to the
///   block holding that metadata.
///
/// The builder only knows where these blocks ended up after writing them, which
/// is why the pointers sit at the end, in a place whose position is known
/// without reading anything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footer {
    /// The meta-index block.
    pub metaindex_handle: BlockHandle,
    /// The index block.
    pub index_handle: BlockHandle,
}

impl Footer {
    /// Appends exactly [`FOOTER_ENCODED_LENGTH`] bytes: the four handle fields,
    /// then [`TABLE_MAGIC_NUMBER`], each a little-endian `u64`.
    pub fn encode_to(&self, dst: &mut Vec<u8>) {
        todo!()
    }

    /// Reads a footer from the front of `src` and advances `src` past its
    /// [`FOOTER_ENCODED_LENGTH`] bytes.
    ///
    /// # Errors
    /// [`Error::Corruption`](crate::Error::Corruption) if `src` is shorter than a
    /// footer, or the magic number does not match ("not an sstable").
    pub fn decode_from(src: &mut &[u8]) -> crate::error::Result<Footer> {
        todo!()
    }

    /// Reads and decodes the footer from the end of `file`, which is
    /// `file_size` bytes long.
    ///
    /// The size is a parameter because [`RandomAccessFile`] has no way to ask
    /// for it. A table opener takes it from the file system.
    ///
    /// # Errors
    /// [`Error::Corruption`](crate::Error::Corruption) if the file is shorter
    /// than a footer or the footer is invalid, and [`Error::Io`](crate::Error::Io)
    /// if the read fails.
    pub fn read_from(file: &dyn RandomAccessFile, file_size: u64) -> crate::error::Result<Footer> {
        todo!()
    }
}

/// Appends `block` and its [trailer](BLOCK_TRAILER_SIZE) to `file`, and returns
/// the handle that points at it.
///
/// If `compression` is not [`CompressionType::None`], the block is compressed,
/// but the compressed form is kept only when it is at least 1/8 smaller.
/// Otherwise, the raw block is stored with type `None`, so a block that does not
/// compress costs nothing and the reader never has to decompress it.
///
/// `offset` is where this block starts in the file and becomes the handle's
/// `offset`. The caller tracks it: the next block starts at
/// `offset + handle.size + BLOCK_TRAILER_SIZE`.
///
/// # Errors
/// [`Error::Io`](crate::Error::Io) if writing to `file` fails. After an error
/// the file should be discarded.
pub fn write_block(
    file: &mut dyn WritableFile,
    offset: u64,
    block: &[u8],
    compression: CompressionType,
) -> crate::error::Result<BlockHandle> {
    todo!()
}

/// Reads the block that `handle` points at and returns its original bytes, ready
/// for [`Block::new`](super::block::block::Block::new).
///
/// This reads `handle.size` stored bytes plus the trailer in one read, checks
/// the checksum (when `verify_checksum` is set) *before* decompressing so damaged
/// data never reaches a decompressor, then decodes according to the type byte.
/// A block stored as `None` is returned without a copy.
///
/// Set `verify_checksum` to `false` only when the cost of the CRC matters more
/// than detecting damage. A damaged type byte still fails, because it is checked
/// whether or not the CRC is.
///
/// # Errors
/// * [`Error::Corruption`](crate::Error::Corruption): the file ends before
///   `handle.size + 5` bytes ("truncated block read"), the checksum does not
///   match ("block checksum mismatch"), the type byte is not a known
///   [`CompressionType`], or the stored bytes do not decompress.
/// * [`Error::NotSupported`](crate::Error::NotSupported): the type is known but
///   its algorithm is not built in.
/// * [`Error::Io`](crate::Error::Io): the read fails.
pub fn read_block(
    file: &dyn RandomAccessFile,
    handle: BlockHandle,
    verify_checksum: bool,
) -> crate::error::Result<Vec<u8>> {
    todo!()
}

/// The 4 checksum bytes of a block's trailer.
///
/// It is the CRC32C of the `stored` bytes, continued over `type_byte`, then
/// masked. The writer stores this value and the reader recomputes it, so both
/// use this one function.
///
/// * **Why `stored`:** the bytes as they are on disk, so damage is detected
///   before decompression.
/// * **Why `type_byte` is included:** it says how to interpret `stored`. A bit
///   flip there would otherwise turn compressed data into a block that looks
///   valid.
/// * **Why masked:** a CRC computed over data that itself contains CRCs is
///   weak, and masking breaks that pattern. It is reversible (`unmask` in
///   `util::crc32c`).
///
/// It detects accidental damage, not tampering.
fn trailer_crc(stored: &[u8], type_byte: u8) -> u32 {
    mask(extend(value(stored), &[type_byte]))
}
