//! Dynamic value model for decoded structured types.
//!
//! Struct fields are stored positionally (in schema order); field names live
//! once in the [`TypeDictionary`], never per instance.

use std::fmt;

use compact_str::CompactString;

use crate::error::{Error, ErrorKind, Result};
use crate::schema::{Builtin, EnumId, FieldType, StructId, TypeDictionary};

/// A decoded OPC-UA value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Boolean(bool),
    SByte(i8),
    Byte(u8),
    Int16(i16),
    UInt16(u16),
    Int32(i32),
    UInt32(u32),
    Int64(i64),
    UInt64(u64),
    Float(f32),
    Double(f64),
    /// `None` = null string (length -1 on the wire). Strings up to 24 bytes
    /// are stored inline, without heap allocation.
    String(Option<CompactString>),
    /// 100 ns ticks since 1601-01-01 UTC (see [`crate::datetime`]).
    DateTime(i64),
    /// Raw 16 bytes, as encoded on the wire.
    Guid([u8; 16]),
    /// `None` = null byte string.
    ByteString(Option<Vec<u8>>),
    StatusCode(u32),
    Enum(EnumValue),
    Struct(StructValue),
    Array(Vec<Value>),
}

/// Value of an enumerated type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumValue {
    pub ty: EnumId,
    pub value: i32,
}

/// Value of a structured type: fields in schema order.
#[derive(Debug, Clone, PartialEq)]
pub struct StructValue {
    pub ty: StructId,
    pub fields: Box<[Value]>,
}

impl StructValue {
    /// Field value by name.
    pub fn get<'a>(&'a self, dict: &TypeDictionary, name: &str) -> Option<&'a Value> {
        let idx = dict.struct_type(self.ty).field_index(name)?;
        self.fields.get(idx)
    }

    /// Mutable field value by name.
    pub fn get_mut<'a>(&'a mut self, dict: &TypeDictionary, name: &str) -> Option<&'a mut Value> {
        let idx = dict.struct_type(self.ty).field_index(name)?;
        self.fields.get_mut(idx)
    }

    /// Iterates over `(field name, value)` pairs.
    pub fn iter<'a>(
        &'a self,
        dict: &'a TypeDictionary,
    ) -> impl Iterator<Item = (&'a str, &'a Value)> {
        dict.struct_type(self.ty)
            .fields
            .iter()
            .map(|f| &*f.name)
            .zip(self.fields.iter())
    }
}

impl Value {
    /// Variant name, used in error messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Boolean(_) => "Boolean",
            Value::SByte(_) => "SByte",
            Value::Byte(_) => "Byte",
            Value::Int16(_) => "Int16",
            Value::UInt16(_) => "UInt16",
            Value::Int32(_) => "Int32",
            Value::UInt32(_) => "UInt32",
            Value::Int64(_) => "Int64",
            Value::UInt64(_) => "UInt64",
            Value::Float(_) => "Float",
            Value::Double(_) => "Double",
            Value::String(_) => "String",
            Value::DateTime(_) => "DateTime",
            Value::Guid(_) => "Guid",
            Value::ByteString(_) => "ByteString",
            Value::StatusCode(_) => "StatusCode",
            Value::Enum(_) => "Enum",
            Value::Struct(_) => "Struct",
            Value::Array(_) => "Array",
        }
    }

    pub fn as_struct(&self) -> Option<&StructValue> {
        match self {
            Value::Struct(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_struct_mut(&mut self) -> Option<&mut StructValue> {
        match self {
            Value::Struct(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(Some(s)) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Boolean(b) => Some(b),
            _ => None,
        }
    }

    /// Any integer variant (including enums and status codes) as `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        Some(match *self {
            Value::SByte(v) => v as i64,
            Value::Byte(v) => v as i64,
            Value::Int16(v) => v as i64,
            Value::UInt16(v) => v as i64,
            Value::Int32(v) => v as i64,
            Value::UInt32(v) => v as i64,
            Value::Int64(v) | Value::DateTime(v) => v,
            Value::UInt64(v) => i64::try_from(v).ok()?,
            Value::StatusCode(v) => v as i64,
            Value::Enum(e) => e.value as i64,
            _ => return None,
        })
    }

    /// Any numeric variant as `f64`.
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Value::Float(v) => Some(v as f64),
            Value::Double(v) => Some(v),
            Value::UInt64(v) => Some(v as f64),
            _ => self.as_i64().map(|v| v as f64),
        }
    }

    /// Wraps the value with its dictionary for human-readable display.
    pub fn display<'a>(&'a self, dict: &'a TypeDictionary) -> DisplayValue<'a> {
        DisplayValue { value: self, dict }
    }
}

