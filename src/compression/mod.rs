//! Block compression: how a block's bytes are stored on disk, and the code that
//! converts between the stored and the original form.
//!
//! Every block in a table ends with a one-byte [`CompressionType`]. Reading
//! looks the byte up with [`compressor_for`] and decodes with the returned
//! [`Compressor`]. To add an algorithm, add a [`CompressionType`] variant,
//! implement [`Compressor`] in its own file, and add an arm to
//! [`compressor_for`].

mod enums;
mod no_compression;
mod registry;
mod traits;

pub use enums::CompressionType;
pub use registry::compressor_for;
pub use traits::Compressor;
