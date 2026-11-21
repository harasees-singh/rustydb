use crate::compression::Compressor;
use crate::compression::enums::CompressionType;

pub struct NoCompression;

impl Compressor for NoCompression {
    fn compression_type(&self) -> CompressionType {
        CompressionType::None
    }

    fn compress(&self, input: &[u8], output: &mut Vec<u8>) -> crate::Result<()> {
        output.extend_from_slice(&input);
        Ok(())
    }

    fn decompress(&self, input: &[u8], output: &mut Vec<u8>) -> crate::Result<()> {
        output.extend_from_slice(&input);
        Ok(())
    }
}