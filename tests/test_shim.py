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


def test_fallback_matches_peewee(db, Item):
    q = Item.select().where(Item.qty > 3).order_by(Item.name)
    assert db.get_sql_context().sql(q).query() == _reference(q)
    assert rufee.stats.fallback >= 1


def test_strict_raises_unsupported(db, Item):
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


def test_renderer_exception_falls_back(db, Item, monkeypatch, caplog):
    def boom(*a, **k):
        raise RuntimeError("rust exploded")

    monkeypatch.setattr(_context._native, "render", boom)
    q = Item.select()
    with caplog.at_level(logging.ERROR, logger="rufee"):
        assert db.get_sql_context().sql(q).query() == _reference(q)
    assert rufee.stats.fallback == 1
    assert "rust exploded" in caplog.text


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
    assert "shadow mismatch" in caplog.text
    assert "offset" in caplog.text


def test_shadow_mode_param_mismatch_is_reported(db, Item, monkeypatch, caplog):
    rufee.configure(mode="shadow")
    q = Item.select().where(Item.qty == 5)
    sql, _params = _reference(q)
    monkeypatch.setattr(_context._native, "render", lambda *a, **k: (sql, [6]))
    with caplog.at_level(logging.WARNING, logger="rufee"):
        assert db.get_sql_context().sql(q).query() == _reference(q)
    assert rufee.stats.mismatch == 1
    assert "params differ" in caplog.text


def test_shadow_mode_silent_when_identical(db, Item, monkeypatch, caplog):
    rufee.configure(mode="shadow")
    q = Item.select()
    monkeypatch.setattr(_context._native, "render", lambda *a, **k: _reference(q))
    with caplog.at_level(logging.WARNING, logger="rufee"):
        db.get_sql_context().sql(q).query()
    assert rufee.stats.shadow == 1
    assert rufee.stats.mismatch == 0
    assert caplog.text == ""


def test_shadow_mode_still_falls_back_when_unsupported(db, Item):
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
