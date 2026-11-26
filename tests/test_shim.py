"""Behaviour of rufee.Context that peewee's own suite cannot observe."""

import logging

import peewee
import pytest

import rufee
from rufee import _context


def _reference(query):
    return peewee.Context(**query._database.get_context_options()).sql(query).query()


def test_is_a_peewee_context(db):
    ctx = db.get_sql_context()
    assert isinstance(ctx, rufee.Context)
    assert isinstance(ctx, peewee.Context)


@pytest.fixture
def unsupported_native(monkeypatch):
    """Force the Rust side to decline every query."""

    def decline(query, scope, parentheses, settings):
        raise rufee.Unsupported("forced by test")

    monkeypatch.setattr(_context._native, "render", decline)


def test_fallback_matches_peewee(db, Item, unsupported_native):
    q = Item.select().where(Item.qty > 3).order_by(Item.name)
    assert db.get_sql_context().sql(q).query() == _reference(q)
    assert rufee.stats.fallback == 1


def test_rust_matches_peewee_on_simple_select(db, Item):
    q = Item.select().where(Item.qty > 3).order_by(Item.name)
    assert db.get_sql_context().sql(q).query() == _reference(q)
    assert rufee.stats.rust == 1
    assert rufee.stats.fallback == 0


def test_strict_raises_unsupported(db, Item, unsupported_native):
    rufee.configure(fallback=False)
    with pytest.raises(rufee.Unsupported):
        db.get_sql_context().sql(Item.select()).query()


def test_off_mode_never_calls_native(db, Item, monkeypatch):
    rufee.configure(mode="off")
    monkeypatch.setattr(_context._native, "render",
                        lambda *a, **k: pytest.fail("render called in off mode"))
    q = Item.select()
    assert db.get_sql_context().sql(q).query() == _reference(q)
    assert rufee.stats.rust == rufee.stats.fallback == 0


def test_non_query_nodes_take_peewee_path(db, Item, monkeypatch):
    monkeypatch.setattr(_context._native, "render",
                        lambda *a, **k: pytest.fail("render called for a bare node"))
    ctx = db.get_sql_context()
    assert ctx.sql(Item.qty + 1).query() == ('("t1"."qty" + ?)', [1])


def test_nested_calls_are_not_intercepted(db, Item, monkeypatch):
    calls = []

    def fake_render(query, scope, parentheses, settings):
        calls.append(type(query).__name__)
        raise rufee.Unsupported("stub")

    monkeypatch.setattr(_context._native, "render", fake_render)
    sub = Item.select(Item.id).where(Item.qty > 1)
    q = Item.select().where(Item.id.in_(sub))
    db.get_sql_context().sql(q).query()
    # Only the outer query reached Rust; the subquery was rendered by peewee's
    # own __sql__ while we were already inside the fallback.
    assert calls == ["ModelSelect"]


def test_ddl_uses_peewee_path(db, Item, monkeypatch):
    monkeypatch.setattr(_context._native, "render",
                        lambda *a, **k: pytest.fail("render called for DDL"))
    db.connect()
    db.create_tables([Item])
    assert Item.table_exists()


def test_rust_result_is_used_when_render_succeeds(db, Item, monkeypatch):
    monkeypatch.setattr(_context._native, "render",
                        lambda *a, **k: ("SELECT 1", [42]))
    assert db.get_sql_context().sql(Item.select()).query() == ("SELECT 1", [42])
    assert rufee.stats.rust == 1


def test_initial_state_is_forwarded(db, Item, monkeypatch):
    seen = {}

    def fake_render(query, scope, parentheses, settings):
        seen.update(scope=scope, parentheses=parentheses, settings=settings)
        return ("", [])

    monkeypatch.setattr(_context._native, "render", fake_render)
    db.get_sql_context(value_literals=True).sql(Item.select())
    assert seen["scope"] == peewee.SCOPE_NORMAL
    assert seen["parentheses"] is False
    assert seen["settings"]["value_literals"] is True
    assert seen["settings"]["quote"] == '""'
    assert seen["settings"]["param"] == "?"


