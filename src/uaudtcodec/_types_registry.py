"""Configurable types registry replacing the hard-coded `import Types`."""

import inspect

_types_module = None


def register_types_module(module):
    """Register a module containing OPC-UA structured type classes.

    Args:
        module: A Python module whose attributes are type classes
                (e.g. ``import my_types; register_types_module(my_types)``).
    """
    global _types_module
    _types_module = module


def get_type_class(type_name):
    """Look up a type class by name from the registered module.

    Returns:
        The class if found, otherwise ``None``.
    """
    if _types_module is None:
        return None
    cls = getattr(_types_module, type_name, None)
    if cls is not None and isinstance(cls, type):
        return cls
    return None
