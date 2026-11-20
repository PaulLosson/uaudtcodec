//! Binary (OPC-UA little-endian) decoder and encoder.

use crate::MAX_DEPTH;
use crate::error::{Error, ErrorKind, Result};
use crate::schema::{Builtin, FieldType, StructId, TypeDictionary};
use crate::value::{EnumValue, StructValue, Value};

// ── Reader ──────────────────────────────────────────────────────────────

/// Bounds-checked little-endian cursor over a byte slice.
#[derive(Clone, Copy)]
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pub(crate) pos: usize,
}

impl<'a> Reader<'a> {
    #[inline]
    pub(crate) fn new(buf: &'a [u8], pos: usize) -> Self {
        Reader { buf, pos }
    }

    #[inline]
    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    #[cold]
    #[inline(never)]
    fn eof(&self, n: usize) -> Error {
        ErrorKind::UnexpectedEof {
            offset: self.pos,
            needed: n - self.remaining(),
        }
        .into()
    }

    #[inline(always)]
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(self.eof(n));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    #[inline(always)]
    pub(crate) fn advance(&mut self, n: usize) -> Result<()> {
        if self.remaining() < n {
            return Err(self.eof(n));
        }
        self.pos += n;
        Ok(())
    }

    #[inline(always)]
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        if self.remaining() < N {
            return Err(self.eof(N));
        }
        let mut out = [0u8; N];
        out.copy_from_slice(&self.buf[self.pos..self.pos + N]);
        self.pos += N;
        Ok(out)
    }

    #[inline(always)]
    pub(crate) fn i32(&mut self) -> Result<i32> {
        self.array::<4>().map(i32::from_le_bytes)
    }

    /// Reads an Int32 length prefix: `None` for -1 (null).
    #[inline(always)]
    pub(crate) fn length(&mut self) -> Result<Option<usize>> {
        let offset = self.pos;
        match self.i32()? {
            n if n >= 0 => Ok(Some(n as usize)),
            -1 => Ok(None),
            len => Err(ErrorKind::InvalidLength { offset, len }.into()),
        }
    }
}

// ── Decoding ────────────────────────────────────────────────────────────

impl TypeDictionary {
    /// Decodes one structured value from the start of `buf` (trailing bytes are ignored).
    pub fn decode(&self, id: StructId, buf: &[u8]) -> Result<StructValue> {
        let mut r = Reader::new(buf, 0);
        self.decode_struct(id, &mut r, 0)
    }

    /// Decodes one structured value and returns it with the number of bytes consumed.
    pub fn decode_with_len(&self, id: StructId, buf: &[u8]) -> Result<(StructValue, usize)> {
        let mut r = Reader::new(buf, 0);
        let v = self.decode_struct(id, &mut r, 0)?;
        Ok((v, r.pos))
    }

    /// Decodes an Int32-prefixed array of structured values.
    pub fn decode_array(&self, id: StructId, buf: &[u8]) -> Result<Vec<StructValue>> {
        let mut r = Reader::new(buf, 0);
        let len = r.length()?.unwrap_or(0);
        let mut out = Vec::with_capacity(len.min(r.remaining()));
        for _ in 0..len {
            out.push(self.decode_struct(id, &mut r, 0)?);
        }
        Ok(out)
    }

    pub(crate) fn decode_struct(
        &self,
        id: StructId,
        r: &mut Reader<'_>,
        depth: usize,
    ) -> Result<StructValue> {
        if depth > MAX_DEPTH {
            return Err(ErrorKind::DepthExceeded.into());
        }
        let st = self.struct_type(id);
        if let Some(reason) = st.unsupported {
            return Err(ErrorKind::Unsupported {
                name: st.name.to_string(),
                reason,
            }
            .into());
        }
        let mut fields = Vec::with_capacity(st.fields.len());
        for f in st.fields.iter() {
            let v = if f.is_array {
                self.decode_array_field(&f.ty, r, depth)
                    .map_err(|e| self.ctx(e, id, &f.name))?
            } else {
                self.decode_scalar(&f.ty, r, depth)
                    .map_err(|e| self.ctx(e, id, &f.name))?
            };
            fields.push(v);
        }
        Ok(StructValue {
            ty: id,
            fields: fields.into_boxed_slice(),
        })
    }

