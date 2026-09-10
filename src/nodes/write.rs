//! Insert, Update, Delete: ports of peewee's `_WriteQuery.__sql__`,
//! `Update.__sql__`, `Insert.__sql__` (with `_simple_insert`,
//! `_generate_insert`, `_query_insert`, `_default_values` and the on-conflict
//! hooks) and `Delete.__sql__`.
//!
//! Output must be byte-identical to peewee's, including parameter order, so
//! the control flow below mirrors the Python line by line. Intermediate
//! peewee nodes (`Value(v, unpack=False)`, `NodeList((k, SQL('='), v))`,
//! `EnclosedNodeList(...)`) are rendered directly instead of being built;
//! rare helpers (`qualify_names`, `get_sort_key` on exotic nodes, the dialect's
//! `conflict_statement` / `conflict_update` / `default_values_insert`) are
//! called in Python.

use pyo3::exceptions::{PyIndexError, PyKeyError, PyTypeError, PyValueError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyList, PySet, PyString, PyTuple};

use crate::classes::{isa, SCOPE_NORMAL, SCOPE_SOURCE, SCOPE_VALUES};
use crate::ctx::{Converter, Ctx, Over};
use crate::nodes::{basic, query};

// Insert._query_type values.
const INSERT_SIMPLE: u8 = 0;
const INSERT_QUERY: u8 = 1;
const INSERT_MULTI: u8 = 2;

/// `_WriteQuery.__sql__`: `Query.__sql__` (the WITH clause) followed by
/// `_set_table_alias`, which pins the target table's alias to its own name so
/// that correlated sub-selects refer to it as `table.column` instead of `t2`.
fn write_query_prelude<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    query::query_ctes(ctx, node)?;
    // `_ModelWriteQueryHelper._set_table_alias` uses `self.model._meta.table`;
    // the plain `_WriteQuery` version uses `self.table` (a `Table`).
    let table = if isa(node, &ctx.c.model_write_query_helper)? {
        node.getattr(intern!(py, "model"))?
            .getattr(intern!(py, "_meta"))?
            .getattr(intern!(py, "table"))?
    } else {
        node.getattr(intern!(py, "table"))?
    };
    let name: String = table.getattr(intern!(py, "__name__"))?.extract()?;
    ctx.aliases.set(&table, &name)
}

/// `_WriteQuery.apply_returning`.
fn apply_returning<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let returning = node.getattr(intern!(ctx.py, "_returning"))?;
    if returning.is_truthy()? {
        ctx.with(Over::default().scope(SCOPE_SOURCE), |ctx| {
            ctx.literal(" RETURNING ");
            basic::comma_list(ctx, &returning)
        })?;
    }
    Ok(())
}

/// A throw-away peewee `Context` carrying our current state, for Python
/// helpers that take a context (`Node.get_sort_key(ctx)`).
fn python_ctx<'py>(ctx: &Ctx<'py>) -> PyResult<Bound<'py, PyAny>> {
    let py = ctx.py;
    let raw = ctx.dialect.raw.bind(py);
    let pctx = ctx.c.context.bind(py).call((), Some(raw))?;
    let state_cls = py.import("peewee")?.getattr("State")?;
    let st = state_cls.call((ctx.state.scope, ctx.state.parentheses), Some(raw))?;
    pctx.setattr(intern!(py, "state"), st)?;
    Ok(pctx)
}

