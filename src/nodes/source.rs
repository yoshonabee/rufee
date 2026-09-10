//! Tables, joins, models, fields — the "source" side of a query.

use pyo3::intern;
use pyo3::prelude::*;

use crate::classes::{SCOPE_COLUMN, SCOPE_NORMAL, SCOPE_SOURCE, SCOPE_VALUES};
use crate::ctx::{Converter, Ctx, Over};
use crate::nodes::basic;

fn path_of(node: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    let py = node.py();
    node.getattr(intern!(py, "_path"))?.extract()
}

/// `Source.apply_alias(ctx)`: in SCOPE_SOURCE write ` AS "alias"`.
fn apply_alias<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    if ctx.scope() == SCOPE_SOURCE {
        let explicit = node.getattr(intern!(py, "_alias"))?;
        if explicit.is_truthy()? {
            ctx.aliases.set(node, &explicit.extract::<String>()?)?;
        }
        let a = ctx.aliases.get(node)?;
        ctx.literal(" AS ");
        ctx.entity(&[&a])?;
    }
    Ok(())
}

/// `Source.apply_column(ctx)`: refer to the source by alias.
fn apply_column<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let explicit = node.getattr(intern!(py, "_alias"))?;
    if explicit.is_truthy()? {
        ctx.aliases.set(node, &explicit.extract::<String>()?)?;
    }
    let a = ctx.aliases.get(node)?;
    ctx.entity(&[&a])
}

// Table.__sql__
pub fn table<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let path = path_of(node)?;
    let parts: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
    if ctx.scope() == SCOPE_VALUES {
        return entity_path(ctx, &parts);
    }
    let explicit = node.getattr(intern!(py, "_alias"))?;
    if explicit.is_truthy()? {
        ctx.aliases.set(node, &explicit.extract::<String>()?)?;
    }
    if ctx.scope() == SCOPE_SOURCE {
        entity_path(ctx, &parts)?;
        return apply_alias(ctx, node);
    }
    apply_column(ctx, node)
}

/// `Entity(*path)`: Entity.__init__ escapes `"` as `""` and drops empty parts.
fn entity_path(ctx: &mut Ctx<'_>, parts: &[&str]) -> PyResult<()> {
    ctx.entity(parts)
}

// Join.__sql__
pub fn join<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let lhs = node.getattr(intern!(py, "lhs"))?;
    let rhs = node.getattr(intern!(py, "rhs"))?;
    let join_type: String = node.getattr(intern!(py, "join_type"))?.extract()?;
    ctx.sql(&lhs)?;
    ctx.literal(&format!(" {} ", join_type));
    ctx.sql(&rhs)?;
    let on = node.getattr(intern!(py, "_on"))?;
    if !on.is_none() {
        ctx.literal(" ON ");
        ctx.sql(&on)?;
    }
    Ok(())
}

// ValuesList.__sql__
pub fn values_list<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let explicit = node.getattr(intern!(py, "_alias"))?;
    if explicit.is_truthy()? {
        ctx.aliases.set(node, &explicit.extract::<String>()?)?;
    }
    let scope = ctx.scope();
    if scope == SCOPE_SOURCE || scope == SCOPE_NORMAL {
        let rows: Vec<Bound<'py, PyAny>> = node
            .getattr(intern!(py, "_values"))?
            .try_iter()?
            .collect::<PyResult<_>>()?;
        let parens = !ctx.state.parentheses;
        ctx.with(Over::default().parens(parens), |ctx| {
            ctx.literal("VALUES ");
            // CommaNodeList([EnclosedNodeList(row) for row in values])
            ctx.with(Over::default().parens(false), |ctx| {
                let n = rows.len();
                for (i, row) in rows.iter().enumerate() {
                    let items: Vec<Bound<'py, PyAny>> = row.try_iter()?.collect::<PyResult<_>>()?;
                    basic::enclosed_items(ctx, &items)?;
                    if i + 1 < n {
                        ctx.literal(", ");
                    }
                }
                Ok(())
            })
        })?;
        if scope == SCOPE_SOURCE {
            let a = ctx.aliases.get(node)?;
            ctx.literal(" AS ");
            ctx.entity(&[&a])?;
            let columns = node.getattr(intern!(py, "_columns"))?;
            if columns.is_truthy()? {
                let names: Vec<String> = columns.extract()?;
                ctx.with(Over::default().parens(true), |ctx| {
                    let n = names.len();
                    for (i, name) in names.iter().enumerate() {
                        ctx.entity(&[name])?;
                        if i + 1 < n {
                            ctx.literal(", ");
                        }
                    }
                    Ok(())
                })?;
            }
        }
        return Ok(());
    }
    let a = ctx.aliases.get(node)?;
    ctx.entity(&[&a])
}

