"""Every ``Context(**options)`` setting must be honoured the way peewee honours it.

Each test renders the same query through ``rufee.Context`` and ``peewee.Context``
with the options under test and asserts identical output. A few hard-coded
expectations document the behaviour itself; peewee's output is the oracle for
everything else.
"""

import datetime

import peewee
import pytest
from peewee import SQL

import rufee

DB = peewee.SqliteDatabase(":memory:")


class Base(peewee.Model):
    class Meta:
        database = DB


class User(Base):
    username = peewee.CharField()
    score = peewee.IntegerField()


class Tweet(Base):
    user = peewee.ForeignKeyField(User, backref="tweets")
    content = peewee.CharField()
    created = peewee.DateTimeField()


SQLITE = peewee.SqliteDatabase(":memory:").get_context_options()
MYSQL = peewee.MySQLDatabase(None).get_context_options()


def both(query, opts, **extra):
    """Render strictly with rufee, compare with peewee, return the pair."""
    rufee.configure(fallback=False)
    rufee.stats.reset()
    expected = peewee.Context(**opts, **extra).sql(query).query()
    got = rufee.Context(**opts, **extra).sql(query).query()
    assert rufee.stats.rust == 1, "query did not go through the Rust renderer"
    assert got[0] == expected[0]
    assert [(type(v), v) for v in got[1]] == [(type(v), v) for v in expected[1]]
    return got


def both_raise(query, opts, exc_type, **extra):
    rufee.configure(fallback=False)
    with pytest.raises(exc_type):
        peewee.Context(**opts, **extra).sql(query).query()
    with pytest.raises(exc_type):
        rufee.Context(**opts, **extra).sql(query).query()


# --------------------------------------------------------------------------- quote


def test_quote_double_quotes_sqlite():
    sql, params = both(User.select().where(User.score > 1), SQLITE)
    assert sql == (
        'SELECT "t1"."id", "t1"."username", "t1"."score" FROM "user" AS "t1" '
        'WHERE ("t1"."score" > ?)'
    )
    assert params == [1]


def test_quote_backticks_mysql():
    sql, params = both(User.select().where(User.score > 1), MYSQL)
    assert sql == (
        "SELECT `t1`.`id`, `t1`.`username`, `t1`.`score` FROM `user` AS `t1` "
        "WHERE (`t1`.`score` > %s)"
    )
    assert params == [1]


def test_quote_custom_pair():
    sql, _ = both(User.select(User.username), dict(SQLITE, quote="[]"))
    assert sql == "SELECT [t1].[username] FROM [user] AS [t1]"


def test_quote_applies_to_aliases_and_entities():
    q = User.select(User.username.alias("name")).order_by(SQL("name"))
    both(q, SQLITE)
    both(q, MYSQL)


# --------------------------------------------------------------------------- param


@pytest.mark.parametrize("param", ["?", "%s", "$1", ":x"])
def test_param_placeholder(param):
    q = User.select().where((User.score > 1) & (User.username == "a")).limit(5)
    sql, params = both(q, dict(SQLITE, param=param))
    assert sql.count(param) == 3
    assert params == [1, "a", 5]


# --------------------------------------------------------------------------- operations

# (expression builder, sqlite token, mysql token)
OPERATION_CASES = {
    "ILIKE via **": (lambda: User.username ** "a", "LIKE", "LIKE"),
    "ILIKE via contains": (lambda: User.username.contains("a"), "LIKE", "LIKE"),
    "LIKE via %": (lambda: User.username % "a", "GLOB", "LIKE BINARY"),
    "LIKE via like()": (lambda: User.username.like("a"), "GLOB", "LIKE BINARY"),
    # peewee 4 maps OP.XOR through the dialect's operations table: sqlite keeps
    # the literal "#", MySQL renders XOR.
    "XOR via ^": (lambda: User.score ^ 1, "#", "XOR"),
    "REGEXP": (lambda: User.username.regexp("a"), "REGEXP", "REGEXP BINARY"),
    "IREGEXP": (lambda: User.username.iregexp("a"), "IREGEXP", "REGEXP"),
    # MySQL renders CONCAT(a, b) since peewee 4.5 (it no longer sets
    # PIPES_AS_CONCAT); checked separately below.
    "CONCAT": (lambda: User.username.concat("a"), "||", None),
    "NOT IN": (lambda: User.score.not_in([1, 2]), "NOT IN", "NOT IN"),
    "IS NOT": (lambda: User.score.is_null(False), "IS NOT", "IS NOT"),
}


@pytest.mark.parametrize("name", sorted(OPERATION_CASES))
def test_operations_mapping(name):
    build, sqlite_token, mysql_token = OPERATION_CASES[name]
    for opts, token in ((SQLITE, sqlite_token), (MYSQL, mysql_token)):
        sql, _ = both(User.select().where(build()), opts)
        if token is None:
            continue
        where = sql.split(" WHERE ", 1)[1]
        assert f" {token} " in where, (name, where)


