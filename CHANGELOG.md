# Changelog

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
