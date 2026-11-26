# rufee

A drop-in replacement for [peewee](https://github.com/coleifer/peewee)'s SQL
rendering `Context`, implemented in Rust.

peewee builds queries as a tree of Python `Node` objects and renders them to SQL
by walking that tree through a `Context` object. Profiling shows that most of
the rendering cost is in the `Context`/`State` machinery itself (a `with ctx(...)`
per node, a settings-dict copy per scope change, attribute lookups going through
`State.__getattr__`), not in the AST nodes. rufee keeps every peewee class
untouched (`Model`, `Field`, `Expression`, `Select`, ...) and only replaces the
renderer: a Rust walker reads the existing Python node tree and emits the SQL
string and parameter list.

Status: **renders peewee 4.5.3's entire test-suite byte-identically** (strict
mode 100% on sqlite and MySQL, including peewee's asyncio tests, see below).
peewee 3.x is no longer supported; use rufee `v0.1.0` for peewee 3.17.
Install with `pip install rufee`; other options are in
[`docs/installation.md`](docs/installation.md). The API may still move.

## How it plugs in

peewee already exposes the renderer as a class attribute:

```python
# peewee 4.5.x
class Database:
    context_class = Context

    def execute(self, query, **context_options):
        ctx = self.get_sql_context(**context_options)   # self.context_class(**options)
        sql, params = ctx.sql(query).query()
        return self.execute_sql(sql, params)
```

So using rufee is a one-line change on the application side:

```python
import rufee

class MyDatabase(MySQLDatabase):
    context_class = rufee.Context
```

Everything else (`Model.select()`, `.dicts()`, `insert_many`, `query.sql()`,
`db.execute_sql(raw)`) keeps working unchanged. The same line works for
`playhouse.pwasyncio`'s async databases: async queries still render through the
synchronous `context_class` path inside a greenlet, so rufee needs nothing
async-specific.

Full instructions — wheels, building from source, the Docker recipe, the
`RUFEE_MODE` / `RUFEE_FALLBACK` / `RUFEE_FORCE` knobs and how to verify the
install — are in [`docs/installation.md`](docs/installation.md). To put it in
front of production traffic, go through shadow mode first:
[`docs/migration-guide.md`](docs/migration-guide.md) covers the rollout, what
rufee logs and counts, and how to read a mismatch.

## Design

- **Rust walker over peewee's Python AST.** `rufee.Context.sql(node)` hands the
  node to a PyO3 extension that dispatches on peewee's node classes
  (`Select`, `Insert`, `Update`, `Delete`, `Expression`, `Field`, `Column`,
  `Value`, `Ordering`, `Join`, `Function`, `SQL`, `Case`, ...), reads their
  attributes, assigns table aliases and collects parameters. Per-field value
  conversion (`Field.db_value`) is still called back into Python, so custom
  field types behave exactly as before.
- **Dialect comes from context options**, not from hard-coded MySQL: `quote`,
  `param`, `operations`, `conflict_statement`, `conflict_update`,
  `for_update`, `value_literals` are honoured the same way peewee's own
  `Context` honours them. This is what lets the sqlite-based peewee test suite
  act as a spec.
- **Safety nets.**
  1. *Whole-query fallback*: any node type the Rust side does not support, and
     any Rust-side error, makes the whole query re-render through peewee's
     original `Context`. Incomplete coverage is slower, never wrong.
  2. *Shadow mode*: render both ways, log any mismatch (SQL string or params,
     never parameter values) with the application call site, and return
     peewee's result. Meant for production traffic before switching over.
  3. *Version gate*: rufee reads peewee's private attributes (`_from_list`,
     `_where`, `_returning`, `Join._on`, ...). It targets peewee **4.5.x**;
     on any other version it degrades to a pure pass-through.

Rendering time, `Context(**opts).sql(q).query()` including context
construction (Apple Silicon, CPython 3.11, peewee 4.5.3, MySQL dialect options):

| query | peewee | rufee | ratio |
|---|---|---|---|
| join select (2 tables, IN + 2 predicates, ORDER BY, LIMIT/OFFSET) | 77 µs | 9.3 µs | 8.2x |
| simple select (1 predicate, ORDER BY, LIMIT) | 48 µs | 6.5 µs | 7.4x |
| `SELECT COUNT(...) WHERE` | 25 µs | 4.3 µs | 5.8x |
| insert (7 columns) | 31 µs | 6.3 µs | 5.0x |
| `insert_many`, 100 rows | 1093 µs | 168 µs | 6.5x |
| update (expression + where) | 29 µs | 5.0 µs | 5.8x |
| delete (IN) | 15 µs | 3.8 µs | 3.9x |

