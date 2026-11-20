//! Compiled representation of an OPC-UA binary type dictionary (`opc:TypeDictionary`).
//!
//! Type names are resolved to integer ids once, at parse time, so that the
//! decode/encode hot paths never perform string lookups.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::error::{Error, ErrorKind, Result};

/// Index of a structured type inside a [`TypeDictionary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StructId(pub(crate) u32);

/// Index of an enumerated type inside a [`TypeDictionary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EnumId(pub(crate) u32);

impl StructId {
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl EnumId {
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// OPC-UA builtin types supported by the binary codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    Boolean,
    SByte,
    Byte,
    Int16,
    UInt16,
    Int32,
    UInt32,
    Int64,
    UInt64,
    Float,
    Double,
    /// UTF-8 string prefixed by an Int32 length (-1 = null). Also used for `CharArray`.
    String,
    /// Int64, 100 ns ticks since 1601-01-01 UTC.
    DateTime,
    Guid,
    /// Raw bytes prefixed by an Int32 length (-1 = null). Also used for `XmlElement`.
    ByteString,
    StatusCode,
}

impl Builtin {
    /// Maps an `opc:` type name to a builtin.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "Boolean" => Builtin::Boolean,
            "SByte" | "Char" => Builtin::SByte,
            "Byte" => Builtin::Byte,
            "Int16" | "short" => Builtin::Int16,
            "UInt16" => Builtin::UInt16,
            "Int32" => Builtin::Int32,
            "UInt32" => Builtin::UInt32,
            "Int64" => Builtin::Int64,
            "UInt64" => Builtin::UInt64,
            "Float" => Builtin::Float,
            "Double" => Builtin::Double,
            "String" | "CharArray" => Builtin::String,
            "DateTime" => Builtin::DateTime,
            "Guid" => Builtin::Guid,
            "ByteString" | "XmlElement" => Builtin::ByteString,
            "StatusCode" => Builtin::StatusCode,
            _ => return None,
        })
    }

    /// Encoded size in bytes, `None` for variable-length types.
    #[inline]
    pub const fn fixed_size(self) -> Option<usize> {
        match self {
            Builtin::Boolean | Builtin::SByte | Builtin::Byte => Some(1),
            Builtin::Int16 | Builtin::UInt16 => Some(2),
            Builtin::Int32 | Builtin::UInt32 | Builtin::Float | Builtin::StatusCode => Some(4),
            Builtin::Int64 | Builtin::UInt64 | Builtin::Double | Builtin::DateTime => Some(8),
            Builtin::Guid => Some(16),
            Builtin::String | Builtin::ByteString => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Builtin::Boolean => "Boolean",
            Builtin::SByte => "SByte",
            Builtin::Byte => "Byte",
            Builtin::Int16 => "Int16",
            Builtin::UInt16 => "UInt16",
            Builtin::Int32 => "Int32",
            Builtin::UInt32 => "UInt32",
            Builtin::Int64 => "Int64",
            Builtin::UInt64 => "UInt64",
            Builtin::Float => "Float",
            Builtin::Double => "Double",
            Builtin::String => "String",
            Builtin::DateTime => "DateTime",
            Builtin::Guid => "Guid",
            Builtin::ByteString => "ByteString",
            Builtin::StatusCode => "StatusCode",
        }
    }
}

/// Resolved type of a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldType {
    Builtin(Builtin),
    Struct(StructId),
    Enum(EnumId),
    /// Type not available in this dictionary; decoding/encoding it fails.
    Unresolved(Box<str>),
}

/// A field of a structured type (length fields are folded into `is_array`).
#[derive(Debug, Clone)]
pub struct Field {
    pub name: Box<str>,
    pub ty: FieldType,
    pub is_array: bool,
}

/// A structured type (`opc:StructuredType`).
#[derive(Debug, Clone)]
pub struct StructType {
    pub name: Box<str>,
    pub fields: Box<[Field]>,
    /// Encoded size if every field has a fixed size.
    pub(crate) fixed_size: Option<usize>,
    /// Byte offset of each field from the start of the struct, as long as all
    /// preceding fields are fixed-size (lets path lookups jump directly).
    pub(crate) fixed_offsets: Box<[usize]>,
    /// Set when the type uses features the codec does not support.
    pub(crate) unsupported: Option<&'static str>,
}

