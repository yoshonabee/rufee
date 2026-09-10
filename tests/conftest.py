import peewee
import pytest

import rufee
from rufee import _context


@pytest.fixture(autouse=True)
def _reset_rufee(monkeypatch):
    """Every test starts in rust mode with fallback on, version ok, zero stats."""
    monkeypatch.setattr(_context._config, "mode", _context.MODE_RUST)
    monkeypatch.setattr(_context._config, "fallback", True)
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
