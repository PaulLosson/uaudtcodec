//! Discovery and caching of the server's binary type dictionaries (`i=93`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use opcua::client::Session;
use opcua::types::{
    AttributeId, BrowseDirection, NamespaceMap, NodeClassMask, NodeId, ObjectId, ReferenceTypeId,
    Variant,
};
use serde::{Deserialize, Serialize};
use uaudt_core::{StructId, TypeDictionary};

use crate::error::{Error, Result};
use crate::services;

/// `(namespace URI, identifier)` of a node, stable across sessions
/// (identifier formatted as `i=123`, `s=Name`, ...).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeKey {
    pub namespace_uri: String,
    pub identifier: String,
}

impl NodeKey {
    pub(crate) fn new(node_id: &NodeId, namespaces: &[String]) -> Option<Self> {
        let uri = namespaces.get(node_id.namespace as usize)?;
        Some(NodeKey {
            namespace_uri: uri.clone(),
            identifier: node_id.identifier.to_string(),
        })
    }
}

/// Type described by a type description node or its "Default Binary" encoding node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeRef {
    pub type_name: String,
    pub dict_name: String,
}

struct DictEntry {
    name: String,
    xml: Vec<u8>,
    parsed: OnceLock<Result<Arc<TypeDictionary>, Arc<uaudt_core::Error>>>,
}

impl DictEntry {
    fn new(name: String, xml: Vec<u8>) -> Self {
        DictEntry {
            name,
            xml,
            parsed: OnceLock::new(),
        }
    }

    fn dictionary(&self) -> Result<Arc<TypeDictionary>> {
        self.parsed
            .get_or_init(|| {
                TypeDictionary::parse(&self.xml)
                    .map(Arc::new)
                    .map_err(Arc::new)
            })
            .clone()
            .map_err(|source| Error::Dictionary {
                name: self.name.clone(),
                source,
            })
    }

    /// Cheap pre-filter: does the XML mention `Name="type_name"`?
    fn may_contain(&self, type_name: &str) -> bool {
        let needle_dq = format!("Name=\"{type_name}\"");
        let needle_sq = format!("Name='{type_name}'");
        memchr::memmem::find(&self.xml, needle_dq.as_bytes()).is_some()
            || memchr::memmem::find(&self.xml, needle_sq.as_bytes()).is_some()
    }
}

/// All binary type dictionaries of a server, parsed lazily.
///
/// Build it once per server with [`TypeSystem::browse`] (a handful of batched
/// requests) or [`TypeSystem::load`] (no request at all), then share it
/// between [`UdtNode`](crate::UdtNode)s.
pub struct TypeSystem {
    dicts: Vec<DictEntry>,
    type_nodes: HashMap<NodeKey, TypeRef>,
    /// Data type name of variable nodes, learnt when opening them (cached on save).
    node_types: RwLock<HashMap<String, String>>,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    /// Kept for compatibility with the Python implementation's cache format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    type_name: Option<String>,
    raw_type_dicts: serde_json::Map<String, serde_json::Value>,
    type_node_map: HashMap<String, TypeRef>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    node_types: HashMap<String, String>,
}

impl TypeSystem {
    /// Builds a type system from raw dictionaries (`name`, XML).
    pub fn from_dictionaries(dicts: impl IntoIterator<Item = (String, Vec<u8>)>) -> Self {
        TypeSystem {
            dicts: dicts
                .into_iter()
                .map(|(n, x)| DictEntry::new(n, x))
                .collect(),
            type_nodes: HashMap::new(),
            node_types: RwLock::new(HashMap::new()),
        }
    }

