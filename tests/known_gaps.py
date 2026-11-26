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
STATE = "peewee 4 State(scope, parentheses, settings) constructor"
QUERY_TYPE = "Insert._query_type is a read-only property in peewee 4"
OPERATORS = "MySQL CONCAT() instead of || (peewee 4.5)"
ORDERING = "Ordering.collate() renders COLLATE before the direction"
FOR_UPDATE = "FOR UPDATE was reworked (no _for_update_of etc.)"
COMPOUND = "compound select parentheses / alias scoping changed"
VALUE_LITERALS = "value_literals escapes single quotes"

KNOWN_GAPS = {
    "test_shim.py::test_reference_render_survives_rebinding_peewee_context": STATE,
    "test_shim.py::test_shadow_mode_param_mismatch_is_reported": STATE,
    "test_shim.py::test_shadow_mode_returns_peewee_result_on_mismatch": STATE,
    "test_shim.py::test_shadow_mode_silent_when_identical": STATE,
    **{
        f"test_differential.py::test_representative_queries[{name}-{dialect}]": QUERY_TYPE
        for name in ("insert_from_query", "insert_ignore", "insert_kwargs",
                     "insert_many_dicts", "insert_many_tuples", "insert_replace",
                     "insert_table")
        for dialect in ("sqlite", "mysql")
    },
    "test_differential.py::test_representative_queries[concat-mysql]": OPERATORS,
    "test_options.py::test_concat_mysql_is_a_function": OPERATORS,
    "test_options.py::test_operations_mapping[CONCAT]": OPERATORS,
    "test_differential.py::test_representative_queries[ordering-sqlite]": ORDERING,
    "test_differential.py::test_representative_queries[ordering-mysql]": ORDERING,
    "test_options.py::test_for_update_mysql": FOR_UPDATE,
    "test_options.py::test_for_update_option_alone_decides": FOR_UPDATE,
    "test_options.py::test_for_update_variants[kwargs0-FOR SHARE]": FOR_UPDATE,
    "test_options.py::test_for_update_variants[kwargs1-FOR UPDATE NOWAIT]": FOR_UPDATE,
    "test_options.py::test_for_update_variants[kwargs2-FOR UPDATE NOWAIT]": FOR_UPDATE,
    "test_options.py::test_for_update_variants[kwargs3-FOR UPDATE OF `t1`]": FOR_UPDATE,
    "test_options.py::test_for_update_variants[kwargs4-FOR SHARE OF `t1` NOWAIT]": FOR_UPDATE,
    "test_differential.py::test_representative_queries[union_all_intersect-sqlite]": COMPOUND,
    "test_options.py::test_compound_select_parentheses_modes[sqlite-0]": COMPOUND,
    "test_options.py::test_compound_select_parentheses_modes[mysql-0]": COMPOUND,
    "test_options.py::test_value_literals_types": VALUE_LITERALS,
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
