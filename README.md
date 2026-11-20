# uaudtcodec (Rust)

High-performance OPC-UA structured type (UDT) codec and client, driven by the
server's XML binary type dictionaries (`opc:TypeDictionary` under `i=93`).

Rust rewrite of the original Python package (still in `src/uaudtcodec`, see
[README.python.md](README.python.md)), built on
[async-opcua](https://crates.io/crates/async-opcua) and tokio.

| Crate | Role |
|---|---|
| [`uaudt-core`](crates/uaudt-core) | XML dictionary parser + binary decoder/encoder + field paths + byte-level patching. No OPC-UA dependency. |
| [`uaudt-client`](crates/uaudt-client) | Type discovery (`TypeSystem`) and read/write of UDT variables (`UdtNode`) through an async-opcua `Session`. |

## Quick start

```toml
[dependencies]
uaudt-client = { git = "https://github.com/PaulLosson/uaudtcodec" }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

```rust
use uaudt_client::opcua::client::{ClientBuilder, IdentityToken};
use uaudt_client::opcua::types::{EndpointDescription, MessageSecurityMode, NodeId, UserTokenPolicy};
use uaudt_client::{TypeSystem, UdtNode, uaudt_core::Value};

#[tokio::main]
async fn main() -> uaudt_client::Result<()> {
    // async-opcua session. `with_udt_limits` raises the default decoding limits
    // (64 KiB byte strings, 1000-element arrays) that dictionaries exceed.
    let mut client = uaudt_client::with_udt_limits(ClientBuilder::new())
        .application_name("my app")
        .application_uri("urn:my-app")
        .trust_server_certs(true)
        .client()
        .unwrap();
    let endpoint: EndpointDescription = ("opc.tcp://plc:4840/", "None",
        MessageSecurityMode::None, UserTokenPolicy::anonymous()).into();
    let (session, event_loop) = client.connect_to_matching_endpoint(endpoint, IdentityToken::Anonymous).await?;
    let _loop = event_loop.spawn();
    session.wait_for_connection().await;

    // Browse the dictionaries once, then reuse the cache file (no request at all).
    let types = TypeSystem::load_or_browse(&session, "types.json").await?;

    let node_id: NodeId = "ns=4;s=MyNode.Data".parse().unwrap();
    let mut node = UdtNode::open(session.clone(), node_id, &types).await?;
    let dict = node.dictionary().clone();

    // Read
    let items = node.read().await?;
    println!("{}", Value::Struct(items[0].clone()).display(&dict));
    let name = dict.get_str(&items[0], "SubItems[1].Description")?;

    // Partial update: only the targeted bytes are re-encoded
    node.patch(1, &[("Name", "Updated".into()), ("SubItems[1].Description", "Modified".into())]).await?;

    // Edit a decoded copy
    node.edit(1, |dict, v| dict.set_str(v, "Name", "Edited".into())).await?;

    // Replace a full element
    let mut v = node.new_value()?;
    dict.set_str(&mut v, "Name", "New".into())?;
    node.write(0, &v).await?;

    types.save("types.json")?; // also caches the node's type: next open = 1 request
    Ok(())
}
```

Runnable version: [`crates/uaudt-client/examples/read_write.rs`](crates/uaudt-client/examples/read_write.rs).

### Codec only

```rust
use uaudt_core::{TypeDictionary, Value};

let dict = TypeDictionary::parse(xml_bytes)?;
let recipe = dict.require_struct("Recipe")?;

let value = dict.decode(recipe, body)?;                 // bytes -> StructValue
let bytes = dict.encode(&value)?;                       // StructValue -> bytes

let path = dict.compile_path(recipe, "SubItems[1].Description")?; // compile once
let v = dict.read_path(body, &path)?;                   // decode one field only
let patched = dict.patch(body, &[(&path, "New".into())])?; // rewrite only those bytes
```

## API overview

**`TypeSystem`** — all dictionaries of a server, parsed lazily (only the dictionary declaring the requested type is parsed).
`browse(&session)` · `load(path)` · `save(path)` · `load_or_browse(&session, path)` · `find_struct(name)` · `dictionary(name)` · `type_nodes()`.
The cache file format is compatible with the Python `types_cache=` files in both directions.

**`UdtNode`** — one variable (array or scalar of a structured type). Raw bodies are cached; writes update the cache on success, without a re-read.

| Method | Description |
|---|---|
| `open(session, node_id, &types)` | Resolves the data type and reads the value **in parallel** |
| `open_with_type(session, node_id, type_name, &types)` | One request when the type is known |
| `read()` / `refresh()` | Read (and decode) from the server |
| `decode_all()` / `decode(i)` / `read_path(i, &path)` | Decode from the cache, no request |
| `write(i, &value)` | Replace one element |
| `patch(i, &[("A.B[2]", value)])` / `patch_paths(i, &[(&path, value)])` | Byte-level partial update |
| `edit(i, \|dict, v\| ...)` | Decode, modify, write back |
| `write_all(&values)` | Replace the whole array |
| `set_write_mode(WriteMode::IndexRange)` | Write only the element (server dependent, see below) |

**`Value`** — dynamic value: `Boolean`, `SByte`…`UInt64`, `Float`, `Double`, `String` (inline up to 24 bytes), `DateTime` (ticks, see `uaudt_core::datetime`), `Guid`, `ByteString`, `StatusCode`, `Enum`, `Struct`, `Array`.
Setters (`set`, `set_str`, `patch`) convert values to the schema type: integers ↔ floats with range checks, enum by name or number.

## Performance

Same workloads for both implementations (`bench/`, 4-core container, release build).

Codec only — 1 000 `Recipe` bodies with 10 sub-items each
(`cargo bench -p uaudt-core` vs `python bench/bench_python.py`):

| Operation | Python | Rust | Speed-up |
|---|---|---|---|
| Parse dictionary | 560 µs | 11.7 µs | ×48 |
| Decode 1 000 elements | 38.1 ms | 1.9 ms (1.3 ms with mimalloc) | ×20 (×29) |
| Encode 1 000 elements | 21.5 ms | 0.46 ms | ×47 |
| Patch 2 fields of one element | 68 µs | 0.36 µs | ×190 |

End to end against the demo server (`crates/uaudt-client/examples/bench_e2e.rs` vs
`bench/bench_python_e2e.py`). The server is Python asyncua, so these Rust timings
are mostly server time — a PLC would widen the gap:

| Scenario | Python `UdtHandler` | Rust `UdtNode` | Speed-up |
|---|---|---|---|
| Init: browse `i=93` (incl. the standard `Opc.Ua` dictionary) + open | 426 ms | 9.7 ms | ×44 |
| Read + decode 1 000 elements | 52 ms | 4.9 ms | ×11 |
| Patch 1 element of a 1 000-element array | 18.9 ms | 4.6 ms | ×4 |
| Patch 1 element of a 3-element array | 1.9 ms | 0.29 ms | ×6.5 |

Where it comes from:
- type discovery batches Browse/Read requests (≈4 round-trips whatever the number of types, instead of several per type);
- type names are resolved to indices once; decoding never compares strings; struct fields are stored positionally;
- patches rewrite only the targeted bytes, jumping over fixed-size fields by precomputed offsets;
- short strings are stored inline, errors are boxed to keep hot-path results small.

For allocation-heavy decoding, applications can use a faster global allocator such as `mimalloc`.

## Supported types and limitations

- Builtins: Boolean, SByte, Byte, Int16 (`short`), UInt16, Int32, UInt32, Int64, UInt64, Float, Double, String/CharArray, DateTime, Guid, ByteString/XmlElement, StatusCode; enumerations of 8, 16 or 32 bits; nested structures; arrays (`LengthField`).
- Not supported (reported as `Unsupported`/`UnresolvedType` when used, without preventing the rest of the dictionary from loading): optional fields (`SwitchField`), bit fields (`opc:Bit`, bit-sized enums), and builtins such as NodeId, LocalizedText, QualifiedName, Variant, ExtensionObject inside a UDT.
- `WriteMode::IndexRange` sends only the modified element. Some servers refuse it, and some (e.g. Python asyncua) answer Good but replace the whole array: validate against your server first. The default `FullArray` mode matches the Python behaviour.
- Python extras not ported (on purpose, for now): JSON/CSV/Excel exports, `read_csv`, registered Python classes.

## Tests

```bash
cargo test --workspace
```

The client integration tests need the demo server (Python asyncua):

```bash
pip install asyncua beautifulsoup4 lxml
python crates/uaudt-client/tests/demo_server.py 48400 &
UAUDT_TEST_ENDPOINT=opc.tcp://127.0.0.1:48400/ cargo test -p uaudt-client
```

`crates/uaudt-core/tests/fixtures/recipe.bin` was produced by the Python encoder: the Rust codec decodes it and re-encodes it byte for byte.

## License

Apache-2.0
