# uaudtcodec

Generic OPC-UA structured type encoder/decoder from XML type dictionaries.

Parses OPC-UA binary schema dictionaries (XML), decodes/encodes binary buffers into typed Python objects, and provides a high-level handler for reading/writing UDTs on an OPC-UA server.

## Installation

```bash
pip install uaudtcodec
```

With OPC-UA server support (requires `asyncua`):

```bash
pip install uaudtcodec[opcua]
```

## Quick Start

### Using UdtHandler (high-level API)

```python
import asyncua.sync
from uaudtcodec import UdtHandler

server = asyncua.sync.Client(url="opc.tcp://localhost:4840")
server.connect()

# Auto-discovers the type dictionary under i=93
udt = UdtHandler(server, "ns=4;s=MyNode.Data")

# Or with explicit dict_node_id (faster, skips discovery):
# udt = UdtHandler(server, "ns=4;s=MyNode.Data", dict_node_id="ns=2;i=1234")

# Read
items     = udt.read()                  # UdtResult of typed objects
items_dict = udt.read(as_dict=True)     # UdtResult of dicts

# Export
items_dict.to_json("output.json")
items_dict.to_csv("output.csv")

# Read from CSV export
results = udt.read_csv("export.csv", as_dict=True)
results.to_json("decoded.json")
results.to_csv("decoded.csv")

# Write: replace a full element
items[0].Name = "NewValue"
udt.write(index=0, instance=items[0])

# Write: partial update with dot-notation paths
udt.patch(index=1, modifications={
    "Name": "Updated",
    "SubItems[1].Description": "Modified",
})

# Write: edit in-place via context manager
with udt.edit(index=1) as item:
    item.Name = "Edited"
    item.SubItems[0].Description = "Changed"

# Write: from a dict
udt.write(index=0, data={"Name": "FromDict", "SubItems": [...]})

server.disconnect()
```

### Caching types (skip server browsing)

```python
# types_cache: if the file exists, loads types from it.
# If not, browses the server and saves to the file automatically.
udt = UdtHandler(server, "ns=4;s=MyNode.Data", types_cache="types.json")
```

### Using the low-level API

```python
from uaudtcodec import StructuredTypeParser, StructuredTypeUnpacker, StructuredTypeEncoder

# Parse XML type dictionary
parser           = StructuredTypeParser(xml_bytes)
structured_types = parser.get_structured_types()
enumeration_types = parser.get_enumeration_types()

# Decode binary buffer
unpacker        = StructuredTypeUnpacker(structured_types, enumeration_types)
result, offset  = unpacker.unpack("MyTypeName", byte_buffer)

# Encode back to binary
encoder = StructuredTypeEncoder(structured_types, enumeration_types)
encoded = encoder.encode(result)
```

## Architecture

```
src/uaudtcodec/
  _constants.py      # TYPE_FORMAT_MAP, PY_TYPE_FORMAT_MAP
  _utils.py          # sanitize_name(), extract_fields()
  _types_registry.py # register_types_module(), get_type_class()
  _parser.py         # StructuredTypeParser  — XML dict parsing
  _decoder.py        # StructuredTypeUnpacker — binary -> Python objects
  _encoder.py        # StructuredTypeEncoder  — Python objects -> binary
  _handler.py        # UdtHandler, UdtResult  — high-level OPC-UA API
```

### Three-layer API

| Layer          | Classes                                          | Description                                        |
|----------------|--------------------------------------------------|----------------------------------------------------|
| **Low-level**  | `StructuredTypeParser`, `StructuredTypeUnpacker`  | XML parsing and binary decoding                    |
| **Mid-level**  | `StructuredTypeEncoder`                           | Encoding, decoding, patching with type awareness   |
| **High-level** | `UdtHandler`, `UdtResult`                         | Full OPC-UA read/write with auto-discovery, export |

## API Reference

### UdtHandler

```python
UdtHandler(client, node_id, dict_node_id=None, types_cache=None)
```

| Parameter      | Description                                                                          |
|----------------|--------------------------------------------------------------------------------------|
| `client`       | Connected `asyncua.sync.Client` instance                                             |
| `node_id`      | NodeId of the target variable (e.g. `"ns=4;s=MyNode.Data"`)                          |
| `dict_node_id` | Optional. NodeId of the type dictionary. Auto-discovered if `None`                   |
| `types_cache`  | Optional. Path to JSON cache file. Loads if exists, saves after browsing if not       |