macro_rules! from_impl {
    ($($t:ty => $v:ident),* $(,)?) => {
        $(impl From<$t> for Value {
            #[inline]
            fn from(v: $t) -> Self { Value::$v(v) }
        })*
    };
}

from_impl!(
    bool => Boolean, i8 => SByte, u8 => Byte, i16 => Int16, u16 => UInt16,
    i32 => Int32, u32 => UInt32, i64 => Int64, u64 => UInt64, f32 => Float, f64 => Double,
    StructValue => Struct, EnumValue => Enum, Vec<Value> => Array,
);

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(Some(v.into()))
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(Some(v.into()))
    }
}

impl From<CompactString> for Value {
    fn from(v: CompactString) -> Self {
        Value::String(Some(v))
    }
}

// ── Defaults & coercion ─────────────────────────────────────────────────

impl TypeDictionary {
    /// Default (zero) value of a structured type, with every field initialised.
    pub fn default_struct(&self, id: StructId) -> Result<StructValue> {
        self.default_struct_depth(id, 0)
    }

    fn default_struct_depth(&self, id: StructId, depth: usize) -> Result<StructValue> {
        if depth > crate::MAX_DEPTH {
            return Err(ErrorKind::DepthExceeded.into());
        }
        let st = self.struct_type(id);
        let fields = st
            .fields
            .iter()
            .map(|f| {
                if f.is_array {
                    Ok(Value::Array(Vec::new()))
                } else {
                    self.default_for_depth(&f.ty, &st.name, &f.name, depth + 1)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(StructValue {
            ty: id,
            fields: fields.into_boxed_slice(),
        })
    }

    /// Default value for a scalar field type.
    pub fn default_for(&self, ty: &FieldType) -> Result<Value> {
        self.default_for_depth(ty, "", "", 0)
    }

    fn default_for_depth(
        &self,
        ty: &FieldType,
        owner: &str,
        field: &str,
        depth: usize,
    ) -> Result<Value> {
        Ok(match ty {
            FieldType::Builtin(b) => match b {
                Builtin::Boolean => Value::Boolean(false),
                Builtin::SByte => Value::SByte(0),
                Builtin::Byte => Value::Byte(0),
                Builtin::Int16 => Value::Int16(0),
                Builtin::UInt16 => Value::UInt16(0),
                Builtin::Int32 => Value::Int32(0),
                Builtin::UInt32 => Value::UInt32(0),
                Builtin::Int64 => Value::Int64(0),
                Builtin::UInt64 => Value::UInt64(0),
                Builtin::Float => Value::Float(0.0),
                Builtin::Double => Value::Double(0.0),
                Builtin::String => Value::String(Some(CompactString::default())),
                Builtin::DateTime => Value::DateTime(0),
                Builtin::Guid => Value::Guid([0; 16]),
                Builtin::ByteString => Value::ByteString(None),
                Builtin::StatusCode => Value::StatusCode(0),
            },
            FieldType::Enum(id) => {
                let value = self.enum_type(*id).values.first().map_or(0, |(_, v)| *v);
                Value::Enum(EnumValue { ty: *id, value })
            }
            FieldType::Struct(id) => Value::Struct(self.default_struct_depth(*id, depth)?),
            FieldType::Unresolved(t) => {
                return Err(ErrorKind::UnresolvedType {
                    owner: owner.to_owned(),
                    field: field.to_owned(),
                    ty: t.to_string(),
                }
                .into());
            }
        })
    }

    /// Converts `value` to the representation required by a field type.
    ///
    /// Numeric values are converted (with range checks), strings are accepted
    /// for enumerations (by name), integers for enums and status codes.
    pub fn coerce(
        &self,
        ty: &FieldType,
        is_array: bool,
        value: Value,
        path: &str,
    ) -> Result<Value> {
        if is_array {
            return match value {
                Value::Array(items) => items
                    .into_iter()
                    .map(|v| self.coerce(ty, false, v, path))
                    .collect::<Result<Vec<_>>>()
                    .map(Value::Array),
                other => {
                    Err(self.mismatch(path, &format!("array of {}", self.type_name(ty)), &other))
                }
            };
        }
        let mismatch = |v: &Value| self.mismatch(path, self.type_name(ty), v);
        match ty {
            FieldType::Builtin(b) => {
                macro_rules! int {
                    ($variant:ident, $t:ty) => {{
                        let i = value.as_i64().ok_or_else(|| mismatch(&value))?;
                        let v = <$t>::try_from(i).map_err(|_| mismatch(&value))?;
                        Value::$variant(v)
                    }};
                }
                Ok(match (b, &value) {
                    (Builtin::Boolean, Value::Boolean(_))
                    | (Builtin::String, Value::String(_))
                    | (Builtin::Guid, Value::Guid(_))
                    | (Builtin::ByteString, Value::ByteString(_))
                    | (Builtin::DateTime, Value::DateTime(_)) => value,
                    (Builtin::UInt64, Value::UInt64(_)) => value,
                    (Builtin::SByte, _) => int!(SByte, i8),
                    (Builtin::Byte, _) => int!(Byte, u8),
                    (Builtin::Int16, _) => int!(Int16, i16),
                    (Builtin::UInt16, _) => int!(UInt16, u16),
                    (Builtin::Int32, _) => int!(Int32, i32),
                    (Builtin::UInt32, _) => int!(UInt32, u32),
                    (Builtin::Int64, _) => int!(Int64, i64),
                    (Builtin::UInt64, _) => int!(UInt64, u64),
                    (Builtin::StatusCode, _) => int!(StatusCode, u32),
                    (Builtin::DateTime, _) => int!(DateTime, i64),
                    (Builtin::Float, _) => {
                        Value::Float(value.as_f64().ok_or_else(|| mismatch(&value))? as f32)
                    }
                    (Builtin::Double, _) => {
                        Value::Double(value.as_f64().ok_or_else(|| mismatch(&value))?)
                    }
                    _ => return Err(mismatch(&value)),
                })
            }
            FieldType::Enum(id) => {
                let et = self.enum_type(*id);
                let v = match &value {
                    Value::Enum(e) if e.ty == *id => return Ok(value),
                    Value::String(Some(name)) => {
                        et.value_of(name).ok_or_else(|| ErrorKind::InvalidEnum {
                            enumeration: et.name.to_string(),
                            value: name.to_string(),
                        })?
                    }
                    other => {
                        let i = other.as_i64().ok_or_else(|| mismatch(other))?;
                        i32::try_from(i).map_err(|_| mismatch(other))?
                    }
                };
                Ok(Value::Enum(EnumValue { ty: *id, value: v }))
            }
            FieldType::Struct(id) => match value {
                Value::Struct(ref s) if s.ty == *id => Ok(value),
                other => Err(mismatch(&other)),
            },
            FieldType::Unresolved(t) => Err(ErrorKind::UnresolvedType {
                owner: String::new(),
                field: path.to_owned(),
                ty: t.to_string(),
            }
            .into()),
        }
    }

    fn mismatch(&self, path: &str, expected: &str, got: &Value) -> Error {
        ErrorKind::TypeMismatch {
            path: path.to_owned(),
            expected: expected.to_owned(),
            got: got.kind(),
        }
        .into()
    }
}

// ── Display ─────────────────────────────────────────────────────────────

/// Human-readable rendering of a [`Value`] (field and enum names resolved).
pub struct DisplayValue<'a> {
    value: &'a Value,
    dict: &'a TypeDictionary,
}

impl fmt::Display for DisplayValue<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.dict;
        match self.value {
            Value::Boolean(v) => write!(f, "{v}"),
            Value::SByte(v) => write!(f, "{v}"),
            Value::Byte(v) => write!(f, "{v}"),
            Value::Int16(v) => write!(f, "{v}"),
            Value::UInt16(v) => write!(f, "{v}"),
            Value::Int32(v) => write!(f, "{v}"),
            Value::UInt32(v) => write!(f, "{v}"),
            Value::Int64(v) => write!(f, "{v}"),
            Value::UInt64(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Double(v) => write!(f, "{v}"),
            Value::String(Some(s)) => write!(f, "{s:?}"),
            Value::String(None) | Value::ByteString(None) => f.write_str("null"),
            Value::DateTime(t) => write!(f, "DateTime({t})"),
            Value::Guid(g) => write!(f, "Guid({g:02x?})"),
            Value::ByteString(Some(b)) => write!(f, "ByteString({} bytes)", b.len()),
            Value::StatusCode(s) => write!(f, "StatusCode(0x{s:08X})"),
            Value::Enum(e) => match d.enum_type(e.ty).name_of(e.value) {
                Some(n) => f.write_str(n),
                None => write!(f, "{}", e.value),
            },
            Value::Struct(s) => {
                write!(f, "{} {{ ", d.struct_type(s.ty).name)?;
                for (i, (name, v)) in s.iter(d).enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{name}: {}", v.display(d))?;
                }
                f.write_str(" }")
            }
            Value::Array(items) => {
                f.write_str("[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", v.display(d))?;
                }
                f.write_str("]")
            }
        }
    }
}
