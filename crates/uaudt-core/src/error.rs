use std::fmt;

/// Result alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Error produced while parsing dictionaries, decoding or encoding values.
///
/// Boxed so that `Result<T, Error>` stays pointer-sized on the hot paths;
/// inspect it with [`Error::kind`].
pub struct Error(Box<ErrorKind>);

impl Error {
    /// The kind of error.
    #[inline]
    pub fn kind(&self) -> &ErrorKind {
        &self.0
    }

    /// Consumes the error, returning its kind.
    pub fn into_kind(self) -> ErrorKind {
        *self.0
    }

    #[inline]
    pub(crate) fn kind_mut(&mut self) -> &mut ErrorKind {
        &mut self.0
    }

    #[cold]
    pub(crate) fn path(path: &str, reason: impl fmt::Display) -> Self {
        ErrorKind::InvalidPath {
            path: path.to_owned(),
            reason: reason.to_string(),
        }
        .into()
    }
}

impl From<ErrorKind> for Error {
    #[cold]
    #[inline(never)]
    fn from(kind: ErrorKind) -> Self {
        Error(Box::new(kind))
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for Error {}

/// Kinds of [`Error`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The XML type dictionary could not be parsed.
    #[error("invalid type dictionary: {0}")]
    Xml(String),

    /// A structured type name was not found in the dictionary.
    #[error("structured type '{0}' not found")]
    UnknownStruct(String),

    /// A field refers to a type that is neither a supported builtin nor a type of the dictionary.
    #[error("field '{field}' of '{owner}' has unsupported or unknown type '{ty}'")]
    UnresolvedType {
        owner: String,
        field: String,
        ty: String,
    },

    /// A structured type uses a feature not supported by the codec (optional fields, bit fields...).
    #[error("structured type '{name}' is not supported: {reason}")]
    Unsupported { name: String, reason: &'static str },

    /// The buffer ended before the value was fully decoded.
    #[error("unexpected end of buffer at offset {offset} (needed {needed} more bytes)")]
    UnexpectedEof { offset: usize, needed: usize },

    /// A string field does not contain valid UTF-8.
    #[error("invalid UTF-8 in string at offset {0}")]
    InvalidUtf8(usize),

    /// A negative length (other than -1 = null) or an absurd length was read.
    #[error("invalid length {len} at offset {offset}")]
    InvalidLength { offset: usize, len: i32 },

    /// Nesting is deeper than [`MAX_DEPTH`](crate::MAX_DEPTH) (recursive or malicious data).
    #[error("maximum nesting depth exceeded")]
    DepthExceeded,

    /// A value does not match the type expected by the schema.
    #[error("type mismatch at '{path}': expected {expected}, got {got}")]
    TypeMismatch {
        path: String,
        expected: String,
        got: &'static str,
    },

    /// An enumeration name is not part of the enumeration.
    #[error("invalid value '{value}' for enumeration '{enumeration}'")]
    InvalidEnum { enumeration: String, value: String },

    /// A path expression is malformed or does not match the schema.
    #[error("invalid path '{path}': {reason}")]
    InvalidPath { path: String, reason: String },

    /// An array index is out of range.
    #[error("index {index} out of range (len {len}) in path '{path}'")]
    IndexOutOfRange {
        path: String,
        index: usize,
        len: usize,
    },
}
