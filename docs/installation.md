# Installing rufee

rufee replaces peewee's SQL rendering `Context`. Nothing else about your
application changes: models, fields, query building and row hydration are still
peewee's, so installing rufee is (a) getting the package in, and (b) one
assignment to `Database.context_class`.

## Requirements

| | |
|---|---|
| Python | CPython 3.11, 3.12 or 3.13. Wheels are built `abi3-py311`, so one wheel per platform covers all three. |
| peewee | **4.5.x** (developed against 4.5.3). rufee reads peewee's private attributes; on any other version it turns itself off — see [Version gate](#version-gate). For peewee 3.17 use rufee 0.1.0. |
| Platform | Linux `x86_64` / `aarch64`, macOS `arm64`. Anywhere else, build from source — you need a Rust stable toolchain. |

rufee needs no database driver, no configuration file and no connection: it only
renders SQL.

## Install

### From a wheel

CI builds wheels on every `v*` tag — job `wheels` in `.github/workflows/release.yml`,
one artifact per platform (linux `x86_64`, linux `aarch64`, macOS `arm64`).
From v0.2.0 on, the same files are attached to each
[GitHub release](https://github.com/yoshonabee/rufee/releases) and published to
PyPI. For older tags, download them from that tag's workflow run:

```sh
gh run download --repo yoshonabee/rufee --name wheels-ubuntu-latest-x86_64
```

Then:

```sh
pip install rufee-0.2.0-cp311-abi3-manylinux_2_17_x86_64.whl
```

rufee declares `peewee>=4.5.3,<4.6` as a dependency. If your project already
pins peewee through a lockfile, install with `--no-deps` so the resolver does
not touch it:

```sh
pip install --no-deps rufee-0.2.0-cp311-abi3-manylinux_2_17_x86_64.whl
```

### From PyPI

```sh
pip install rufee        # or: uv add rufee
```

PyPI has the same wheels as the GitHub release, plus the sdist (which builds
from source, so it needs Rust) for other platforms.

### From source

Needs Rust stable (`rustup update`) and, for the editable workflow,
[maturin](https://www.maturin.rs/):

```sh
git clone https://github.com/yoshonabee/rufee.git
cd rufee
maturin build --release            # wheel lands in target/wheels/
pip install target/wheels/rufee-*.whl
```

`pip install .` also works — maturin is the build backend, so pip drives the
Rust build itself. For hacking on rufee, use `make venv dev` instead (see the
README's *Development setup*).

### In a Docker image

Build the wheel in a throwaway stage so the Rust toolchain never reaches the
final image, and install it with `--no-deps` so your locked peewee survives:

```dockerfile
FROM python:3.11-slim AS rufee-build
RUN apt-get update && apt-get install -y rustc cargo build-essential
RUN pip install --no-cache-dir 'maturin>=1.15,<2.0'
COPY rufee /src/rufee
WORKDIR /src/rufee
# The wheel is installed into an image built from the same base, so there is no
# reason to pay for an auditwheel/manylinux repair.
RUN maturin build --release --compatibility linux --out /rufee-dist

FROM python:3.11-slim AS final
# ... your application's dependencies ...
COPY --from=rufee-build /rufee-dist /tmp/rufee-dist
RUN pip install --no-deps /tmp/rufee-dist/*.whl && rm -rf /tmp/rufee-dist
```

Keep the two stages on the same base image: `--compatibility linux` tags the
wheel `linux_<arch>` rather than `manylinux`, which pip will only install on a
matching platform.

## Wire it in

peewee exposes the renderer as a class attribute, so one assignment is enough:

```python
import rufee
from playhouse.pool import PooledMySQLDatabase


class MyDatabase(PooledMySQLDatabase):
    context_class = rufee.Context
```

It is a class attribute, so setting it on your `Database` subclass covers every
instance of it — primary, replica, and any extra connection built from the same
class. Patching an already-defined class works too:

```python
MyDatabase.context_class = rufee.Context
```

Prefer a subclass or an explicit assignment on your own class over patching
`peewee.Database` globally; rufee's own test harness does the latter only
because it has to bind peewee's suite.

To keep the switch reversible in a running system, gate it on an environment
variable so a restart is enough to go back to peewee:

```python
import os

if os.environ.get('RUFEE_MODE', 'off') != 'off':
    MyDatabase.context_class = rufee.Context
```

Everything else keeps working unchanged: `Model.select()`, `.dicts()`,
`insert_many`, `query.sql()`, `db.execute_sql(raw)`. DDL (`create_table`),
nested `__sql__` calls and bare expressions still go through peewee's own
renderer — rufee only intercepts a fresh, top-level query render.

### asyncio (`playhouse.pwasyncio`)

The same assignment works for peewee's async databases:

```python
from playhouse.pwasyncio import AsyncMySQLDatabase


class MyAsyncDatabase(AsyncMySQLDatabase):
    context_class = rufee.Context
```

pwasyncio only makes the driver I/O asynchronous. `await db.aexecute(q)`,
`db.list(q)`, `db.count(q)` and friends run the synchronous `query.execute()`
inside a greenlet, and `db.iterate(q)` calls `query.sql()`; both build their
SQL through `context_class`. Rendering is synchronous CPU work either way
(rufee just finishes it sooner), and it keeps no shared per-query state, so
concurrent tasks do not interfere. peewee's own asyncio test-suite passes
through rufee on sqlite and MySQL.

## Configure

Three environment variables, all read **once at import time**:

| Variable | Default | Meaning |
|---|---|---|
| `RUFEE_MODE` | `rust` | `rust` renders in Rust; `shadow` renders both ways, logs any difference and returns peewee's result; `off` is a pure pass-through. An unrecognised value warns and falls back to `rust`. |
| `RUFEE_FALLBACK` | `1` | `0` / `false` / `no` makes an unsupported node raise `rufee.Unsupported` instead of re-rendering with peewee. Strict mode — for tests, not for production. |
| `RUFEE_FORCE` | unset | `1` runs rufee against an unsupported peewee version anyway. See below. |

Because they are read at import, setting `RUFEE_MODE` after `import rufee` has
no effect. Change it at runtime through the API instead:

```python
rufee.configure(mode='shadow')   # or mode='rust' / 'off'
rufee.configure(fallback=False)  # strict
```

Counters live in `rufee.stats` (`rust`, `fallback`, `errors`, `shadow`,
`mismatch`; `snapshot()` reads them all, `reset()` zeroes them), and rufee
logs on the `rufee` logger: shadow mismatches and Rust errors on queries peewee
renders fine at `WARNING`, every fallback at `DEBUG`. The WARNING records carry
their details in `record.rufee`; see
[the migration guide](migration-guide.md#what-rufee-reports) for the fields and
for exporting the counters.

```python
import logging

logging.getLogger('rufee').setLevel(logging.WARNING)
```

## Verify the install

```python
import peewee
import rufee

opts = db.get_context_options()
query = MyModel.select().where(MyModel.id > 1).limit(5)

print(db.context_class)                                   # rufee._context.Context
print(rufee.Context(**opts).sql(query).query())
print(peewee.Context(**opts).sql(query).query())          # must be identical
print(rufee.stats)                                        # rust=1 ...
```

If `rufee.stats` stays all-zero, rufee is installed but not rendering: either
`context_class` was never swapped, `RUFEE_MODE=off`, or the version gate fired
(look for a `rufee:` warning at import).

## Rolling out

Go through shadow mode first: deploy with `RUFEE_MODE=shadow`, where every
query is rendered both ways and peewee's result is the one executed, watch for
mismatches, then switch to `RUFEE_MODE=rust`. The full procedure, what to
monitor, when to switch and how to roll back, is in
[`migration-guide.md`](migration-guide.md).

## Version gate

rufee reads peewee internals (`_from_list`, `_where`, `_returning`,
`Join._on`, ...) and targets peewee 4.5.x. On any other version it logs

```
rufee: peewee X.Y.Z is not supported (need 4.5.x); rufee.Context is a
pass-through. Set RUFEE_FORCE=1 to override.
```

and `rufee.Context` behaves exactly like `peewee.Context`. This is deliberate:
a peewee upgrade degrades performance, not correctness. `RUFEE_FORCE=1` lifts
the gate for testing a new peewee version — pair it with `RUFEE_MODE=shadow`.

## Troubleshooting

**Everything works but nothing is faster.** rufee is a pass-through. Check
`rufee.stats`, then in order: is `context_class` really `rufee.Context`
(`print(db.context_class)`), is `RUFEE_MODE` `off`, did the version gate warn at
import?

**`stats.fallback` is climbing.** Some node type is not implemented in Rust, or
the render raised. Both are logged at `DEBUG` on the `rufee` logger with the
query class and the reason; turn that on to see which. A Rust error on a query
peewee renders fine is also logged at `WARNING` as `rufee: renderer error`,
with the application's `callsite`.

**pip: "not a supported wheel on this platform".** The wheel was built for a
different platform tag — for `--compatibility linux` wheels, build and install
on the same base image.
