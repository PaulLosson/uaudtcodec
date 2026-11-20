//! Read and write OPC-UA structured types (UDTs) through
//! [async-opcua](https://crates.io/crates/async-opcua), decoded with
//! [`uaudt_core`].
//!
//! ```no_run
//! use std::sync::Arc;
//! use uaudt_client::{TypeSystem, UdtNode};
//!
//! # async fn demo(session: Arc<opcua::client::Session>) -> uaudt_client::Result<()> {
//! // Discover the type dictionaries once (or load them from a cache file).
//! let types = TypeSystem::load_or_browse(&session, "types.json").await?;
//!
//! let mut node = UdtNode::open(session.clone(), "ns=4;s=MyNode.Data".parse::<opcua::types::NodeId>().unwrap(), &types).await?;
//! let items = node.read().await?;
//! println!("{}", uaudt_core::Value::Struct(items[0].clone()).display(node.dictionary()));
//!
//! // Partial update: only the targeted bytes are re-encoded.
//! node.patch(1, &[("Name", "Updated".into()), ("SubItems[1].Description", "Modified".into())]).await?;
//!
//! // Edit a decoded copy.
//! node.edit(0, |dict, v| dict.set_str(v, "Name", "Edited".into())).await?;
//! # Ok(()) }
//! ```

mod error;
mod node;
mod services;
mod type_system;

pub use error::{Error, Result};
pub use node::{RawBody, UdtNode, WriteMode};
pub use type_system::{NodeKey, TypeRef, TypeSystem};

/// Raises async-opcua's decoding limits to values suited to UDT traffic.
///
/// The defaults (64 KiB byte strings, 1 000-element arrays, ~320 KiB
/// messages) are too small for type dictionaries and large UDT arrays: the
/// client closes the connection (`BadConnectionClosed`) when one is exceeded.
pub fn with_udt_limits(builder: opcua::client::ClientBuilder) -> opcua::client::ClientBuilder {
    builder
        .max_message_size(256 * 1024 * 1024)
        .max_chunk_count(8192)
        .max_byte_string_length(64 * 1024 * 1024)
        .max_string_length(16 * 1024 * 1024)
        .max_array_length(1_000_000)
}

/// Re-export of async-opcua, to build sessions with matching versions.
pub use opcua;
/// Re-export of the codec crate.
pub use uaudt_core;
