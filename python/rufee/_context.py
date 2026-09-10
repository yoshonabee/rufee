"""The ``peewee.Context`` subclass that routes top-level renders to Rust."""

from __future__ import annotations

import logging
import os
import threading
import warnings
from dataclasses import dataclass, field

import peewee

from rufee import _native
from rufee._native import Unsupported

# Captured at import time: test harnesses may rebind ``peewee.Context`` to
# ``rufee.Context`` and we must keep rendering the reference with the original.
_PeeweeContext = peewee.Context
_PeeweeState = peewee.State

log = logging.getLogger("rufee")

SUPPORTED_PEEWEE = "3.17."

MODE_RUST = "rust"      # render with Rust, fall back to peewee when unsupported
MODE_SHADOW = "shadow"  # render both, log mismatches, return peewee's result
MODE_OFF = "off"        # pure pass-through to peewee


@dataclass
class _Config:
    mode: str = MODE_RUST
    fallback: bool = True
    # Set when peewee's version is outside the supported range.
    version_ok: bool = True


@dataclass
class Stats:
    """Counters, mostly for tests and for shadow-mode monitoring."""

    rust: int = 0        # queries rendered by Rust and returned
    fallback: int = 0    # queries rendered by peewee after Rust declined
    shadow: int = 0      # shadow comparisons performed
    mismatch: int = 0    # shadow comparisons that differed
    _lock: threading.Lock = field(default_factory=threading.Lock, repr=False)

    def _bump(self, name: str) -> None:
        with self._lock:
            setattr(self, name, getattr(self, name) + 1)

    def reset(self) -> None:
        with self._lock:
            self.rust = self.fallback = self.shadow = self.mismatch = 0


stats = Stats()
_config = _Config()


def _env_mode() -> str:
    mode = os.environ.get("RUFEE_MODE", MODE_RUST).lower()
    if mode not in (MODE_RUST, MODE_SHADOW, MODE_OFF):
        warnings.warn(f"RUFEE_MODE={mode!r} is not one of rust/shadow/off; using rust")
        mode = MODE_RUST
    return mode


def _env_fallback() -> bool:
    return os.environ.get("RUFEE_FALLBACK", "1") not in ("0", "false", "no")


def configure(*, mode: str | None = None, fallback: bool | None = None) -> None:
    """Change how :class:`Context` behaves.

    ``mode``: ``"rust"`` (default), ``"shadow"`` or ``"off"``.
    ``fallback``: when ``False`` (strict), an unsupported node raises
    :class:`rufee.Unsupported` instead of falling back to peewee.

    Defaults come from the ``RUFEE_MODE`` and ``RUFEE_FALLBACK`` environment
    variables.
    """
    if mode is not None:
        if mode not in (MODE_RUST, MODE_SHADOW, MODE_OFF):
            raise ValueError(f"mode must be rust/shadow/off, got {mode!r}")
        _config.mode = mode
    if fallback is not None:
        _config.fallback = bool(fallback)


def _check_version() -> None:
    version = getattr(peewee, "__version__", "")
    if version.startswith(SUPPORTED_PEEWEE):
        return
    if os.environ.get("RUFEE_FORCE") == "1":
        log.warning("rufee: peewee %s is untested (supports %sx); RUFEE_FORCE=1 set",
                    version, SUPPORTED_PEEWEE)
        return
    _config.version_ok = False
    log.warning("rufee: peewee %s is not supported (need %sx); rufee.Context is a "
                "pass-through. Set RUFEE_FORCE=1 to override.", version, SUPPORTED_PEEWEE)


def _reference_render(query, scope, parentheses, settings):
    """Render with peewee's own Context, reproducing the caller's initial state."""
    ctx = _PeeweeContext(**settings)
    ctx.state = _PeeweeState(scope, parentheses, **settings)
    return ctx.sql(query).query()


def _describe_mismatch(rust, ref) -> str:
    rsql, rparams = rust
    psql, pparams = ref
    if rsql != psql:
        n = min(len(rsql), len(psql))
        pos = next((i for i in range(n) if rsql[i] != psql[i]), n)
        return (f"sql differs at offset {pos}: rust={rsql[max(0, pos - 30):pos + 30]!r} "
                f"peewee={psql[max(0, pos - 30):pos + 30]!r}")
    return f"params differ: rust={rparams!r} peewee={pparams!r}"


class Context(_PeeweeContext):
    """Drop-in ``peewee.Context`` whose top-level query rendering runs in Rust.

    Assign it to ``Database.context_class``. Every other use of the context
    (DDL generation, nested ``__sql__`` calls, ``literal``/``value``/alias
    management) is inherited from peewee unchanged.
    """

    def sql(self, obj):
        # Intercept only a *fresh, top-level* render of a query object. Anything
        # nested (peewee's own __sql__ methods calling ctx.sql on children) or
        # any non-query node (DDL, bare expressions) takes peewee's path.
        if (
            not self._sql
            and not self.stack
            and _config.version_ok
            and _config.mode != MODE_OFF
            and isinstance(obj, peewee.BaseQuery)
        ):
            return self._render_query(obj)
        return super().sql(obj)

    def _render_query(self, query):
        state = self.state
        settings = state.settings
        try:
            result = _native.render(query, state.scope, state.parentheses, settings)
        except Unsupported as exc:
            return self._fallback(query, exc)
        except Exception as exc:  # noqa: BLE001 - any Rust-side failure is a fallback
            log.error("rufee: renderer raised %r for %s; falling back",
                      exc, type(query).__name__)
            return self._fallback(query, exc)

        if _config.mode == MODE_SHADOW:
            ref = _reference_render(query, state.scope, state.parentheses, settings)
            stats._bump("shadow")
            if (result[0], list(result[1])) != (ref[0], list(ref[1])):
                stats._bump("mismatch")
                log.warning("rufee: shadow mismatch for %s: %s",
                            type(query).__name__, _describe_mismatch(result, ref))
            result = ref
        else:
            stats._bump("rust")

        sql, params = result
        self._sql.append(sql)
        self._values.extend(params)
        return self

    def _fallback(self, query, exc):
        if not _config.fallback:
            raise exc
        stats._bump("fallback")
        log.debug("rufee: fallback for %s: %s", type(query).__name__, exc)
        return super().sql(query)


_config.mode = _env_mode()
_config.fallback = _env_fallback()
_check_version()
