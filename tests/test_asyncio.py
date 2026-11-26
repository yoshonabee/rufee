"""rufee under peewee's asyncio extension (playhouse.pwasyncio).

pwasyncio only makes the driver I/O asynchronous: queries still render through
the synchronous ``Database.execute()`` -> ``get_sql_context()`` ->
``context_class`` path inside a greenlet, so ``rufee.Context`` needs no async
support of its own. These tests pin that down.
"""

import asyncio

import peewee
import pytest

import rufee

pytest.importorskip("greenlet")
pytest.importorskip("aiosqlite")

from playhouse.pwasyncio import AsyncSqliteDatabase  # noqa: E402


class RufeeAsyncSqliteDatabase(AsyncSqliteDatabase):
    context_class = rufee.Context


def _make_model(db):
    class Note(peewee.Model):
        title = peewee.CharField()
        rank = peewee.IntegerField(default=0)

        class Meta:
            database = db
            table_name = "note"

    return Note


async def _scenario():
    db = RufeeAsyncSqliteDatabase(":memory:", pool_size=1)
    Note = _make_model(db)
    try:
        await db.acreate_tables([Note])
        await db.aexecute(Note.insert_many([{"title": f"n{i}", "rank": i} for i in range(5)]))
        await db.aexecute(Note.update(rank=Note.rank * 10).where(Note.rank >= 3))
        await db.aexecute(Note.delete().where(Note.title == "n0"))
        rows = await db.list(Note.select().where(Note.rank > 1).order_by(Note.rank.desc()))
        count = await db.count(Note.select())
        streamed = [n.title async for n in db.iterate(Note.select().order_by(Note.id))]
        return [(n.title, n.rank) for n in rows], count, streamed
    finally:
        await db.close_pool()


def test_async_queries_render_through_rufee():
    rows, count, streamed = asyncio.run(_scenario())
    assert rows == [("n4", 40), ("n3", 30), ("n2", 2)]
    assert count == 4
    assert streamed == ["n1", "n2", "n3", "n4"]
    # insert, update, delete, select, count and the iterate() select all
    # rendered in Rust; none fell back.
    assert rufee.stats.rust >= 6
    assert rufee.stats.fallback == 0


def test_concurrent_tasks_render_independently(tmp_path):
    """Many tasks rendering at once share no rendering state."""

    async def run():
        db = RufeeAsyncSqliteDatabase(str(tmp_path / "notes.db"), pool_size=5)
        Note = _make_model(db)
        try:
            await db.acreate_tables([Note])
            await db.aexecute(Note.insert_many([{"title": f"n{i}", "rank": i} for i in range(20)]))

            async def one(i):
                q = Note.select(Note.title).where(Note.rank == i)
                return [n.title for n in await db.list(q)]

            return await asyncio.gather(*(one(i) for i in range(20)))
        finally:
            await db.close_pool()

    results = asyncio.run(run())
    assert results == [[f"n{i}"] for i in range(20)]
    assert rufee.stats.fallback == 0
