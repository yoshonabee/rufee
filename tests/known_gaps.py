"""Known rendering gaps against peewee 4.5.3, tracked until they are fixed.

Each entry maps a test id (relative to ``tests/``) to the gap it exercises.
``conftest.py`` turns them into ``xfail(strict=True)``: the suite stays green,
and a fix that makes one of these tests pass fails the run until its entry is
removed here. That is how each fix proves what it fixed.

Property-based tests draw random queries, so they can hit several gaps at once
and may miss a gap on a given run. They are listed in ``FLAKY_GAPS`` as
non-strict xfails and are removed once every gap is fixed.
"""

# Gap categories. Keep these in sync with the README TODO.
# All tracked gaps are fixed; add new ones here with a category constant.
KNOWN_GAPS: dict[str, str] = {
}

FLAKY_GAPS = {
    name: "property test; covers several gaps"
    for name in (
        "test_differential.py::test_select",
        "test_differential.py::test_subquery_sources",
        "test_differential.py::test_compound",
        "test_differential.py::test_insert",
        "test_differential.py::test_update",
        "test_differential.py::test_delete",
        "test_differential.py::test_select_value_literals",
    )
}
