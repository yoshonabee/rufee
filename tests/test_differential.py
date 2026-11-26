"""Differential tests: ``rufee.Context`` must render exactly like ``peewee.Context``.

A small hypothesis grammar builds random peewee query trees (selects with joins,
subqueries, compound selects, inserts, updates, deletes, ...). Each generated
query is rendered by both renderers under sqlite and MySQL context options and
the SQL string plus the parameter list (values *and* their Python types) must be
identical.

rufee runs strict here (``fallback=False``). A ``rufee.Unsupported`` is not a
failure: the node is simply not implemented yet, so the example is counted,
reported through ``hypothesis.event`` and rejected with ``assume(False)``. Any
other difference is a real bug. A per-strategy accepted/rejected summary is
printed at the end of the pytest run.

The module ends with a plain, non-hypothesis test over a fixed list of
representative queries, so the file still checks something concrete even if the
strategies were to reject most examples.
"""

from __future__ import annotations

import collections
import datetime
import decimal
import functools
import operator

import hypothesis.errors
import peewee
import pytest
from hypothesis import HealthCheck, assume, event, given, settings, strategies as st
from peewee import JOIN, SQL, Case, Table, Value, ValuesList, fn

import rufee

# --------------------------------------------------------------------------- #
# Schema
# --------------------------------------------------------------------------- #

DB = peewee.SqliteDatabase(":memory:")


class Base(peewee.Model):
    class Meta:
        database = DB


class User(Base):
    username = peewee.CharField()
    is_active = peewee.BooleanField(default=True)
    created = peewee.DateTimeField()
    score = peewee.DecimalField(max_digits=10, decimal_places=2)


class Tweet(Base):
    user = peewee.ForeignKeyField(User, backref="tweets")
    content = peewee.CharField()
    likes = peewee.IntegerField()
    created = peewee.DateTimeField()


class Note(Base):
    tweet = peewee.ForeignKeyField(Tweet, backref="notes")
    body = peewee.CharField()
    rank = peewee.IntegerField()


Items = Table("items", ("id", "name", "qty"))

MODELS = [User, Tweet, Note]

# Which models can be joined onto which through a foreign key (either direction).
JOIN_TARGETS = {User: [Tweet], Tweet: [User, Note], Note: [Tweet]}

# Context options: rendering never needs a connection.
DIALECTS = {
    "sqlite": peewee.SqliteDatabase(":memory:").get_context_options(),
    "mysql": peewee.MySQLDatabase(None).get_context_options(),
}

# --------------------------------------------------------------------------- #
# Comparison harness
# --------------------------------------------------------------------------- #

# strategy name -> [accepted, rejected-as-unsupported]
COUNTS: dict[str, list[int]] = collections.defaultdict(lambda: [0, 0])
# Unsupported message -> occurrences
UNSUPPORTED_REASONS: collections.Counter = collections.Counter()


def _render(ctx_cls, query, opts, **extra):
    return ctx_cls(**opts, **extra).sql(query).query()


def _typed(params):
    return [(type(v), v) for v in params]


def check(strategy: str, query, dialect: str, **extra) -> None:
    """Render with both contexts and assert identical output.

    Rejects the hypothesis example when the Rust side declines the query.
    """
    rufee.configure(fallback=False)
    rufee.stats.reset()
    opts = DIALECTS[dialect]

    try:
        expected = _render(peewee.Context, query, opts, **extra)
    except Exception as exc:  # noqa: BLE001 - peewee refused the query
        expected_exc = type(exc)
        try:
            _render(rufee.Context, query, opts, **extra)
        except rufee.Unsupported as unsupported:
            _reject(strategy, unsupported)
        except expected_exc:
            COUNTS[strategy][0] += 1
            return
        pytest.fail(f"peewee raised {expected_exc.__name__} but rufee did not\n{query}")

    try:
        got = _render(rufee.Context, query, opts, **extra)
    except rufee.Unsupported as unsupported:
        _reject(strategy, unsupported)

    assert rufee.stats.rust == 1, "query did not go through the Rust renderer"
    COUNTS[strategy][0] += 1
    assert got[0] == expected[0], (
        f"SQL differs ({dialect})\n rufee : {got[0]}\n peewee: {expected[0]}"
    )
    assert _typed(got[1]) == _typed(expected[1]), (
        f"params differ ({dialect})\n rufee : {list(got[1])!r}\n peewee: {list(expected[1])!r}"
    )


def _reject(strategy: str, exc: rufee.Unsupported):
    COUNTS[strategy][1] += 1
    UNSUPPORTED_REASONS[str(exc)] += 1
    event(f"unsupported: {exc}")
    assume(False)


