use std::sync::Arc;

use opcua::types::StatusCode;

/// Result alias of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors of the UDT client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A service call failed (transport, session, encoding...).
    #[error("OPC-UA request failed: {0}")]
    Opcua(#[from] opcua::types::Error),

    /// The server answered with a bad status code for an operation.
    #[error("{context}: {status}")]
    BadStatus { status: StatusCode, context: String },

    /// Decoding or encoding a structured value failed.
    #[error(transparent)]
    Codec(#[from] uaudt_core::Error),

    /// A type dictionary found on the server (or in the cache) is invalid.
    #[error("type dictionary '{name}': {source}")]
    Dictionary {
        name: String,
        source: Arc<uaudt_core::Error>,
    },

    /// No dictionary contains the requested structured type.
    #[error("no type dictionary contains structured type '{0}'")]
    TypeNotFound(String),

    /// The server returned something unexpected.
    #[error("unexpected server data: {0}")]
    Unexpected(String),

    /// An element index is out of range.
    #[error("index {index} out of range ({len} elements)")]
    IndexOutOfRange { index: usize, len: usize },

    /// Reading or writing the types cache failed.
    #[error("types cache: {0}")]
    Io(#[from] std::io::Error),

    /// The types cache is malformed.
    #[error("types cache: {0}")]
    Cache(String),
}

impl Error {
    pub(crate) fn unexpected(msg: impl Into<String>) -> Self {
        Error::Unexpected(msg.into())
    }
}

/// Fails with [`Error::BadStatus`] unless `status` is good.
#[inline]
pub(crate) fn check(status: StatusCode, context: impl FnOnce() -> String) -> Result<()> {
    if status.is_good() {
        Ok(())
    } else {
        Err(Error::BadStatus {
            status,
            context: context(),
        })
    }
}
