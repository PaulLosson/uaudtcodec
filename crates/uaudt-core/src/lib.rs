//! High-performance OPC-UA structured type (UDT) codec.
//!
//! Parses an OPC-UA binary type dictionary (XML, `opc:TypeDictionary`) into a
//! compiled [`TypeDictionary`], then decodes/encodes binary bodies of
//! `ExtensionObject`s into dynamic [`Value`]s.
//!
//! ```
//! use uaudt_core::{TypeDictionary, Value};
//!
//! let xml = br#"<opc:TypeDictionary xmlns:opc="http://opcfoundation.org/BinarySchema/" xmlns:tns="urn:demo">
//!   <opc:StructuredType Name="Point">
//!     <opc:Field Name="X" TypeName="opc:Float"/>
//!     <opc:Field Name="Label" TypeName="opc:String"/>
//!   </opc:StructuredType>
//! </opc:TypeDictionary>"#;
//!
//! let dict = TypeDictionary::parse(xml)?;
//! let point = dict.require_struct("Point")?;
//!
//! let mut v = dict.default_struct(point)?;
//! dict.set_str(&mut v, "X", 1.5f32.into())?;
//! dict.set_str(&mut v, "Label", "A".into())?;
//!
//! let bytes = dict.encode(&v)?;
//! assert_eq!(dict.decode(point, &bytes)?, v);
//!
//! // Rewrite a single field directly in the encoded bytes.
//! let label = dict.compile_path(point, "Label")?;
//! let patched = dict.patch(&bytes, &[(&label, "Hello".into())])?;
//! assert_eq!(dict.read_path(&patched, &label)?, Value::from("Hello"));
//! # Ok::<(), uaudt_core::Error>(())
//! ```

mod codec;
pub mod datetime;
mod error;
mod path;
mod schema;
mod value;

pub use compact_str::CompactString;
pub use error::{Error, ErrorKind, Result};
pub use path::Path;
pub use schema::{
    Builtin, EnumId, EnumType, Field, FieldType, StructId, StructType, TypeDictionary,
};
pub use value::{DisplayValue, EnumValue, StructValue, Value};

/// Maximum nesting depth accepted when decoding/encoding.
pub const MAX_DEPTH: usize = 64;
