//! Field paths (`"Items[1].Name"`) compiled against a schema, and
//! byte-level patching that rewrites only the targeted bytes.

use crate::codec::Reader;
use crate::error::{Error, ErrorKind, Result};
use crate::schema::{FieldType, StructId, TypeDictionary};
use crate::value::{StructValue, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Field(usize),
    Index(usize),
}

/// A path compiled against a root structured type.
///
/// Compile once with [`TypeDictionary::compile_path`] and reuse: lookups are
/// then pure index arithmetic, without any string comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    root: StructId,
    steps: Box<[Step]>,
    leaf_ty: FieldType,
    leaf_is_array: bool,
    text: Box<str>,
}

impl Path {
    /// Root structured type of the path.
    pub fn root(&self) -> StructId {
        self.root
    }

    /// Source text of the path.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Type of the value the path points to (`is_array` = whole array field).
    pub fn leaf_type(&self) -> (&FieldType, bool) {
        (&self.leaf_ty, self.leaf_is_array)
    }
}

enum Ctx {
    Struct(StructId),
    Array(FieldType),
    Scalar(FieldType),
}

fn scalar_ctx(ty: &FieldType) -> Ctx {
    match ty {
        FieldType::Struct(id) => Ctx::Struct(*id),
        other => Ctx::Scalar(other.clone()),
    }
}

impl TypeDictionary {
    /// Compiles a path such as `"Name"`, `"Items[2]"` or `"A[0].B[2].C"`.
    pub fn compile_path(&self, root: StructId, path: &str) -> Result<Path> {
        if path.is_empty() {
            return Err(Error::path(path, "empty path"));
        }
        let mut steps = Vec::new();
        let mut ctx = Ctx::Struct(root);
        for token in path.split('.') {
            let (name, mut rest) = match token.find('[') {
                Some(i) => token.split_at(i),
                None => (token, ""),
            };
            if name.is_empty() {
                return Err(Error::path(
                    path,
                    format!("missing field name in '{token}'"),
                ));
            }
            let Ctx::Struct(sid) = ctx else {
                return Err(Error::path(
                    path,
                    format!("'{name}': parent is not a structure"),
                ));
            };
            let st = self.struct_type(sid);
            let idx = st
                .field_index(name)
                .ok_or_else(|| Error::path(path, format!("no field '{name}' in '{}'", st.name)))?;
            let f = &st.fields[idx];
            steps.push(Step::Field(idx));
            ctx = if f.is_array {
                Ctx::Array(f.ty.clone())
            } else {
                scalar_ctx(&f.ty)
            };

            while !rest.is_empty() {
                let close = rest
                    .find(']')
                    .filter(|_| rest.starts_with('['))
                    .ok_or_else(|| Error::path(path, format!("malformed index in '{token}'")))?;
                let n: usize = rest[1..close]
                    .trim()
                    .parse()
                    .map_err(|_| Error::path(path, format!("invalid index in '{token}'")))?;
                let Ctx::Array(ty) = ctx else {
                    return Err(Error::path(path, format!("'{name}' is not an array")));
                };
                steps.push(Step::Index(n));
                ctx = scalar_ctx(&ty);
                rest = &rest[close + 1..];
            }
        }
        let (leaf_ty, leaf_is_array) = match ctx {
            Ctx::Struct(id) => (FieldType::Struct(id), false),
            Ctx::Array(ty) => (ty, true),
            Ctx::Scalar(ty) => (ty, false),
        };
        Ok(Path {
            root,
            steps: steps.into_boxed_slice(),
            leaf_ty,
            leaf_is_array,
            text: path.into(),
        })
    }

    // ── Value-level access ──────────────────────────────────────────────

