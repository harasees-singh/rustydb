use crate::compression::r#enum::compression_type::CompressionType;

pub trait Compressor: Send + Sync {
    fn compression_type(&self) -> CompressionType;
    fn compress(&self, input: &[u8], output: &mut Vec<u8>) -> crate::error::Result<()>;
    fn decompress(&self, input: &[u8], output: &mut Vec<u8>) -> crate::error::Result<()>;
}