    #[inline]
    pub(crate) fn decode_field(
        &self,
        ty: &FieldType,
        is_array: bool,
        r: &mut Reader<'_>,
        depth: usize,
    ) -> Result<Value> {
        if is_array {
            self.decode_array_field(ty, r, depth)
        } else {
            self.decode_scalar(ty, r, depth)
        }
    }

    fn decode_array_field(
        &self,
        ty: &FieldType,
        r: &mut Reader<'_>,
        depth: usize,
    ) -> Result<Value> {
        let len = r.length()?.unwrap_or(0);
        let mut items = Vec::with_capacity(len.min(r.remaining()));
        for _ in 0..len {
            items.push(self.decode_scalar(ty, r, depth)?);
        }
        Ok(Value::Array(items))
    }

    #[inline]
    pub(crate) fn decode_scalar(
        &self,
        ty: &FieldType,
        r: &mut Reader<'_>,
        depth: usize,
    ) -> Result<Value> {
        Ok(match ty {
            FieldType::Builtin(b) => decode_builtin(*b, r)?,
            FieldType::Struct(id) => Value::Struct(self.decode_struct(*id, r, depth + 1)?),
            FieldType::Enum(id) => {
                let value = match self.enum_type(*id).size {
                    1 => r.array::<1>()?[0] as i8 as i32,
                    2 => i16::from_le_bytes(r.array()?) as i32,
                    _ => r.i32()?,
                };
                Value::Enum(EnumValue { ty: *id, value })
            }
            FieldType::Unresolved(t) => {
                return Err(ErrorKind::UnresolvedType {
                    owner: String::new(),
                    field: String::new(),
                    ty: t.to_string(),
                }
                .into());
            }
        })
    }

    /// Advances `r` past one field without materialising it.
    pub(crate) fn skip_field(
        &self,
        ty: &FieldType,
        is_array: bool,
        r: &mut Reader<'_>,
        depth: usize,
    ) -> Result<()> {
        if is_array {
            let len = r.length()?.unwrap_or(0);
            if let Some(sz) = self.scalar_fixed_size(ty) {
                let total = len.checked_mul(sz).ok_or(ErrorKind::InvalidLength {
                    offset: r.pos,
                    len: len as i32,
                })?;
                return r.advance(total);
            }
            for _ in 0..len {
                self.skip_field(ty, false, r, depth)?;
            }
            return Ok(());
        }
        if let Some(sz) = self.scalar_fixed_size(ty) {
            return r.advance(sz);
        }
        match ty {
            FieldType::Builtin(_) => {
                // String / ByteString
                let n = r.length()?.unwrap_or(0);
                r.advance(n)
            }
            FieldType::Struct(id) => {
                if depth > MAX_DEPTH {
                    return Err(ErrorKind::DepthExceeded.into());
                }
                let st = self.struct_type(*id);
                if let Some(reason) = st.unsupported {
                    return Err(ErrorKind::Unsupported {
                        name: st.name.to_string(),
                        reason,
                    }
                    .into());
                }
                for f in st.fields.iter() {
                    self.skip_field(&f.ty, f.is_array, r, depth + 1)?;
                }
                Ok(())
            }
            FieldType::Unresolved(t) => Err(ErrorKind::UnresolvedType {
                owner: String::new(),
                field: String::new(),
                ty: t.to_string(),
            }
            .into()),
            FieldType::Enum(id) => Err(ErrorKind::Unsupported {
                name: self.enum_type(*id).name.to_string(),
                reason: "bit-sized enumerations",
            }
            .into()),
        }
    }

