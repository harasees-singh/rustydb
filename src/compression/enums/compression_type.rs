#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum CompressionType {
    None = 0,
    // Zstd,
    // Snappy
}
