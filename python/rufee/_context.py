"""The ``peewee.Context`` subclass that routes top-level renders to Rust."""

from __future__ import annotations

import logging
import os
import sys
import threading
from collections import deque
from collections.abc import Callable
from contextlib import contextmanager
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

SUPPORTED_PEEWEE = "4.5."

MODE_RUST = "rust"      # render with Rust, fall back to peewee when unsupported
MODE_SHADOW = "shadow"  # render both, log mismatches, return peewee's result
MODE_OFF = "off"        # pure pass-through to peewee


@dataclass
class _Config:
    mode: str = MODE_RUST
    fallback: bool = True
    # Whether mismatch logs carry the SQL strings (see _mismatch_details).
    log_sql: bool = True
    # Set when peewee's version is outside the supported range.
    version_ok: bool = True


@dataclass
class Stats:
    """Counters, mostly for tests and for shadow-mode monitoring."""

    rust: int = 0        # queries rendered by Rust and returned
    fallback: int = 0    # queries rendered by peewee after Rust declined
    errors: int = 0      # of those, fallbacks caused by a non-Unsupported exception
    shadow: int = 0      # shadow comparisons performed
    mismatch: int = 0    # shadow comparisons that differed
    _lock: threading.Lock = field(default_factory=threading.Lock, repr=False)

    def _bump(self, name: str) -> None:
        with self._lock:
            setattr(self, name, getattr(self, name) + 1)

    def reset(self) -> None:
        with self._lock:
            self.rust = self.fallback = self.errors = self.shadow = self.mismatch = 0

    def snapshot(self) -> dict[str, int]:
        """All counters, read together under the lock."""
        with self._lock:
            return {name: getattr(self, name) for name in _COUNTERS}


_COUNTERS = ("rust", "fallback", "errors", "shadow", "mismatch")


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


def _env_log_sql() -> bool:
    return os.environ.get("RUFEE_LOG_SQL", "1") not in ("0", "false", "no")


def configure(
    *,
    mode: str | None = None,
    fallback: bool | None = None,
    log_sql: bool | None = None,
) -> None:
    """Change how :class:`Context` behaves.

    ``mode``: ``"rust"`` (default), ``"shadow"`` or ``"off"``.
    ``fallback``: when ``False`` (strict), an unsupported node raises
    :class:`rufee.Unsupported` instead of falling back to peewee.
    ``log_sql``: when ``False``, shadow-mismatch logs leave out the SQL strings
    and excerpts and keep only positions, lengths, counts and types.

    Defaults come from the ``RUFEE_MODE``, ``RUFEE_FALLBACK`` and
    ``RUFEE_LOG_SQL`` environment variables.
    """
    if mode is not None:
        if mode not in (MODE_RUST, MODE_SHADOW, MODE_OFF):
            raise ValueError(f"mode must be rust/shadow/off, got {mode!r}")
        _config.mode = mode
    if fallback is not None:
        _config.fallback = bool(fallback)
    if log_sql is not None:
        _config.log_sql = bool(log_sql)


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
    ctx.state = _PeeweeState(scope, parentheses, dict(settings))
    return ctx.sql(query).query()


class _DefaultReplay:
    """Make both shadow renders of an insert see the same callable defaults.

    peewee and the Rust renderer both fill a column missing from an insert row
    with ``query.get_default_data()[column]``, calling it when it is callable
    (``uuid4``, ``datetime.now``, an id generator). Shadow mode renders twice,
    so each call would produce a different value and every such insert would
    report a params mismatch. This records the values returned during the Rust
    render and hands the same values, in the same order per column, to the
    reference render.

    The override is an attribute on the query instance, not a change to
    ``model._meta.defaults``, which every thread inserting into the table
    shares. Both renderers look ``get_default_data`` up on the instance.
    """

    _MISSING = object()

    def __init__(self, query):
        self.query = query
        self.active = False
        get_default_data = getattr(query, "get_default_data", None)
        if get_default_data is None:
            return
        defaults = get_default_data()
        if not defaults or not any(isinstance(v, Callable) for v in defaults.values()):
            return
        self.active = True
        self.recording = {}
        self.replaying = {}
        for column, value in defaults.items():
            if isinstance(value, Callable):
                queue = deque()
                self.recording[column] = self._recorder(value, queue)
                self.replaying[column] = self._replayer(value, queue)
            else:
                self.recording[column] = self.replaying[column] = value

    @staticmethod
    def _recorder(default, queue):
        def record():
            value = default()
            queue.append(value)
            return value
        return record

    @staticmethod
    def _replayer(default, queue):
        # An empty queue means the reference render needs more defaults than
        # the Rust render used. A fresh value then makes the params differ, so
        # the disagreement still shows up as a mismatch.
        def replay():
            return queue.popleft() if queue else default()
        return replay

    @contextmanager
    def _serving(self, defaults):
        if not self.active:
            yield
            return
        query = self.query
        previous = query.__dict__.get("get_default_data", self._MISSING)
        query.get_default_data = lambda: defaults
        try:
            yield
        finally:
            if previous is self._MISSING:
                del query.get_default_data
            else:
                query.get_default_data = previous

    def record(self):
        return self._serving(self.recording if self.active else None)

    def replay(self):
        return self._serving(self.replaying if self.active else None)


