"""OPC-UA structured type binary decoder (unpacker)."""

import inspect
import logging
import struct
from datetime import datetime, timedelta, timezone

from ._constants import TYPE_FORMAT_MAP
from ._types_registry import get_type_class

# OPC-UA epoch: January 1, 1601 UTC (Windows FILETIME)
_EPOCH_DIFF = (datetime(1970, 1, 1) - datetime(1601, 1, 1)).total_seconds()


def _filetime_to_datetime(ticks):
    """Convert OPC-UA DateTime (100ns ticks since 1601-01-01) to Python datetime."""
    if ticks <= 0:
        return datetime(1601, 1, 1, tzinfo=timezone.utc)
    seconds = ticks / 10_000_000 - _EPOCH_DIFF
    try:
        return datetime.fromtimestamp(seconds, tz=timezone.utc)
    except (OSError, OverflowError, ValueError):
        return datetime(1601, 1, 1, tzinfo=timezone.utc)

logger = logging.getLogger(__name__)


class _DynamicType:
    """Simple namespace object for decoded UDTs when no registered type class exists."""

    def __init__(self, type_name, **kwargs):
        self.__type_name__ = type_name
        for k, v in kwargs.items():
            setattr(self, k, v)

    def __repr__(self):
        attrs = {k: v for k, v in self.__dict__.items() if not k.startswith('_')}
        fields = ', '.join(f'{k}={v!r}' for k, v in attrs.items())
        return f'{self.__type_name__}({fields})'


class StructuredTypeUnpacker:
    def __init__(self, structured_types, enumeration_types, verbose=False):
        self.structured_types = structured_types
        self.enumeration_types = enumeration_types
        self.verbose = verbose
        self._structured_type_index = {
            st["StructuredTypeName"]: st for st in structured_types
        }
        self._enumeration_type_index = {
            et["Name"]: et for et in enumeration_types
        }

    def _unpack_field(self, field_type, byte_buffer, offset, is_array):
        """Unpack a single field (or array of fields) from the byte buffer."""
        if is_array:
            array_length = struct.unpack_from('i', byte_buffer, offset)[0]
            offset += 4
            values = []
            for _ in range(array_length):
                value, offset = self._unpack_field(field_type, byte_buffer, offset, False)
                values.append(value)
            return values, offset

        if field_type in TYPE_FORMAT_MAP:
            if field_type == "String":
                str_len = struct.unpack_from('i', byte_buffer, offset)[0]
                offset += 4
                value = struct.unpack_from(f'{str_len}s', byte_buffer, offset)[0].decode('utf-8')
                offset += str_len
            elif field_type == "DateTime":
                fmt = TYPE_FORMAT_MAP[field_type]
                ticks = struct.unpack_from(fmt, byte_buffer, offset)[0]
                value = _filetime_to_datetime(ticks)
                offset += struct.calcsize(fmt)
            else:
                fmt = TYPE_FORMAT_MAP[field_type]
                value = struct.unpack_from(fmt, byte_buffer, offset)[0]
                offset += struct.calcsize(fmt)
            return value, offset

        custom_type = self._find_structured_type(field_type)
        if custom_type:
            return self._unpack_structured_type(custom_type, byte_buffer, offset)

        enum_type = self._find_enumeration_type(field_type)
        if enum_type:
            return self._unpack_enumeration_type(enum_type, byte_buffer, offset)

        raise ValueError(f"Unknown field type: {field_type}")

    def _unpack_structured_type(self, structured_type, byte_buffer, offset, element_index=None):
        """Unpack a structured type into a typed instance (or raw dict of values)."""
        type_name = structured_type["StructuredTypeName"]
        if element_index is not None:
            logger.debug("    [%d] %s at offset=%d", element_index, type_name, offset)
        else:
            logger.debug("    [unpack] %s at offset=%d", type_name, offset)
        init_args = {}

        cls = get_type_class(type_name)
        if cls is not None:
            param_names = set(inspect.signature(cls.__init__).parameters) - {'self'}

            for field in structured_type["Fields"]:
                field_name = field["Name"]
                value, offset = self._unpack_field(field["Type"], byte_buffer, offset, field["IsArray"])
                logger.debug("      %s = %s", field_name, value)

                if field_name not in param_names:
                    raise ValueError(
                        f"Field '{field_name}' does not match any constructor parameter in '{type_name}'."
                    )
                init_args[field_name] = value

            return cls(**init_args), offset

        # Fallback: no matching type class, return a dynamic namespace object
        for field in structured_type["Fields"]:
            value, offset = self._unpack_field(field["Type"], byte_buffer, offset, field["IsArray"])
            logger.debug("      %s = %s", field["Name"], value)
            init_args[field["Name"]] = value

        return _DynamicType(type_name, **init_args), offset

    def _unpack_enumeration_type(self, enum_type, byte_buffer, offset):
        """Unpack an enumeration value from the byte buffer."""
        value, offset = self._unpack_field("Int32", byte_buffer, offset, False)
        for field in enum_type["Fields"]:
            if value == int(field["Value"]):
                return field["Name"], offset

        return value, offset

    def _find_structured_type(self, name):
        return self._structured_type_index.get(name)

    def _find_enumeration_type(self, name):
        return self._enumeration_type_index.get(name)

    def unpack(self, structure_name, byte_buffer, offset=0, element_index=None):
        """Unpack a structured type by name from the byte buffer."""
        structured_type = self._find_structured_type(structure_name)
        if not structured_type:
            raise ValueError(f"Structured type '{structure_name}' not found.")
        return self._unpack_structured_type(structured_type, byte_buffer, offset, element_index)

    def unpack_array(self, structure_name, byte_buffer, offset=0):
        """Unpack an array of structured types from the byte buffer."""
        structured_type = self._find_structured_type(structure_name)
        if not structured_type:
            raise ValueError(f"Structured type '{structure_name}' not found.")
        array_length = struct.unpack_from('i', byte_buffer, offset)[0]
        offset += 4
        logger.debug("[unpack_array] %s: %d elements, offset=%d", structure_name, array_length, offset)
        results = []
        for i in range(array_length):
            value, offset = self._unpack_structured_type(structured_type, byte_buffer, offset, element_index=i)
            results.append(value)
        logger.debug("[unpack_array] done, final offset=%d", offset)
        return results, offset