**Properties:**

| Property         | Type   | Description                                                                    |
|------------------|--------|--------------------------------------------------------------------------------|
| `type_name`      | `str`  | Resolved UDT type name                                                         |
| `count`          | `int`  | Number of elements in the node value                                           |
| `type_node_map`  | `dict` | Mapping of `(namespace_uri, identifier)` to `(type_name, dict_name)`           |
| `raw_type_dicts` | `dict` | All discovered dictionaries as `{dict_name: xml_bytes}`                        |
| `type_dicts`     | `dict` | Parsed dictionaries `{dict_name: {"structured_types", "enumeration_types"}}`   |

**Methods:**

| Method                                  | Returns           | Description                                      |
|-----------------------------------------|-------------------|--------------------------------------------------|
| `read(as_dict=False)`                   | `UdtResult`       | Decode all elements from the server              |
| `read_csv(filepath, as_dict=False)`     | `UdtResult`       | Decode UDT values from a CSV export file         |
| `write(index, instance=None, data=None)`| —                 | Write a full element (typed object or dict)       |
| `patch(index, modifications)`           | —                 | Partial update with dot-notation paths            |
| `edit(index)`                           | context manager   | Decode, yield for modification, re-encode, write |
| `save_types(filepath)`                  | —                 | Serialize types to JSON file for reuse            |

### UdtResult

Extends `list`. Returned by `read()` and `read_csv()`. Supports indexing, iteration, `len()`, etc.

| Method                            | Description                                                          |
|-----------------------------------|----------------------------------------------------------------------|
| `to_json(filepath, indent=2)`     | Export to JSON file                                                  |
| `to_csv(filepath, delimiter=';')` | Export to CSV file (nested structures flattened, lists as rows)      |

### StructuredTypeParser

```python
StructuredTypeParser(dict_value)
```

| Method                         | Returns                                        |
|--------------------------------|------------------------------------------------|
| `get_structured_types()`       | `list[dict]` — parsed structured type defs     |
| `get_enumeration_types()`      | `list[dict]` — parsed enumeration defs         |
| `find_structured_type(name)`   | `dict` or `None`                               |

### StructuredTypeUnpacker

```python
StructuredTypeUnpacker(structured_types, enumeration_types, verbose=False)
```

| Method                                                  | Returns              |
|---------------------------------------------------------|----------------------|
| `unpack(name, byte_buffer, offset=0, element_index=None)` | `(instance, offset)` |
| `unpack_array(name, byte_buffer, offset=0)`             | `(list, offset)`     |

### StructuredTypeEncoder

```python
StructuredTypeEncoder(structured_types, enumeration_types)
```

| Method                                        | Returns          |
|-----------------------------------------------|------------------|
| `encode(instance)`                            | `bytes`          |
| `encode_by_name(type_name, data)`             | `bytes`          |
| `encode_list(instances)`                      | `list[bytes]`    |
| `decode(type_name, byte_buffer)`              | typed instance   |
| `decode_array(type_name, byte_buffer)`        | `list`           |
| `decode_list(type_name, raw_values)`          | `list`           |
| `patch(type_name, byte_buffer, modifications)`| `bytes`          |

### Utilities

| Function                         | Description                                                  |
|----------------------------------|--------------------------------------------------------------|
| `sanitize_name(name)`            | Convert OPC-UA names to valid Python identifiers             |
| `extract_fields(obj)`            | Recursively extract public attributes to dict                |
| `register_types_module(module)`  | Register a module containing type classes for the unpacker   |

## Supported OPC-UA Types

| OPC-UA Type | Python Type | struct format |
|-------------|-------------|---------------|
| `Float`     | `float`     | `f`           |
| `Int32`     | `int`       | `i`           |
| `String`    | `str`       | `s`           |
| `Boolean`   | `bool`      | `?`           |
| `DateTime`  | `int`       | `q`           |
| `short`     | `int`       | `h`           |

Custom structured types and enumerations are resolved from the XML dictionary.

## CSV Export Format

The `read_csv()` method expects CSV files with columns:
`nodeid,datatype,opcservertimestamp,opcsourcetimestamp,statuscode,opcvalue`

Where `opcvalue` format is: `count;namespace_uri;encoding_node_id;base64_element1;base64_element2;...`

Each element is a separate base64-encoded binary blob.