def _rufee_records(caplog, message):
    return [r for r in caplog.records if r.name == "rufee" and r.getMessage() == message]


def test_renderer_exception_falls_back(db, Item, monkeypatch, caplog):
    def boom(*a, **k):
        raise RuntimeError("rust exploded")

    monkeypatch.setattr(_context._native, "render", boom)
    q = Item.select()
    with caplog.at_level(logging.DEBUG, logger="rufee"):
        assert db.get_sql_context().sql(q).query() == _reference(q)
    assert rufee.stats.fallback == 1
    assert rufee.stats.errors == 1
    assert "rust exploded" in caplog.text


def test_renderer_exception_peewee_renders_is_a_warning(db, Item, monkeypatch, caplog):
    # Rust failed where peewee succeeds: a Rust bug, reported at WARNING with
    # a fixed message and the details in ``extra``.
    def boom(*a, **k):
        raise RuntimeError("rust exploded")

    monkeypatch.setattr(_context._native, "render", boom)
    with caplog.at_level(logging.WARNING, logger="rufee"):
        db.get_sql_context().sql(Item.select()).query()
    [record] = _rufee_records(caplog, "rufee: renderer error")
    assert record.levelno == logging.WARNING
    assert record.rufee["query_type"] == "ModelSelect"
    assert record.rufee["error"] == "RuntimeError: rust exploded"
    assert record.rufee["callsite"].startswith(__file__)


def test_expected_errors_are_not_warned(db, Item, caplog):
    # peewee raises the same error, so it is the caller's problem, not rufee's.
    with caplog.at_level(logging.WARNING, logger="rufee"), pytest.raises(ValueError):
        db.get_sql_context().sql(Item.select().for_update()).query()
    assert _rufee_records(caplog, "rufee: renderer error") == []


def test_expected_errors_come_from_peewee_too(db, Item):
    # sqlite has no FOR UPDATE: Rust raises ValueError, the fallback re-renders
    # with peewee, which raises the same ValueError. Same behaviour, one fallback.
    with pytest.raises(ValueError):
        db.get_sql_context().sql(Item.select().for_update()).query()
    assert rufee.stats.errors == 1


def test_renderer_exception_in_strict_mode_propagates(db, Item, monkeypatch):
    rufee.configure(fallback=False)

    def boom(*a, **k):
        raise RuntimeError("rust exploded")

    monkeypatch.setattr(_context._native, "render", boom)
    with pytest.raises(RuntimeError):
        db.get_sql_context().sql(Item.select()).query()


def test_shadow_mode_returns_peewee_result_on_mismatch(db, Item, monkeypatch, caplog):
    rufee.configure(mode="shadow")
    monkeypatch.setattr(_context._native, "render",
                        lambda *a, **k: ("SELECT wrong", []))
    q = Item.select()
    with caplog.at_level(logging.WARNING, logger="rufee"):
        assert db.get_sql_context().sql(q).query() == _reference(q)
    assert rufee.stats.shadow == 1
    assert rufee.stats.mismatch == 1
    [record] = _rufee_records(caplog, "rufee: shadow mismatch")
    details = record.rufee
    assert details["diff"] == "sql"
    assert details["query_type"] == "ModelSelect"
    assert details["offset"] == len("SELECT ")
    assert details["rust_sql"] == "SELECT wrong"
    assert details["peewee_sql"] == _reference(q)[0]
    assert details["callsite"].startswith(__file__)


def test_shadow_mode_param_mismatch_is_reported(db, Item, monkeypatch, caplog):
    rufee.configure(mode="shadow")
    q = Item.select().where(Item.qty == 5)
    sql, _params = _reference(q)
    monkeypatch.setattr(_context._native, "render", lambda *a, **k: (sql, [6]))
    with caplog.at_level(logging.WARNING, logger="rufee"):
        assert db.get_sql_context().sql(q).query() == _reference(q)
    assert rufee.stats.mismatch == 1
    [record] = _rufee_records(caplog, "rufee: shadow mismatch")
    details = record.rufee
    assert details["diff"] == "params"
    assert details["param_index"] == 0
    assert details["rust_param_type"] == details["peewee_param_type"] == "int"
    assert details["rust_param_count"] == details["peewee_param_count"] == 1