// CTE.__sql__
pub fn cte<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let alias: String = node.getattr(intern!(py, "_alias"))?.extract()?;
    if ctx.scope() != crate::classes::SCOPE_CTE {
        return ctx.entity(&[&alias]);
    }
    ctx.with_alias_scope(|ctx| {
        ctx.aliases.set(node, &alias)?;
        ctx.entity(&[&alias])?;
        let columns = node.getattr(intern!(py, "_columns"))?;
        if columns.is_truthy()? {
            ctx.literal(" ");
            basic::enclosed_list(ctx, &columns)?;
        }
        ctx.literal(" AS ");
        let materialized = node.getattr(intern!(py, "_materialized"))?;
        if materialized.is_truthy()? {
            ctx.literal("MATERIALIZED ");
        } else if !materialized.is_none() {
            // `elif self._materialized is False`
            ctx.literal("NOT MATERIALIZED ");
        }
        let query = node.getattr(intern!(py, "_query"))?;
        ctx.with(Over::default().scope(SCOPE_NORMAL).parens(true), |ctx| {
            ctx.sql(&query)
        })
    })
}

// Field.__sql__ → ctx.sql(Column(model._meta.table, column_name))
pub fn field<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let column_name: String = node.getattr(intern!(py, "column_name"))?.extract()?;
    if ctx.scope() == SCOPE_VALUES {
        return ctx.entity(&[&column_name]);
    }
    let table = node
        .getattr(intern!(py, "model"))?
        .getattr(intern!(py, "_meta"))?
        .getattr(intern!(py, "table"))?;
    ctx.with(Over::default().scope(SCOPE_COLUMN), |ctx| {
        ctx.sql(&table)?;
        ctx.literal(".");
        ctx.entity(&[&column_name])
    })
}

// FieldAlias.__sql__ → ctx.sql(Column(self.source, self.field.column_name))
pub fn field_alias<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let column_name: String = node
        .getattr(intern!(py, "field"))?
        .getattr(intern!(py, "column_name"))?
        .extract()?;
    if ctx.scope() == SCOPE_VALUES {
        return ctx.entity(&[&column_name]);
    }
    let source = node.getattr(intern!(py, "source"))?;
    ctx.with(Over::default().scope(SCOPE_COLUMN), |ctx| {
        ctx.sql(&source)?;
        ctx.literal(".");
        ctx.entity(&[&column_name])
    })
}

// CompositeKey.__sql__
pub fn composite_key<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let parens = ctx.scope() != SCOPE_SOURCE;
    let fields = node
        .getattr(intern!(py, "model"))?
        .getattr(intern!(py, "_meta"))?
        .getattr(intern!(py, "fields"))?;
    let names: Vec<String> = node.getattr(intern!(py, "field_names"))?.extract()?;
    let mut items = Vec::with_capacity(names.len());
    for n in &names {
        items.push(fields.get_item(n)?);
    }
    basic::render_list(ctx, &items, ", ", parens)
}

// ModelAlias.__sql__
pub fn model_alias<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let model = node.getattr(intern!(py, "model"))?;
    if ctx.scope() == SCOPE_VALUES {
        return ctx.sql(&model);
    }
    let alias = node.getattr(intern!(py, "alias"))?;
    if alias.is_truthy()? {
        ctx.aliases.set(node, &alias.extract::<String>()?)?;
    }
    if ctx.scope() == SCOPE_SOURCE {
        let entity = model
            .getattr(intern!(py, "_meta"))?
            .getattr(intern!(py, "entity"))?;
        ctx.sql(&entity)?;
        ctx.literal(" AS ");
        let a = ctx.aliases.get(node)?;
        return ctx.entity(&[&a]);
    }
    let a = ctx.aliases.get(node)?;
    ctx.entity(&[&a])
}

// Model.__sql__ (an *instance* used as a value)
pub fn model_instance<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    if ctx.state.settings.is_fk_expr {
        if let Some(conv) = ctx.state.settings.converter.clone() {
            // try: ctx.sql(Value(self, converter=ctx.state.converter))
            // except (TypeError, ValueError): pass
            let conv = conv.bind(py);
            match ctx.value(node, Converter::Fn(conv), true) {
                Ok(()) => return Ok(()),
                Err(e)
                    if e.is_instance_of::<pyo3::exceptions::PyTypeError>(py)
                        || e.is_instance_of::<pyo3::exceptions::PyValueError>(py) => {}
                Err(e) => return Err(e),
            }
        }
    }
    let pk = node
        .getattr(intern!(py, "_meta"))?
        .getattr(intern!(py, "primary_key"))?;
    let pk_name: String = pk.getattr(intern!(py, "name"))?.extract()?;
    let v = node.getattr(pk_name.as_str())?;
    let db_value = pk.getattr(intern!(py, "db_value"))?;
    ctx.value(&v, Converter::Fn(&db_value), true)
}
