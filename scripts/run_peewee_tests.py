#!/usr/bin/env python
"""Run peewee's own test-suite through rufee.Context.

peewee's tests reach the SQL renderer two ways; both are bound to rufee here:

* ``assertSQL()`` / ``__sql__()`` in ``tests/base.py`` build ``Context(**state)``
  directly. ``Context`` comes from ``from peewee import *``, so rebinding
  ``peewee.Context`` before ``tests`` is imported covers it.
* Executing tests go through ``Database.execute()`` -> ``context_class``.

Modes (``RUFEE_FALLBACK`` or ``--strict`` / ``--safe``):

* strict: an unsupported node raises ``rufee.Unsupported`` and the test errors.
  The pass rate is the coverage progress. Failures that are *not* ``Unsupported``
  are listed separately: once a node type is implemented they are rendering
  bugs; while the renderer is incomplete some are artefacts of aborting a test
  half-way (a test expecting ValueError gets Unsupported first, a thread swallows
  it, a query counter comes up short). Strict mode is informational and always
  exits 0 unless ``--fail-on-unexpected`` is given.
* safe: fallback on. Everything must pass except the allow-listed, pre-existing
  failures of the chosen backend; otherwise exit 1.

Usage::

    scripts/run_peewee_tests.py [-e sqlite|mysql] [--strict|--safe] [--json out]
                                [--peewee-src DIR] [tests.module ...]
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent

# Tests that fail with unmodified peewee on that backend (``--baseline``), i.e.
# not rufee's doing. Empty for peewee 4.5.3 on sqlite and MySQL 8. A listed
# test that passes under rufee means rufee rendered something peewee would not.
ALLOWLIST: dict[str, set[str]] = {
    "sqlite": set(),
    "mysql": set(),
}

# Known rufee rendering gaps, one file per engine: ``<test id>  # <reason>``.
# They are reported but do not fail the run; one that starts passing does,
# until it is removed (``--update-known-failures`` rewrites the file).
KNOWN_FAILURES_DIR = HERE / "known_failures"


def load_known_failures(engine: str) -> dict[str, str]:
    path = KNOWN_FAILURES_DIR / f"{engine}.txt"
    known = {}
    if path.exists():
        for line in path.read_text().splitlines():
            test_id, _, reason = line.partition("#")
            if test_id.strip():
                known[test_id.strip()] = reason.strip()
    return known


def write_known_failures(engine: str, outcomes) -> Path:
    path = KNOWN_FAILURES_DIR / f"{engine}.txt"
    path.parent.mkdir(exist_ok=True)
    lines = [f"{test_id}  # {kind}: {msg}"[:240]
             for test_id, (kind, msg) in sorted(outcomes.items())
             if kind in ("fail", "error", "unsupported")]
    path.write_text("".join(f"{line}\n" for line in lines))
    return path


MYSQL_DEFAULTS = {"host": "127.0.0.1", "port": "33306", "user": "root", "password": "peewee"}


def parse_args(argv):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("-e", "--engine", default="sqlite", choices=["sqlite", "mysql"])
    mode = p.add_mutually_exclusive_group()
    mode.add_argument("--strict", action="store_true", help="fallback off (default from RUFEE_FALLBACK)")
    mode.add_argument("--safe", action="store_true", help="fallback on")
    p.add_argument("--peewee-src", default=str(ROOT / "peewee-src"),
                   help="checkout of peewee at the supported tag (default: ./peewee-src)")
    mode.add_argument("--baseline", action="store_true",
                      help="rufee off (pure pass-through): measure unmodified peewee for the allow-list")
    p.add_argument("--json", help="write per-test outcomes and the summary to this file")
    p.add_argument("--update-known-failures", action="store_true",
                   help="rewrite scripts/known_failures/<engine>.txt from this run's failures")
    p.add_argument("-v", "--verbosity", type=int, default=1)
    p.add_argument("--failfast", action="store_true")
    p.add_argument("--fail-on-unexpected", action="store_true",
                   help="in strict mode, exit 1 when non-Unsupported failures occur (safe mode always does)")
    for k, v in MYSQL_DEFAULTS.items():
        p.add_argument(f"--mysql-{k}", default=os.environ.get(f"PEEWEE_MYSQL_{k.upper()}", v))
    p.add_argument("modules", nargs="*", help="e.g. sql model_sql (default: whole suite)")
    return p.parse_args(argv)


class RecordingResult(unittest.TextTestResult):
    """TextTestResult that also keeps a machine-readable outcome per test."""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.outcomes: dict[str, tuple[str, str]] = {}

    def addSuccess(self, test):
        super().addSuccess(test)
        self.outcomes[test.id()] = ("pass", "")

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        self.outcomes[test.id()] = ("skip", reason)

    def addFailure(self, test, err):
        super().addFailure(test, err)
        self.outcomes[test.id()] = ("fail", self._short(err))

    def addError(self, test, err):
        super().addError(test, err)
        kind = "unsupported" if self._is_unsupported(err) else "error"
        # In strict mode an Unsupported error inside the test body is often
        # followed by a second error from tearDown (e.g. closing a database
        # with a transaction still open). Keep the root classification.
        if self.outcomes.get(test.id(), ("", ""))[0] == "unsupported":
            return
        self.outcomes[test.id()] = (kind, self._short(err))

    def stopTest(self, test):
        super().stopTest(test)
        _reset_open_transactions()

    def addExpectedFailure(self, test, err):
        super().addExpectedFailure(test, err)
        self.outcomes[test.id()] = ("pass", "expected failure")

    def addUnexpectedSuccess(self, test):
        super().addUnexpectedSuccess(test)
        self.outcomes[test.id()] = ("fail", "unexpected success")

    @staticmethod
    def _is_unsupported(err):
        import rufee

        exc = err[1]
        while exc is not None:
            if isinstance(exc, rufee.Unsupported):
                return True
            exc = exc.__cause__ or exc.__context__
        return False

    @staticmethod
    def _short(err):
        exc = err[1]
        return f"{type(exc).__name__}: {exc}".splitlines()[0][:300]


def _reset_open_transactions() -> None:
    """Strict mode aborts tests mid-transaction; do not let that leak into the
    next test through peewee's module-level database object."""
    import tests.base as base

    for db in (getattr(base, "db", None),):
        if db is None or db.is_closed():
            continue
        if db.in_transaction():
            try:
                db.rollback()
            except Exception:  # noqa: BLE001
                pass
            db._state.transactions.clear()


