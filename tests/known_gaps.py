"""Known rendering gaps against peewee 4.5.3, tracked until they are fixed.

Each entry maps a test id (relative to ``tests/``) to the gap it exercises.
``conftest.py`` turns them into ``xfail(strict=True)``: the suite stays green,
and a fix that makes one of these tests pass fails the run until its entry is
removed here. That is how each fix proves what it fixed.
"""

# Gap categories. Keep these in sync with the README TODO.
# All tracked gaps are fixed; add new ones here with a category constant.
KNOWN_GAPS: dict[str, str] = {
}