/// `obj.get_sort_key(ctx)`. `Field` and `Column` (the only key types peewee
/// produces) are answered in Rust; anything else goes through Python.
fn sort_key<'py>(
    ctx: &Ctx<'py>,
    obj: &Bound<'py, PyAny>,
    pctx: &mut Option<Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let py = ctx.py;
    let c = ctx.c;
    if isa(obj, &c.field)? {
        // Field.get_sort_key: return self._sort_key
        return obj.getattr(intern!(py, "_sort_key"));
    }
    if obj.get_type().is(c.column.bind(py)) && ctx.scope() == SCOPE_VALUES {
        // Column.get_sort_key in SCOPE_VALUES: (self.name,)
        let name = obj.getattr(intern!(py, "name"))?;
        return Ok(PyTuple::new(py, [name])?.into_any());
    }
    let p = match pctx {
        Some(p) => p.clone(),
        None => {
            let p = python_ctx(ctx)?;
            *pctx = Some(p.clone());
            p
        }
    };
    obj.call_method1(intern!(py, "get_sort_key"), (p,))
}

/// `sorted(nodes, key=lambda obj: obj.get_sort_key(ctx))`, as the permutation
/// of indices. Sorting is done by Python on `(key, index)` tuples so that
/// comparison semantics (and stability) are exactly `sorted()`'s.
fn sort_order<'py>(ctx: &Ctx<'py>, nodes: &[Bound<'py, PyAny>]) -> PyResult<Vec<usize>> {
    let py = ctx.py;
    let mut pctx = None;
    let list = PyList::empty(py);
    for (i, n) in nodes.iter().enumerate() {
        let key = sort_key(ctx, n, &mut pctx)?;
        list.append((key, i))?;
    }
    if nodes.len() > 1 {
        list.sort()?;
    }
    let mut order = Vec::with_capacity(nodes.len());
    for item in list.iter() {
        let (_, i): (Bound<'py, PyAny>, usize) = item.extract()?;
        order.push(i);
    }
    Ok(order)
}

fn sorted_nodes<'py>(
    ctx: &Ctx<'py>,
    nodes: Vec<Bound<'py, PyAny>>,
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    let order = sort_order(ctx, &nodes)?;
    Ok(order.into_iter().map(|i| nodes[i].clone()).collect())
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

// Update.__sql__
pub fn update<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    write_query_prelude(ctx, node)?;

    ctx.with(Over::default().scope(SCOPE_VALUES).subquery(true), |ctx| {
        ctx.literal("UPDATE ");

        // sorted(self._update.items(), key=ctx.column_sort_key)
        let update = node.getattr(intern!(py, "_update"))?;
        let mut items: Vec<(Bound<'py, PyAny>, Bound<'py, PyAny>)> = Vec::new();
        for item in update.call_method0(intern!(py, "items"))?.try_iter()? {
            items.push(item?.extract()?);
        }
        let keys: Vec<Bound<'py, PyAny>> = items.iter().map(|(k, _)| k.clone()).collect();
        let order = sort_order(ctx, &keys)?;

        let table = node.getattr(intern!(py, "table"))?;
        ctx.sql(&table)?;
        ctx.literal(" SET ");
        // CommaNodeList([NodeList((k, SQL('='), v)), ...])
        if !order.is_empty() {
            ctx.with(Over::default().parens(false), |ctx| {
                let n = order.len();
                for (i, idx) in order.iter().enumerate() {
                    let (k, v) = &items[*idx];
                    ctx.with(Over::default().parens(false), |ctx| {
                        ctx.sql(k)?;
                        ctx.literal(" = ");
                        update_value(ctx, k, v)
                    })?;
                    if i + 1 < n {
                        ctx.literal(", ");
                    }
                }
                Ok(())
            })?;
        }

        let from = node.getattr(intern!(py, "_from"))?;
        if from.is_truthy()? {
            ctx.with(Over::default().scope(SCOPE_SOURCE).parens(false), |ctx| {
                ctx.literal(" FROM ");
                basic::comma_list(ctx, &from)
            })?;
        }

        let where_ = node.getattr(intern!(py, "_where"))?;
        if where_.is_truthy()? {
            ctx.with(Over::default().scope(SCOPE_NORMAL), |ctx| {
                ctx.literal(" WHERE ");
                ctx.sql(&where_)
            })?;
        }
        query::apply_ordering(ctx, node)?;
        apply_returning(ctx, node)
    })
}