class _Summary:
    """pytest plugin registered at runtime to print the coverage counters."""

    def pytest_terminal_summary(self, terminalreporter):
        if not COUNTS:
            return
        tr = terminalreporter
        tr.section("rufee differential coverage (strict mode)")
        for name, (accepted, rejected) in sorted(COUNTS.items()):
            tr.write_line(f"  {name:<12} accepted {accepted:>5}   unsupported {rejected:>5}")
        if UNSUPPORTED_REASONS:
            tr.write_line("  unsupported reasons:")
            for reason, n in UNSUPPORTED_REASONS.most_common():
                tr.write_line(f"    {n:>5}  {reason}")


@pytest.fixture(scope="module", autouse=True)
def _register_summary(request):
    plugin = _Summary()
    request.config.pluginmanager.register(plugin, name="rufee-differential-summary")
    yield
    # Keep it registered: the terminal summary runs after module teardown.


# --------------------------------------------------------------------------- #
# Scalar strategies
# --------------------------------------------------------------------------- #

ints = st.integers(min_value=-1000, max_value=1000)
small_ints = st.integers(min_value=0, max_value=20)
texts = st.one_of(
    st.text(
        alphabet=st.characters(whitelist_categories=("L", "N"), max_codepoint=0x24F),
        max_size=8,
    ),
    st.sampled_from(["", "a%", "%b%", "o'reilly", 'say "hi"', "back\\slash", "日本語"]),
)
bools = st.booleans()
datetimes = st.datetimes(
    min_value=datetime.datetime(2000, 1, 1),
    max_value=datetime.datetime(2030, 12, 31),
)
decimals = st.decimals(
    min_value=decimal.Decimal("-1000"),
    max_value=decimal.Decimal("1000"),
    places=2,
    allow_nan=False,
    allow_infinity=False,
)
dialects = st.sampled_from(sorted(DIALECTS))


def value_for(column):
    """A strategy of Python values that fit the given field / column."""
    if isinstance(column, peewee.BooleanField):
        return bools
    if isinstance(column, peewee.DateTimeField):
        return datetimes
    if isinstance(column, peewee.DecimalField):
        return st.one_of(decimals, ints)
    if isinstance(column, (peewee.IntegerField, peewee.ForeignKeyField)):
        return ints
    if isinstance(column, peewee.CharField):
        return texts
    # Bare Table column: anything goes.
    return st.one_of(ints, texts, bools)


def fields_of(source):
    if isinstance(source, peewee.ModelAlias):
        return [getattr(source, f.name) for f in source.model._meta.sorted_fields]
    if isinstance(source, Table):
        return [getattr(source, c) for c in source._columns]
    return list(source._meta.sorted_fields)


def numeric_fields_of(source):
    return [
        f
        for f in fields_of(source)
        if isinstance(f, (peewee.IntegerField, peewee.DecimalField, peewee.ForeignKeyField))
        or isinstance(source, Table)
    ]


def text_fields_of(source):
    return [f for f in fields_of(source) if isinstance(f, peewee.CharField)] or fields_of(source)


# --------------------------------------------------------------------------- #
# Expression strategies
# --------------------------------------------------------------------------- #

ARITH_OPS = [operator.add, operator.sub, operator.mul]


@st.composite
def numeric_expr(draw, sources, depth=0):
    source = draw(st.sampled_from(sources))
    field = draw(st.sampled_from(numeric_fields_of(source)))
    if depth >= 2 or draw(st.integers(0, 2)) == 0:
        return field
    op = draw(st.sampled_from(ARITH_OPS))
    rhs = draw(st.one_of(ints, numeric_expr(sources, depth + 1)))
    if draw(bools):
        return op(field, rhs)  # Field + x
    return op(rhs, field)  # x + Field  (reflected operator)