    /// Encoded size of a scalar of type `ty`, if fixed.
    #[inline]
    pub(crate) fn scalar_fixed_size(&self, ty: &FieldType) -> Option<usize> {
        match ty {
            FieldType::Builtin(b) => b.fixed_size(),
            FieldType::Enum(id) => match self.enum_type(*id).size {
                0 => None,
                n => Some(n as usize),
            },
            FieldType::Struct(id) => self.struct_type(*id).fixed_size,
            FieldType::Unresolved(_) => None,
        }
    }

    /// Adds `Owner.field` context to an error (cold path only).
    #[cold]
    fn ctx(&self, mut e: Error, owner: StructId, field: &str) -> Error {
        self.fill_unresolved(&mut e, owner, field);
        e
    }

    #[cold]
    fn fill_unresolved(&self, e: &mut Error, owner: StructId, field: &str) {
        if let ErrorKind::UnresolvedType {
            owner: o, field: f, ..
        } = e.kind_mut()
        {
            if o.is_empty() && f.is_empty() {
                *o = self.struct_type(owner).name.to_string();
                *f = field.to_owned();
            }
        }
    }
}

#[inline(always)]
fn decode_builtin(b: Builtin, r: &mut Reader<'_>) -> Result<Value> {
    Ok(match b {
        Builtin::Boolean => Value::Boolean(r.array::<1>()?[0] != 0),
        Builtin::SByte => Value::SByte(r.array::<1>()?[0] as i8),
        Builtin::Byte => Value::Byte(r.array::<1>()?[0]),
        Builtin::Int16 => Value::Int16(i16::from_le_bytes(r.array()?)),
        Builtin::UInt16 => Value::UInt16(u16::from_le_bytes(r.array()?)),
        Builtin::Int32 => Value::Int32(i32::from_le_bytes(r.array()?)),
        Builtin::UInt32 => Value::UInt32(u32::from_le_bytes(r.array()?)),
        Builtin::Int64 => Value::Int64(i64::from_le_bytes(r.array()?)),
        Builtin::UInt64 => Value::UInt64(u64::from_le_bytes(r.array()?)),
        Builtin::Float => Value::Float(f32::from_le_bytes(r.array()?)),
        Builtin::Double => Value::Double(f64::from_le_bytes(r.array()?)),
        Builtin::DateTime => Value::DateTime(i64::from_le_bytes(r.array()?)),
        Builtin::StatusCode => Value::StatusCode(u32::from_le_bytes(r.array()?)),
        Builtin::Guid => Value::Guid(r.array()?),
        Builtin::String => match r.length()? {
            None => Value::String(None),
            Some(n) => {
                let start = r.pos;
                let bytes = r.take(n)?;
                let s = std::str::from_utf8(bytes).map_err(|_| ErrorKind::InvalidUtf8(start))?;
                Value::String(Some(compact_str::CompactString::from(s)))
            }
        },
        Builtin::ByteString => match r.length()? {
            None => Value::ByteString(None),
            Some(n) => Value::ByteString(Some(r.take(n)?.to_vec())),
        },
    })
}

// ── Encoding ────────────────────────────────────────────────────────────

