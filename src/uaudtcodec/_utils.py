"""Utility functions for name sanitization, path resolution, and field extraction."""

import re

from ._constants import SANITIZE_CHARS


def sanitize_name(name):
    """Sanitize a field/type name for use as a Python identifier."""
    name = name.translate(SANITIZE_CHARS)
    if name[0].isdigit():
        name = "_" + name
    return name


def _resolve_path(obj, path):
    """Resolve a dot/bracket path on an object, returning (parent, attr_or_key).

    Supports paths like:
        "Name"              -> (obj, "Name")
        "Items[2]"          -> (obj.Items, 2)
        "Items[1].Name"     -> (obj.Items[1], "Name")
        "A[0].B[2].C"      -> (obj.A[0].B[2], "C")
    """
    tokens = re.split(r'\.', path)
    current = obj

    for i, token in enumerate(tokens):
        match = re.match(r'^(\w+)\[(\d+)\]$', token)
        if match:
            field_name, index = match.group(1), int(match.group(2))
            if i == len(tokens) - 1:
                arr = getattr(current, field_name) if not isinstance(current, dict) else current[field_name]
                return arr, index
            else:
                arr = getattr(current, field_name) if not isinstance(current, dict) else current[field_name]
                current = arr[index]
        else:
            if i == len(tokens) - 1:
                return current, token
            else:
                current = getattr(current, token) if not isinstance(current, dict) else current[token]

    return current, None


def _apply_modifications(instance, modifications):
    """Apply a dict of modifications to an instance, supporting path notation."""
    for path, value in modifications.items():
        parent, key = _resolve_path(instance, path)
        if isinstance(key, int):
            parent[key] = value
        elif isinstance(parent, dict):
            parent[key] = value
        else:
            setattr(parent, key, value)


def extract_fields(obj):
    """Recursively extract all public attributes from an object into a dict."""
    if isinstance(obj, dict):
        return {k: extract_fields(v) if hasattr(v, "__dict__") and not isinstance(v, type) else
                [extract_fields(item) if hasattr(item, "__dict__") and not isinstance(item, type) else item for item in v] if isinstance(v, list) else v
                for k, v in obj.items()}
    fields = {}
    for attr in dir(obj):
        if attr.startswith("_"):
            continue
        value = getattr(obj, attr)
        if callable(value):
            continue
        if isinstance(value, list):
            fields[attr] = [extract_fields(item) if hasattr(item, "__dict__") and not isinstance(item, type) else item for item in value]
        elif hasattr(value, "__dict__") and not isinstance(value, type):
            fields[attr] = extract_fields(value)
        else:
            fields[attr] = value
    return fields