impl StructType {
    /// Position of a field by name.
    #[inline]
    pub fn field_index(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| &*f.name == name)
    }

    /// Encoded size in bytes when the type is fixed-size.
    #[inline]
    pub fn fixed_size(&self) -> Option<usize> {
        self.fixed_size
    }
}

/// An enumerated type (`opc:EnumeratedType`).
#[derive(Debug, Clone)]
pub struct EnumType {
    pub name: Box<str>,
    pub values: Box<[(Box<str>, i32)]>,
    /// Encoded size in bytes (1, 2 or 4; `LengthInBits`, default 32), or 0
    /// for bit-sized enumerations, which are not supported.
    pub size: u8,
}

impl EnumType {
    pub fn name_of(&self, value: i32) -> Option<&str> {
        self.values
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(n, _)| &**n)
    }

    pub fn value_of(&self, name: &str) -> Option<i32> {
        self.values
            .iter()
            .find(|(n, _)| &**n == name)
            .map(|(_, v)| *v)
    }
}

/// A parsed and compiled OPC-UA binary type dictionary.
#[derive(Debug, Clone, Default)]
pub struct TypeDictionary {
    pub(crate) structs: Vec<StructType>,
    pub(crate) enums: Vec<EnumType>,
    struct_index: HashMap<Box<str>, StructId>,
    enum_index: HashMap<Box<str>, EnumId>,
}

// ── Raw (unresolved) parse output ───────────────────────────────────────

struct RawField {
    name: String,
    type_name: String,
    length_field: Option<String>,
    switch: bool,
}

/// (name, size in bytes, values) of the enumeration being parsed.
type RawEnum = (String, u8, Vec<(Box<str>, i32)>);

struct RawStruct {
    name: String,
    fields: Vec<RawField>,
}

/// Strips a namespace prefix: `tns:MyType` -> `MyType`.
#[inline]
fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, n)| n)
}

fn attr(e: &BytesStart<'_>, key: &[u8]) -> Result<Option<String>> {
    for a in e.attributes() {
        let a = a.map_err(|err| ErrorKind::Xml(err.to_string()))?;
        if a.key.local_name().as_ref() == key {
            let v = a
                .unescape_value()
                .map_err(|err| ErrorKind::Xml(err.to_string()))?;
            return Ok(Some(v.into_owned()));
        }
    }
    Ok(None)
}

fn req_attr(e: &BytesStart<'_>, key: &[u8]) -> Result<String> {
    attr(e, key)?.ok_or_else(|| {
        ErrorKind::Xml(format!(
            "<{}> without '{}' attribute",
            String::from_utf8_lossy(e.local_name().as_ref()),
            String::from_utf8_lossy(key)
        ))
        .into()
    })
}

impl TypeDictionary {
    /// Parses an XML binary type dictionary (the value of a node under `i=93`).
    pub fn parse(xml: &[u8]) -> Result<Self> {
        let (raw_structs, raw_enums) = Self::parse_raw(xml)?;
        Self::compile(raw_structs, raw_enums)
    }