def test_shadow_mismatch_never_logs_param_values(db, Item, monkeypatch, caplog):
    rufee.configure(mode="shadow")
    q = Item.select().where(Item.name == "secret-value-1")
    sql, _params = _reference(q)
    monkeypatch.setattr(_context._native, "render",
                        lambda *a, **k: (sql, ["secret-value-2"]))
    with caplog.at_level(logging.WARNING, logger="rufee"):
        db.get_sql_context().sql(q).query()
    [record] = _rufee_records(caplog, "rufee: shadow mismatch")
    dumped = repr(record.__dict__)
    assert "secret-value" not in dumped
    assert record.rufee["rust_param_type"] == "str"


def test_shadow_mismatch_param_count_differs(db, Item, monkeypatch, caplog):
    rufee.configure(mode="shadow")
    q = Item.select().where(Item.qty == 5)
    sql, _params = _reference(q)
    monkeypatch.setattr(_context._native, "render", lambda *a, **k: (sql, [5, 6]))
    with caplog.at_level(logging.WARNING, logger="rufee"):
        db.get_sql_context().sql(q).query()
    [record] = _rufee_records(caplog, "rufee: shadow mismatch")
    assert record.rufee["param_index"] == 1
    assert record.rufee["rust_param_type"] == "int"
    assert record.rufee["peewee_param_type"] is None


def test_shadow_mismatch_without_sql(db, Item, monkeypatch, caplog):
    rufee.configure(mode="shadow", log_sql=False)
    q = Item.select().where(Item.name == "inlined-value")
    monkeypatch.setattr(_context._native, "render",
                        lambda *a, **k: ("SELECT 'inlined-value'", []))
    with caplog.at_level(logging.WARNING, logger="rufee"):
        db.get_sql_context().sql(q).query()
    [record] = _rufee_records(caplog, "rufee: shadow mismatch")
    assert "inlined-value" not in repr(record.__dict__)
    details = record.rufee
    assert details["diff"] == "sql"
    assert details["offset"] == len("SELECT ")
    assert details["rust_param_count"] == 0
    assert details["peewee_param_count"] == 1
    assert not {"rust_sql", "peewee_sql", "rust_excerpt", "peewee_excerpt"} & details.keys()


def test_params_mismatch_without_sql(db, Item, monkeypatch, caplog):
    rufee.configure(mode="shadow", log_sql=False)
    q = Item.select().where(Item.qty == 5)
    sql, _params = _reference(q)
    monkeypatch.setattr(_context._native, "render", lambda *a, **k: (sql, [6]))
    with caplog.at_level(logging.WARNING, logger="rufee"):
        db.get_sql_context().sql(q).query()
    [record] = _rufee_records(caplog, "rufee: shadow mismatch")
    assert "sql" not in record.rufee
    assert record.rufee["param_index"] == 0


def test_log_sql_env(monkeypatch):
    monkeypatch.setenv("RUFEE_LOG_SQL", "0")
    assert _context._env_log_sql() is False
    monkeypatch.delenv("RUFEE_LOG_SQL")
    assert _context._env_log_sql() is True


def test_shadow_mismatch_long_sql_is_clipped(db, Item, monkeypatch, caplog):
    rufee.configure(mode="shadow")
    monkeypatch.setattr(_context._native, "render",
                        lambda *a, **k: ("SELECT " + "x" * 10_000, []))
    with caplog.at_level(logging.WARNING, logger="rufee"):
        db.get_sql_context().sql(Item.select()).query()
    [record] = _rufee_records(caplog, "rufee: shadow mismatch")
    assert len(record.rufee["rust_sql"]) == _context._SQL_MAX + len("...")


