# Changelog

## Unreleased

- Postgres is a tested backend: peewee's suite runs on Postgres 17 through
  psycopg2 and psycopg 3 in CI, strict 100%.
- `playhouse.postgres_ext` renders in Rust: `JsonLookup` (`data['a']['b']`),
  `JsonPath` (`data.path(...)`), `ObjectSlice` (array indexes and slices) and
  `ArrayValue` (array comparisons and `contains*`). They used to fall back to
  peewee.
- Python 3.14 is tested in CI and listed in the classifiers.
- The dev extras add `psycopg2-binary`, `psycopg[binary]` and `apsw`, so the
  sqlite lane also runs peewee's APSW tests.

## 0.2.1

Observability for shadow-mode rollouts. Rendering is unchanged.

- Shadow mismatches are logged with a fixed message, `rufee: shadow mismatch`,
  and their details in `record.rufee`: query class, the application call site
  (first frame outside peewee / playhouse / rufee), and either both full SQL
  strings with an excerpt around the first difference, or the position, count
  and type of the first differing parameter. **Parameter values are no longer
  logged**; 0.2.0 printed both parameter lists.
- `RUFEE_LOG_SQL=0` / `rufee.configure(log_sql=False)` leaves the SQL strings
  out of mismatch logs too. They normally hold placeholders, not values, but an
  application that inlines values, or a Rust bug that does, puts them there.
- A Rust error on a query peewee renders fine is logged at WARNING as
  `rufee: renderer error` (query class, call site, exception). It was DEBUG
  only. Errors peewee raises too stay at DEBUG.
- `rufee.stats.snapshot()` returns all counters in one read.
- New [`docs/migration-guide.md`](docs/migration-guide.md): shadow → rust
  rollout, what rufee logs and counts, exporting the counters, mismatch triage,
  rollback.

## 0.2.0

**Targets peewee 4.5.3. peewee 3.x is no longer supported**; stay on rufee
0.1.0 for peewee 3.17. The version gate turns rufee into a pass-through on any
peewee other than 4.5.x.

Renders peewee 4.5.3's whole test-suite byte-identically on sqlite and MySQL 8
with fallback off, including peewee's asyncio tests.

Rendering changes, matching peewee 4:

- `State(scope, parentheses, settings)` constructor and the read-only
  `Insert._query_type` property.
- `Ordering`: `COLLATE` before the direction.
- Window names, entity names: quoted and escaped per part at render time.
- `value_literals` escapes `'` as `''`.
- Compound selects: member styles (flat / parenthesised / `SELECT * FROM (...)`),
  `CSQ_PARENTHESES_GROUPED`, alias scoping for correlated compound subqueries,
  `EXISTS(compound)`.
- Sub-selects keep their parentheses in function and `CASE` arguments, in
  `UPDATE ... SET` values and `ON CONFLICT` updates; default `Model.select()`
  used as a source keeps its columns; `IN (VALUES ...)`.
- `FOR UPDATE` as a `ForUpdate` node, including `SKIP LOCKED`.
- MySQL string concatenation as `CONCAT(a, b)` (`use_concat_function`).
- Empty inserts backfill python-side defaults; `Table.insert(select)` without
  columns; unbound on-conflict inserts raise peewee's `InterfaceError`.
- JSON path nodes (`peewee.JSONPath`, playhouse `JSONPath` / `JSONBPath`).

Other:

- Works with `playhouse.pwasyncio` async databases with no extra code
  (`context_class = rufee.Context`); covered by tests.
- Test harness: `--baseline` mode, known-failure lists that fail the run when a
  listed test starts passing, faster local MySQL container.

## 0.1.0

First release: renders peewee 3.17.9's test-suite byte-identically.
