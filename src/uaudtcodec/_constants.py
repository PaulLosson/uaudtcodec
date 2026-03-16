"""Shared constants for OPC-UA type mapping and name sanitization."""

# Mapping of OPC-UA type names to struct format characters
TYPE_FORMAT_MAP = {
    "Float": 'f',
    "Int32": 'i',
    "String": 's',
    "Boolean": "?",
    "DateTime": "q",
    "short": "h",
}

PY_TYPE_FORMAT_MAP = {
    "Float": 'float',
    "Int32": 'int',
    "String": 'str',
    "Boolean": "bool",
    "DateTime": "int",
    "short": "int",
}

SANITIZE_CHARS = str.maketrans({
    " ": "_", "(": "_", ")": "_", "-": "_", "/": "_",
})