impl TypeDictionary {
    /// Encodes a structured value into a new buffer.
    pub fn encode(&self, value: &StructValue) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(self.capacity_hint(value.ty));
        self.encode_struct(value, &mut out, 0)?;
        Ok(out)
    }

    /// Encodes a structured value, appending to `out`.
    pub fn encode_into(&self, value: &StructValue, out: &mut Vec<u8>) -> Result<()> {
        self.encode_struct(value, out, 0)
    }

    /// Encodes an Int32-prefixed array of structured values.
    pub fn encode_array(&self, values: &[StructValue]) -> Result<Vec<u8>> {
        let hint = values.first().map_or(0, |v| self.capacity_hint(v.ty));
        let mut out = Vec::with_capacity(4 + hint * values.len());
        write_len(&mut out, values.len());
        for v in values {
            self.encode_struct(v, &mut out, 0)?;
        }
        Ok(out)
    }

    pub(crate) fn encode_struct(
        &self,
        v: &StructValue,
        out: &mut Vec<u8>,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(ErrorKind::DepthExceeded.into());
        }
        let st = self.struct_type(v.ty);
        if let Some(reason) = st.unsupported {
            return Err(ErrorKind::Unsupported {
                name: st.name.to_string(),
                reason,
            }
            .into());
        }
        if v.fields.len() != st.fields.len() {
            return Err(ErrorKind::TypeMismatch {
                path: st.name.to_string(),
                expected: format!("{} fields", st.fields.len()),
                got: "Struct with a different field count",
            }
            .into());
        }
        for (f, fv) in st.fields.iter().zip(v.fields.iter()) {
            self.encode_field(&f.ty, f.is_array, fv, out, depth)
                .map_err(|e| self.enc_ctx(e, v.ty, &f.name))?;
        }
        Ok(())
    }

    #[inline]
    pub(crate) fn encode_field(
        &self,
        ty: &FieldType,
        is_array: bool,
        v: &Value,
        out: &mut Vec<u8>,
        depth: usize,
    ) -> Result<()> {
        if is_array {
            let Value::Array(items) = v else {
                return Err(self.enc_mismatch(ty, true, v));
            };
            write_len(out, items.len());
            for item in items {
                self.encode_scalar(ty, item, out, depth)?;
            }
            return Ok(());
        }
        self.encode_scalar(ty, v, out, depth)
    }

    #[inline]
    fn encode_scalar(
        &self,
        ty: &FieldType,
        v: &Value,
        out: &mut Vec<u8>,
        depth: usize,
    ) -> Result<()> {
        match (ty, v) {
            (FieldType::Builtin(b), _) => {
                encode_builtin(*b, v, out).ok_or_else(|| self.enc_mismatch(ty, false, v))
            }
            (FieldType::Struct(id), Value::Struct(s)) if s.ty == *id => {
                self.encode_struct(s, out, depth + 1)
            }
            (FieldType::Enum(id), Value::Enum(e)) if e.ty == *id => {
                self.write_enum(*id, e.value, out);
                Ok(())
            }
            (FieldType::Enum(id), Value::Int32(i)) => {
                self.write_enum(*id, *i, out);
                Ok(())
            }
            (FieldType::Unresolved(t), _) => Err(ErrorKind::UnresolvedType {
                owner: String::new(),
                field: String::new(),
                ty: t.to_string(),
            }
            .into()),
            _ => Err(self.enc_mismatch(ty, false, v)),
        }
    }

    #[inline]
    fn write_enum(&self, id: crate::schema::EnumId, value: i32, out: &mut Vec<u8>) {
        match self.enum_type(id).size {
            1 => out.push(value as u8),
            2 => put(out, (value as i16).to_le_bytes()),
            _ => put(out, value.to_le_bytes()),
        }
    }

    #[cold]
    fn enc_mismatch(&self, ty: &FieldType, is_array: bool, v: &Value) -> Error {
        let name = self.type_name(ty);
        ErrorKind::TypeMismatch {
            path: String::new(),
            expected: if is_array {
                format!("array of {name}")
            } else {
                name.to_owned()
            },
            got: v.kind(),
        }
        .into()
    }

    #[cold]
    fn enc_ctx(&self, mut e: Error, owner: StructId, field: &str) -> Error {
        if let ErrorKind::TypeMismatch { path, .. } = e.kind_mut() {
            if path.is_empty() {
                *path = format!("{}.{}", self.struct_type(owner).name, field);
            }
        }
        self.fill_unresolved(&mut e, owner, field);
        e
    }

    /// Initial buffer size for encoding a struct: exact when fixed-size,
    /// otherwise a guess (a full sizing pass costs more than a rare regrowth).
    #[inline]
    fn capacity_hint(&self, id: StructId) -> usize {
        self.struct_type(id).fixed_size.unwrap_or(256)
    }

    // ── Exact encoded length ────────────────────────────────────────────

    pub(crate) fn struct_encoded_len(&self, v: &StructValue) -> usize {
        let st = self.struct_type(v.ty);
        if let Some(sz) = st.fixed_size {
            return sz;
        }
        st.fields
            .iter()
            .zip(v.fields.iter())
            .map(|(f, fv)| self.field_encoded_len(&f.ty, f.is_array, fv))
            .sum()
    }

    pub(crate) fn field_encoded_len(&self, ty: &FieldType, is_array: bool, v: &Value) -> usize {
        if is_array {
            let Value::Array(items) = v else { return 0 };
            return 4 + match self.scalar_fixed_size(ty) {
                Some(sz) => sz * items.len(),
                None => items
                    .iter()
                    .map(|i| self.field_encoded_len(ty, false, i))
                    .sum(),
            };
        }
        if let Some(sz) = self.scalar_fixed_size(ty) {
            return sz;
        }
        match v {
            Value::String(s) => 4 + s.as_ref().map_or(0, |s| s.len()),
            Value::ByteString(b) => 4 + b.as_ref().map_or(0, |b| b.len()),
            Value::Struct(s) => self.struct_encoded_len(s),
            _ => 0,
        }
    }
}

