use crate::compression::Compressor;
use crate::compression::enums::CompressionType;
use crate::compression::no_compression::NoCompression;

pub fn compressor_for(t: CompressionType) -> crate::error::Result<&'static dyn Compressor> {
    match t {
        CompressionType::None => Ok(&NoCompression),
        // CompressionType::Snappy => Ok(&SnappyCompressor),   // behind #[cfg(feature = "snappy")]
        // CompressionType::Zstd => Ok(&ZstdCompressor),   // behind #[cfg(feature = "zstd")]
    }
}
