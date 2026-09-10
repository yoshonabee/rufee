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

Status: **renders peewee 3.17.9's entire test-suite byte-identically** (strict
mode 100% on sqlite and MySQL, see below). Not yet released; API may still move.

## How it plugs in

peewee already exposes the renderer as a class attribute:

```python
# peewee 3.17.x
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
`db.execute_sql(raw)`) keeps working unchanged.

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
  2. *Shadow mode*: render both ways, log any mismatch (SQL string or params),
     and return peewee's result. Meant for staging before switching over.
  3. *Version gate*: rufee reads peewee's private attributes (`_from_list`,
     `_where`, `_returning`, `Join._on`, ...). It targets peewee **3.17.x**;
     on any other version it degrades to a pure pass-through.

Rendering time, `Context(**opts).sql(q).query()` including context
construction, best of 5 × 5000 (Apple Silicon, CPython 3.11, peewee 3.17.9,
MySQL dialect options):

| query | peewee | rufee | ratio |
|---|---|---|---|
| join select (2 tables, IN + 2 predicates, ORDER BY, LIMIT/OFFSET) | 107 µs | 15 µs | 7.2x |
| simple select (1 predicate, ORDER BY, LIMIT) | 64 µs | 10 µs | 6.5x |
| `SELECT COUNT(...) WHERE` | 34 µs | 6 µs | 5.6x |
| insert (7 columns) | 37 µs | 10 µs | 3.8x |
| `insert_many`, 100 rows | 1225 µs | 270 µs | 4.5x |
| update (expression + where) | 39 µs | 7 µs | 5.4x |
| delete (IN) | 28 µs | 5 µs | 5.2x |

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

Current result (`scripts/run_peewee_tests.py`, peewee 3.17.9, Python 3.11):

| lane | result | queries rendered by Rust |
|---|---|---|
| sqlite, strict | **860 / 860 pass** (100%), 105 skipped | 5,236, 0 fallbacks |
| sqlite, safe | 860 / 860 pass | 5,236, 9 fallbacks* |
| MySQL 8, strict | **813 / 813 pass** (100%), 148 skipped, 4 allow-listed | 5,028, 0 fallbacks |
| MySQL 8, safe | 813 / 813 pass, 4 allow-listed | 5,028, 14 fallbacks* |

\* safe-mode fallbacks are queries where the Rust renderer raised the same
error peewee raises (`ValueError` from an on-conflict hook, `FOR UPDATE` on
sqlite); the fallback re-renders with peewee, which raises it again.

Baseline measured with **unmodified** peewee 3.17.9:

| backend | result | note |
|---|---|---|
| sqlite | 965 tests, OK, 105 skipped, 2.3 s | |
| MySQL 8.x (pymysql) | 965 tests, 1 failure + 3 errors, 148 skipped, 98 s | pre-existing MySQL 8 incompatibilities in peewee's suite, reproduced without rufee: `test_for_update_nested` (1093 target table in FROM), `test_regexp_iregexp` (3995 utf8mb4_0900_ai_ci vs binary in `regexp_like`), `test_update_examples_2` (1064 syntax), `test_get_views` (view definition text normalisation). These are the harness allow-list; if one of them ever passes, rufee rendered something peewee would not. |

```sh
make venv dev peewee-src               # once
make peewee-tests ARGS="-e sqlite --safe"
make peewee-tests ARGS="-e sqlite --strict"
make mysql-up && make peewee-tests ARGS="-e mysql --strict"
```

Gotcha: on macOS the PyPI wheel's bundled `playhouse/_sqlite_ext` C extension
segfaults on connect when `tests/sqlite.py` is collected. Install peewee without
it (`NO_SQLITE=1 pip install --no-binary peewee peewee==3.17.9`) or delete the
`_sqlite_ext*.so` / `_sqlite_udf*.so` files. Linux CI is unaffected.

### rufee's own tests (`tests/`, 177 cases)

1. **Shim behaviour** (`test_shim.py`): fallback triggers (unsupported node,
   Rust error, Rust panic) never leak exceptions; shadow mode logs mismatches
   and returns peewee's result; version gate degrades to pass-through; the
   initial state and every context option are forwarded.
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
5. `cargo test` links against the venv's Python (`make cargo-test`); there is
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
   - fetch peewee source at tag `3.17.9` (git checkout in CI; not vendored);
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
8. **Docs**: user guide (installation, `context_class`, shadow mode, logging),
   contributor guide (how to add a node type: Rust match arm + hypothesis rule
   + which peewee tests it unlocks).
9. **Performance pass**: profile the walker (attribute reads, `Settings`
   clones per scope change, Python-dict alias manager, `as_new()` probe for
   empty `IN`) — the PoC walker reached ~9x on the join query, the faithful
   port is at 7.2x.
10. **Release**: version, changelog, publish wheels from the `v*` tag workflow.
11. **Shadow-mode field report**: run a real application in `RUFEE_MODE=shadow`
    and collect mismatches before recommending `rust` mode.

## Development setup

Requirements: Rust stable (`rustup update`), Python 3.11 (`uv python install 3.11`),
`uv`, Docker (only for the MySQL lane).

```sh
make venv        # .venv with dev deps; peewee installed without its C extensions
make dev         # maturin develop --release into .venv
make test        # cargo test + pytest
make lint        # cargo fmt --check + clippy
make peewee-src  # check out peewee's test-suite at the supported tag
```

Configuration at runtime: `RUFEE_MODE=rust|shadow|off` (default `rust`),
`RUFEE_FALLBACK=1|0` (default `1`; `0` makes unsupported nodes raise
`rufee.Unsupported`), or `rufee.configure(mode=..., fallback=...)`. Counters
live in `rufee.stats`; shadow-mode mismatches are logged on the `rufee`
logger at WARNING.

## License

MIT. See `LICENSE`.
