use super::{CompressionType, Compressor};

/// Stores bytes unchanged, so that every [`CompressionType`] has a
/// [`Compressor`] and callers need no special case for `None`.
///
/// It is reached through [`compressor_for`](super::compressor_for).
pub struct NoCompression;

impl Compressor for NoCompression {
    fn compression_type(&self) -> CompressionType {
        CompressionType::None
    }

    fn compress(&self, input: &[u8], output: &mut Vec<u8>) -> crate::Result<()> {
        output.extend_from_slice(input);
        Ok(())
    }

    fn decompress(&self, input: &[u8], output: &mut Vec<u8>) -> crate::Result<()> {
        output.extend_from_slice(input);
        Ok(())
    }
}