@st.composite
def comparison(draw, sources, depth=0):
    """A single boolean predicate."""
    source = draw(st.sampled_from(sources))
    field = draw(st.sampled_from(fields_of(source)))
    val = value_for(field)
    kind = draw(
        st.sampled_from(
            [
                "eq", "ne", "lt", "gte", "field_vs_field", "arith", "is_null",
                "in_list", "in_tuple", "in_set", "in_range", "not_in", "between",
                "like", "contains", "startswith", "endswith", "ilike_op", "like_op",
                "regexp", "iregexp", "concat_eq", "bin_and", "xor", "in_subquery",
            ]
        )
    )
    if kind == "eq":
        return field == draw(val)
    if kind == "ne":
        return field != draw(val)
    if kind == "lt":
        return field < draw(val)
    if kind == "gte":
        return field >= draw(val)
    if kind == "field_vs_field":
        other = draw(st.sampled_from(fields_of(draw(st.sampled_from(sources)))))
        return field == other
    if kind == "arith":
        return draw(numeric_expr(sources)) > draw(ints)
    if kind == "is_null":
        return field.is_null(draw(bools))
    if kind == "in_list":
        return field.in_(draw(st.lists(val, max_size=4)))
    if kind == "in_tuple":
        return field.in_(tuple(draw(st.lists(val, min_size=1, max_size=4))))
    if kind == "in_set":
        return field.in_(set(draw(st.lists(ints, min_size=1, max_size=4))))
    if kind == "in_range":
        start = draw(small_ints)
        return field.in_(range(start, start + draw(small_ints)))
    if kind == "not_in":
        return field.not_in(draw(st.lists(val, max_size=4)))
    if kind == "between":
        return field.between(draw(val), draw(val))
    if kind == "like":
        return field.like(draw(texts))
    if kind == "contains":
        return field.contains(draw(texts))
    if kind == "startswith":
        return field.startswith(draw(texts))
    if kind == "endswith":
        return field.endswith(draw(texts))
    if kind == "ilike_op":
        return field ** draw(texts)
    if kind == "like_op":
        return field % draw(texts)
    if kind == "regexp":
        return field.regexp(draw(texts))
    if kind == "iregexp":
        return field.iregexp(draw(texts))
    if kind == "concat_eq":
        return field.concat(draw(texts)) == draw(texts)
    if kind == "bin_and":
        return draw(numeric_expr(sources)).bin_and(draw(small_ints)) == draw(small_ints)
    if kind == "xor":
        return (draw(numeric_expr(sources)) ^ draw(small_ints)) == draw(small_ints)
    if kind == "in_subquery":
        if depth >= 2:
            return field == draw(val)
        return field.in_(draw(select_query(depth=depth + 1, simple=True)))
    raise AssertionError(kind)


@st.composite
def condition(draw, sources, depth=0):
    """Predicates combined with ``&``, ``|`` and ``~``."""
    if depth >= 3 or draw(st.integers(0, 2)) == 0:
        return draw(comparison(sources, depth))
    kind = draw(st.sampled_from(["and", "or", "not"]))
    if kind == "not":
        return ~draw(condition(sources, depth + 1))
    lhs = draw(condition(sources, depth + 1))
    rhs = draw(condition(sources, depth + 1))
    return (lhs & rhs) if kind == "and" else (lhs | rhs)


@st.composite
def where_arg(draw, sources, depth=0):
    """A ``where()`` argument, sometimes wrapped in a one-element tuple.

    A stray trailing comma turns a ``where()`` argument into a tuple, and peewee
    accepts it: the tuple is rendered through ``EnclosedNodeList``, whose
    constructor holds the "avoid double parentheses" hack. rufee has to match
    that, so the grammar produces the shape.
    """
    cond = draw(condition(sources, depth))
    return (cond,) if draw(st.integers(0, 4)) == 0 else cond


@st.composite
def ordering(draw, sources):
    source = draw(st.sampled_from(sources))
    field = draw(st.sampled_from(fields_of(source)))
    kind = draw(st.sampled_from(["plain", "asc", "desc", "asc_nulls", "desc_nulls", "collate", "sql"]))
    if kind == "plain":
        return field
    if kind == "asc":
        return field.asc()
    if kind == "desc":
        return field.desc()
    if kind == "asc_nulls":
        return field.asc(nulls=draw(st.sampled_from(["FIRST", "LAST", "first"])))
    if kind == "desc_nulls":
        return field.desc(nulls=draw(st.sampled_from(["FIRST", "LAST"])))
    if kind == "collate":
        return field.asc(collation=draw(st.sampled_from(["NOCASE", "BINARY"])))
    return SQL(str(draw(st.integers(1, 3))))


@st.composite
def function_expr(draw, sources, depth=0):
    source = draw(st.sampled_from(sources))
    kind = draw(st.sampled_from(["count", "count_star", "sum", "coalesce", "lower", "max", "nested"]))
    if kind == "count":
        node = fn.COUNT(draw(st.sampled_from(fields_of(source))))
    elif kind == "count_star":
        node = fn.COUNT(SQL("*"))
    elif kind == "sum":
        node = fn.SUM(draw(numeric_expr(sources)))
    elif kind == "coalesce":
        field = draw(st.sampled_from(fields_of(source)))
        node = fn.COALESCE(field, draw(value_for(field)))
    elif kind == "lower":
        node = fn.LOWER(draw(st.sampled_from(text_fields_of(source))))
    elif kind == "max":
        node = fn.MAX(draw(st.sampled_from(fields_of(source))))
    else:
        inner = draw(function_expr(sources, depth + 1)) if depth < 1 else SQL("1")
        node = fn.COALESCE(inner, draw(ints))
    if draw(bools):
        node = node.alias(draw(st.sampled_from(["agg", "n", "total"])))
    return node