/// The right-hand side of one `SET k = v`, as `Update.__sql__` normalises it:
///
/// ```text
/// if not isinstance(v, Node):
///     v = k.to_value(v) if isinstance(k, Field) else Value(v, unpack=False)
/// elif isinstance(v, Model) and isinstance(k, ForeignKeyField):
///     v = k.to_value(v)
/// if not isinstance(v, Value):
///     v = qualify_names(v)
/// ```
///
/// `Field.to_value(v)` is `Value(v, self.db_value, unpack=False)`, rendered
/// here as `ctx.value(v, db_value)` without building the node.
fn update_value<'py>(
    ctx: &mut Ctx<'py>,
    k: &Bound<'py, PyAny>,
    v: &Bound<'py, PyAny>,
) -> PyResult<()> {
    let py = ctx.py;
    let c = ctx.c;
    if !isa(v, &c.node)? {
        if isa(k, &c.field)? {
            let db_value = k.getattr(intern!(py, "db_value"))?;
            return ctx.value(v, Converter::Fn(&db_value), true);
        }
        return ctx.value(v, Converter::Default, true);
    }
    if isa(v, &c.model)? && isa(k, &c.foreign_key_field)? {
        let db_value = k.getattr(intern!(py, "db_value"))?;
        return ctx.value(v, Converter::Fn(&db_value), true);
    }
    if isa(v, &c.value)? {
        return ctx.sql(v);
    }
    let qualified = c.qualify_names.bind(py).call1((v,))?;
    ctx.sql(&qualified)
}

// ---------------------------------------------------------------------------
// Insert
// ---------------------------------------------------------------------------

fn default_values_exception_class<'py>(ctx: &Ctx<'py>) -> PyResult<Bound<'py, PyAny>> {
    ctx.c
        .insert
        .bind(ctx.py)
        .getattr(intern!(ctx.py, "DefaultValuesException"))
}

/// `raise self.DefaultValuesException(msg)`.
fn default_values_exception<'py>(ctx: &Ctx<'py>, msg: &str) -> PyResult<PyErr> {
    let cls = default_values_exception_class(ctx)?;
    Ok(PyErr::from_value(cls.call1((msg,))?))
}

/// `ctx.state.conflict_statement(on_conflict, query)` /
/// `ctx.state.conflict_update(on_conflict, query)`: dialect hooks stored in the
/// context settings. Errors they raise (ValueError, NotImplementedError)
/// propagate unchanged.
fn conflict_hook<'py>(
    ctx: &Ctx<'py>,
    hook: &Option<Py<PyAny>>,
    on_conflict: &Bound<'py, PyAny>,
    node: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    match hook {
        Some(f) => f.bind(ctx.py).call1((on_conflict, node)),
        None => Err(PyTypeError::new_err("'NoneType' object is not callable")),
    }
}

