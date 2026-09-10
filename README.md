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

Status: **planning / scaffolding**. Nothing usable yet.

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

Early measurements on one join query (Apple Silicon, CPython 3.13, peewee
3.17.9): peewee renders in ~113 µs, the Rust walker over the same Python tree
in ~13 µs (~9x); including Python-side query construction the end-to-end gain is
~5x. Row hydration (`CursorWrapper`, `Model.__init__`) is out of scope and
unchanged.

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

Baseline measured on 2026-09-10 with **unmodified** peewee 3.17.9, Python 3.11.13:

| backend | result | note |
|---|---|---|
| sqlite | 965 tests, OK, 105 skipped, 2.3 s | |
| MySQL 8.x (pymysql) | 965 tests, 1 failure + 3 errors, 148 skipped, 98 s | pre-existing MySQL 8 incompatibilities in peewee's suite, reproduced without rufee: `test_for_update_nested` (1093 target table in FROM), `test_regexp_iregexp` (3995 utf8mb4_0900_ai_ci vs binary in `regexp_like`), `test_update_examples_2` (1064 syntax), `test_get_views` (view definition text normalisation). These become the allow-list for the harness. |

Gotcha: on macOS the PyPI wheel's bundled `playhouse/_sqlite_ext` C extension
segfaults on connect when `tests/sqlite.py` is collected. Install peewee without
it (`NO_SQLITE=1 pip install --no-binary peewee peewee==3.17.9`) or delete the
`_sqlite_ext*.so` / `_sqlite_udf*.so` files. Linux CI is unaffected.

### rufee's own tests

1. **Shim behaviour (pytest)**: fallback triggers (unsupported node, Rust error,
   Rust panic) never leak exceptions; shadow mode logs mismatches and returns
   peewee's result; version gate degrades to pass-through; every context option
   is forwarded.
2. **Differential testing (hypothesis)**: generate random query trees from a
   small grammar and assert `rufee.Context` and `peewee.Context` produce
   identical output.
3. **Rust unit tests (`cargo test`)**: quoting, alias numbering, parameter
   ordering, escaping.
4. **Benchmark regression**: render time must stay below 1/5 of peewee's.

## TODO

Ordered. Each item should land as its own PR.

1. **Scaffold the repo.**
   - maturin mixed layout: `Cargo.toml` + `src/lib.rs` (crate `rufee`,
     `#[pymodule] _native`), `python/rufee/__init__.py`, `pyproject.toml`.
   - Target `abi3-py311` so one wheel per platform covers 3.11+.
   - Dev tooling: `uv` for the Python side, `rustup` stable, `just` or `make`
     for the common commands.
   - GitHub Actions: `cargo test`, `pytest`, wheel builds for
     linux x86_64 / aarch64 and macOS arm64.
2. **Shim without a renderer.** `rufee.Context` with fallback, shadow mode,
   version gate, and a `_native.render()` stub that always raises
   `Unsupported`. At this point safe mode must already pass 100% of peewee's
   suite.
3. **peewee test harness.** `scripts/run_peewee_tests.py`:
   - fetch peewee source at tag `3.17.9` (git checkout in CI; not vendored);
   - apply both bindings from the table above;
   - `RUFEE_FALLBACK=0|1` selects strict / safe;
   - `-e sqlite` and `-e mysql` (MySQL via `docker compose`);
   - allow-list the four pre-existing MySQL failures;
   - print and export the strict pass rate; CI comments it on PRs.
4. **rufee's own tests** (section above): shim pytest, hypothesis grammar
   covering the P0 nodes, `cargo test`, benchmark.
5. **P0 nodes**: `Select`/`ModelSelect`, `Expression`, `Field`/`Column`,
   `Value`, `Ordering`, `Join`, `Table`, `Entity`, `NodeList` family,
   `Insert` (incl. `insert_many`, `on_conflict`), `Update`, `Delete`.
6. **P1 nodes**: `Function` (`fn.`), `SQL`, `Alias`, `Negated`, `Cast`, `Case`.
7. **P2 nodes** (may stay on fallback for a long time): subqueries as sources,
   CTE, `Window`.
8. **Docs**: user guide (installation, `context_class`, shadow mode, logging),
   contributor guide (how to add a node type: Rust match arm + hypothesis rule
   + which peewee tests it unlocks).

## Development setup

Requirements: Rust stable (`rustup update`), Python 3.11 (`uv python install 3.11`),
`uv`, Docker (only for the MySQL lane).

```sh
uv venv -p 3.11 .venv
VIRTUAL_ENV=$PWD/.venv uv run maturin develop --release
```

(Commands will be finalised with the scaffold in TODO 1.)

## License

MIT. See `LICENSE`.