/// Appends a fixed-size byte array (inlined, no `memcpy` call for small sizes).
#[inline(always)]
fn put<const N: usize>(out: &mut Vec<u8>, bytes: [u8; N]) {
    out.reserve(N);
    let len = out.len();
    // SAFETY: `reserve` guarantees capacity for `len + N` bytes, which are
    // initialised by the copy before `set_len`.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out.as_mut_ptr().add(len), N);
        out.set_len(len + N);
    }
}

#[inline(always)]
fn write_len(out: &mut Vec<u8>, len: usize) {
    put(out, (len as i32).to_le_bytes());
}

#[inline(always)]
fn encode_builtin(b: Builtin, v: &Value, out: &mut Vec<u8>) -> Option<()> {
    match (b, v) {
        (Builtin::Boolean, Value::Boolean(x)) => out.push(*x as u8),
        (Builtin::SByte, Value::SByte(x)) => out.push(*x as u8),
        (Builtin::Byte, Value::Byte(x)) => out.push(*x),
        (Builtin::Int16, Value::Int16(x)) => put(out, x.to_le_bytes()),
        (Builtin::UInt16, Value::UInt16(x)) => put(out, x.to_le_bytes()),
        (Builtin::Int32, Value::Int32(x)) => put(out, x.to_le_bytes()),
        (Builtin::UInt32, Value::UInt32(x)) => put(out, x.to_le_bytes()),
        (Builtin::Int64, Value::Int64(x)) => put(out, x.to_le_bytes()),
        (Builtin::UInt64, Value::UInt64(x)) => put(out, x.to_le_bytes()),
        (Builtin::Float, Value::Float(x)) => put(out, x.to_le_bytes()),
        (Builtin::Double, Value::Double(x)) => put(out, x.to_le_bytes()),
        (Builtin::DateTime, Value::DateTime(x)) => put(out, x.to_le_bytes()),
        (Builtin::StatusCode, Value::StatusCode(x)) => put(out, x.to_le_bytes()),
        (Builtin::Guid, Value::Guid(x)) => put(out, *x),
        (Builtin::String, Value::String(s)) => match s {
            None => put(out, (-1i32).to_le_bytes()),
            Some(s) => {
                write_len(out, s.len());
                out.extend_from_slice(s.as_bytes());
            }
        },
        (Builtin::ByteString, Value::ByteString(s)) => match s {
            None => put(out, (-1i32).to_le_bytes()),
            Some(s) => {
                write_len(out, s.len());
                out.extend_from_slice(s);
            }
        },
        _ => return None,
    }
    Some(())
}
