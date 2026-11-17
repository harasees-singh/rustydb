//! The error type shared by the whole crate.
//!
//! LevelDB reports every failure through a `Status` value. Here success is
//! `Ok(..)` in a [`Result`], and a failure is one of the [`Error`] variants,
//! which correspond to LevelDB's status codes.

use std::fmt;
use std::io;
use std::sync::Arc;

/// `Result` with this crate's [`Error`] as the error type.
pub type Result<T> = std::result::Result<T, Error>;

/// Why an operation failed.
///
/// `Error` is `Clone`, because the database keeps its first background error
/// and hands a copy of it to every later caller. The I/O error inside is shared
/// through an `Arc`, since [`io::Error`] cannot be cloned.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Error {
    /// A key, file or other item that was asked for does not exist.
    NotFound(String),
    /// Stored data is damaged or does not have the expected format.
    Corruption(String),
    /// The operation or option is not supported.
    NotSupported(String),
    /// The caller passed something invalid.
    InvalidArgument(String),
    /// An operating system or file error. `context` says what was being done
    /// (it may be empty).
    Io {
        context: String,
        source: Arc<io::Error>,
    },
}

impl Error {
    pub fn not_found(msg: impl Into<String>) -> Self {
        Error::NotFound(msg.into())
    }

    pub fn corruption(msg: impl Into<String>) -> Self {
        Error::Corruption(msg.into())
    }

    pub fn not_supported(msg: impl Into<String>) -> Self {
        Error::NotSupported(msg.into())
    }

    pub fn invalid_argument(msg: impl Into<String>) -> Self {
        Error::InvalidArgument(msg.into())
    }

    /// Wraps an I/O error, with a description of what was being done, such as
    /// `"reading 000012.ldb"`.
    ///
    /// A missing file or directory ([`io::ErrorKind::NotFound`]) becomes
    /// [`Error::NotFound`], as in LevelDB, because callers branch on it (for
    /// example "is there a CURRENT file?"). Every other kind becomes
    /// [`Error::Io`].
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        let context = context.into();
        if source.kind() == io::ErrorKind::NotFound {
            let msg = if context.is_empty() {
                source.to_string()
            } else {
                format!("{context}: {source}")
            };
            return Error::NotFound(msg);
        }
        Error::Io {
            context,
            source: Arc::new(source),
        }
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, Error::NotFound(_))
    }

    pub fn is_corruption(&self) -> bool {
        matches!(self, Error::Corruption(_))
    }

    pub fn is_not_supported(&self) -> bool {
        matches!(self, Error::NotSupported(_))
    }

    pub fn is_invalid_argument(&self) -> bool {
        matches!(self, Error::InvalidArgument(_))
    }

    pub fn is_io_error(&self) -> bool {
        matches!(self, Error::Io { .. })
    }
}

/// The text follows LevelDB's `Status::ToString`.
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(msg) => write!(f, "NotFound: {msg}"),
            Error::Corruption(msg) => write!(f, "Corruption: {msg}"),
            Error::NotSupported(msg) => write!(f, "Not implemented: {msg}"),
            Error::InvalidArgument(msg) => write!(f, "Invalid argument: {msg}"),
            Error::Io { context, source } if context.is_empty() => {
                write!(f, "IO error: {source}")
            }
            Error::Io { context, source } => write!(f, "IO error: {context}: {source}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(&**source),
            _ => None,
        }
    }
}

