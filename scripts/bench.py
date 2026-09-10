#!/usr/bin/env python
"""Render-time benchmark: rufee.Context vs peewee.Context on fixed queries.

Measures `Context(**opts).sql(q).query()` including context construction,
best of REPEAT runs of N iterations each, with MySQL dialect options (no
connection is needed to render). Also checks that both outputs are identical.

    scripts/bench.py [--sqlite] [-n N] [--json out.json]
"""

from __future__ import annotations

import argparse
import json
import sys
import time

import peewee
from peewee import (BooleanField, CharField, DateTimeField, DecimalField, ForeignKeyField,
                    IntegerField, Model, MySQLDatabase, SqliteDatabase, fn)

import rufee


def make_queries():
    db = MySQLDatabase(None)

    class Base(Model):
        class Meta:
            database = db

    class Shop(Base):
        sid = CharField(primary_key=True)
        name = CharField()
        created = DateTimeField()

    class Item(Base):
        tid = CharField(primary_key=True)
        shop = ForeignKeyField(Shop, column_name="sid", backref="items")
        title = CharField()
        price = DecimalField()
        stock = IntegerField()
        is_active = BooleanField()
        created = DateTimeField()

    rows = [dict(tid=str(i), shop="abc", title="t", price=10, stock=1, is_active=True, created=None)
            for i in range(100)]
    return {
        "join select": (Item.select(Item.tid, Item.title, Shop.name).join(Shop)
                        .where((Shop.sid.in_(["a", "b", "c"])) & (Item.price > 100) & (Item.stock > 0))
                        .order_by(Item.created.desc(), Item.tid).limit(50).offset(100)),
        "simple select": Item.select().where(Item.shop == "abc").order_by(Item.created.desc()).limit(20),
        "count": Item.select(fn.COUNT(Item.tid)).where(Item.is_active == True),  # noqa: E712
        "insert": Item.insert(tid="x", shop="abc", title="t", price=10, stock=1, is_active=True, created=None),
        "insert_many x100": Item.insert_many(rows),
        "update": Item.update(stock=Item.stock - 1, is_active=False).where(Item.tid == "x"),
        "delete": Item.delete().where(Item.tid.in_(["a", "b"])),
    }


def timeit(fn, n, repeat):
    fn()
    best = float("inf")
    for _ in range(repeat):
        start = time.perf_counter()
        for _ in range(n):
            fn()
        best = min(best, (time.perf_counter() - start) / n * 1e6)
    return best


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--sqlite", action="store_true", help="use sqlite dialect options instead of MySQL")
    p.add_argument("-n", type=int, default=5000)
    p.add_argument("--repeat", type=int, default=5)
    p.add_argument("--json")
    args = p.parse_args(argv)

    opts = (SqliteDatabase(":memory:") if args.sqlite else MySQLDatabase(None)).get_context_options()
    rufee.configure(mode="rust", fallback=False)
    results = {}
    print(f"{'query':20s}{'peewee µs':>10s}{'rufee µs':>10s}{'ratio':>7s}  identical")
    for name, q in make_queries().items():
        ref = peewee.Context(**opts).sql(q).query()
        got = rufee.Context(**opts).sql(q).query()
        same = ref == got
        tp = timeit(lambda: peewee.Context(**opts).sql(q).query(), args.n, args.repeat)
        tr = timeit(lambda: rufee.Context(**opts).sql(q).query(), args.n, args.repeat)
        results[name] = {"peewee_us": tp, "rufee_us": tr, "ratio": tp / tr, "identical": same}
        print(f"{name:20s}{tp:10.1f}{tr:10.1f}{tp / tr:6.1f}x  {same}")
    if args.json:
        with open(args.json, "w") as f:
            json.dump(results, f, indent=1)
    return 0 if all(r["identical"] for r in results.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