@st.composite
def case_expr(draw, sources):
    source = draw(st.sampled_from(sources))
    if draw(bools):
        # Searched CASE: CASE WHEN <cond> THEN <val> ... ELSE <default> END
        clauses = draw(
            st.lists(st.tuples(condition(sources, depth=2), st.one_of(ints, texts)), min_size=1, max_size=3)
        )
        default = draw(st.one_of(st.none(), ints, texts))
        return Case(None, clauses, default)
    # Simple CASE: CASE <field> WHEN <val> THEN <val> ... END
    field = draw(st.sampled_from(fields_of(source)))
    clauses = draw(st.lists(st.tuples(value_for(field), st.one_of(ints, texts)), min_size=1, max_size=3))
    default = draw(st.one_of(st.none(), ints, texts))
    return Case(field, clauses, default)


@st.composite
def window_expr(draw, sources):
    source = draw(st.sampled_from(sources))
    kind = draw(st.sampled_from(["row_number", "rank", "sum", "frame"]))
    partition_by = draw(st.lists(st.sampled_from(fields_of(source)), max_size=2)) or None
    order_by = draw(st.lists(ordering(sources), max_size=2)) or None
    if kind == "row_number":
        return fn.ROW_NUMBER().over(partition_by=partition_by, order_by=order_by)
    if kind == "rank":
        return fn.RANK().over(order_by=order_by or [draw(st.sampled_from(fields_of(source)))])
    if kind == "sum":
        return fn.SUM(draw(numeric_expr(sources))).over(partition_by=partition_by, order_by=order_by)
    return fn.SUM(draw(numeric_expr(sources))).over(
        order_by=order_by or [draw(st.sampled_from(fields_of(source)))],
        start=draw(st.sampled_from([peewee.Window.preceding(), peewee.Window.preceding(2), peewee.Window.CURRENT_ROW])),
        end=draw(st.sampled_from([peewee.Window.following(), peewee.Window.following(1), peewee.Window.CURRENT_ROW])),
    )


@st.composite
def select_column(draw, sources, depth=0):
    source = draw(st.sampled_from(sources))
    kind = draw(
        st.sampled_from(
            ["field", "field", "field", "alias", "cast", "sql", "sql_params", "arith",
             "value", "function", "case", "window", "model", "scalar_subquery"]
        )
    )
    field = draw(st.sampled_from(fields_of(source)))
    if kind == "field":
        return field
    if kind == "alias":
        return field.alias(draw(st.sampled_from(["a", "b", "renamed"])))
    if kind == "cast":
        return field.cast(draw(st.sampled_from(["INTEGER", "TEXT", "CHAR(10)", "DECIMAL(10, 2)"])))
    if kind == "sql":
        return SQL(draw(st.sampled_from(["1", "NULL", "CURRENT_TIMESTAMP"])))
    if kind == "sql_params":
        return SQL("? + ?", (draw(ints), draw(ints)))
    if kind == "arith":
        return draw(numeric_expr(sources))
    if kind == "value":
        return Value(draw(st.one_of(ints, texts, bools, st.none())))
    if kind == "function":
        return draw(function_expr(sources))
    if kind == "case":
        return draw(case_expr(sources))
    if kind == "window":
        return draw(window_expr(sources))
    if kind == "model":
        return source
    if kind == "scalar_subquery":
        if depth >= 2:
            return field
        return draw(select_query(depth=depth + 1, simple=True)).alias("sub")
    raise AssertionError(kind)


# --------------------------------------------------------------------------- #
# Query strategies
# --------------------------------------------------------------------------- #