    fn parse_raw(xml: &[u8]) -> Result<(Vec<RawStruct>, Vec<EnumType>)> {
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(true);

        let mut structs: Vec<RawStruct> = Vec::new();
        let mut enums: Vec<EnumType> = Vec::new();
        let mut cur_struct: Option<RawStruct> = None;
        let mut cur_enum: Option<RawEnum> = None;
        let mut buf = Vec::with_capacity(256);

        loop {
            let ev = reader
                .read_event_into(&mut buf)
                .map_err(|e| ErrorKind::Xml(format!("at {}: {e}", reader.buffer_position())))?;
            match ev {
                Event::Start(ref e) | Event::Empty(ref e) => {
                    let empty = matches!(ev, Event::Empty(_));
                    match e.local_name().as_ref() {
                        b"StructuredType" => {
                            let s = RawStruct {
                                name: req_attr(e, b"Name")?,
                                fields: Vec::new(),
                            };
                            if empty {
                                structs.push(s);
                            } else {
                                cur_struct = Some(s);
                            }
                        }
                        b"Field" => {
                            if let Some(s) = cur_struct.as_mut() {
                                s.fields.push(RawField {
                                    name: req_attr(e, b"Name")?,
                                    type_name: req_attr(e, b"TypeName")?,
                                    length_field: attr(e, b"LengthField")?,
                                    switch: attr(e, b"SwitchField")?.is_some(),
                                });
                            }
                        }
                        b"EnumeratedType" => {
                            let name = req_attr(e, b"Name")?;
                            let bits: u32 = match attr(e, b"LengthInBits")? {
                                Some(b) => b.trim().parse().map_err(|_| {
                                    Error::from(ErrorKind::Xml(format!(
                                        "enum '{name}': invalid LengthInBits '{b}'"
                                    )))
                                })?,
                                None => 32,
                            };
                            // Bit-sized enums (e.g. NodeIdType, 6 bits) only appear inside
                            // bit-packed structures: size 0 marks them unsupported.
                            let size = match bits {
                                8 => 1,
                                16 => 2,
                                32 => 4,
                                _ => 0,
                            };
                            if empty {
                                enums.push(EnumType {
                                    name: name.into(),
                                    values: Box::new([]),
                                    size,
                                });
                            } else {
                                cur_enum = Some((name, size, Vec::new()));
                            }
                        }
                        b"EnumeratedValue" => {
                            if let Some((ename, _, values)) = cur_enum.as_mut() {
                                let name = req_attr(e, b"Name")?;
                                let raw = req_attr(e, b"Value")?;
                                let v: i32 = local(raw.trim()).parse().map_err(|_| {
                                    Error::from(ErrorKind::Xml(format!(
                                        "enum '{ename}': invalid value '{raw}'"
                                    )))
                                })?;
                                values.push((name.into(), v));
                            }
                        }
                        _ => {}
                    }
                }
                Event::End(ref e) => match e.local_name().as_ref() {
                    b"StructuredType" => structs.extend(cur_struct.take()),
                    b"EnumeratedType" => {
                        if let Some((name, size, values)) = cur_enum.take() {
                            enums.push(EnumType {
                                name: name.into(),
                                values: values.into_boxed_slice(),
                                size,
                            });
                        }
                    }
                    _ => {}
                },
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok((structs, enums))
    }

    fn compile(raw_structs: Vec<RawStruct>, enums: Vec<EnumType>) -> Result<Self> {
        let mut struct_index = HashMap::with_capacity(raw_structs.len());
        for (i, s) in raw_structs.iter().enumerate() {
            struct_index
                .entry(s.name.as_str().into())
                .or_insert(StructId(i as u32));
        }
        let mut enum_index = HashMap::with_capacity(enums.len());
        for (i, e) in enums.iter().enumerate() {
            enum_index.entry(e.name.clone()).or_insert(EnumId(i as u32));
        }

        let resolve = |type_name: &str| -> FieldType {
            let n = local(type_name);
            if let Some(b) = Builtin::from_name(n) {
                return FieldType::Builtin(b);
            }
            if let Some(&id) = struct_index.get(n) {
                return FieldType::Struct(id);
            }
            if let Some(&id) = enum_index.get(n) {
                return FieldType::Enum(id);
            }
            FieldType::Unresolved(n.into())
        };

        let mut structs = Vec::with_capacity(raw_structs.len());
        for rs in raw_structs {
            let length_fields: Vec<&str> = rs
                .fields
                .iter()
                .filter_map(|f| f.length_field.as_deref())
                .collect();
            let mut unsupported = None;
            let mut fields = Vec::with_capacity(rs.fields.len());
            for f in &rs.fields {
                if f.switch {
                    unsupported = Some("optional fields (SwitchField)");
                }
                if local(&f.type_name) == "Bit" {
                    unsupported = Some("bit fields");
                }
                if let FieldType::Enum(id) = resolve(&f.type_name) {
                    if enums[id.index()].size == 0 {
                        unsupported = Some("bit-sized enumerations");
                    }
                }
                if length_fields.contains(&f.name.as_str()) {
                    continue;
                }
                fields.push(Field {
                    name: f.name.as_str().into(),
                    ty: resolve(&f.type_name),
                    is_array: f.length_field.is_some(),
                });
            }
            structs.push(StructType {
                name: rs.name.into(),
                fields: fields.into_boxed_slice(),
                fixed_size: None,
                fixed_offsets: Box::new([]),
                unsupported,
            });
        }

        let mut dict = TypeDictionary {
            structs,
            enums,
            struct_index,
            enum_index,
        };
        dict.compute_layouts();
        Ok(dict)
    }

    /// Precomputes fixed sizes and fixed field offsets.
    fn compute_layouts(&mut self) {
        #[derive(Clone, Copy)]
        enum State {
            Todo,
            Visiting,
            Done(Option<usize>),
        }

        fn size_of(dict: &TypeDictionary, id: usize, states: &mut [State]) -> Option<usize> {
            match states[id] {
                State::Done(s) => return s,
                State::Visiting => return None, // recursive type: variable size
                State::Todo => {}
            }
            states[id] = State::Visiting;
            let mut total = Some(0usize);
            for f in dict.structs[id].fields.iter() {
                let fs = if f.is_array {
                    None
                } else {
                    field_size(dict, &f.ty, states)
                };
                total = match (total, fs) {
                    (Some(t), Some(s)) => Some(t + s),
                    _ => None,
                };
            }
            states[id] = State::Done(total);
            total
        }

        fn field_size(
            dict: &TypeDictionary,
            ty: &FieldType,
            states: &mut [State],
        ) -> Option<usize> {
            match ty {
                FieldType::Builtin(b) => b.fixed_size(),
                FieldType::Enum(e) => match dict.enums[e.index()].size {
                    0 => None,
                    n => Some(n as usize),
                },
                FieldType::Struct(s) => size_of(dict, s.index(), states),
                FieldType::Unresolved(_) => None,
            }
        }

        let n = self.structs.len();
        let mut states = vec![State::Todo; n];
        for id in 0..n {
            size_of(self, id, &mut states);
        }
        for id in 0..n {
            let fixed_size = match states[id] {
                State::Done(s) => s,
                _ => None,
            };
            let mut offsets = Vec::new();
            let mut off = 0usize;
            for f in self.structs[id].fields.iter() {
                offsets.push(off);
                let fs = if f.is_array {
                    None
                } else {
                    field_size(self, &f.ty, &mut states)
                };
                match fs {
                    Some(s) => off += s,
                    None => break,
                }
            }
            let s = &mut self.structs[id];
            s.fixed_size = if s.unsupported.is_some() {
                None
            } else {
                fixed_size
            };
            s.fixed_offsets = offsets.into_boxed_slice();
        }
    }

    // ── Lookups ─────────────────────────────────────────────────────────

    /// Id of a structured type by name (namespace prefix ignored).
    #[inline]
    pub fn struct_id(&self, name: &str) -> Option<StructId> {
        self.struct_index.get(local(name)).copied()
    }

    /// Id of a structured type by name, or [`ErrorKind::UnknownStruct.into()`].
    pub fn require_struct(&self, name: &str) -> Result<StructId> {
        self.struct_id(name)
            .ok_or_else(|| ErrorKind::UnknownStruct(name.to_owned()).into())
    }

    /// Id of an enumerated type by name (namespace prefix ignored).
    #[inline]
    pub fn enum_id(&self, name: &str) -> Option<EnumId> {
        self.enum_index.get(local(name)).copied()
    }

    #[inline]
    pub fn struct_type(&self, id: StructId) -> &StructType {
        &self.structs[id.index()]
    }

    #[inline]
    pub fn enum_type(&self, id: EnumId) -> &EnumType {
        &self.enums[id.index()]
    }

    /// All structured types, indexed by [`StructId`].
    pub fn structs(&self) -> &[StructType] {
        &self.structs
    }

    /// All enumerated types, indexed by [`EnumId`].
    pub fn enums(&self) -> &[EnumType] {
        &self.enums
    }

    /// Human readable name of a field type.
    pub fn type_name<'a>(&'a self, ty: &'a FieldType) -> &'a str {
        match ty {
            FieldType::Builtin(b) => b.name(),
            FieldType::Struct(id) => &self.struct_type(*id).name,
            FieldType::Enum(id) => &self.enum_type(*id).name,
            FieldType::Unresolved(n) => n,
        }
    }
}