peewee 4's own renderer is about 30% faster than 3.17's (join select 108 →
77 µs), so the ratios are lower than rufee 0.1's against peewee 3.17.9
(6–11x); rufee's absolute times are unchanged or slightly better.

Reproduce with `scripts/bench.py` (`--sqlite` for sqlite options, `--json`).

Query *construction* on the Python side (`Model.select().where(...)`) is
unchanged, as is row hydration (`CursorWrapper`, `Model.__init__`); both are
out of scope.

## Testing strategy

Tests come first; node coverage is implemented against them.

### peewee's own test suite as the spec

peewee's tests reach the renderer through two paths, and both must be bound to
rufee, otherwise the most valuable tests are silently skipped:

| path | how it builds the context | how rufee is bound |
|---|---|---|
| `assertSQL()` string comparisons (~420 call sites, mostly `tests/sql.py` and `tests/model_sql.py`) | `tests/base.py` calls `Context(**state)` directly, bypassing the database | bind `peewee.Context = rufee.Context` before importing `tests` |
| tests that execute queries (`tests/models.py` and friends) | `db.execute()` → `get_sql_context()` → `context_class` | set `peewee.Database.context_class = rufee.Context` |

Two modes, run separately:

| mode | fallback | meaning | requirement |
|---|---|---|---|
| strict | off: unsupported node fails the test | pass rate = coverage progress | reported on every PR |
| safe | on | proves "incomplete coverage is never wrong" | always 100% green |