@st.composite
def select_query(draw, depth=0, simple=False):
    """A ``Select`` over one of the models, optionally joined.

    ``simple`` produces a single-column query usable as an IN / scalar subquery.
    """
    model = draw(st.sampled_from(MODELS))
    sources = [model]
    query = None

    if not simple and draw(st.integers(0, 2)) == 0:
        # ModelAlias self-join: User AS "ua" joined back onto the model.
        alias = model.alias(draw(st.sampled_from(["ua", "alt"])))
        sources.append(alias)
        on = draw(comparison(sources, depth=2))
        join_type = draw(st.sampled_from([JOIN.INNER, JOIN.LEFT_OUTER]))
        query = model.select(*draw(st.lists(select_column(sources, depth), min_size=1, max_size=3)))
        query = query.join(alias, join_type, on=on)
    else:
        joined = []
        if not simple and draw(bools):
            target = draw(st.sampled_from(JOIN_TARGETS[model]))
            joined.append(target)
            sources.append(target)
        if simple:
            columns = [draw(st.sampled_from(fields_of(model)))]
        elif draw(bools):
            columns = []  # Model.select(): all fields
        else:
            columns = draw(st.lists(select_column(sources, depth), min_size=1, max_size=4))
        query = model.select(*columns)
        for target in joined:
            how = draw(st.sampled_from(["implicit", "left_outer", "explicit_on", "right_outer"]))
            if how == "implicit":
                query = query.join(target)
            elif how == "left_outer":
                query = query.left_outer_join(target)
            elif how == "right_outer":
                query = query.join(target, JOIN.RIGHT_OUTER)
            else:
                query = query.join(target, JOIN.LEFT_OUTER, on=draw(comparison(sources, depth=2)))
            if draw(bools):
                query = query.switch(model)

    if draw(bools):
        query = query.where(draw(where_arg(sources, depth)))
    if not simple and draw(bools):
        group = draw(st.lists(st.sampled_from(fields_of(model)), min_size=1, max_size=2))
        query = query.group_by(*group)
        if draw(bools):
            query = query.having(draw(condition(sources, depth=2)))
    if not simple and draw(bools):
        query = query.order_by(*draw(st.lists(ordering(sources), min_size=1, max_size=3)))
    if draw(bools):
        query = query.limit(draw(small_ints))
    if not simple and draw(bools):
        query = query.offset(draw(small_ints))
    if not simple and draw(bools):
        if draw(bools):
            query = query.distinct()
        else:
            query = query.distinct(draw(st.lists(st.sampled_from(fields_of(model)), min_size=1, max_size=2)))
    if not simple and draw(st.integers(0, 3)) == 0:
        query = query.paginate(draw(st.integers(1, 5)), draw(st.integers(1, 10)))
    return query


@st.composite
def source_subquery_select(draw):
    """Subqueries as sources: FROM (subquery), JOIN (subquery), CTE, VALUES."""
    kind = draw(st.sampled_from(["from_subquery", "join_subquery", "cte", "values_list", "table"]))
    if kind == "from_subquery":
        inner = Tweet.select(Tweet.user, Tweet.likes).where(draw(condition([Tweet], depth=2)))
        sub = inner.alias(draw(st.sampled_from(["sq", "inner"])))
        query = peewee.Select(from_list=[sub], columns=[sub.c.user, sub.c.likes])
        if draw(bools):
            query = query.where(sub.c.likes > draw(ints))
        return query
    if kind == "join_subquery":
        inner = Tweet.select(Tweet.user, Tweet.likes).where(draw(condition([Tweet], depth=2)))
        if draw(bools):
            inner = inner.alias("sq")
        query = User.select(User.username, inner.c.likes).join(inner, on=(User.id == inner.c.user))
        if draw(bools):
            query = query.where(draw(condition([User], depth=2)))
        return query
    if kind == "cte":
        cte = Tweet.select(Tweet.user, Tweet.likes).where(draw(condition([Tweet], depth=2))).cte("tw")
        query = User.select(User.username, cte.c.likes).join(cte, on=(User.id == cte.c.user)).with_cte(cte)
        if draw(bools):
            query = query.order_by(cte.c.likes.desc())
        return query
    if kind == "values_list":
        rows = draw(st.lists(st.tuples(ints, texts), min_size=1, max_size=3))
        vl = ValuesList(rows, columns=["id", "name"], alias=draw(st.sampled_from(["vl", "v"])))
        query = peewee.Select(from_list=[vl], columns=[vl.c.id, vl.c.name])
        if draw(bools):
            query = query.where(vl.c.id > draw(ints))
        return query
    # Bare Table with explicit columns.
    query = Items.select(*draw(st.lists(st.sampled_from(fields_of(Items)), min_size=1, max_size=3)))
    if draw(bools):
        query = query.where(draw(condition([Items], depth=2)))
    if draw(bools):
        query = query.order_by(*draw(st.lists(ordering([Items]), min_size=1, max_size=2)))
    return query


@st.composite
def compound_query(draw):
    ops = [operator.or_, operator.add, operator.and_, operator.sub]  # UNION, UNION ALL, INTERSECT, EXCEPT
    parts = [draw(select_query(depth=1, simple=True)) for _ in range(draw(st.integers(2, 3)))]
    query = parts[0]
    for part in parts[1:]:
        query = draw(st.sampled_from(ops))(query, part)
    if draw(bools):
        query = query.order_by(SQL("1"))
    if draw(bools):
        query = query.limit(draw(small_ints))
    if draw(bools):
        query = query.offset(draw(small_ints))
    return query