/// Lets `?` convert an [`io::Error`], with no context. Use [`Error::io`] where
/// the file name or the operation is known.
impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        Error::io("", source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    fn io_err(kind: io::ErrorKind, msg: &str) -> io::Error {
        io::Error::new(kind, msg)
    }

    #[test]
    fn display_matches_leveldb_status_text() {
        assert_eq!(Error::not_found("k").to_string(), "NotFound: k");
        assert_eq!(
            Error::corruption("bad block").to_string(),
            "Corruption: bad block"
        );
        assert_eq!(
            Error::not_supported("snappy").to_string(),
            "Not implemented: snappy"
        );
        assert_eq!(
            Error::invalid_argument("empty key").to_string(),
            "Invalid argument: empty key"
        );
    }

    #[test]
    fn display_of_an_io_error_includes_the_context_when_there_is_one() {
        let e = Error::io(
            "reading 000012.ldb",
            io_err(io::ErrorKind::Other, "disk on fire"),
        );
        assert_eq!(e.to_string(), "IO error: reading 000012.ldb: disk on fire");

        let e = Error::io("", io_err(io::ErrorKind::Other, "disk on fire"));
        assert_eq!(e.to_string(), "IO error: disk on fire");
    }

    #[test]
    fn constructors_accept_str_and_string() {
        assert_eq!(Error::corruption("a").to_string(), "Corruption: a");
        assert_eq!(
            Error::corruption(format!("a {}", 1)).to_string(),
            "Corruption: a 1"
        );
    }

    #[test]
    fn each_predicate_is_true_for_its_own_variant_only() {
        let all = [
            Error::not_found("x"),
            Error::corruption("x"),
            Error::not_supported("x"),
            Error::invalid_argument("x"),
            Error::io("x", io_err(io::ErrorKind::Other, "x")),
        ];
        for (i, e) in all.iter().enumerate() {
            let flags = [
                e.is_not_found(),
                e.is_corruption(),
                e.is_not_supported(),
                e.is_invalid_argument(),
                e.is_io_error(),
            ];
            for (j, flag) in flags.iter().enumerate() {
                assert_eq!(*flag, i == j, "error {i}, predicate {j}");
            }
        }
    }

    #[test]
    fn a_missing_file_becomes_not_found() {
        let e = Error::io(
            "opening CURRENT",
            io_err(io::ErrorKind::NotFound, "no such file"),
        );
        assert!(e.is_not_found());
        assert!(!e.is_io_error());
        assert_eq!(e.to_string(), "NotFound: opening CURRENT: no such file");

        let e: Error = io_err(io::ErrorKind::NotFound, "no such file").into();
        assert_eq!(e.to_string(), "NotFound: no such file");
    }

    #[test]
    fn other_io_errors_keep_their_kind_and_are_the_source() {
        let e = Error::io("writing", io_err(io::ErrorKind::PermissionDenied, "denied"));
        assert!(e.is_io_error());
        let source = e.source().expect("an Io error has a source");
        let io = source.downcast_ref::<io::Error>().unwrap();
        assert_eq!(io.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn errors_without_an_io_cause_have_no_source() {
        assert!(Error::corruption("x").source().is_none());
        assert!(Error::not_found("x").source().is_none());
    }

    #[test]
    fn clones_are_equal_in_content_and_share_the_io_error() {
        let e = Error::io("ctx", io_err(io::ErrorKind::Other, "boom"));
        let c = e.clone();
        assert_eq!(e.to_string(), c.to_string());
        match (&e, &c) {
            (Error::Io { source: a, .. }, Error::Io { source: b, .. }) => {
                assert!(Arc::ptr_eq(a, b), "the clone shares the same io::Error");
            }
            _ => panic!("expected Io errors"),
        }
        assert_eq!(Error::corruption("x").clone().to_string(), "Corruption: x");
    }

    #[test]
    fn question_mark_converts_an_io_error() {
        fn open() -> Result<()> {
            Err(io_err(io::ErrorKind::UnexpectedEof, "short read"))?;
            Ok(())
        }
        let e = open().unwrap_err();
        assert!(e.is_io_error());
        assert_eq!(e.to_string(), "IO error: short read");
    }

    #[test]
    fn works_as_a_boxed_std_error() {
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(Error::corruption("bad"));
        assert_eq!(boxed.to_string(), "Corruption: bad");
    }

    #[test]
    fn error_is_send_sync_and_static() {
        fn assert_traits<T: Send + Sync + 'static>() {}
        assert_traits::<Error>();
    }

    #[test]
    fn debug_output_names_the_variant() {
        let s = format!("{:?}", Error::corruption("bad"));
        assert!(s.contains("Corruption") && s.contains("bad"), "{s}");
    }
}