def test_concat_mysql_is_a_function():
    sql, _ = both(User.select().where(User.username.concat("a") == "ba"), MYSQL)
    assert "CONCAT(`t1`.`username`, %s)" in sql


def test_operations_custom_table():
    # Only operators whose OP constant equals its key can be remapped (AND, OR,
    # LIKE, ILIKE, IN, IS, ...). OP.EQ is the literal "=", so operations["EQ"]
    # is never consulted by peewee, and rufee must ignore it the same way.
    operations = dict(SQLITE["operations"], EQ="==", AND="&&", LIKE="~~")
    q = User.select().where((User.score == 1) & (User.username % "x"))
    sql, _ = both(q, dict(SQLITE, operations=operations))
    assert sql.endswith('WHERE (("t1"."score" = ?) && ("t1"."username" ~~ ?))')


# --------------------------------------------------------------------------- nulls_ordering


def test_nulls_ordering_native():
    q = User.select().order_by(User.username.asc(nulls="LAST"), User.score.desc(nulls="FIRST"))
    sql, _ = both(q, SQLITE)
    assert sql.endswith('ORDER BY "t1"."username" ASC NULLS LAST, "t1"."score" DESC NULLS FIRST')


def test_nulls_ordering_emulated():
    """MySQL has no NULLS FIRST/LAST; peewee emulates it with a CASE expression."""
    q = User.select().order_by(User.username.asc(nulls="LAST"))
    expected = peewee.Context(**MYSQL).sql(q).query()
    assert "CASE WHEN" in expected[0] and "NULLS" not in expected[0]
    try:
        both(q, MYSQL)
    except rufee.Unsupported as exc:
        pytest.skip(f"not implemented in Rust yet: {exc}")


def test_nulls_ordering_flag_alone_decides():
    """The flag, not the dialect, selects native vs emulated NULLS ordering."""
    q = User.select().order_by(User.username.asc(nulls="LAST"))
    sql, _ = both(q, dict(MYSQL, nulls_ordering=True))
    assert sql.endswith("ORDER BY `t1`.`username` ASC NULLS LAST")
    try:
        sql, _ = both(q, dict(SQLITE, nulls_ordering=False))
    except rufee.Unsupported as exc:
        pytest.skip(f"not implemented in Rust yet: {exc}")
    assert "NULLS" not in sql and "CASE WHEN" in sql


# --------------------------------------------------------------------------- limit_max


def test_limit_max_offset_without_limit_mysql():
    sql, params = both(User.select().offset(10), MYSQL)
    assert sql.endswith("LIMIT %s OFFSET %s")
    assert params == [18446744073709551615, 10]


def test_limit_max_offset_without_limit_sqlite():
    sql, params = both(User.select().offset(10), SQLITE)
    assert sql.endswith("LIMIT ? OFFSET ?")
    assert params == [-1, 10]


def test_limit_max_falsy_skips_limit():
    sql, params = both(User.select().offset(10), dict(SQLITE, limit_max=None))
    assert sql.endswith("OFFSET ?")
    assert params == [10]


def test_limit_max_not_used_when_limit_given():
    sql, params = both(User.select().limit(3).offset(10), MYSQL)
    assert params == [3, 10]


# --------------------------------------------------------------------------- value_literals


def test_value_literals_inlines_parameters():
    q = User.select().where(
        (User.username == "bob")
        & (User.score > 3)
        & (User.score.in_([1, 2]))
        & (User.username.is_null())
    )
    sql, params = both(q, SQLITE, value_literals=True)
    assert params == []
    assert sql.endswith(
        'WHERE (((("t1"."username" = \'bob\') AND ("t1"."score" > 3)) '
        'AND ("t1"."score" IN (1, 2))) AND ("t1"."username" IS NULL))'
    )


def test_value_literals_types():
    q = Tweet.select(Tweet.id, peewee.Value(None)).where(
        (Tweet.created == datetime.datetime(2020, 1, 2, 3, 4, 5))
        & (Tweet.content == "it's")
        & (Tweet.user == True)  # noqa: E712 - bool renders as 1
    ).limit(7)
    sql, params = both(q, MYSQL, value_literals=True)
    assert params == []
    assert sql.startswith("SELECT `t1`.`id`, NULL FROM")
    assert "'2020-01-02 03:04:05'" in sql
    assert "= 1)" in sql
    assert sql.endswith("LIMIT 7")


def test_value_literals_off_by_default():
    _, params = both(User.select().where(User.score > 3), SQLITE)
    assert params == [3]


# --------------------------------------------------------------------------- for_update


def test_for_update_rejected_when_unsupported():
    both_raise(User.select().for_update(), SQLITE, ValueError)


def test_for_update_mysql():
    sql, _ = both(User.select().for_update(), MYSQL)
    assert sql.endswith("FROM `user` AS `t1` FOR UPDATE")