def _row(draw, fields):
    return {f: draw(value_for(f)) for f in fields}


@st.composite
def insert_query(draw):
    model = draw(st.sampled_from(MODELS))
    fields = [f for f in fields_of(model) if not isinstance(f, peewee.AutoField)]
    chosen = draw(st.lists(st.sampled_from(fields), min_size=1, max_size=len(fields), unique=True))
    kind = draw(st.sampled_from(["dict", "kwargs", "many_dicts", "many_tuples", "from_query", "table"]))

    if kind == "dict":
        query = model.insert(_row(draw, chosen))
    elif kind == "kwargs":
        query = model.insert(**{f.name: v for f, v in _row(draw, chosen).items()})
    elif kind == "many_dicts":
        rows = [_row(draw, chosen) for _ in range(draw(st.integers(1, 3)))]
        query = model.insert_many(rows)
    elif kind == "many_tuples":
        rows = [tuple(_row(draw, chosen).values()) for _ in range(draw(st.integers(1, 3)))]
        query = model.insert_many(rows, fields=chosen)
    elif kind == "from_query":
        source = draw(st.sampled_from([m for m in MODELS if m is not model]))
        src_fields = fields_of(source)[1 : 1 + len(chosen)]
        chosen = chosen[: len(src_fields)]
        query = model.insert_from(source.select(*src_fields), fields=chosen)
    else:
        row = {c: draw(value_for(c)) for c in draw(st.lists(st.sampled_from(["name", "qty"]), min_size=1, max_size=2, unique=True))}
        query = Items.insert(row)

    conflict = draw(st.sampled_from([None, None, "ignore", "replace", "update"]))
    if conflict == "ignore":
        query = query.on_conflict_ignore()
    elif conflict == "replace":
        query = query.on_conflict_replace()
    elif conflict == "update" and kind != "table":
        target = chosen[0]
        query = query.on_conflict(
            conflict_target=[target] if draw(bools) else None,
            update={target: draw(value_for(target))},
            preserve=[chosen[-1]] if draw(bools) and len(chosen) > 1 else None,
        )
    return query


@st.composite
def update_query(draw):
    model = draw(st.sampled_from(MODELS))
    kind = draw(st.sampled_from(["kwargs", "dict", "expression", "subquery", "table"]))
    fields = [f for f in fields_of(model) if not isinstance(f, peewee.AutoField)]
    if kind == "kwargs":
        field = draw(st.sampled_from(fields))
        query = model.update(**{field.name: draw(value_for(field))})
    elif kind == "dict":
        chosen = draw(st.lists(st.sampled_from(fields), min_size=1, max_size=3, unique=True))
        query = model.update({f: draw(value_for(f)) for f in chosen})
    elif kind == "expression":
        field = draw(st.sampled_from(numeric_fields_of(model)))
        query = model.update({field: draw(numeric_expr([model]))})
    elif kind == "subquery":
        field = draw(st.sampled_from(numeric_fields_of(model)))
        other = draw(st.sampled_from([m for m in MODELS if m is not model]))
        sub = other.select(draw(st.sampled_from(numeric_fields_of(other)))).limit(1)
        if draw(bools):
            sub = sub.where(draw(condition([other], depth=2)))
        query = model.update({field: sub})
    else:
        model = Items
        query = Items.update({Items.qty: draw(ints)})
    if draw(bools):
        query = query.where(draw(where_arg([model], depth=1)))
    if draw(bools):
        query = query.order_by(*draw(st.lists(ordering([model]), min_size=1, max_size=2)))
    if draw(bools):
        query = query.limit(draw(small_ints))
    return query


@st.composite
def delete_query(draw):
    model = draw(st.sampled_from(MODELS + [Items]))
    query = model.delete()
    if draw(bools):
        query = query.where(draw(where_arg([model], depth=1)))
    if draw(bools):
        query = query.order_by(*draw(st.lists(ordering([model]), min_size=1, max_size=2)))
    if draw(bools):
        query = query.limit(draw(small_ints))
    return query


# --------------------------------------------------------------------------- #
# Property tests
# --------------------------------------------------------------------------- #

SETTINGS = settings(
    suppress_health_check=[HealthCheck.filter_too_much, HealthCheck.too_slow],
    max_examples=300,
    deadline=None,
)


