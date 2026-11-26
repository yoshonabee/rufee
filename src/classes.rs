//! Handles to the peewee classes the renderer dispatches on, resolved once.

use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::PyType;

macro_rules! classes {
    ($($field:ident = $module:literal :: $name:literal),* $(,)?) => {
        pub struct Classes {
            $(pub $field: Py<PyAny>,)*
        }

        impl Classes {
            fn load(py: Python<'_>) -> PyResult<Self> {
                Ok(Classes {
                    $($field: py.import($module)?.getattr($name)?.unbind(),)*
                })
            }
        }
    };
}

classes! {
    node = "peewee"::"Node",
    field = "peewee"::"Field",
    field_alias = "peewee"::"FieldAlias",
    composite_key = "peewee"::"CompositeKey",
    foreign_key_field = "peewee"::"ForeignKeyField",
    wrapped_node = "peewee"::"WrappedNode",
    expression = "peewee"::"Expression",
    string_expression = "peewee"::"StringExpression",
    entity = "peewee"::"Entity",
    sql = "peewee"::"SQL",
    value = "peewee"::"Value",
    value_literals = "peewee"::"ValueLiterals",
    column = "peewee"::"Column",
    node_list = "peewee"::"NodeList",
    alias = "peewee"::"Alias",
    bind_to = "peewee"::"BindTo",
    negated = "peewee"::"Negated",
    bitwise_negated = "peewee"::"BitwiseNegated",
    cast = "peewee"::"Cast",
    ordering = "peewee"::"Ordering",
    function = "peewee"::"Function",
    window = "peewee"::"Window",
    window_alias = "peewee"::"WindowAlias",
    in_function = "peewee"::"_InFunction",
    case = "peewee"::"Case",
    for_update = "peewee"::"ForUpdate",
    star = "peewee"::"Star",
    qualified_names = "peewee"::"QualifiedNames",
    namespace_attribute = "peewee"::"NamespaceAttribute",
    table = "peewee"::"Table",
    join = "peewee"::"Join",
    values_list = "peewee"::"ValuesList",
    cte = "peewee"::"CTE",
    model = "peewee"::"Model",
    model_alias = "peewee"::"ModelAlias",
    select = "peewee"::"Select",
    noop_model_select = "peewee"::"NoopModelSelect",
    model_select = "peewee"::"ModelSelect",
    select_query = "peewee"::"SelectQuery",
    compound_select_query = "peewee"::"CompoundSelectQuery",
    update = "peewee"::"Update",
    insert = "peewee"::"Insert",
    delete = "peewee"::"Delete",
    raw_query = "peewee"::"RawQuery",
    context = "peewee"::"Context",
    on_conflict = "peewee"::"OnConflict",
    model_write_query_helper = "peewee"::"_ModelWriteQueryHelper",
    qualify_names = "peewee"::"qualify_names",
    mapping = "collections.abc"::"Mapping",
    range = "builtins"::"range",
    datetime = "datetime"::"datetime",
    date = "datetime"::"date",
    time = "datetime"::"time",
}

static CLASSES: PyOnceLock<Classes> = PyOnceLock::new();

pub fn classes(py: Python<'_>) -> PyResult<&'static Classes> {
    CLASSES.get_or_try_init(py, || Classes::load(py))
}

/// `isinstance(obj, cls)` for one of the cached classes.
#[inline]
pub fn isa(obj: &Bound<'_, PyAny>, cls: &Py<PyAny>) -> PyResult<bool> {
    obj.is_instance(cls.bind(obj.py()))
}

/// peewee's `is_model(o)`: a class that subclasses `Model`.
pub fn is_model(obj: &Bound<'_, PyAny>, c: &Classes) -> PyResult<bool> {
    match obj.cast::<PyType>() {
        Ok(t) => t.is_subclass(c.model.bind(obj.py())),
        Err(_) => Ok(false),
    }
}

// peewee scope constants.
pub const SCOPE_NORMAL: u8 = 1;
pub const SCOPE_SOURCE: u8 = 2;
pub const SCOPE_VALUES: u8 = 4;
pub const SCOPE_CTE: u8 = 8;
pub const SCOPE_COLUMN: u8 = 16;

// CompoundSelectQuery parentheses settings.
pub const CSQ_PARENTHESES_NEVER: i64 = 0;
pub const CSQ_PARENTHESES_ALWAYS: i64 = 1;
pub const CSQ_PARENTHESES_UNNESTED: i64 = 2;
