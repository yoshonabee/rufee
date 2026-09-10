"""rufee: peewee's SQL rendering Context, implemented in Rust.

Usage::

    import rufee

    class MyDatabase(peewee.MySQLDatabase):
        context_class = rufee.Context

See ``rufee.configure`` for modes (``rust`` / ``shadow`` / ``off``) and the
fallback switch.
"""

from rufee._native import Unsupported, __version__  # noqa: F401
from rufee._context import Context, configure, stats  # noqa: F401

__all__ = ["Context", "Unsupported", "configure", "stats", "__version__"]