Two backends: sqlite in-memory (no infrastructure, `"` quoting, `?` params) and
MySQL 8 in Docker (`` ` `` quoting, `%s` params, `ON DUPLICATE KEY UPDATE`).
Postgres/CockroachDB/APSW/SQLCipher-specific tests are out of scope.

Current result (`scripts/run_peewee_tests.py`, peewee 4.5.3, Python 3.11,
including peewee's `tests.pwasyncio`):

| lane | result | queries rendered by Rust |
|---|---|---|
| sqlite, strict | **1823 / 1823 pass** (100%), 385 skipped | 13,101, 0 fallbacks |
| sqlite, safe | 1823 / 1823 pass | 13,101, 14 fallbacks* |
| MySQL 8, strict | **1853 / 1853 pass** (100%), 355 skipped | 13,524, 0 fallbacks |
| MySQL 8, safe | 1853 / 1853 pass | 13,524, 12 fallbacks* |

\* safe-mode fallbacks are queries where the Rust renderer raised the same
error peewee raises (`ValueError` from an on-conflict hook, `FOR UPDATE` on
sqlite); the fallback re-renders with peewee, which raises it again.

The baseline (`--baseline`, rufee off) is clean with **unmodified** peewee
4.5.3 on both backends, so the allow-list of pre-existing failures is empty.

Gaps are tracked as tests, not prose: `scripts/known_failures/<engine>.txt`
lists peewee tests rufee is known to fail (empty today). Safe mode fails on any
failure not listed *and* on any listed test that passes, so a fix must delete
its entries. `tests/known_gaps.py` does the same for rufee's own tests as strict
xfails. `scripts/run_peewee_tests.py` also keeps a short `FLAKY` set of
timing-dependent upstream tests whose outcome is ignored.

```sh
make venv dev peewee-src               # once
make peewee-tests ARGS="-e sqlite --safe"
make peewee-tests ARGS="-e sqlite --strict"
make mysql-up && make peewee-tests ARGS="-e mysql --strict"
make peewee-tests ARGS="-e sqlite --baseline"   # unmodified peewee
```

### rufee's own tests (`tests/`)

1. **Shim behaviour** (`test_shim.py`): fallback triggers (unsupported node,
   Rust error, Rust panic) never leak exceptions; shadow mode logs mismatches
   with their call site and without parameter values, and returns peewee's
   result; version gate degrades to pass-through; the initial state and every
   context option are forwarded.
2. **Differential testing** (`test_differential.py`, hypothesis): random query
   trees from a small grammar (selects, joins, subqueries, compound selects,
   inserts, updates, deletes, functions, windows, CASE) rendered with both
   sqlite and MySQL options must give identical SQL *and* params. Plus 50
   hand-written representative queries × 2 dialects.
3. **Context options** (`test_options.py`): `quote`, `param`, `operations`,
   `nulls_ordering`, `limit_max`, `value_literals`, `for_update`,
   `compound_select_parentheses`, each compared against peewee.
4. **Benchmark regression** (`test_bench.py`): rufee must render at least 3x
   faster than peewee on a fixed join query.
5. **asyncio** (`test_asyncio.py`): CRUD, `iterate()` and concurrent tasks
   through `playhouse.pwasyncio` with `rufee.Context`, all rendered in Rust.
6. `cargo test` links against the venv's Python (`make cargo-test`); there is
   no pure-Rust logic worth unit-testing yet.

## TODO

Ordered. Done items are kept for the record.

1. ~~**Scaffold the repo.**~~ Done.
   - maturin mixed layout: `Cargo.toml` + `src/lib.rs` (crate `rufee`,
     `#[pymodule] _native`), `python/rufee/__init__.py`, `pyproject.toml`.
   - Target `abi3-py311` so one wheel per platform covers 3.11+.
   - Dev tooling: `uv` for the Python side, `rustup` stable, `just` or `make`
     for the common commands.
   - GitHub Actions: `cargo test`, `pytest`, wheel builds for
     linux x86_64 / aarch64 and macOS arm64.
2. ~~**Shim without a renderer.**~~ Done: `rufee.Context` with fallback, shadow
   mode, version gate.
3. ~~**peewee test harness.**~~ Done: `scripts/run_peewee_tests.py`:
   - fetch peewee source at tag `PEEWEE_TAG` (git checkout in CI; not vendored);
   - apply both bindings from the table above;
   - `RUFEE_FALLBACK=0|1` selects strict / safe;
   - `-e sqlite` and `-e mysql` (MySQL via `docker compose`);
   - allow-list the four pre-existing MySQL failures;
   - print and export the strict pass rate; CI comments it on PRs.
4. ~~**rufee's own tests**~~ Done (section above).
5. ~~**P0 nodes**~~ Done: `Select`/`ModelSelect`, `Expression`, `Field`/`Column`,
   `Value`, `Ordering`, `Join`, `Table`, `Entity`, `NodeList` family,
   `Insert` (incl. `insert_many`, `on_conflict`), `Update`, `Delete`.
6. ~~**P1 nodes**~~ Done: `Function` (`fn.`), `SQL`, `Alias`, `Negated`, `Cast`, `Case`.
7. ~~**P2 nodes**~~ Done: subqueries as sources, CTE, `Window`, `ValuesList`,
   `CompoundSelectQuery`, `BitField` flags. Every node type peewee's suite
   exercises now renders in Rust.
8. **Docs**: installation / user guide done — [`docs/installation.md`](docs/installation.md)
   (install, `context_class`, configuration, verification, version gate,
   troubleshooting) and [`docs/migration-guide.md`](docs/migration-guide.md)
   (shadow → rust rollout, logs and counters, mismatch triage, rollback).
   Still to write: a contributor guide (how to add a node type: Rust match arm
   + hypothesis rule + which peewee tests it unlocks).
9. ~~**Performance pass**~~ Done. Join select went from 14.7 to 9.8 µs
   (7.3x → 11.0x), `insert_many` ×100 from 275 to 195 µs, by: keeping the
   dialect options off the per-scope state so a scope change clones seven
   flags instead of the whole settings; an identity fast path in front of the
   alias dict (each lookup otherwise ran a Python-level `__hash__`); skipping
   the empty-`IN` probe when the rhs is a plain non-empty collection;
   exception-free dict lookups for plain-dict insert rows; sorting `Field`
   columns in Rust; no set arithmetic when a model has no defaults; fewer
   String allocations. What remains is reading the Python node tree through
   the C API and calling each field's `db_value` converter, both required for
   byte-identical output. Going further means not reading the Python tree at
   all, which is the rejected "AST in Rust" design.
10. ~~**Release**~~ Done: a `v*` tag runs CI, builds wheels and the sdist,
    publishes to PyPI (trusted publishing) and attaches them to the GitHub
    release.
11. ~~**peewee 4.5.3**~~ Done (0.2.0): State / `_query_type` interface,
    ordering and window quoting, compound selects, FOR UPDATE, MySQL `CONCAT()`,
    sub-selects as operands and sources, JSON paths, asyncio coverage.
12. **Shadow-mode field report**: run a real application in `RUFEE_MODE=shadow`
    and collect mismatches before recommending `rust` mode.

## Development setup

Requirements: Rust stable (`rustup update`), Python 3.11 (`uv python install 3.11`),
`uv`, Docker (only for the MySQL lane).

```sh
make venv        # .venv with dev deps
make dev         # maturin develop --release into .venv
make test        # cargo test + pytest
make lint        # cargo fmt --check + clippy
make peewee-src  # check out peewee's test-suite at the supported tag
```

Configuration at runtime: `RUFEE_MODE=rust|shadow|off` (default `rust`),
`RUFEE_FALLBACK=1|0` (default `1`; `0` makes unsupported nodes raise
`rufee.Unsupported`), or `rufee.configure(mode=..., fallback=...)`. Counters
live in `rufee.stats`; shadow-mode mismatches are logged on the `rufee`
logger at WARNING (see [`docs/migration-guide.md`](docs/migration-guide.md)).

## License

MIT. See `LICENSE`.
