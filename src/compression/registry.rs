use super::no_compression::NoCompression;
use super::{CompressionType, Compressor};

/// The compressor that decodes blocks stored as `t`.
///
/// This is the one place that lists which algorithms exist. The `match` is
/// exhaustive, so adding a [`CompressionType`] variant without an arm here does
/// not compile.
///
/// # Errors
/// [`Error::NotSupported`](crate::Error::NotSupported) for a type whose
/// algorithm was not compiled into this build, so reading such a table fails
/// with a clear message instead of returning garbage. It cannot happen yet,
/// because `None` is the only type.
pub fn compressor_for(t: CompressionType) -> crate::error::Result<&'static dyn Compressor> {
    match t {
        CompressionType::None => Ok(&NoCompression),
        // CompressionType::Snappy => Ok(&SnappyCompressor),   // behind #[cfg(feature = "snappy")]
        // CompressionType::Zstd => Ok(&ZstdCompressor),   // behind #[cfg(feature = "zstd")]
    }
}