// Insert.__sql__
pub fn insert<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    write_query_prelude(ctx, node)?;

    ctx.with(Over::default().scope(SCOPE_VALUES), |ctx| {
        let c = ctx.c;
        let on_conflict = node.getattr(intern!(py, "_on_conflict"))?;
        let has_conflict = !on_conflict.is_none();

        // stmt = self._on_conflict.get_conflict_statement(ctx, self)
        // ctx.sql(stmt or SQL('INSERT'))
        let mut stmt = None;
        if has_conflict {
            let conflict_statement = ctx.dialect.conflict_statement.clone();
            let s = conflict_hook(ctx, &conflict_statement, &on_conflict, node)?;
            if s.is_truthy()? {
                stmt = Some(s);
            }
        }
        match &stmt {
            Some(s) => ctx.sql(s)?,
            None => {
                ctx.literal("INSERT");
            }
        }
        ctx.literal(" INTO ");
        let table = node.getattr(intern!(py, "table"))?;
        ctx.sql(&table)?;
        ctx.literal(" ");

        let insert_data = node.getattr(intern!(py, "_insert"))?;
        let columns = node.getattr(intern!(py, "_columns"))?;
        let query_type = if isa(&insert_data, &c.mapping)? && !columns.is_truthy()? {
            match simple_insert(ctx, node, &insert_data, &columns) {
                Ok(()) => {}
                Err(e) => {
                    let cls = default_values_exception_class(ctx)?;
                    if e.is_instance(py, &cls) {
                        default_values(ctx, node)?;
                    } else {
                        return Err(e);
                    }
                }
            }
            INSERT_SIMPLE
        } else if isa(&insert_data, &c.select_query)? || isa(&insert_data, &c.sql)? {
            query_insert(ctx, &insert_data, &columns)?;
            INSERT_QUERY
        } else {
            let rows = insert_data.try_iter()?;
            generate_insert(ctx, node, rows, &columns)?;
            INSERT_MULTI
        };
        node.setattr(intern!(py, "_query_type"), query_type)?;

        if has_conflict {
            let conflict_update = ctx.dialect.conflict_update.clone();
            let upd = conflict_hook(ctx, &conflict_update, &on_conflict, node)?;
            if !upd.is_none() {
                ctx.literal(" ");
                ctx.sql(&upd)?;
            }
        }
        apply_returning(ctx, node)
    })
}

/// `Insert._simple_insert`.
fn simple_insert<'py>(
    ctx: &mut Ctx<'py>,
    node: &Bound<'py, PyAny>,
    insert_data: &Bound<'py, PyAny>,
    columns: &Bound<'py, PyAny>,
) -> PyResult<()> {
    if !insert_data.is_truthy()? {
        return Err(default_values_exception(ctx, "Error: no data to insert.")?);
    }
    let py = ctx.py;
    let rows = PyTuple::new(py, [insert_data])?.into_any().try_iter()?;
    generate_insert(ctx, node, rows, columns)
}

/// `Insert._query_insert`.
fn query_insert<'py>(
    ctx: &mut Ctx<'py>,
    insert_data: &Bound<'py, PyAny>,
    columns: &Bound<'py, PyAny>,
) -> PyResult<()> {
    basic::enclosed_list(ctx, columns)?;
    ctx.literal(" ");
    ctx.sql(insert_data)
}

/// `Insert._default_values`.
fn default_values<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let database = node.getattr(intern!(ctx.py, "_database"))?;
    if !database.is_truthy()? {
        ctx.literal("DEFAULT VALUES");
        return Ok(());
    }
    ctx.via_python_ctx(|pctx| {
        database.call_method1("default_values_insert", (pctx,))?;
        Ok(())
    })
}

/// One cell of a VALUES row after `_generate_insert`'s normalisation: either a
/// node rendered as-is, or a plain value wrapped as
/// `Value(val, converter=column.db_value or None, unpack=False)`.
enum Cell<'py> {
    Node(Bound<'py, PyAny>),
    Val(Bound<'py, PyAny>, Option<Bound<'py, PyAny>>),
}