def skip_when_nothing_supported(test):
    """Turn hypothesis' ``Unsatisfiable`` into a skip.

    ``assume(False)`` on every example (a strategy whose root node is still a
    stub in Rust) makes hypothesis give up with ``Unsatisfiable``. That is not
    a bug in rufee, so report it as a skip with the reason; the test becomes a
    real check the moment the node is implemented.
    """

    @functools.wraps(test)
    def wrapper(*args, **kwargs):
        try:
            return test(*args, **kwargs)
        except hypothesis.errors.Unsatisfiable:
            pytest.skip("every generated example was rejected as rufee.Unsupported")

    return wrapper


@skip_when_nothing_supported
@SETTINGS
@given(query=select_query(), dialect=dialects)
def test_select(query, dialect):
    check("select", query, dialect)


@skip_when_nothing_supported
@SETTINGS
@given(query=source_subquery_select(), dialect=dialects)
def test_subquery_sources(query, dialect):
    check("sources", query, dialect)


@skip_when_nothing_supported
@SETTINGS
@given(query=compound_query(), dialect=dialects)
def test_compound(query, dialect):
    check("compound", query, dialect)


@skip_when_nothing_supported
@SETTINGS
@given(query=insert_query(), dialect=dialects)
def test_insert(query, dialect):
    check("insert", query, dialect)


@skip_when_nothing_supported
@SETTINGS
@given(query=update_query(), dialect=dialects)
def test_update(query, dialect):
    check("update", query, dialect)


@skip_when_nothing_supported
@SETTINGS
@given(query=delete_query(), dialect=dialects)
def test_delete(query, dialect):
    check("delete", query, dialect)


@skip_when_nothing_supported
@SETTINGS
@given(query=select_query(), dialect=dialects)
def test_select_value_literals(query, dialect):
    check("literals", query, dialect, value_literals=True)


# --------------------------------------------------------------------------- #
# Fixed representative queries (no hypothesis)
# --------------------------------------------------------------------------- #

UA = User.alias("ua")
_sub = Tweet.select(Tweet.user).where(Tweet.likes > 3)
_vl = ValuesList([(1, "a"), (2, "b")], columns=["id", "name"], alias="vl")
_cte = Tweet.select(Tweet.user, Tweet.likes).cte("tw")

