"""uaudtcodec — Generic OPC-UA structured type encoder/decoder."""

from ._parser import StructuredTypeParser
from ._decoder import StructuredTypeUnpacker
from ._encoder import StructuredTypeEncoder
from ._types_registry import register_types_module
from ._utils import extract_fields, sanitize_name
from ._constants import TYPE_FORMAT_MAP, PY_TYPE_FORMAT_MAP

__all__ = [
    "StructuredTypeParser",
    "StructuredTypeUnpacker",
    "StructuredTypeEncoder",
    "register_types_module",
    "extract_fields",
    "sanitize_name",
    "TYPE_FORMAT_MAP",
    "PY_TYPE_FORMAT_MAP",
    "UdtHandler",
    "UdtResult",
]


def __getattr__(name):
    if name == "UdtHandler":
        from ._handler import UdtHandler
        return UdtHandler
    if name == "UdtResult":
        from ._handler import UdtResult
        return UdtResult
    raise AttributeError(f"module 'uaudtcodec' has no attribute {name!r}")