def test_shadow_mode_silent_when_identical(db, Item, monkeypatch, caplog):
    rufee.configure(mode="shadow")
    q = Item.select()
    monkeypatch.setattr(_context._native, "render", lambda *a, **k: _reference(q))
    with caplog.at_level(logging.WARNING, logger="rufee"):
        db.get_sql_context().sql(q).query()
    assert rufee.stats.shadow == 1
    assert rufee.stats.mismatch == 0
    assert caplog.text == ""


def test_shadow_mode_still_falls_back_when_unsupported(db, Item, unsupported_native):
    rufee.configure(mode="shadow")
    q = Item.select()
    assert db.get_sql_context().sql(q).query() == _reference(q)
    assert rufee.stats.fallback == 1
    assert rufee.stats.shadow == 0


def test_version_gate_disables_rust(db, Item, monkeypatch):
    monkeypatch.setattr(_context._config, "version_ok", False)
    monkeypatch.setattr(_context._native, "render",
                        lambda *a, **k: pytest.fail("render called with bad version"))
    q = Item.select()
    assert db.get_sql_context().sql(q).query() == _reference(q)


def test_check_version_rejects_other_peewee(monkeypatch):
    monkeypatch.setattr(peewee, "__version__", "3.18.0")
    monkeypatch.delenv("RUFEE_FORCE", raising=False)
    monkeypatch.setattr(_context._config, "version_ok", True)
    _context._check_version()
    assert _context._config.version_ok is False


def test_check_version_force(monkeypatch):
    monkeypatch.setattr(peewee, "__version__", "3.18.0")
    monkeypatch.setenv("RUFEE_FORCE", "1")
    monkeypatch.setattr(_context._config, "version_ok", True)
    _context._check_version()
    assert _context._config.version_ok is True


def test_configure_validates_mode():
    with pytest.raises(ValueError):
        rufee.configure(mode="turbo")


def test_env_defaults(monkeypatch):
    monkeypatch.setenv("RUFEE_MODE", "shadow")
    monkeypatch.setenv("RUFEE_FALLBACK", "0")
    assert _context._env_mode() == "shadow"
    assert _context._env_fallback() is False
    monkeypatch.setenv("RUFEE_MODE", "bogus")
    with pytest.warns(UserWarning):
        assert _context._env_mode() == "rust"


def test_reference_render_survives_rebinding_peewee_context(db, Item, monkeypatch):
    # Harnesses bind peewee.Context = rufee.Context; shadow mode must not recurse.
    monkeypatch.setattr(peewee, "Context", rufee.Context)
    rufee.configure(mode="shadow")
    monkeypatch.setattr(_context._native, "render", lambda *a, **k: ("SELECT wrong", []))
    q = Item.select()
    sql, params = db.get_sql_context().sql(q).query()
    assert sql.startswith('SELECT "t1"."id"')
    assert rufee.stats.mismatch == 1


def test_stats_snapshot(db, Item):
    db.get_sql_context().sql(Item.select()).query()
    assert rufee.stats.snapshot() == {
        "rust": 1, "fallback": 0, "errors": 0, "shadow": 0, "mismatch": 0,
    }


def test_callsite_skips_peewee_frames(db, Item, monkeypatch, caplog):
    # The query runs through Model.select() -> execute -> peewee internals;
    # the callsite must still point here, not into peewee.py.
    rufee.configure(mode="shadow")
    monkeypatch.setattr(_context._native, "render", lambda *a, **k: ("SELECT wrong", []))
    db.connect()
    db.create_tables([Item])
    with caplog.at_level(logging.WARNING, logger="rufee"):
        list(Item.select())
    [record] = _rufee_records(caplog, "rufee: shadow mismatch")
    assert record.rufee["callsite"].startswith(f"{__file__}:")
    assert record.rufee["callsite"].endswith(" in test_callsite_skips_peewee_frames")
