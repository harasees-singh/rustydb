use std::sync::Arc;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Error {
    NotFound(String),
    Corruption(String),
    NotSupported(String),
    InvalidArgument(String),
    IO {
        context: String,
        source: Arc<std::io::Error>,
    },
}