# Frames skipped when looking for the code that ran the query: peewee is a
# single module (peewee.py), playhouse and rufee are packages.
_INTERNAL_FILES = frozenset({os.path.abspath(peewee.__file__)})
_INTERNAL_DIRS = (os.path.dirname(os.path.abspath(__file__)) + os.sep,)
_SQL_EXCERPT = 60
_SQL_MAX = 4000


def _is_internal(filename: str) -> bool:
    path = os.path.abspath(filename)
    if path in _INTERNAL_FILES or path.startswith(_INTERNAL_DIRS):
        return True
    # playhouse ships next to peewee.py, as its own package
    return f"{os.sep}playhouse{os.sep}" in path


def _callsite() -> str | None:
    """``file:line in function`` of the first frame outside peewee / playhouse / rufee."""
    frame = sys._getframe(1)
    while frame is not None:
        code = frame.f_code
        if not _is_internal(code.co_filename):
            return f"{code.co_filename}:{frame.f_lineno} in {code.co_name}"
        frame = frame.f_back
    return None


def _clip(sql: str) -> str:
    return sql if len(sql) <= _SQL_MAX else sql[:_SQL_MAX] + "..."


def _mismatch_details(query, rust, ref) -> dict:
    """What differs between the Rust and the peewee render, without parameter values.

    Parameter values are user data (emails, addresses, tokens) and are never
    copied into the log; only their position, count and type are. The SQL
    strings normally contain no values either, because peewee renders every
    value as a placeholder and the database driver fills them in later. They
    do contain values the application inlined itself (``SQL("... 'x'")``,
    ``value_literals``), and a Rust bug that inlines a value instead of
    emitting a placeholder shows up in ``rust_sql``. ``log_sql=False`` drops
    the strings for applications that cannot accept that.
    """
    rsql, rparams = rust
    psql, pparams = ref
    details = {"query_type": type(query).__name__, "callsite": _callsite()}
    if rsql != psql:
        n = min(len(rsql), len(psql))
        pos = next((i for i in range(n) if rsql[i] != psql[i]), n)
        details.update(
            diff="sql",
            offset=pos,
            rust_sql_length=len(rsql),
            peewee_sql_length=len(psql),
            rust_param_count=len(rparams),
            peewee_param_count=len(pparams),
        )
        if _config.log_sql:
            start = max(0, pos - _SQL_EXCERPT)
            details.update(
                rust_excerpt=rsql[start:pos + _SQL_EXCERPT],
                peewee_excerpt=psql[start:pos + _SQL_EXCERPT],
                rust_sql=_clip(rsql),
                peewee_sql=_clip(psql),
            )
        return details
    rparams, pparams = list(rparams), list(pparams)
    n = min(len(rparams), len(pparams))
    index = next((i for i in range(n) if rparams[i] != pparams[i]), n)
    details.update(
        diff="params",
        param_index=index,
        rust_param_count=len(rparams),
        peewee_param_count=len(pparams),
        rust_param_type=type(rparams[index]).__name__ if index < len(rparams) else None,
        peewee_param_type=type(pparams[index]).__name__ if index < len(pparams) else None,
    )
    if _config.log_sql:
        details["sql"] = _clip(psql)
    return details


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
        replay = _DefaultReplay(query) if _config.mode == MODE_SHADOW else None
        try:
            if replay is None:
                result = _native.render(query, state.scope, state.parentheses, settings)
            else:
                with replay.record():
                    result = _native.render(query, state.scope, state.parentheses, settings)
        except Unsupported as exc:
            return self._fallback(query, exc)
        except Exception as exc:  # noqa: BLE001 - any Rust-side failure is a fallback
            # Usually a legitimate error peewee raises too (ValueError from an
            # on-conflict hook, "FOR UPDATE not supported", ...): peewee's own
            # renderer raises it again during the fallback and the caller sees
            # it. When peewee renders the query fine instead, the Rust side has
            # a bug: the result is still correct, but it is worth a warning.
            stats._bump("errors")
            log.debug("rufee: renderer raised %r for %s; falling back",
                      exc, type(query).__name__)
            self._fallback(query, exc)
            log.warning("rufee: renderer error", extra={"rufee": {
                "query_type": type(query).__name__,
                "callsite": _callsite(),
                "error": f"{type(exc).__name__}: {exc}",
            }})
            return self

        if replay is not None:
            with replay.replay():
                ref = _reference_render(query, state.scope, state.parentheses, settings)
            stats._bump("shadow")
            if (result[0], list(result[1])) != (ref[0], list(ref[1])):
                stats._bump("mismatch")
                log.warning("rufee: shadow mismatch",
                            extra={"rufee": _mismatch_details(query, result, ref)})
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
_config.log_sql = _env_log_sql()
_check_version()