# Files peewee's tests write into the working directory (same list as runtests.py).
SCRATCH_FILES = ["peewee_test.db", "peewee_test.db-shm", "peewee_test.db-wal",
                 "peewee_test.dbc", "peewee_pwiz.db", "peewee_test", "tmp.db",
                 "peewee_test.bdb.db", "peewee_test.cipher.db", "peewee_test.bdb.db-journal"]


def cleanup_scratch_files() -> None:
    import shutil

    for name in SCRATCH_FILES:
        p = Path(name)
        if p.is_dir():
            shutil.rmtree(p)
        elif p.exists():
            p.unlink()


def bind_rufee(strict: bool, baseline: bool = False) -> None:
    import peewee

    import rufee

    rufee.configure(mode="off" if baseline else "rust", fallback=not strict)
    peewee.Context = rufee.Context  # assertSQL() / __sql__() path
    peewee.Database.context_class = rufee.Context  # execute() path


def configure_backend(args) -> None:
    os.environ["PEEWEE_TEST_BACKEND"] = args.engine
    os.environ["PEEWEE_TEST_VERBOSITY"] = str(args.verbosity)
    if args.engine == "mysql":
        for k in MYSQL_DEFAULTS:
            os.environ[f"PEEWEE_MYSQL_{k.upper()}"] = str(getattr(args, f"mysql_{k}"))


def collect(modules):
    import tests  # noqa: F401  (peewee's test package, from --peewee-src)

    loader = unittest.TestLoader()
    if not modules:
        return loader.loadTestsFromModule(tests)
    names = [m if m.startswith("tests.") else f"tests.{m}" for m in modules]
    return loader.loadTestsFromNames(names)