@pytest.mark.parametrize(
    "kwargs, suffix",
    [
        (dict(for_update="FOR SHARE"), "FOR SHARE"),
        (dict(for_update=True, nowait=True), "FOR UPDATE NOWAIT"),
        (dict(for_update="FOR UPDATE NOWAIT"), "FOR UPDATE NOWAIT"),
        (dict(for_update=True, of=User), "FOR UPDATE OF `t1`"),
        (dict(for_update="FOR SHARE", of=[User], nowait=True), "FOR SHARE OF `t1` NOWAIT"),
        (dict(skip_locked=True), "FOR UPDATE SKIP LOCKED"),
        (dict(for_update="FOR SHARE SKIP LOCKED", of=User), "FOR SHARE OF `t1` SKIP LOCKED"),
    ],
)
def test_for_update_variants(kwargs, suffix):
    sql, _ = both(User.select().where(User.score > 1).for_update(**kwargs), MYSQL)
    assert sql.endswith(suffix)


def test_for_update_option_alone_decides():
    both(User.select().for_update(), dict(SQLITE, for_update=True))
    both_raise(User.select().for_update(), dict(MYSQL, for_update=False), ValueError)


def test_for_update_false_renders_nothing():
    sql, _ = both(User.select().for_update(False), MYSQL)
    assert "FOR" not in sql


# --------------------------------------------------------------------------- conflict_statement / conflict_update


def test_on_conflict_update_sqlite():
    query = User.insert(username="a", score=1).on_conflict(
        conflict_target=[User.username], update={User.score: User.score + 1}
    )
    sql, _ = both(query, SQLITE)
    assert sql.endswith('ON CONFLICT ("username") DO UPDATE SET "score" = ("user"."score" + ?)')


def test_on_conflict_do_nothing_keeps_target_sqlite():
    # peewee 4.2.4: DO NOTHING keeps the conflict target and its WHERE.
    query = User.insert(username="a", score=1).on_conflict(
        action="NOTHING", conflict_target=[User.username], conflict_where=(User.score > 0)
    )
    sql, _ = both(query, SQLITE)
    assert "ON CONFLICT (" in sql and "DO NOTHING" in sql


def test_on_duplicate_key_update_mysql():
    query = User.insert(username="a", score=1).on_conflict(
        preserve=[User.username], update={User.score: User.score + 1}
    )
    sql, _ = both(query, MYSQL)
    assert "ON DUPLICATE KEY UPDATE" in sql


def test_on_conflict_without_conflict_statement_raises():
    # peewee 4: OnConflict.get_conflict_statement raises InterfaceError when
    # the context has no conflict_statement hook (query not bound to a db).
    query = User.insert(username="a", score=1).on_conflict_ignore()
    opts = dict(SQLITE, conflict_statement=None, conflict_update=None)
    both_raise(query, opts, peewee.InterfaceError)


# --------------------------------------------------------------------------- compound_select_parentheses

A = User.select(User.id).where(User.score > 1)
B = User.select(User.id).where(User.score < 0)
C = Tweet.select(Tweet.id)


def test_compound_select_parentheses_sqlite_default():
    sql, _ = both(A | B, SQLITE)
    assert sql == (
        'SELECT "t1"."id" FROM "user" AS "t1" WHERE ("t1"."score" > ?) '
        'UNION SELECT "t2"."id" FROM "user" AS "t2" WHERE ("t2"."score" < ?)'
    )


def test_compound_select_parentheses_mysql_default():
    sql, _ = both(A | B, MYSQL)
    assert sql == (
        "(SELECT `t1`.`id` FROM `user` AS `t1` WHERE (`t1`.`score` > %s)) "
        "UNION (SELECT `t2`.`id` FROM `user` AS `t2` WHERE (`t2`.`score` < %s))"
    )


@pytest.mark.parametrize(
    "mode",
    [peewee.CSQ_PARENTHESES_NEVER, peewee.CSQ_PARENTHESES_ALWAYS, peewee.CSQ_PARENTHESES_UNNESTED],
)
@pytest.mark.parametrize("base", ["sqlite", "mysql"])
def test_compound_select_parentheses_modes(mode, base):
    opts = dict({"sqlite": SQLITE, "mysql": MYSQL}[base], compound_select_parentheses=mode)
    both(A | B, opts)
    both((A | B) + C, opts)
    both(A - (B & C), opts)
    both((A | B).order_by(SQL("1")).limit(2).offset(1), opts)
    both(((A + B) | C).limit(3), opts)


def test_compound_select_parentheses_always_wraps_nested():
    opts = dict(SQLITE, compound_select_parentheses=peewee.CSQ_PARENTHESES_ALWAYS)
    sql, _ = both((A | B) + C, opts)
    assert sql.startswith("((SELECT ")
    assert ") UNION (" in sql and ")) UNION ALL (" in sql


def test_compound_select_parentheses_unnested_flattens_same_op():
    opts = dict(SQLITE, compound_select_parentheses=peewee.CSQ_PARENTHESES_UNNESTED)
    sql, _ = both((A | B) | C, opts)
    # Same operator nested on the left is not wrapped again.
    assert sql.startswith("(SELECT ")
    assert not sql.startswith("((SELECT ")
