"""Performance regression check: rufee must stay well ahead of peewee's renderer.

No extra dependencies: ``time.perf_counter`` around a fixed join query rendered
``N`` times, best of ``REPEATS`` runs. The README quotes ~5x on the Rust walker
alone; the assertion uses 3x so CI noise cannot turn it red. The measured ratio
is in the assertion message.
"""

import time

import peewee
import pytest

import rufee

N = 2000
REPEATS = 5
MIN_RATIO = 3.0

DB = peewee.SqliteDatabase(":memory:")


class Base(peewee.Model):
    class Meta:
        database = DB


class User(Base):
    username = peewee.CharField()
    is_active = peewee.BooleanField()
    created = peewee.DateTimeField()


class Tweet(Base):
    user = peewee.ForeignKeyField(User, backref="tweets")
    content = peewee.CharField()
    likes = peewee.IntegerField()
    created = peewee.DateTimeField()


QUERY = (
    Tweet.select(Tweet, User)
    .join(User)
    .where((User.is_active == True) & (Tweet.likes > 10) & User.username.startswith("a"))  # noqa: E712
    .order_by(Tweet.created.desc(), Tweet.id)
    .limit(20)
    .offset(40)
)
OPTIONS = DB.get_context_options()


def _best_of(ctx_cls, opts):
    best = float("inf")
    for _ in range(REPEATS):
        start = time.perf_counter()
        for _ in range(N):
            ctx_cls(**opts).sql(QUERY).query()
        best = min(best, time.perf_counter() - start)
    return best


@pytest.mark.slow
def test_rufee_renders_at_least_3x_faster_than_peewee():
    rufee.configure(fallback=False)
    # The comparison is only meaningful if both produce the same thing.
    assert rufee.Context(**OPTIONS).sql(QUERY).query() == peewee.Context(**OPTIONS).sql(QUERY).query()

    peewee_time = _best_of(peewee.Context, OPTIONS)
    rufee_time = _best_of(rufee.Context, OPTIONS)
    ratio = peewee_time / rufee_time
    per_query = 1e6 / N
    assert ratio >= MIN_RATIO, (
        f"rufee is only {ratio:.2f}x faster than peewee (need >= {MIN_RATIO}x): "
        f"peewee {peewee_time * per_query:.1f} us/query, rufee {rufee_time * per_query:.1f} us/query"
    )