    /// Value at `path` inside a decoded struct.
    pub fn get<'a>(&self, value: &'a StructValue, path: &Path) -> Result<&'a Value> {
        check_root(value, path)?;
        let mut cur: Option<&Value> = None;
        for step in path.steps.iter() {
            let next = match (cur, *step) {
                (None, Step::Field(i)) => &value.fields[i],
                (Some(Value::Struct(s)), Step::Field(i)) => &s.fields[i],
                (Some(Value::Array(a)), Step::Index(i)) => {
                    a.get(i).ok_or_else(|| ErrorKind::IndexOutOfRange {
                        path: path.text.to_string(),
                        index: i,
                        len: a.len(),
                    })?
                }
                _ => return Err(Error::path(&path.text, "value does not match the schema")),
            };
            cur = Some(next);
        }
        cur.ok_or_else(|| Error::path(&path.text, "empty path"))
    }

    /// Mutable value at `path` inside a decoded struct.
    pub fn get_mut<'a>(&self, value: &'a mut StructValue, path: &Path) -> Result<&'a mut Value> {
        check_root(value, path)?;
        let mut steps = path.steps.iter();
        let Some(Step::Field(first)) = steps.next() else {
            return Err(Error::path(&path.text, "empty path"));
        };
        let mut cur: &mut Value = &mut value.fields[*first];
        for step in steps {
            cur = match (cur, *step) {
                (Value::Struct(s), Step::Field(i)) => &mut s.fields[i],
                (Value::Array(a), Step::Index(i)) => {
                    let len = a.len();
                    a.get_mut(i).ok_or_else(|| ErrorKind::IndexOutOfRange {
                        path: path.text.to_string(),
                        index: i,
                        len,
                    })?
                }
                _ => return Err(Error::path(&path.text, "value does not match the schema")),
            };
        }
        Ok(cur)
    }

    /// Sets the value at `path`, converting it to the schema type (see [`TypeDictionary::coerce`]).
    pub fn set(&self, value: &mut StructValue, path: &Path, new: Value) -> Result<()> {
        let new = self.coerce(&path.leaf_ty, path.leaf_is_array, new, &path.text)?;
        *self.get_mut(value, path)? = new;
        Ok(())
    }

    /// Compiles `path` and sets the value (convenience for one-off updates).
    pub fn set_str(&self, value: &mut StructValue, path: &str, new: Value) -> Result<()> {
        let p = self.compile_path(value.ty, path)?;
        self.set(value, &p, new)
    }

    /// Compiles `path` and returns the value (convenience for one-off reads).
    pub fn get_str<'a>(&self, value: &'a StructValue, path: &str) -> Result<&'a Value> {
        let p = self.compile_path(value.ty, path)?;
        self.get(value, &p)
    }

    // ── Byte-level access ───────────────────────────────────────────────

    /// Byte range `[start, end)` of the value at `path` inside an encoded struct.
    pub fn locate(&self, buf: &[u8], path: &Path) -> Result<(usize, usize)> {
        let mut r = Reader::new(buf, 0);
        let mut ctx_struct = Some(path.root);
        let mut ctx_field: Option<(&FieldType, bool)> = None;

        for step in path.steps.iter() {
            match *step {
                Step::Field(k) => {
                    let sid = ctx_struct
                        .ok_or_else(|| Error::path(&path.text, "parent is not a structure"))?;
                    let st = self.struct_type(sid);
                    if let Some(reason) = st.unsupported {
                        return Err(ErrorKind::Unsupported {
                            name: st.name.to_string(),
                            reason,
                        }
                        .into());
                    }
                    let start = r.pos;
                    let known = st.fixed_offsets.len();
                    if k < known {
                        r.pos = start + st.fixed_offsets[k];
                    } else {
                        // Jump to the first variable-size field, then walk.
                        r.pos = start + st.fixed_offsets[known - 1];
                        for f in &st.fields[known - 1..k] {
                            self.skip_field(&f.ty, f.is_array, &mut r, 0)?;
                        }
                    }
                    let f = &st.fields[k];
                    ctx_field = Some((&f.ty, f.is_array));
                    ctx_struct = match (&f.ty, f.is_array) {
                        (FieldType::Struct(id), false) => Some(*id),
                        _ => None,
                    };
                }
                Step::Index(i) => {
                    let Some((ty, true)) = ctx_field else {
                        return Err(Error::path(&path.text, "indexing a non-array"));
                    };
                    let len = r.length()?.unwrap_or(0);
                    if i >= len {
                        return Err(ErrorKind::IndexOutOfRange {
                            path: path.text.to_string(),
                            index: i,
                            len,
                        }
                        .into());
                    }
                    match self.scalar_fixed_size(ty) {
                        Some(sz) => r.advance(i * sz)?,
                        None => {
                            for _ in 0..i {
                                self.skip_field(ty, false, &mut r, 0)?;
                            }
                        }
                    }
                    ctx_field = Some((ty, false));
                    ctx_struct = match ty {
                        FieldType::Struct(id) => Some(*id),
                        _ => None,
                    };
                }
            }
        }
        let start = r.pos;
        self.skip_field(&path.leaf_ty, path.leaf_is_array, &mut r, 0)?;
        Ok((start, r.pos))
    }

    /// Decodes only the value at `path` from an encoded struct.
    pub fn read_path(&self, buf: &[u8], path: &Path) -> Result<Value> {
        let (start, _) = self.locate(buf, path)?;
        let mut r = Reader::new(buf, start);
        self.decode_field(&path.leaf_ty, path.leaf_is_array, &mut r, 0)
    }

    /// Applies modifications directly on an encoded struct.
    ///
    /// Only the bytes of the targeted values are rewritten; everything else is
    /// copied verbatim, without decoding. Overlapping paths (e.g. `"A"` and
    /// `"A.B"`) fall back to decode → modify → encode, applied in order.
    pub fn patch(&self, buf: &[u8], mods: &[(&Path, Value)]) -> Result<Vec<u8>> {
        let mut edits: Vec<(usize, usize, Vec<u8>)> = Vec::with_capacity(mods.len());
        if let Some((first, _)) = mods.first() {
            if let Some((p, _)) = mods.iter().find(|(p, _)| p.root != first.root) {
                return Err(Error::path(
                    &p.text,
                    "all paths must share the same root type",
                ));
            }
        }
        for (path, v) in mods {
            let v = self.coerce(&path.leaf_ty, path.leaf_is_array, v.clone(), &path.text)?;
            let (start, end) = self.locate(buf, path)?;
            let mut bytes =
                Vec::with_capacity(self.field_encoded_len(&path.leaf_ty, path.leaf_is_array, &v));
            self.encode_field(&path.leaf_ty, path.leaf_is_array, &v, &mut bytes, 0)?;
            edits.push((start, end, bytes));
        }
        edits.sort_by_key(|e| e.0);
        if edits.windows(2).any(|w| w[0].1 > w[1].0) {
            return self.patch_slow(buf, mods);
        }
        let delta: isize = edits
            .iter()
            .map(|(s, e, b)| b.len() as isize - (e - s) as isize)
            .sum();
        let mut out = Vec::with_capacity((buf.len() as isize + delta).max(0) as usize);
        let mut pos = 0;
        for (start, end, bytes) in &edits {
            out.extend_from_slice(&buf[pos..*start]);
            out.extend_from_slice(bytes);
            pos = *end;
        }
        out.extend_from_slice(&buf[pos..]);
        Ok(out)
    }

    fn patch_slow(&self, buf: &[u8], mods: &[(&Path, Value)]) -> Result<Vec<u8>> {
        let root = mods
            .first()
            .map(|(p, _)| p.root)
            .ok_or_else(|| Error::path("", "no modification"))?;
        let mut value = self.decode(root, buf)?;
        for (path, v) in mods {
            self.set(&mut value, path, v.clone())?;
        }
        self.encode(&value)
    }

    /// Like [`TypeDictionary::patch`], compiling string paths on the fly.
    pub fn patch_str(&self, root: StructId, buf: &[u8], mods: &[(&str, Value)]) -> Result<Vec<u8>> {
        let paths = mods
            .iter()
            .map(|(p, _)| self.compile_path(root, p))
            .collect::<Result<Vec<_>>>()?;
        let mods: Vec<(&Path, Value)> = paths
            .iter()
            .zip(mods)
            .map(|(p, (_, v))| (p, v.clone()))
            .collect();
        self.patch(buf, &mods)
    }
}

#[inline]
fn check_root(value: &StructValue, path: &Path) -> Result<()> {
    if value.ty != path.root {
        return Err(Error::path(
            &path.text,
            "path was compiled for another structured type",
        ));
    }
    Ok(())
}
