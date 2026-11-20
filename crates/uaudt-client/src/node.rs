//! High-level read/write access to a variable holding structured values.

use std::sync::Arc;

use opcua::client::Session;
use opcua::types::type_loader::ByteStringBody;
use opcua::types::{
    Array, AttributeId, BrowseDirection, ByteString, DataValue, ExtensionObject, NodeClassMask,
    NodeId, NumericRange, ReferenceTypeId, Variant, VariantScalarTypeId, WriteValue,
};
use uaudt_core::{Path, StructId, StructValue, TypeDictionary, Value};

use crate::error::{Error, Result, check};
use crate::services;
use crate::type_system::TypeSystem;

/// Encoded body of one element, as stored on the server.
#[derive(Debug, Clone, PartialEq)]
pub struct RawBody {
    /// NodeId of the "Default Binary" encoding (the ExtensionObject TypeId).
    pub encoding_id: NodeId,
    /// Binary body, decodable with [`TypeDictionary::decode`].
    pub bytes: Vec<u8>,
}

/// How element writes are sent to the server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WriteMode {
    /// Write the whole array with one element replaced (what most servers
    /// accept; same behaviour as the Python implementation).
    #[default]
    FullArray,
    /// Write only the element, using an index range. Much less traffic for
    /// large arrays, but server dependent: some servers refuse it, and some
    /// (e.g. Python asyncua) answer Good yet replace the whole array with the
    /// single element. Validate against your server before using it.
    IndexRange,
}

/// A variable holding a structured value or an array of structured values.
///
/// The raw element bodies are cached after each read; writes and patches
/// operate on that cache and update it on success (no re-read round-trip).
pub struct UdtNode {
    session: Arc<Session>,
    node_id: NodeId,
    type_name: String,
    dict: Arc<TypeDictionary>,
    struct_id: StructId,
    bodies: Vec<RawBody>,
    scalar: bool,
    write_mode: WriteMode,
}

impl UdtNode {
    /// Opens a variable: resolves its data type (unless known by `types`)
    /// and reads its value, in parallel.
    pub async fn open(
        session: Arc<Session>,
        node_id: impl Into<NodeId>,
        types: &TypeSystem,
    ) -> Result<Self> {
        let node_id = node_id.into();
        let key = node_id.to_string();
        if let Some(type_name) = types.node_type(&key) {
            let value = read_value(&session, &node_id).await?;
            return Self::build(session, node_id, type_name, types, value);
        }
        let (type_name, value) = futures::try_join!(
            resolve_type_name(&session, &node_id),
            read_value(&session, &node_id)
        )?;
        types.remember_node_type(key, type_name.clone());
        Self::build(session, node_id, type_name, types, value)
    }

    /// Opens a variable whose structured type name is already known (one round-trip).
    pub async fn open_with_type(
        session: Arc<Session>,
        node_id: impl Into<NodeId>,
        type_name: impl Into<String>,
        types: &TypeSystem,
    ) -> Result<Self> {
        let node_id = node_id.into();
        let value = read_value(&session, &node_id).await?;
        Self::build(session, node_id, type_name.into(), types, value)
    }

    fn build(
        session: Arc<Session>,
        node_id: NodeId,
        type_name: String,
        types: &TypeSystem,
        value: DataValue,
    ) -> Result<Self> {
        let (dict, struct_id) = types.find_struct(&type_name)?;
        let (bodies, scalar) = parse_value(value, &node_id)?;
        Ok(UdtNode {
            session,
            node_id,
            type_name,
            dict,
            struct_id,
            bodies,
            scalar,
            write_mode: WriteMode::default(),
        })
    }

    // ── Accessors ───────────────────────────────────────────────────────

    pub fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    /// Name of the structured type of the elements.
    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    /// Dictionary declaring the type (use it to build or inspect values).
    pub fn dictionary(&self) -> &Arc<TypeDictionary> {
        &self.dict
    }

    pub fn struct_id(&self) -> StructId {
        self.struct_id
    }

    /// Number of cached elements (1 for a scalar variable).
    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    /// `true` if the variable holds a single value rather than an array.
    pub fn is_scalar(&self) -> bool {
        self.scalar
    }

    /// Cached raw bodies.
    pub fn raw(&self) -> &[RawBody] {
        &self.bodies
    }

    pub fn write_mode(&self) -> WriteMode {
        self.write_mode
    }

    pub fn set_write_mode(&mut self, mode: WriteMode) {
        self.write_mode = mode;
    }

    /// Compiles a field path (e.g. `"SubItems[1].Description"`) for the element type.
    pub fn path(&self, path: &str) -> Result<Path> {
        Ok(self.dict.compile_path(self.struct_id, path)?)
    }

    /// A default-initialised element, to fill and [`write`](Self::write).
    pub fn new_value(&self) -> Result<StructValue> {
        Ok(self.dict.default_struct(self.struct_id)?)
    }

    // ── Read ────────────────────────────────────────────────────────────