    /// Discovers every type dictionary under `i=93`, its type description
    /// nodes and their "Default Binary" encoding nodes.
    ///
    /// Requests are batched: about four round-trips whatever the number of types.
    pub async fn browse(session: &Session) -> Result<Self> {
        let type_system: NodeId = ObjectId::OPCBinarySchema_TypeSystem.into();
        let hierarchical: NodeId = ReferenceTypeId::HierarchicalReferences.into();
        let (namespaces, dict_refs) = futures::try_join!(
            async { session.read_namespace_array().await.map_err(Error::from) },
            services::browse(
                session,
                std::slice::from_ref(&type_system),
                BrowseDirection::Forward,
                hierarchical.clone(),
                NodeClassMask::VARIABLE,
            ),
        )?;
        let ns = namespace_uris(&namespaces);

        let dict_nodes: Vec<(NodeId, String)> = dict_refs
            .into_iter()
            .flatten()
            .filter_map(|r| {
                let id = r.node_id.try_resolve(&namespaces)?.into_owned();
                Some((id, r.browse_name.name.as_ref().to_owned()))
            })
            .collect();
        let ids: Vec<NodeId> = dict_nodes.iter().map(|(id, _)| id.clone()).collect();

        let (values, children) = futures::try_join!(
            services::read_attribute(session, &ids, AttributeId::Value),
            services::browse(
                session,
                &ids,
                BrowseDirection::Forward,
                hierarchical,
                NodeClassMask::VARIABLE
            ),
        )?;

        let mut dicts = Vec::with_capacity(dict_nodes.len());
        let mut type_nodes = HashMap::new();
        let mut desc_nodes: Vec<(NodeId, TypeRef)> = Vec::new();
        for (((_, dict_name), value), refs) in dict_nodes.iter().zip(values).zip(children) {
            let xml = match value.value {
                Some(Variant::ByteString(b)) => b.value,
                Some(Variant::String(s)) => s.value().clone().map(String::into_bytes),
                _ => None,
            };
            let Some(xml) = xml else { continue };
            dicts.push(DictEntry::new(dict_name.clone(), xml));
            for r in refs {
                let Some(id) = r.node_id.try_resolve(&namespaces) else {
                    continue;
                };
                let tref = TypeRef {
                    type_name: r.browse_name.name.as_ref().to_owned(),
                    dict_name: dict_name.clone(),
                };
                if let Some(key) = NodeKey::new(&id, &ns) {
                    type_nodes.insert(key, tref.clone());
                }
                desc_nodes.push((id.into_owned(), tref));
            }
        }

        // Encoding nodes reference their description with HasDescription.
        let desc_ids: Vec<NodeId> = desc_nodes.iter().map(|(id, _)| id.clone()).collect();
        let encodings = services::browse(
            session,
            &desc_ids,
            BrowseDirection::Inverse,
            ReferenceTypeId::HasDescription.into(),
            NodeClassMask::all(),
        )
        .await?;
        for ((_, tref), refs) in desc_nodes.iter().zip(encodings) {
            for r in refs {
                if r.browse_name.name.as_ref() != "Default Binary" {
                    continue;
                }
                if let Some(key) = r
                    .node_id
                    .try_resolve(&namespaces)
                    .and_then(|id| NodeKey::new(&id, &ns))
                {
                    type_nodes.insert(key, tref.clone());
                }
            }
        }

        Ok(TypeSystem {
            dicts,
            type_nodes,
            node_types: RwLock::new(HashMap::new()),
        })
    }

    /// Loads the type system from a cache file (no server request).
    ///
    /// Also reads cache files written by the Python implementation (`types_cache=`).
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let data = std::fs::read(path)?;
        let cache: CacheFile =
            serde_json::from_slice(&data).map_err(|e| Error::Cache(e.to_string()))?;
        let mut dicts = Vec::with_capacity(cache.raw_type_dicts.len());
        for (name, v) in cache.raw_type_dicts {
            let b64 = v
                .as_str()
                .ok_or_else(|| Error::Cache(format!("dictionary '{name}' is not a string")))?;
            let xml = B64
                .decode(b64)
                .map_err(|e| Error::Cache(format!("dictionary '{name}': {e}")))?;
            dicts.push(DictEntry::new(name, xml));
        }
        let mut type_nodes = HashMap::with_capacity(cache.type_node_map.len());
        for (k, v) in cache.type_node_map {
            let (uri, id) = k
                .split_once('|')
                .ok_or_else(|| Error::Cache(format!("invalid key '{k}'")))?;
            type_nodes.insert(
                NodeKey {
                    namespace_uri: uri.to_owned(),
                    identifier: id.to_owned(),
                },
                v,
            );
        }
        Ok(TypeSystem {
            dicts,
            type_nodes,
            node_types: RwLock::new(cache.node_types),
        })
    }

    /// Writes the type system (and the node types learnt so far) to a cache file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let node_types = self
            .node_types
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let cache = CacheFile {
            type_name: (node_types.len() == 1)
                .then(|| node_types.values().next().cloned())
                .flatten(),
            raw_type_dicts: self
                .dicts
                .iter()
                .map(|d| {
                    (
                        d.name.clone(),
                        serde_json::Value::String(B64.encode(&d.xml)),
                    )
                })
                .collect(),
            type_node_map: self
                .type_nodes
                .iter()
                .map(|(k, v)| (format!("{}|{}", k.namespace_uri, k.identifier), v.clone()))
                .collect(),
            node_types,
        };
        let json = serde_json::to_vec_pretty(&cache).map_err(|e| Error::Cache(e.to_string()))?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Loads `path` if it exists, otherwise browses the server and saves to `path`.
    pub async fn load_or_browse(session: &Session, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.exists() {
            return Self::load(path);
        }
        let ts = Self::browse(session).await?;
        ts.save(path)?;
        Ok(ts)
    }

    /// Names of the dictionaries.
    pub fn dictionary_names(&self) -> impl Iterator<Item = &str> {
        self.dicts.iter().map(|d| d.name.as_str())
    }

    /// Raw XML of a dictionary.
    pub fn raw_dictionary(&self, name: &str) -> Option<&[u8]> {
        self.dicts
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.xml.as_slice())
    }

    /// Parsed dictionary by name (parsed on first use, then shared).
    pub fn dictionary(&self, name: &str) -> Result<Arc<TypeDictionary>> {
        self.dicts
            .iter()
            .find(|d| d.name == name)
            .ok_or_else(|| Error::TypeNotFound(name.to_owned()))?
            .dictionary()
    }

    /// Type description / encoding node map (`NodeKey` -> type and dictionary).
    pub fn type_nodes(&self) -> &HashMap<NodeKey, TypeRef> {
        &self.type_nodes
    }

    /// Finds the dictionary declaring structured type `type_name`.
    ///
    /// Only dictionaries whose XML mentions the name are parsed.
    pub fn find_struct(&self, type_name: &str) -> Result<(Arc<TypeDictionary>, StructId)> {
        // Prefer the dictionary named by the type nodes, then scan.
        let hinted = self
            .type_nodes
            .values()
            .find(|t| t.type_name == type_name)
            .map(|t| t.dict_name.as_str());
        let candidates = hinted
            .into_iter()
            .filter_map(|n| self.dicts.iter().find(|d| d.name == n))
            .chain(self.dicts.iter().filter(|d| d.may_contain(type_name)));
        for d in candidates {
            if let Ok(dict) = d.dictionary() {
                if let Some(id) = dict.struct_id(type_name) {
                    return Ok((dict, id));
                }
            }
        }
        Err(Error::TypeNotFound(type_name.to_owned()))
    }

    pub(crate) fn node_type(&self, node: &str) -> Option<String> {
        self.node_types
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(node)
            .cloned()
    }

    pub(crate) fn remember_node_type(&self, node: String, type_name: String) {
        self.node_types
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(node, type_name);
    }
}

