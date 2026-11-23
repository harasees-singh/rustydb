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
///
/// derived as follows: ```echo "glory is fleeting, obscurity is forever" | sha1sum | cut -c1-16```
pub const TABLE_MAGIC_NUMBER: u64 = 0xfc71d71f11ed5ea7;

/// Fixed footer size: four u64 fields + the magic number.
pub const FOOTER_ENCODED_LENGTH: usize = 40;

/// Bytes after every block: type byte + masked CRC32C.
pub const BLOCK_TRAILER_SIZE: usize = 5;

/// A compressed block is kept only if it saves at least 1/8 of its size.
const MIN_COMPRESSION_SAVING_DIVISOR: usize = 8;

/// Largest `BlockHandle::size` that `read_block` accepts (1 GiB). Handles are
/// read from the file, and without a limit a damaged one could ask for an
/// allocation of petabytes.
pub const MAX_BLOCK_SIZE: u64 = 1 << 30;

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
        crate::util::coding::put_varint64(dst, self.offset);
        crate::util::coding::put_varint64(dst, self.size);
    }

    /// Reads one handle from the front of `src` and advances `src` past it, so a
    /// caller can read what follows.
    ///
    /// # Errors
    /// [`Error::Corruption`](crate::Error::Corruption) if `src` ends in the
    /// middle of a varint or a varint is longer than 10 bytes. `src` is
    /// unspecified after an error. Bytes after the handle are left in `src`.
    pub fn decode_from(src: &mut &[u8]) -> crate::error::Result<BlockHandle> {
        let mut offset: u64 = 0;
        let mut size: u64 = 0;
        match crate::util::coding::get_varint64(src, &mut offset) {
            true => match crate::util::coding::get_varint64(src, &mut size) {
                true => {}
                false => {
                    return Err(crate::error::Error::corruption(
                        "unable to read size for block handle from src",
                    ));
                }
            },
            false => {
                return Err(crate::error::Error::corruption(
                    "unable to read offset for block handle from src",
                ));
            }
        }
        Ok(BlockHandle { offset, size })
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
        crate::util::coding::put_fixed64(dst, self.metaindex_handle.offset);
        crate::util::coding::put_fixed64(dst, self.metaindex_handle.size);
        crate::util::coding::put_fixed64(dst, self.index_handle.offset);
        crate::util::coding::put_fixed64(dst, self.index_handle.size);
        // now put magic number as little endian fixed64
        crate::util::coding::put_fixed64(dst, TABLE_MAGIC_NUMBER);
    }

    /// Reads a footer from the front of `src` and advances `src` past its
    /// [`FOOTER_ENCODED_LENGTH`] bytes.
    ///
    /// # Errors
    /// [`Error::Corruption`](crate::Error::Corruption) if `src` is shorter than a
    /// footer, or the magic number does not match.
    pub fn decode_from(src: &mut &[u8]) -> crate::error::Result<Footer> {
        let mut fields = [0u64; 5];
        for field in fields.iter_mut() {
            if !crate::util::coding::get_fixed64(src, field) {
                return Err(crate::error::Error::corruption(
                    "file too short to be a SSTable",
                ));
            }
        }
        let [
            metaindex_offset,
            metaindex_size,
            index_offset,
            index_size,
            magic_number,
        ] = fields;

        if magic_number != TABLE_MAGIC_NUMBER {
            return Err(crate::error::Error::corruption(
                "file contents do not match a SSTable",
            ));
        }
        Ok(Footer {
            metaindex_handle: BlockHandle {
                offset: metaindex_offset,
                size: metaindex_size,
            },
            index_handle: BlockHandle {
                offset: index_offset,
                size: index_size,
            },
        })
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
        let mut buffer = vec![0u8; FOOTER_ENCODED_LENGTH];

        let start = file_size
            .checked_sub(FOOTER_ENCODED_LENGTH as u64)
            .ok_or_else(|| crate::error::Error::corruption("file is too short to be a SSTable"))?;

        file.read_exact_at(start, buffer.as_mut_slice())?;
        Footer::decode_from(&mut buffer.as_slice())
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
    if compression == CompressionType::None {
        return append_block(file, block, CompressionType::None, offset);
    }

    let mut compressed_block: Vec<u8> = Vec::new();

    crate::compression::compressor_for(compression)?.compress(block, &mut compressed_block)?;

    if compressed_block.len() * MIN_COMPRESSION_SAVING_DIVISOR
        > block.len() * (MIN_COMPRESSION_SAVING_DIVISOR - 1)
    {
        // compression savings didn't meet the threshold
        return append_block(file, block, CompressionType::None, offset);
    }

    append_block(file, compressed_block.as_slice(), compression, offset)
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
/// * [`Error::Corruption`](crate::Error::Corruption): `handle.size` is larger
///   than [`MAX_BLOCK_SIZE`], the file ends before `handle.size + 5` bytes, the
///   checksum does not match, the type byte is not a known [`CompressionType`],
///   or the stored bytes do not decompress.
/// * [`Error::NotSupported`](crate::Error::NotSupported): the type is known but
///   its algorithm is not built in.
/// * [`Error::Io`](crate::Error::Io): the read fails.
pub fn read_block(
    file: &dyn RandomAccessFile,
    handle: BlockHandle,
    verify_checksum: bool,
) -> crate::error::Result<Vec<u8>> {
    // The handle comes from the file, so it is untrusted: a huge size would make
    // the allocation below abort the process, which cannot be caught.
    if handle.size > MAX_BLOCK_SIZE {
        return Err(crate::error::Error::corruption("bad block handle"));
    }
    let size = handle.size as usize;

    let mut block_with_trailer: Vec<u8> = vec![0u8; size + BLOCK_TRAILER_SIZE];

    // handle.size excludes the trailer, so the trailer begins at
    // handle.offset + handle.size and is 5 bytes long. A shorter file fails here.
    file.read_exact_at(handle.offset, block_with_trailer.as_mut_slice())?;

    let (block, trailer) = block_with_trailer.split_at(size);
    let type_byte = trailer[0];

    // the last 4 bytes of the trailer are the checksum, little endian
    if verify_checksum && trailer_crc(block, type_byte).to_le_bytes() != trailer[1..] {
        return Err(crate::error::Error::corruption(
            "checksum mismatch for block",
        ));
    }

    let Some(compression_type) = CompressionType::from_u8(type_byte) else {
        return Err(crate::error::Error::corruption("unknown compression type"));
    };

    if compression_type == CompressionType::None {
        // already the original bytes: drop the trailer and hand over the buffer
        block_with_trailer.truncate(size);
        return Ok(block_with_trailer);
    }

    let mut decompressed_block: Vec<u8> = Vec::new();
    crate::compression::compressor_for(compression_type)?
        .decompress(block, &mut decompressed_block)?;
    Ok(decompressed_block)
}

fn append_block(
    file: &mut dyn WritableFile,
    block: &[u8],
    compression: CompressionType,
    offset: u64,
) -> crate::error::Result<BlockHandle> {
    let mut trailer = [0u8; BLOCK_TRAILER_SIZE];
    trailer[0] = compression as u8;
    trailer[1..].copy_from_slice(&trailer_crc(block, compression as u8).to_le_bytes());

    file.append(block)?;
    file.append(&trailer)?;

    Ok(BlockHandle {
        offset,
        size: block.len() as u64,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::file::mem_random_access_file::MemRandomAccessFile;
    use crate::env::file::mem_writable_file::MemWritableFile;
    use std::sync::{Arc, Mutex};

    fn new_sink() -> (Arc<Mutex<Vec<u8>>>, MemWritableFile) {
        let data = Arc::new(Mutex::new(Vec::new()));
        let file = MemWritableFile::new(Arc::clone(&data));
        (data, file)
    }

    /// Writes `blocks` back to back and returns the file bytes and the handles.
    fn write_all(blocks: &[&[u8]]) -> (Vec<u8>, Vec<BlockHandle>) {
        let (data, mut file) = new_sink();
        let mut offset = 0u64;
        let mut handles = Vec::new();
        for b in blocks {
            let h = write_block(&mut file, offset, b, CompressionType::None).unwrap();
            offset = h.offset + h.size + BLOCK_TRAILER_SIZE as u64;
            handles.push(h);
        }
        let bytes = data.lock().unwrap().clone();
        assert_eq!(bytes.len() as u64, offset);
        (bytes, handles)
    }

    fn reader(bytes: Vec<u8>) -> MemRandomAccessFile {
        MemRandomAccessFile::new(bytes)
    }

    // ---- BlockHandle ---------------------------------------------------------

    #[test]
    fn handle_round_trips_at_varint_boundaries() {
        let values = [
            0u64,
            1,
            127,
            128,
            16383,
            16384,
            u32::MAX as u64,
            1 << 56,
            u64::MAX,
        ];
        for &offset in &values {
            for &size in &values {
                let h = BlockHandle { offset, size };
                let mut buf = Vec::new();
                h.encode_to(&mut buf);
                let mut src = buf.as_slice();
                assert_eq!(BlockHandle::decode_from(&mut src).unwrap(), h);
                assert!(src.is_empty());
            }
        }
    }

    #[test]
    fn handle_decode_leaves_trailing_bytes() {
        let mut buf = Vec::new();
        BlockHandle {
            offset: 300,
            size: 7,
        }
        .encode_to(&mut buf);
        buf.extend_from_slice(b"rest");
        let mut src = buf.as_slice();
        BlockHandle::decode_from(&mut src).unwrap();
        assert_eq!(src, b"rest");
    }

    #[test]
    fn handle_decode_truncated_is_corruption() {
        let mut buf = Vec::new();
        BlockHandle {
            offset: 1 << 40,
            size: 1 << 40,
        }
        .encode_to(&mut buf);
        for len in 0..buf.len() {
            let mut src = &buf[..len];
            let err = BlockHandle::decode_from(&mut src).unwrap_err();
            assert!(err.is_corruption(), "prefix {len}");
        }
    }

    #[test]
    fn handle_decode_overlong_varint_is_corruption() {
        let src = [0x80u8; 11];
        assert!(
            BlockHandle::decode_from(&mut &src[..])
                .unwrap_err()
                .is_corruption()
        );
    }

    // ---- Footer --------------------------------------------------------------

    fn sample_footer() -> Footer {
        Footer {
            metaindex_handle: BlockHandle {
                offset: 11,
                size: 22,
            },
            index_handle: BlockHandle {
                offset: u64::MAX,
                size: 1 << 40,
            },
        }
    }

    #[test]
    fn footer_encodes_to_fixed_length_and_round_trips() {
        let mut buf = Vec::new();
        sample_footer().encode_to(&mut buf);
        assert_eq!(buf.len(), FOOTER_ENCODED_LENGTH);
        assert_eq!(&buf[32..], &TABLE_MAGIC_NUMBER.to_le_bytes());
        let mut src = buf.as_slice();
        assert_eq!(Footer::decode_from(&mut src).unwrap(), sample_footer());
        assert!(src.is_empty());
    }

    #[test]
    fn footer_decode_short_input_is_corruption() {
        let mut buf = Vec::new();
        sample_footer().encode_to(&mut buf);
        for len in 0..FOOTER_ENCODED_LENGTH {
            let err = Footer::decode_from(&mut &buf[..len]).unwrap_err();
            assert!(err.is_corruption(), "len {len}");
        }
    }

    #[test]
    fn footer_decode_wrong_magic_is_corruption() {
        let mut buf = Vec::new();
        sample_footer().encode_to(&mut buf);
        buf[39] ^= 1;
        assert!(
            Footer::decode_from(&mut buf.as_slice())
                .unwrap_err()
                .is_corruption()
        );
    }

    #[test]
    fn footer_read_from_file_with_prefix() {
        let mut bytes = vec![0xAAu8; 100];
        sample_footer().encode_to(&mut bytes);
        let size = bytes.len() as u64;
        let f = Footer::read_from(&reader(bytes), size).unwrap();
        assert_eq!(f, sample_footer());
    }

    #[test]
    fn footer_read_from_short_files_is_corruption_not_panic() {
        for size in [0u64, 1, 39] {
            let err = Footer::read_from(&reader(vec![0; size as usize]), size).unwrap_err();
            assert!(err.is_corruption(), "size {size}");
        }
        // exactly 40 bytes but not a footer
        let err = Footer::read_from(&reader(vec![0; 40]), 40).unwrap_err();
        assert!(err.is_corruption());
        // claimed size larger than the real file
        let err = Footer::read_from(&reader(vec![0; 10]), 100).unwrap_err();
        assert!(err.is_corruption());
    }

    // ---- write_block / read_block -------------------------------------------

    #[test]
    fn write_block_layout() {
        let (bytes, handles) = write_all(&[b"hello"]);
        assert_eq!(handles[0], BlockHandle { offset: 0, size: 5 });
        assert_eq!(&bytes[..5], b"hello");
        assert_eq!(bytes[5], CompressionType::None as u8);
        assert_eq!(&bytes[6..], &trailer_crc(b"hello", 0).to_le_bytes());
    }

    #[test]
    fn blocks_round_trip_including_empty_and_large() {
        let big: Vec<u8> = (0..5000u32).map(|i| (i * 31 % 251) as u8).collect();
        let blocks: [&[u8]; 4] = [b"first", b"", &big, b"last"];
        let (bytes, handles) = write_all(&blocks);
        let file = reader(bytes);
        for (b, h) in blocks.iter().zip(&handles) {
            assert_eq!(&read_block(&file, *h, true).unwrap(), b);
            assert_eq!(&read_block(&file, *h, false).unwrap(), b);
        }
        // handles are contiguous
        assert_eq!(handles[1].offset, 5 + BLOCK_TRAILER_SIZE as u64);
    }

    #[test]
    fn write_block_at_nonzero_offset_records_it() {
        let (_data, mut file) = new_sink();
        let h = write_block(&mut file, 1234, b"x", CompressionType::None).unwrap();
        assert_eq!(
            h,
            BlockHandle {
                offset: 1234,
                size: 1
            }
        );
    }

    #[test]
    fn flipping_any_stored_byte_is_detected() {
        let (bytes, handles) = write_all(&[b"some block contents"]);
        for i in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[i] ^= 0x01;
            let err = read_block(&reader(damaged), handles[0], true).unwrap_err();
            assert!(err.is_corruption(), "byte {i}");
        }
    }

    #[test]
    fn unchecked_read_still_rejects_unknown_type_byte() {
        let (mut bytes, handles) = write_all(&[b"abc"]);
        bytes[3] = 0x7F;
        let err = read_block(&reader(bytes), handles[0], false).unwrap_err();
        assert!(err.is_corruption());
    }

    #[test]
    fn unchecked_read_ignores_a_bad_crc() {
        let (mut bytes, handles) = write_all(&[b"abc"]);
        bytes[4] ^= 0xFF;
        assert_eq!(
            read_block(&reader(bytes.clone()), handles[0], false).unwrap(),
            b"abc"
        );
        assert!(
            read_block(&reader(bytes), handles[0], true)
                .unwrap_err()
                .is_corruption()
        );
    }

    #[test]
    fn truncated_file_is_corruption() {
        let (bytes, handles) = write_all(&[b"0123456789"]);
        for len in 0..bytes.len() {
            let file = reader(bytes[..len].to_vec());
            let err = read_block(&file, handles[0], true).unwrap_err();
            assert!(err.is_corruption(), "len {len}");
        }
    }

    #[test]
    fn handles_past_the_end_or_overflowing_are_corruption_not_panic() {
        let (bytes, _) = write_all(&[b"abc"]);
        let file = reader(bytes);
        let bad = [
            BlockHandle {
                offset: 1000,
                size: 3,
            },
            BlockHandle {
                offset: 0,
                size: 1000,
            },
            BlockHandle {
                offset: u64::MAX,
                size: 3,
            },
            BlockHandle {
                offset: u64::MAX - 2,
                size: 3,
            },
            BlockHandle {
                offset: 0,
                size: u64::MAX,
            },
            BlockHandle {
                offset: 0,
                size: u64::MAX - 4,
            },
            // would abort the process if it were allocated
            BlockHandle {
                offset: 0,
                size: 1 << 50,
            },
            BlockHandle {
                offset: 0,
                size: MAX_BLOCK_SIZE + 1,
            },
        ];
        for h in bad {
            let err = read_block(&file, h, true).unwrap_err();
            assert!(err.is_corruption(), "{h:?}");
        }
    }

    #[test]
    fn handle_pointing_into_the_middle_of_another_block_fails_checksum() {
        let (bytes, handles) = write_all(&[b"aaaaaaaa", b"bbbbbbbb"]);
        let shifted = BlockHandle {
            offset: handles[0].offset + 1,
            size: handles[0].size,
        };
        let err = read_block(&reader(bytes), shifted, true).unwrap_err();
        assert!(err.is_corruption());
    }

    // ---- trailer_crc ---------------------------------------------------------

    #[test]
    fn trailer_crc_depends_on_data_and_type_byte() {
        let base = trailer_crc(b"data", 0);
        assert_ne!(base, trailer_crc(b"datb", 0));
        assert_ne!(base, trailer_crc(b"data", 1));
        assert_ne!(base, trailer_crc(b"dat", 0));
        assert_eq!(base, trailer_crc(b"data", 0));
        // the type byte counts as data that follows the stored bytes
        assert_eq!(
            trailer_crc(b"data", 9),
            mask(value(&[b'd', b'a', b't', b'a', 9]))
        );
    }
}
