"""OPC-UA structured type binary encoder."""

import struct
from datetime import datetime

from ._constants import TYPE_FORMAT_MAP

# Seconds between 1601-01-01 and 1970-01-01
_EPOCH_DIFF = 11644473600


def _datetime_to_filetime(value):
    """Convert a datetime or int to OPC-UA DateTime ticks (100ns since 1601-01-01)."""
    if isinstance(value, datetime):
        return int((value.timestamp() + _EPOCH_DIFF) * 10_000_000)
    return int(value)
from ._decoder import StructuredTypeUnpacker
from ._types_registry import get_type_class
from ._utils import _apply_modifications


def _pack_string(value):
    """Pack a string as length-prefixed UTF-8 bytes."""
    if value is None:
        value = ""
    encoded = value.encode('utf-8')
    return struct.pack(f'i{len(encoded)}s', len(encoded), encoded)


class StructuredTypeEncoder:
    def __init__(self, structured_types, enumeration_types):
        self.structured_types = structured_types
        self.enumeration_types = enumeration_types
        self._structured_type_index = {
            st["StructuredTypeName"]: st for st in structured_types
        }
        self._enumeration_type_index = {
            et["Name"]: et for et in enumeration_types
        }

    def _find_structured_type(self, name):
        return self._structured_type_index.get(name)

    def _find_enumeration_type(self, name):
        return self._enumeration_type_index.get(name)

    def _get_default_value(self, field_type):
        """Return a sensible default for a given field type."""
        if field_type in ("Int32", "short"):
            return 0
        if field_type == "Float":
            return 0.0
        if field_type == "Boolean":
            return False
        if field_type == "String":
            return ""
        if field_type == "DateTime":
            return 0
        # Structured type: try to instantiate from types registry
        cls = get_type_class(field_type)
        if cls is not None:
            return cls()
        # Enumeration: return first value name
        enum_type = self._find_enumeration_type(field_type)
        if enum_type and enum_type["Fields"]:
            return enum_type["Fields"][0]["Name"]
        raise ValueError(f"Unknown field type: {field_type}")

    def _pack_field(self, field_type, value, is_array):
        """Encode a field into bytes."""
        if is_array:
            if value is None:
                value = []
            packed = struct.pack('i', len(value))
            for item in value:
                packed += self._pack_field(field_type, item, False)
            return packed

        # Handle None with defaults
        if value is None:
            value = self._get_default_value(field_type)

        # Primitive types
        if field_type in TYPE_FORMAT_MAP:
            if field_type == "String":
                return _pack_string(value)
            fmt = TYPE_FORMAT_MAP[field_type]
            if field_type == "DateTime":
                value = _datetime_to_filetime(value)
            elif field_type in ("Int32", "short"):
                value = int(value)
            elif field_type == "Float":
                value = float(value)
            elif field_type == "Boolean":
                value = bool(value)
            return struct.pack(fmt, value)

        # Structured type (recursive)
        st = self._find_structured_type(field_type)
        if st:
            return self._pack_structured_type(st, value)

        # Enumeration type
        et = self._find_enumeration_type(field_type)
        if et:
            return self._pack_enumeration(et, value)

        raise ValueError(f"Unknown field type: {field_type}")

    def _pack_structured_type(self, structured_type, instance):
        """Pack a structured type into bytes. Accepts a typed instance or a dict."""
        packed = b''
        for field in structured_type["Fields"]:
            field_name = field["Name"]
            field_type = field["Type"]
            is_array = field["IsArray"]

            if isinstance(instance, dict):
                value = instance.get(field_name)
            else:
                value = getattr(instance, field_name, None)

            packed += self._pack_field(field_type, value, is_array)
        return packed

    def _pack_enumeration(self, enum_type, value):
        """Pack an enumeration value. Accepts a name (str) or an int."""
        if isinstance(value, int):
            return struct.pack('i', value)
        for field in enum_type["Fields"]:
            if field["Name"] == value:
                return struct.pack('i', int(field["Value"]))
        raise ValueError(
            f"Invalid enumeration value '{value}' for enum '{enum_type['Name']}'. "
            f"Valid values: {[f['Name'] for f in enum_type['Fields']]}"
        )

    def encode(self, instance):
        """Encode a typed instance (or dict) into bytes."""
        if isinstance(instance, dict):
            raise ValueError("encode() requires a typed instance. Use encode_by_name() for dicts.")
        type_name = getattr(instance, '__type_name__', instance.__class__.__name__)
        st = self._find_structured_type(type_name)
        if not st:
            raise ValueError(f"Structured type '{type_name}' not found.")
        return self._pack_structured_type(st, instance)

    def encode_by_name(self, type_name, data):
        """Encode a dict into bytes for a given structured type name."""
        st = self._find_structured_type(type_name)
        if not st:
            raise ValueError(f"Structured type '{type_name}' not found.")
        return self._pack_structured_type(st, data)

    def _get_unpacker(self):
        """Lazy-create an unpacker using the same type definitions."""
        if not hasattr(self, '_unpacker'):
            self._unpacker = StructuredTypeUnpacker(
                self.structured_types, self.enumeration_types
            )
        return self._unpacker

    def decode(self, type_name, byte_buffer):
        """Decode bytes into a typed instance. Shortcut for the unpacker."""
        return self._get_unpacker().unpack(type_name, byte_buffer)[0]

    def decode_array(self, type_name, byte_buffer):
        """Decode bytes containing an array of structured types into a list of typed instances."""
        return self._get_unpacker().unpack_array(type_name, byte_buffer)[0]

    def decode_list(self, type_name, raw_values):
        """Decode a list of OPC-UA ExtensionObjects (or raw bytes) into typed instances."""
        unpacker = self._get_unpacker()
        result = []
        for value in raw_values:
            data = value.Body if hasattr(value, 'Body') else value
            result.append(unpacker.unpack(type_name, data)[0])
        return result

    def encode_list(self, instances):
        """Encode a list of typed instances into a list of bytes."""
        return [self.encode(inst) for inst in instances]

    def patch(self, type_name, byte_buffer, modifications):
        """Decode, apply modifications, and re-encode a single value."""
        instance = self.decode(type_name, byte_buffer)
        _apply_modifications(instance, modifications)
        return self.encode(instance)

    def patch_list(self, type_name, raw_values, modifications_by_index):
        """Decode a list, apply modifications to specific indices, and re-encode."""
        instances = self.decode_list(type_name, raw_values)
        for idx, modifications in modifications_by_index.items():
            if idx < 0 or idx >= len(instances):
                raise IndexError(
                    f"Index {idx} out of range for list of {len(instances)} elements."
                )
            _apply_modifications(instances[idx], modifications)
        return self.encode_list(instances)