/// Namespace URIs indexed by namespace index.
pub(crate) fn namespace_uris(map: &NamespaceMap) -> Vec<String> {
    let known = map.known_namespaces();
    let len = known.values().map(|&i| i as usize + 1).max().unwrap_or(0);
    let mut out = vec![String::new(); len];
    for (uri, &i) in known {
        out[i as usize] = uri.clone();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &[u8] = include_bytes!("../../uaudt-core/tests/fixtures/recipe.bsd");

    #[test]
    fn finds_struct_and_filters_dictionaries() {
        let ts = TypeSystem::from_dictionaries([
            ("Broken".to_owned(), b"<not xml".to_vec()),
            ("Demo".to_owned(), XML.to_vec()),
        ]);
        let (dict, id) = ts.find_struct("Recipe").unwrap();
        assert_eq!(&*dict.struct_type(id).name, "Recipe");
        // The broken dictionary is never parsed: its XML does not mention the type.
        assert!(ts.dicts[0].parsed.get().is_none());
        assert!(matches!(
            ts.find_struct("Nope"),
            Err(Error::TypeNotFound(_))
        ));
        assert!(matches!(
            ts.dictionary("Broken"),
            Err(Error::Dictionary { .. })
        ));
    }

    #[test]
    fn reads_python_cache_format_and_roundtrips() {
        // Format written by the Python UdtHandler(types_cache=...).
        let json = serde_json::json!({
            "type_name": "Recipe",
            "raw_type_dicts": { "DemoTypes": B64.encode(XML) },
            "type_node_map": {
                "urn:uaudtcodec:demo|i=4000": { "type_name": "Recipe", "dict_name": "DemoTypes" },
                "urn:uaudtcodec:demo|s=Desc.Recipe": { "type_name": "Recipe", "dict_name": "DemoTypes" }
            }
        });
        let dir = std::env::temp_dir();
        let path = dir.join(format!("uaudt-py-cache-{}.json", std::process::id()));
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();

        let ts = TypeSystem::load(&path).unwrap();
        assert!(ts.find_struct("Recipe").is_ok());
        let key = NodeKey {
            namespace_uri: "urn:uaudtcodec:demo".into(),
            identifier: "i=4000".into(),
        };
        assert_eq!(ts.type_nodes()[&key].type_name, "Recipe");

        ts.remember_node_type("ns=2;s=Recipes".into(), "Recipe".into());
        ts.save(&path).unwrap();
        let back = TypeSystem::load(&path).unwrap();
        assert_eq!(back.type_nodes(), ts.type_nodes());
        assert_eq!(back.node_type("ns=2;s=Recipes").as_deref(), Some("Recipe"));
        assert_eq!(back.raw_dictionary("DemoTypes"), Some(XML));
        // Still readable by Python: "type_name" written for a single node.
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["type_name"], "Recipe");
        std::fs::remove_file(&path).ok();
    }
}