    /// Re-reads the raw value from the server.
    pub async fn refresh(&mut self) -> Result<()> {
        let value = read_value(&self.session, &self.node_id).await?;
        (self.bodies, self.scalar) = parse_value(value, &self.node_id)?;
        Ok(())
    }

    /// Reads from the server and decodes every element.
    pub async fn read(&mut self) -> Result<Vec<StructValue>> {
        self.refresh().await?;
        self.decode_all()
    }

    /// Decodes every cached element (no server request).
    pub fn decode_all(&self) -> Result<Vec<StructValue>> {
        self.bodies
            .iter()
            .map(|b| {
                self.dict
                    .decode(self.struct_id, &b.bytes)
                    .map_err(Error::from)
            })
            .collect()
    }

    /// Decodes one cached element (no server request).
    pub fn decode(&self, index: usize) -> Result<StructValue> {
        Ok(self.dict.decode(self.struct_id, &self.body(index)?.bytes)?)
    }

    /// Decodes a single field of a cached element, without decoding the rest.
    pub fn read_path(&self, index: usize, path: &Path) -> Result<Value> {
        Ok(self.dict.read_path(&self.body(index)?.bytes, path)?)
    }

    // ── Write ───────────────────────────────────────────────────────────

    /// Replaces the element at `index`.
    pub async fn write(&mut self, index: usize, value: &StructValue) -> Result<()> {
        let bytes = self.dict.encode(value)?;
        self.write_body(index, bytes).await
    }

    /// Modifies fields of the element at `index`, e.g.
    /// `[("Name", "X".into()), ("SubItems[1].Value", 2.5.into())]`.
    ///
    /// Only the targeted bytes are re-encoded (see [`TypeDictionary::patch`]).
    pub async fn patch(&mut self, index: usize, modifications: &[(&str, Value)]) -> Result<()> {
        let bytes = self
            .dict
            .patch_str(self.struct_id, &self.body(index)?.bytes, modifications)?;
        self.write_body(index, bytes).await
    }

    /// Like [`patch`](Self::patch) with precompiled paths (fastest for repeated updates).
    pub async fn patch_paths(
        &mut self,
        index: usize,
        modifications: &[(&Path, Value)],
    ) -> Result<()> {
        let bytes = self.dict.patch(&self.body(index)?.bytes, modifications)?;
        self.write_body(index, bytes).await
    }

    /// Decodes the element at `index`, lets `f` modify it, then writes it back.
    pub async fn edit<F>(&mut self, index: usize, f: F) -> Result<()>
    where
        F: FnOnce(&TypeDictionary, &mut StructValue) -> uaudt_core::Result<()>,
    {
        let mut value = self.decode(index)?;
        f(&self.dict, &mut value)?;
        self.write(index, &value).await
    }