/// `Insert._generate_insert(insert, ctx)`. `rows` is `iter(insert)`.
fn generate_insert<'py>(
    ctx: &mut Ctx<'py>,
    node: &Bound<'py, PyAny>,
    mut rows: Bound<'py, pyo3::types::PyIterator>,
    columns: &Bound<'py, PyAny>,
) -> PyResult<()> {
    let py = ctx.py;
    let c = ctx.c;
    let table = node.getattr(intern!(py, "table"))?;

    // Load and organize column defaults (if provided).
    let defaults = node.call_method0(intern!(py, "get_default_data"))?;

    // Column resolution: a string names an attribute of the table / model.
    let resolve = |column: Bound<'py, PyAny>| -> PyResult<Bound<'py, PyAny>> {
        if column.is_instance_of::<PyString>() {
            table.getattr(column.cast::<PyString>()?)
        } else {
            Ok(column)
        }
    };

    let mut first_row: Option<Bound<'py, PyAny>> = None;
    let cols: Vec<Bound<'py, PyAny>> = if !columns.is_truthy()? {
        let row = match rows.next() {
            Some(r) => r?,
            None => return Err(default_values_exception(ctx, "Error: no rows to insert.")?),
        };
        let cols = if !isa(&row, &c.mapping)? {
            let default_columns = node.call_method0(intern!(py, "get_default_columns"))?;
            if default_columns.is_none() {
                return Err(PyValueError::new_err("Bulk insert must specify columns."));
            }
            default_columns.try_iter()?.collect::<PyResult<Vec<_>>>()?
        } else {
            // Infer column names from the dict of data being inserted.
            let mut accum = Vec::new();
            for column in row.try_iter()? {
                accum.push(resolve(column?)?);
            }
            // Add any columns present in the default data that are not
            // accounted for by the dictionary of row data:
            //   for col in (set(defaults) - set(accum)): accum.append(col)
            let column_set = PySet::new(py, &accum)?;
            let default_set = PySet::new(py, defaults.try_iter()?.collect::<PyResult<Vec<_>>>()?)?;
            let extra = default_set.sub(&column_set)?;
            for col in extra.try_iter()? {
                accum.push(col?);
            }
            sorted_nodes(ctx, accum)?
        };
        first_row = Some(row);
        cols
    } else {
        let mut clean = Vec::new();
        let seen = PySet::empty(py)?;
        for column in columns.try_iter()? {
            let column_obj = resolve(column?)?;
            seen.add(&column_obj)?;
            clean.push(column_obj);
        }
        let default_keys = defaults.try_iter()?.collect::<PyResult<Vec<_>>>()?;
        for col in sorted_nodes(ctx, default_keys)? {
            if !seen.contains(&col)? {
                clean.push(col);
            }
        }
        clean
    };

    let fk_fields = PySet::empty(py)?;
    let nullable_columns = PySet::empty(py)?;
    let mut lookups: Vec<Vec<Bound<'py, PyAny>>> = Vec::with_capacity(cols.len());
    let mut converters: Vec<Option<Bound<'py, PyAny>>> = Vec::with_capacity(cols.len());
    for column in &cols {
        let mut keys = vec![column.clone(), column.getattr(intern!(py, "name"))?];
        let mut converter = None;
        if isa(column, &c.field)? {
            let name = column.getattr(intern!(py, "name"))?;
            let column_name = column.getattr(intern!(py, "column_name"))?;
            if !name.eq(&column_name)? {
                keys.push(column_name);
            }
            if column.getattr(intern!(py, "null"))?.is_truthy()? {
                nullable_columns.add(column)?;
            }
            if isa(column, &c.foreign_key_field)? {
                fk_fields.add(column)?;
            }
            converter = Some(column.getattr(intern!(py, "db_value"))?);
        }
        lookups.push(keys);
        converters.push(converter);
    }

    basic::enclosed_items(ctx, &cols)?;
    ctx.literal(" VALUES ");

    let mut all_values: Vec<Vec<Cell<'py>>> = Vec::new();
    loop {
        let row = match first_row.take() {
            Some(r) => r,
            None => match rows.next() {
                Some(r) => r?,
                None => break,
            },
        };
        let is_dict = isa(&row, &c.mapping)?;
        let mut values = Vec::with_capacity(cols.len());
        for (i, column) in cols.iter().enumerate() {
            // try: val = row[...] except (KeyError, IndexError): <defaults>
            let mut val: Option<Bound<'py, PyAny>> = None;
            if is_dict {
                // Try each possible key (column, field name, column name);
                // a KeyError moves on to the next one.
                for lookup in &lookups[i] {
                    match row.get_item(lookup) {
                        Ok(v) => {
                            val = Some(v);
                            break;
                        }
                        Err(e) if e.is_instance_of::<PyKeyError>(py) => continue,
                        Err(e) if e.is_instance_of::<PyIndexError>(py) => break,
                        Err(e) => return Err(e),
                    }
                }
            } else {
                match row.get_item(i) {
                    Ok(v) => val = Some(v),
                    Err(e)
                        if e.is_instance_of::<PyKeyError>(py)
                            || e.is_instance_of::<PyIndexError>(py) => {}
                    Err(e) => return Err(e),
                }
            }
            let val = match val {
                Some(v) => v,
                None => {
                    if defaults.contains(column)? {
                        let mut v = defaults.get_item(column)?;
                        if v.is_callable() {
                            v = v.call0()?;
                        }
                        v
                    } else if nullable_columns.contains(column)? {
                        py.None().into_bound(py)
                    } else {
                        let name = column.getattr(intern!(py, "name"))?;
                        return Err(PyValueError::new_err(format!(
                            "Missing value for {}.",
                            name.str()?.to_str()?
                        )));
                    }
                }
            };

            if !isa(&val, &c.node)? || (isa(&val, &c.model)? && fk_fields.contains(column)?) {
                values.push(Cell::Val(val, converters[i].clone()));
            } else {
                values.push(Cell::Node(val));
            }
        }
        all_values.push(values);
    }

    if all_values.is_empty() {
        return Err(default_values_exception(ctx, "Error: no data to insert.")?);
    }

    // with ctx.scope_values(subquery=True):
    //     ctx.sql(CommaNodeList([EnclosedNodeList(values), ...]))
    ctx.with(Over::default().scope(SCOPE_VALUES).subquery(true), |ctx| {
        ctx.with(Over::default().parens(false), |ctx| {
            let n = all_values.len();
            for (i, row) in all_values.iter().enumerate() {
                values_row(ctx, row)?;
                if i + 1 < n {
                    ctx.literal(", ");
                }
            }
            Ok(())
        })
    })
}

