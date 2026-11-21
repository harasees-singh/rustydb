use super::CompressionType;

/// One compression algorithm.
///
/// A compressor is called once per block (about 4 KB), so it is used as a trait
/// object (`&dyn Compressor`) and the call overhead does not matter. It keeps no
/// state between calls, which is why the methods take `&self` and the trait is
/// `Send + Sync`: one instance serves every thread.
pub trait Compressor: Send + Sync {
    /// The type written in the trailer of blocks this compressor produced.
    fn compression_type(&self) -> CompressionType;

    /// Appends the compressed form of `input` to `output`.
    ///
    /// `output` is appended to, not cleared, so one buffer can be reused for
    /// many blocks. The result may be larger than `input`; the caller decides
    /// whether it is worth keeping.
    ///
    /// # Errors
    /// Only if the algorithm itself fails, which is not expected for in-memory
    /// input.
    fn compress(&self, input: &[u8], output: &mut Vec<u8>) -> crate::error::Result<()>;

    /// Appends the original bytes recovered from `input` to `output`.
    ///
    /// `input` comes from a file, so it may be damaged. Implementations must
    /// check it, and must never panic or read out of bounds on bad data.
    ///
    /// # Errors
    /// [`Error::Corruption`](crate::Error::Corruption) if `input` is not valid
    /// data for this algorithm.
    fn decompress(&self, input: &[u8], output: &mut Vec<u8>) -> crate::error::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time check that the trait can be used as a trait object.
    #[allow(dead_code)]
    fn compressor_is_object_safe(_: &dyn Compressor) {}
}
