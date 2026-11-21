/// How a block's bytes are stored on disk.
///
/// The value is written as one byte in the trailer of every block, so the
/// numbers are part of the file format: **never change or reuse one**, even for
/// an algorithm that is later removed. A new algorithm gets the next unused
/// number.
///
/// A variant existing here does not mean the algorithm is built in. Decoding
/// needs a [`Compressor`](crate::compression::Compressor), which
/// [`compressor_for`](crate::compression::compressor_for) provides.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum CompressionType {
    /// Stored as is.
    None = 0,
    // Snappy = 1,
    // Zstd = 2,
}

impl CompressionType {
    fn from_u8(val: u8) -> Option<CompressionType> {
        match val {
            0 => Some(CompressionType::None),
            _ => None,
        }
    }
}