/// `EnclosedNodeList(values)` for one VALUES row, including its single
/// non-flat Expression special case (rendered flat to avoid `((...))`).
fn values_row<'py>(ctx: &mut Ctx<'py>, row: &[Cell<'py>]) -> PyResult<()> {
    if row.is_empty() {
        ctx.literal("()");
        return Ok(());
    }
    if let [Cell::Node(n)] = row {
        if isa(n, &ctx.c.expression)? && !n.getattr(intern!(ctx.py, "flat"))?.is_truthy()? {
            let clone = n.call_method0(intern!(ctx.py, "clone"))?;
            clone.setattr(intern!(ctx.py, "flat"), true)?;
            return ctx.with(Over::default().parens(true), |ctx| ctx.sql(&clone));
        }
    }
    ctx.with(Over::default().parens(true), |ctx| {
        let n = row.len();
        for (i, cell) in row.iter().enumerate() {
            match cell {
                Cell::Node(node) => ctx.sql(node)?,
                Cell::Val(v, Some(conv)) => ctx.value(v, Converter::Fn(conv), true)?,
                Cell::Val(v, None) => ctx.value(v, Converter::Default, true)?,
            }
            if i + 1 < n {
                ctx.literal(", ");
            }
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

// Delete.__sql__
pub fn delete<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    write_query_prelude(ctx, node)?;
    ctx.with(Over::default().scope(SCOPE_VALUES).subquery(true), |ctx| {
        ctx.literal("DELETE FROM ");
        let table = node.getattr(intern!(py, "table"))?;
        ctx.sql(&table)?;
        let where_ = node.getattr(intern!(py, "_where"))?;
        if !where_.is_none() {
            ctx.with(Over::default().scope(SCOPE_NORMAL), |ctx| {
                ctx.literal(" WHERE ");
                ctx.sql(&where_)
            })?;
        }
        query::apply_ordering(ctx, node)?;
        apply_returning(ctx, node)
    })
}
