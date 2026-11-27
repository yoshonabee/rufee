# Migrating an application to rufee

This guide takes an existing peewee 4.5 application from "rufee installed" to
"rufee rendering production traffic", through shadow mode. Installation itself
is in [`installation.md`](installation.md).

The short version:

| phase | `RUFEE_MODE` | who renders the SQL that runs | what you learn |
|---|---|---|---|
| 0. wired, off | `off` | peewee | the deploy itself is safe |
| 1. shadow | `shadow` | peewee (Rust renders too, for comparison) | whether Rust renders your queries byte-identically |
| 2. rust | `rust` | Rust, peewee for anything unsupported | the speed-up |

Every step is reversible by changing `RUFEE_MODE` and restarting, or with
`rufee.configure(mode=...)` at runtime.

## Why shadow mode first

rufee renders peewee's whole test-suite byte-identically, plus a hypothesis
grammar of random queries. That is a strong spec, but not a complete one: real
applications build query shapes neither covers (a stray trailing comma turning
a `where()` argument into a one-element tuple was found exactly this way). A
wrong render can read or write the wrong rows without raising, so it is cheaper
to find the gaps while peewee's output is still the one being executed.

## Phase 0: wire it in, off

1. Add the dependency: `pip install rufee` / `uv add rufee`. rufee requires
   `peewee>=4.5.3,<4.6`.
2. Swap `context_class` behind a switch so the deploy changes nothing by
   default:

   ```python
   import os

   import rufee

   if os.environ.get("RUFEE_MODE", "off") != "off":
       MyDatabase.context_class = rufee.Context
   ```

   `RUFEE_MODE` is also read by rufee itself at import, so the same variable
   selects `shadow` or `rust`.
3. Configure the `rufee` logger and export the counters (next two sections)
   before turning anything on. Shadow mode without them is invisible.

## What rufee reports

### Counters: `rufee.stats`

Per process, cumulative since start (or since `rufee.stats.reset()`).
`rufee.stats.snapshot()` returns all of them at once:

| counter | incremented when | expected |
|---|---|---|
| `rust` | a query was rendered by Rust and that result was used (`rust` mode) | most queries |
| `shadow` | a query was rendered both ways and compared (`shadow` mode) | most queries |
| `mismatch` | the comparison differed; peewee's result was used | **0** |
| `fallback` | Rust declined or raised; peewee rendered the query | low, steady |
| `errors` | of `fallback`, the ones where Rust raised instead of declining | see below |

`errors` counts both kinds of Rust exceptions: ones peewee raises too (an
invalid query, such as `FOR UPDATE` on sqlite, which the caller sees either
way) and genuine Rust bugs. Only the second kind is logged at WARNING, so watch
the log, not the counter, for those.

Counters are plain integers in each process. To get them into a metrics system,
send the delta periodically:

```python
import os
import threading
import time

import rufee

_started_in_pid = None


def _report_forever(send, interval=60):
    last = rufee.stats.snapshot()
    while True:
        time.sleep(interval)
        now = rufee.stats.snapshot()
        for name, value in now.items():
            if value != last[name]:
                send(name, value - last[name])   # e.g. statsd incr, Prometheus counter inc
        last = now


def start_rufee_reporter(send):
    """Call from code that runs in every worker process, e.g. on first request."""
    global _started_in_pid
    if _started_in_pid == os.getpid():
        return
    _started_in_pid = os.getpid()
    threading.Thread(target=_report_forever, args=(send,), daemon=True).start()
```

Start the thread in each worker *after* it forks: a thread started in a
pre-fork master does not exist in the children. If several kinds of processes
(web workers, background jobs) report to the same metric, label them, since
their query shapes differ a lot.

### Logs: the `rufee` logger

rufee logs through the standard `logging` module, logger name `rufee`. Set its
level like any other library logger, e.g. in a `dictConfig`:

```python
LOGGING["loggers"]["rufee"] = {"level": "WARNING"}
```