    /// Replaces the whole array (or the scalar value, with exactly one element).
    pub async fn write_all(&mut self, values: &[StructValue]) -> Result<()> {
        let encoding_id = self.encoding_id()?;
        let bodies = values
            .iter()
            .map(|v| {
                Ok(RawBody {
                    encoding_id: encoding_id.clone(),
                    bytes: self.dict.encode(v)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let variant = if self.scalar {
            match bodies.as_slice() {
                [one] => extension_object(one),
                _ => {
                    return Err(Error::unexpected(
                        "a scalar variable takes exactly one value",
                    ));
                }
            }
        } else {
            array_variant(bodies.iter())?
        };
        self.send(variant, NumericRange::None).await?;
        self.bodies = bodies;
        Ok(())
    }

    fn body(&self, index: usize) -> Result<&RawBody> {
        self.bodies.get(index).ok_or(Error::IndexOutOfRange {
            index,
            len: self.bodies.len(),
        })
    }

    fn encoding_id(&self) -> Result<NodeId> {
        self.bodies
            .first()
            .map(|b| b.encoding_id.clone())
            .ok_or_else(|| {
                Error::unexpected("encoding NodeId unknown: the variable holds no element")
            })
    }

    async fn write_body(&mut self, index: usize, bytes: Vec<u8>) -> Result<()> {
        let current = self.body(index)?;
        let new = RawBody {
            encoding_id: current.encoding_id.clone(),
            bytes,
        };
        let (variant, range) = if self.scalar {
            (extension_object(&new), NumericRange::None)
        } else {
            match self.write_mode {
                WriteMode::IndexRange => (
                    array_variant(std::iter::once(&new))?,
                    NumericRange::Index(index as u32),
                ),
                WriteMode::FullArray => {
                    let items = self
                        .bodies
                        .iter()
                        .enumerate()
                        .map(|(i, b)| if i == index { &new } else { b });
                    (array_variant(items)?, NumericRange::None)
                }
            }
        };
        self.send(variant, range).await?;
        self.bodies[index] = new;
        Ok(())
    }

    async fn send(&self, variant: Variant, index_range: NumericRange) -> Result<()> {
        let wv = WriteValue {
            node_id: self.node_id.clone(),
            attribute_id: AttributeId::Value as u32,
            index_range,
            value: DataValue::value_only(variant),
        };
        let results = self.session.write(std::slice::from_ref(&wv)).await?;
        let status = results
            .first()
            .copied()
            .ok_or_else(|| Error::unexpected("empty write response"))?;
        check(status, || format!("write {}", self.node_id))
    }
}

fn extension_object(body: &RawBody) -> Variant {
    Variant::ExtensionObject(ExtensionObject::new(ByteStringBody::new(
        ByteString::from(body.bytes.clone()),
        body.encoding_id.clone(),
    )))
}

fn array_variant<'a>(bodies: impl Iterator<Item = &'a RawBody>) -> Result<Variant> {
    let values: Vec<Variant> = bodies.map(extension_object).collect();
    let array = Array::new(VariantScalarTypeId::ExtensionObject, values)
        .map_err(|e| Error::unexpected(format!("cannot build array: {e}")))?;
    Ok(Variant::Array(Box::new(array)))
}

async fn read_value(session: &Session, node_id: &NodeId) -> Result<DataValue> {
    services::read_attribute(session, std::slice::from_ref(node_id), AttributeId::Value)
        .await?
        .pop()
        .ok_or_else(|| Error::unexpected("empty read response"))
}

fn parse_value(value: DataValue, node_id: &NodeId) -> Result<(Vec<RawBody>, bool)> {
    check(value.status.unwrap_or_default(), || {
        format!("read {node_id}")
    })?;
    match value.value {
        Some(Variant::Array(array)) => {
            let bodies = array
                .values
                .into_iter()
                .map(|v| match v {
                    Variant::ExtensionObject(eo) => raw_body(eo),
                    other => Err(Error::unexpected(format!(
                        "{node_id}: array element is {:?}",
                        other.type_id()
                    ))),
                })
                .collect::<Result<Vec<_>>>()?;
            Ok((bodies, false))
        }
        Some(Variant::ExtensionObject(eo)) => Ok((vec![raw_body(eo)?], true)),
        Some(Variant::Empty) | None => Ok((Vec::new(), false)),
        Some(other) => Err(Error::unexpected(format!(
            "{node_id}: value is {:?}, not ExtensionObject",
            other.type_id()
        ))),
    }
}

fn raw_body(eo: ExtensionObject) -> Result<RawBody> {
    let name = eo.type_name();
    match eo.into_inner_as::<ByteStringBody>() {
        Some(b) => {
            let encoding_id = b.encoding_id().clone();
            Ok(RawBody {
                encoding_id,
                bytes: b.into_raw().value.unwrap_or_default(),
            })
        }
        None => Err(Error::unexpected(format!(
            "ExtensionObject body is {} instead of raw bytes (type registered in the async-opcua type loader?)",
            name.unwrap_or("null")
        ))),
    }
}

/// Resolves the structured type name of a variable: its DataType's browse
/// name, or the DataType of its type definition for generic data types.
async fn resolve_type_name(session: &Session, node_id: &NodeId) -> Result<String> {
    let data_type = read_node_id_attr(session, node_id, AttributeId::DataType).await?;
    let name = read_browse_name(session, &data_type).await?;
    if !matches!(
        name.as_str(),
        "Structure" | "BaseDataType" | "String" | "ByteString"
    ) {
        return Ok(name);
    }
    let refs = services::browse(
        session,
        std::slice::from_ref(node_id),
        BrowseDirection::Forward,
        ReferenceTypeId::HasTypeDefinition.into(),
        NodeClassMask::all(),
    )
    .await?;
    let type_def = refs
        .into_iter()
        .flatten()
        .next()
        .map(|r| r.node_id.node_id)
        .ok_or_else(|| Error::unexpected(format!("{node_id}: no type definition")))?;
    let data_type = read_node_id_attr(session, &type_def, AttributeId::DataType).await?;
    read_browse_name(session, &data_type).await
}

async fn read_node_id_attr(
    session: &Session,
    node_id: &NodeId,
    attr: AttributeId,
) -> Result<NodeId> {
    let dv = services::read_attribute(session, std::slice::from_ref(node_id), attr)
        .await?
        .pop();
    let dv = dv.ok_or_else(|| Error::unexpected("empty read response"))?;
    check(dv.status.unwrap_or_default(), || {
        format!("read {attr:?} of {node_id}")
    })?;
    match dv.value {
        Some(Variant::NodeId(id)) => Ok(*id),
        other => Err(Error::unexpected(format!(
            "{attr:?} of {node_id} is {other:?}"
        ))),
    }
}

async fn read_browse_name(session: &Session, node_id: &NodeId) -> Result<String> {
    let dv = services::read_attribute(
        session,
        std::slice::from_ref(node_id),
        AttributeId::BrowseName,
    )
    .await?
    .pop();
    let dv = dv.ok_or_else(|| Error::unexpected("empty read response"))?;
    check(dv.status.unwrap_or_default(), || {
        format!("read BrowseName of {node_id}")
    })?;
    match dv.value {
        Some(Variant::QualifiedName(q)) => Ok(q.name.as_ref().to_owned()),
        other => Err(Error::unexpected(format!(
            "BrowseName of {node_id} is {other:?}"
        ))),
    }
}
