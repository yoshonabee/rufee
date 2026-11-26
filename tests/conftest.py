import peewee
import pytest

import rufee
from rufee import _context

from known_gaps import KNOWN_GAPS


def pytest_collection_modifyitems(config, items):
    for item in items:
        test_id = item.nodeid.split("tests/", 1)[-1]
        if test_id in KNOWN_GAPS:
            item.add_marker(pytest.mark.xfail(reason=KNOWN_GAPS[test_id], strict=True))


@pytest.fixture(autouse=True)
def _reset_rufee(monkeypatch):
    """Every test starts in rust mode with fallback on, version ok, zero stats."""
    monkeypatch.setattr(_context._config, "mode", _context.MODE_RUST)
    monkeypatch.setattr(_context._config, "fallback", True)
    monkeypatch.setattr(_context._config, "log_sql", True)
    monkeypatch.setattr(_context._config, "version_ok", True)
    rufee.stats.reset()
    yield
    rufee.stats.reset()


@pytest.fixture
def db():
    database = peewee.SqliteDatabase(":memory:")
    database.context_class = rufee.Context
    return database


@pytest.fixture
def Item(db):
    class Item(peewee.Model):
        name = peewee.CharField()
        qty = peewee.IntegerField()

        class Meta:
            database = db
            table_name = "item"

    return Item