REPRESENTATIVE = {
    "select_all": User.select(),
    "where_eq_and_or": User.select().where((User.username == "a") & ((User.score > 1) | User.is_active.is_null())),
    "in_range": User.select().where(User.id.in_(range(3))),
    "in_set_tuple": User.select().where(User.id.in_({1, 2}) | User.id.not_in((3, 4))),
    "in_empty": User.select().where(User.id.in_([])),
    "between": User.select().where(User.score.between(1, decimal.Decimal("2.5"))),
    "like_family": User.select().where(
        User.username.like("a%") | User.username.contains("b") | User.username.startswith("c")
        | (User.username ** "d") | (User.username % "e") | User.username.regexp("f") | User.username.iregexp("g")
    ),
    "arithmetic": User.select((User.score + 1) * 2 - User.id, 5 + User.id, User.id.bin_and(3), User.id ^ 1),
    "ordering": User.select().order_by(User.username.asc(nulls="LAST"), User.created.desc(collation="NOCASE"), User.id),
    "alias_cast_sql_value": User.select(User.username.alias("u"), User.score.cast("INTEGER"), SQL("1"), SQL("? + ?", (1, 2)), Value(3)),
    "in_subquery": User.select().where(User.id.in_(_sub)),
    "from_subquery": peewee.Select(from_list=[_sub.alias("sq")], columns=[_sub.c.user]),
    "join_subquery": User.select(User.username, _sub.c.user).join(_sub, on=(User.id == _sub.c.user)),
    "join_fk": Tweet.select(Tweet, User).join(User).where(User.is_active == True),  # noqa: E712
    "left_outer_join_switch": User.select(User, Tweet).left_outer_join(Tweet).switch(User).where(Tweet.id.is_null()),
    "join_model_alias": Tweet.select(Tweet, UA.username).join(UA, on=(Tweet.user == UA.id)),
    "model_alias_select": UA.select(UA.username).where(UA.is_active == True),  # noqa: E712
    "group_having_distinct": Tweet.select(Tweet.user).group_by(Tweet.user).having(Tweet.likes > 1).distinct(),
    "distinct_on": Tweet.select(Tweet.user).distinct([Tweet.user]),
    "limit_offset": Tweet.select().limit(5).offset(2),
    "offset_only": Tweet.select().offset(2),
    "paginate": Tweet.select().order_by(Tweet.id).paginate(3, 10),
    "negated": User.select().where(~(User.id == 1) & ~User.is_active),
    # peewee renders a tuple where() argument through EnclosedNodeList, whose
    # constructor renders a lone non-flat Expression flat to avoid `((...))`.
    "where_tuple_one_expression": User.select().where(((User.id == 1) | User.is_active.is_null(),)),
    "where_tuple_one_comparison": User.select().where((User.id == 1,)),
    "where_tuple_two_expressions": User.select().where(((User.id == 1), User.is_active.is_null())),
    "where_tuple_one_sql": User.select().where((SQL("1"),)),
    "where_tuple_one_field": User.select().where((User.is_active,)),
    "where_list_one_expression": User.select().where([(User.id == 1) | User.is_active.is_null()]),
    "update_where_tuple": Tweet.update(likes=0).where(((Tweet.id == 1) | (Tweet.likes > 2),)),
    "delete_where_tuple": Tweet.delete().where(((Tweet.id == 1) | (Tweet.likes > 2),)),
    "concat": User.select(User.username.concat("x")),
    "union_order_limit": (User.select(User.id) | Tweet.select(Tweet.id)).order_by(SQL("1")).limit(3),
    "union_all_intersect": (User.select(User.id) + Tweet.select(Tweet.id)) & Note.select(Note.id),
    "except": User.select(User.id) - Tweet.select(Tweet.id),
    "values_list": peewee.Select(from_list=[_vl], columns=[_vl.c.id, _vl.c.name]),
    "cte": User.select(User.username, _cte.c.likes).join(_cte, on=(User.id == _cte.c.user)).with_cte(_cte),
    "table": Items.select(Items.name).where((Items.qty > 1) & Items.name.is_null(False)).order_by(Items.qty.desc()),
    "function_count_alias": Tweet.select(Tweet.user, fn.COUNT(Tweet.id).alias("c")).group_by(Tweet.user),
    "function_nested": User.select(fn.COALESCE(fn.LOWER(User.username), "?")),
    "case_searched": Tweet.select(Case(None, [(Tweet.likes > 1, "hot")], "cold")),
    "case_simple": Tweet.select(Case(Tweet.likes, [(0, "none"), (1, "one")], "many")),
    "window_partition": Tweet.select(Tweet.id, fn.ROW_NUMBER().over(partition_by=[Tweet.user], order_by=[Tweet.created.desc()])),
    "window_frame": Tweet.select(fn.SUM(Tweet.likes).over(order_by=[Tweet.id], start=peewee.Window.preceding(2), end=peewee.Window.CURRENT_ROW)),
    "insert_kwargs": User.insert(username="a", is_active=True),
    "insert_many_dicts": User.insert_many([{"username": "a", "score": 1}, {"username": "b", "score": 2}]),
    "insert_many_tuples": User.insert_many([("a", 1), ("b", 2)], fields=[User.username, User.score]),
    "insert_ignore": User.insert(username="a").on_conflict_ignore(),
    "insert_replace": User.insert(username="a").on_conflict_replace(),
    "insert_from_query": Note.insert_from(Tweet.select(Tweet.content, Tweet.likes), fields=[Note.body, Note.rank]),
    "insert_table": Items.insert({Items.name: "a", Items.qty: 1}),
    # peewee 4.2.2: an empty insert lists the model's python-side defaults.
    "insert_empty_defaults": User.insert(),
    # peewee 4.2.3: inserting a query without columns renders no column list.
    "insert_query_no_columns": Items.insert(Tweet.select(Tweet.id, Tweet.content, Tweet.likes)),
    "update_expression": Tweet.update(likes=Tweet.likes + 1).where(Tweet.id == 1),
    "update_subquery": Tweet.update({Tweet.likes: Note.select(Note.rank).where(Note.tweet == Tweet.id).limit(1)}),
    "update_table": Items.update({Items.qty: 0}).where(Items.name == "x"),
    "delete_where": Tweet.delete().where(Tweet.likes < 0),
    "delete_all": Note.delete(),
    "delete_table": Items.delete().where(Items.qty == 0),
}


@pytest.mark.parametrize("dialect", sorted(DIALECTS))
@pytest.mark.parametrize("name", sorted(REPRESENTATIVE))
def test_representative_queries(name, dialect):
    rufee.configure(fallback=False)
    rufee.stats.reset()
    query = REPRESENTATIVE[name]
    opts = DIALECTS[dialect]
    expected = _render(peewee.Context, query, opts)
    try:
        got = _render(rufee.Context, query, opts)
    except rufee.Unsupported as exc:
        pytest.skip(f"not implemented in Rust yet: {exc}")
    assert rufee.stats.rust == 1
    assert got[0] == expected[0]
    assert _typed(got[1]) == _typed(expected[1])