| message | level | meaning |
|---|---|---|
| `rufee: shadow mismatch` | WARNING | Rust and peewee rendered the same query differently |
| `rufee: renderer error` | WARNING | Rust raised on a query peewee renders fine (a rufee bug; peewee's result was used) |
| `rufee: fallback for ...` / `rufee: renderer raised ...` | DEBUG | every fallback, with the reason |
| `rufee: peewee X is not supported ...` | WARNING | the version gate fired at import; rufee is a pass-through |

The two WARNING messages are fixed strings, so they can be counted and grouped.
Their details are in one `extra` attribute, `record.rufee`, a dict:

| key | in | content |
|---|---|---|
| `query_type` | both | query class, e.g. `ModelSelect`, `Insert` |
| `callsite` | both | `file:line in function` of the first frame outside peewee, playhouse and rufee: the application code that ran the query |
| `error` | renderer error | `ExceptionType: message` |
| `diff` | mismatch | `"sql"` or `"params"` |
| `offset`, `rust_sql_length`, `peewee_sql_length`, `rust_param_count`, `peewee_param_count` | mismatch, `diff="sql"` | first differing character, both lengths, both parameter counts |
| `rust_excerpt`, `peewee_excerpt` | mismatch, `diff="sql"`, `log_sql` on | 60 characters either side of the first difference |
| `rust_sql`, `peewee_sql` | mismatch, `diff="sql"`, `log_sql` on | both full statements, clipped at 4000 characters |
| `param_index`, `rust_param_count`, `peewee_param_count`, `rust_param_type`, `peewee_param_type` | mismatch, `diff="params"` | where the parameter lists first differ |
| `sql` | mismatch, `diff="params"`, `log_sql` on | the statement both sides agree on |

#### Do the logs contain user data?

**Parameter values are never logged**, only their position, count and type.

The SQL strings normally contain no values either. That is not because rufee
redacts them: peewee (and rufee) render every value as a placeholder (`%s`,
`?`) and return the values separately, and the database driver fills them in
afterwards, just before sending the query. rufee never sees the final string.

SQL strings do contain values in three cases:

| case | where the value shows up |
|---|---|
| the application inlines it: `SQL("... = 'x'")`, string formatting into `SQL()`, `value_literals=True` | both `rust_sql` and `peewee_sql` |
| the mismatch *is* Rust inlining a value instead of emitting a placeholder | `rust_sql`, `rust_excerpt`; tell-tale: `rust_param_count < peewee_param_count` |
| a Rust-only exception message (`rufee: renderer error`, `error`) | unlikely (the messages come from rufee, not from data), not guaranteed |

If the logs must not carry any of that, turn the strings off with
`RUFEE_LOG_SQL=0` or `rufee.configure(log_sql=False)`. Mismatch records then keep
`callsite`, `offset`, lengths, counts and types; reproduce the query from the
`callsite` to see the SQL (see *Triaging a mismatch*).

With a structured logging setup (structlog's `ProcessorFormatter`, a JSON
formatter that copies record attributes) the dict shows up as a nested `rufee`
field without further work. With plain `logging.Formatter`, only the message is
printed; add the details with a filter:

```python
import logging


class RufeeDetails(logging.Filter):
    def filter(self, record):
        details = getattr(record, "rufee", None)
        if details:
            record.msg = f"{record.msg} {details}"
        return True


logging.getLogger("rufee").addFilter(RufeeDetails())
```

## Phase 1: shadow

Deploy with `RUFEE_MODE=shadow`. Each top-level query is rendered by Rust, then
by peewee; the two are compared and **peewee's result is executed**, so
behaviour cannot change. The cost is the Rust render on top of peewee's: with
rufee 4–8x faster than peewee (see the README's benchmark), rendering takes
about 12–25% longer than plain peewee. Rendering is a small part of a query's
wall-clock time.

Turn it on everywhere queries are built, not only in the web tier: batch jobs,
background workers and admin tools tend to build the unusual shapes.

### Inserts in shadow mode

Rendering a query twice matters for inserts in two ways, and rufee handles
both so that the executed result is what plain peewee would have produced:

- **One-shot rows.** `insert_many(generator)`, `insert_many(zip(...),
  fields=...)` and `Model.bulk_create` (which builds its rows with a generator)
  give the query an iterator that only one render could consume. In shadow
  mode rufee turns those rows into a list first; `rust` mode leaves them alone.
- **Callable defaults.** A column missing from an insert row is filled from its
  default; a callable default (`uuid4`, `datetime.now`, an id generator) would
  return a different value per render and show up as a params mismatch. rufee
  records the values the Rust render got and gives the same ones to peewee's
  render, so each default still runs once per row.

**Use a rufee release newer than 0.2.1 for shadow mode.** In 0.2.1 and earlier, shadow
mode broke inserts with one-shot rows (peewee raised "no rows to insert", or
nothing was written) and reported every insert with a callable default as a
mismatch. `rust` mode was not affected.

**Done when**, over a period that covers your whole traffic cycle (a week is a
reasonable default: it covers weekly jobs; include a month boundary if you have
monthly jobs):

- `mismatch` stayed at 0;
- no `rufee: renderer error` warnings;
- `fallback` is low and explained (run with the `rufee` logger at DEBUG for a
  while to see which node types or errors cause it).

### Triaging a mismatch

1. Group warnings by `callsite` and `query_type`: one bad shape usually repeats
   on every request that reaches it.
2. Compare `rust_excerpt` and `peewee_excerpt` (or, with `log_sql` off,
   reproduce first and compare the two renders). Many mismatches are
   semantically equivalent (an extra pair of parentheses) but still bugs:
   rufee's contract is byte-identical output.
3. Reproduce without data. In a shell with the application's models:

   ```python
   import peewee
   import rufee

   opts = db.get_context_options()
   q = ...  # rebuild the query from the code at `callsite`
   print(rufee.Context(**opts).sql(q).query())
   print(peewee.Context(**opts).sql(q).query())
   ```

4. Report the query *shape* (models replaced by placeholders, no data) at
   <https://github.com/yoshonabee/rufee/issues>. Keep shadow mode on until a
   release with the fix is deployed and the mismatch stops.

## Phase 2: rust

Switch `RUFEE_MODE=rust`. Rust's output is now executed; anything it does not
support still falls back to peewee, so keep `RUFEE_FALLBACK` at its default
(`1`). Strict mode (`RUFEE_FALLBACK=0`) is for tests.

If you can, switch one group of processes first (a share of web workers, or one
kind of background job) and compare error rates before switching the rest.

Keep the counters and the WARNING logs in place: `mismatch` stops being
measured in `rust` mode, but `renderer error` warnings and `fallback` counts
still are.

## Rolling back

- `RUFEE_MODE=off` and restart: rufee becomes a pure pass-through.
- Without a restart: `rufee.configure(mode="off")` (or `"shadow"`) in each
  process, e.g. from an admin endpoint or a signal handler.
- Removing the `context_class` assignment restores plain peewee completely.

## Upgrading peewee later

rufee reads peewee's private attributes and only enables itself on the peewee
minor it was built for (currently 4.5.x). On any other version it logs a
warning at import and becomes a pass-through, so a peewee upgrade can slow
rendering back down but cannot make it wrong.

To try a new peewee before rufee supports it, set `RUFEE_FORCE=1` together with
`RUFEE_MODE=shadow` in a test environment, and go back through phase 1.