def summarize(outcomes, allowlist, known=None):
    known = known or {}
    counts = {"pass": 0, "skip": 0, "unsupported": 0, "fail": 0, "error": 0,
              "allowlisted": 0, "known": 0}
    unexpected = []
    allowlisted_but_passing = []
    known_but_passing = []
    for test_id, (kind, msg) in sorted(outcomes.items()):
        if test_id in allowlist:
            if kind == "pass":
                allowlisted_but_passing.append(test_id)
            else:
                counts["allowlisted"] += 1
            continue
        if test_id in known and kind != "skip":
            if kind == "pass":
                known_but_passing.append(test_id)
                counts["pass"] += 1
            else:
                counts["known"] += 1
            continue
        counts[kind] += 1
        if kind in ("fail", "error"):
            unexpected.append((test_id, kind, msg))
    considered = (counts["pass"] + counts["unsupported"] + counts["fail"] + counts["error"]
                  + counts["known"])
    rate = counts["pass"] / considered if considered else 0.0
    return counts, rate, unexpected, allowlisted_but_passing, known_but_passing


def main(argv=None):
    args = parse_args(argv)
    if args.strict:
        strict = True
    elif args.safe:
        strict = False
    else:
        strict = os.environ.get("RUFEE_FALLBACK", "1") in ("0", "false", "no")

    src = Path(args.peewee_src)
    if not (src / "tests" / "base.py").exists():
        sys.exit(f"peewee test-suite not found at {src}; run `make peewee-src` first")
    sys.path.insert(0, str(src))
    if args.json:
        args.json = str(Path(args.json).resolve())  # before chdir below
    os.chdir(src)  # peewee's tests write sqlite files relative to cwd

    cleanup_scratch_files()
    configure_backend(args)
    bind_rufee(strict, args.baseline)

    import peewee

    import rufee

    mode_name = "baseline" if args.baseline else ("strict" if strict else "safe")
    print(f"rufee {rufee.__version__} | peewee {peewee.__version__} | engine={args.engine} | "
          f"mode={mode_name}")

    suite = collect(args.modules)
    runner = unittest.TextTestRunner(verbosity=args.verbosity, failfast=args.failfast,
                                     resultclass=RecordingResult)
    started = time.time()
    result = runner.run(suite)
    elapsed = time.time() - started
    cleanup_scratch_files()

    allowlist = ALLOWLIST[args.engine]
    # Known gaps are tracked against safe mode; strict and baseline runs ignore them.
    known = load_known_failures(args.engine) if not (strict or args.baseline) else {}
    counts, rate, unexpected, allow_pass, known_pass = summarize(result.outcomes, allowlist, known)
    if args.update_known_failures:
        path = write_known_failures(args.engine, result.outcomes)
        print(f"wrote {path}")
        unexpected, known_pass = [], []

    print()
    print(f"== rufee summary ({args.engine}, {mode_name}) ==")
    print(f"pass={counts['pass']} unsupported={counts['unsupported']} fail={counts['fail']} "
          f"error={counts['error']} skip={counts['skip']} allowlisted={counts['allowlisted']} "
          f"known={counts['known']} "
          f"| {elapsed:.1f}s")
    print(f"strict pass rate: {rate:.1%}" if strict else f"pass rate: {rate:.1%}")
    print(f"renders: rust={rufee.stats.rust} fallback={rufee.stats.fallback}")

    ok = True
    if unexpected:
        if not strict or args.fail_on_unexpected:
            ok = False
        print(f"\n{len(unexpected)} unexpected failure(s)" +
              (" (not Unsupported; rendering bugs or strict-mode artefacts)" if strict else "") + ":")
        for test_id, kind, msg in unexpected:
            print(f"  {kind:5s} {test_id}\n        {msg}")
    if allow_pass:
        ok = False
        print("\nallow-listed tests now PASS; rufee output differs from peewee here:")
        for test_id in allow_pass:
            print(f"  {test_id}")
    if known_pass:
        ok = False
        print(f"\n{len(known_pass)} known failure(s) now PASS; remove them from "
              f"scripts/known_failures/{args.engine}.txt (or rerun with --update-known-failures):")
        for test_id in known_pass:
            print(f"  {test_id}")

    if args.json:
        Path(args.json).write_text(json.dumps({
            "engine": args.engine, "strict": strict, "elapsed": elapsed,
            "counts": counts, "pass_rate": rate,
            "stats": {"rust": rufee.stats.rust, "fallback": rufee.stats.fallback},
            "unexpected": unexpected, "allowlisted_passing": allow_pass,
            "known_passing": known_pass,
            "outcomes": {k: list(v) for k, v in result.outcomes.items()},
        }, indent=1))

    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
