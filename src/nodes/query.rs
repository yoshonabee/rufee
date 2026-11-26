//! SELECT-side queries: Select, CompoundSelectQuery, RawQuery, ForUpdate.

use pyo3::intern;
use pyo3::prelude::*;

use crate::classes::{
    isa, CSQ_PARENTHESES_ALWAYS, CSQ_PARENTHESES_NEVER, CSQ_PARENTHESES_UNNESTED, SCOPE_COLUMN,
    SCOPE_CTE, SCOPE_NORMAL, SCOPE_SOURCE, SCOPE_VALUES,
};
use crate::ctx::{Converter, Ctx, Over};
use crate::nodes::basic;

/// `Query.__sql__`: the WITH clause.
pub fn query_ctes<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let cte_list = node.getattr(intern!(py, "_cte_list"))?;
    if !cte_list.is_truthy()? {
        return Ok(());
    }
    let ctes: Vec<Bound<'py, PyAny>> = cte_list.try_iter()?.collect::<PyResult<_>>()?;
    let mut recursive = false;
    for c in &ctes {
        if c.getattr(intern!(py, "_recursive"))?.is_truthy()? {
            recursive = true;
            break;
        }
    }
    ctx.with(Over::default().scope(SCOPE_CTE).subquery(false), |ctx| {
        ctx.literal(if recursive {
            "WITH RECURSIVE "
        } else {
            "WITH "
        });
        basic::render_list(ctx, &ctes, ", ", false)?;
        ctx.literal(" ");
        Ok(())
    })
}

/// `Query._apply_ordering`.
pub fn apply_ordering<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let order_by = node.getattr(intern!(py, "_order_by"))?;
    if order_by.is_truthy()? {
        ctx.literal(" ORDER BY ");
        basic::comma_list(ctx, &order_by)?;
    }
    let limit = node.getattr(intern!(py, "_limit"))?;
    let offset = node.getattr(intern!(py, "_offset"))?;
    let limit_max = ctx.dialect.limit_max.clone();
    if !limit.is_none() || (!offset.is_none() && limit_max.is_some()) {
        let lim = if limit.is_none() {
            limit_max.unwrap().bind(py).clone()
        } else {
            limit
        };
        ctx.literal(" LIMIT ");
        ctx.sql(&lim)?;
    }
    if !offset.is_none() {
        ctx.literal(" OFFSET ");
        ctx.sql(&offset)?;
    }
    Ok(())
}

/// `Source.apply_alias` for a query used as a source.
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

fn apply_column<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let explicit = node.getattr(intern!(py, "_alias"))?;
    if explicit.is_truthy()? {
        ctx.aliases.set(node, &explicit.extract::<String>()?)?;
    }
    let a = ctx.aliases.get(node)?;
    ctx.entity(&[&a])
}

// Select.__sql__ (also ModelSelect via __sql_selection__)
pub fn select<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    if ctx.scope() == SCOPE_COLUMN {
        return apply_column(ctx, node);
    }
    if node.getattr(intern!(py, "_lateral"))?.is_truthy()? && ctx.scope() == SCOPE_SOURCE {
        ctx.literal("LATERAL ");
    }
    let is_subquery = ctx.state.flags.subquery;
    // A real FROM/JOIN source. The SELECT list is SCOPE_SOURCE too, but it is
    // flagged in_projection.
    let is_source = ctx.scope() == SCOPE_SOURCE && !ctx.state.flags.in_projection;
    let over = Over::default()
        .scope(SCOPE_NORMAL)
        .converter(None)
        .in_expr(false)
        .in_function(false)
        .in_projection(false)
        .bare_subquery(false)
        .parens(subquery_parens(ctx))
        .subquery(true);

    ctx.with(over, |ctx| {
        query_ctes(ctx, node)?;
        ctx.literal("SELECT ");
        let simple_distinct = node.getattr(intern!(py, "_simple_distinct"))?.is_truthy()?;
        let distinct = node.getattr(intern!(py, "_distinct"))?;
        if simple_distinct || !distinct.is_none() {
            ctx.literal("DISTINCT ");
            if distinct.is_truthy()? {
                ctx.literal("ON ");
                basic::enclosed_list(ctx, &distinct)?;
                ctx.literal(" ");
            }
        }
        ctx.with(
            Over::default().scope(SCOPE_SOURCE).in_projection(true),
            |ctx| sql_selection(ctx, node, is_subquery && !is_source),
        )?;

        let from_list = node.getattr(intern!(py, "_from_list"))?;
        if from_list.is_truthy()? {
            ctx.with(Over::default().scope(SCOPE_SOURCE).parens(false), |ctx| {
                ctx.literal(" FROM ");
                basic::comma_list(ctx, &from_list)
            })?;
        }
        let where_ = node.getattr(intern!(py, "_where"))?;
        if !where_.is_none() {
            ctx.literal(" WHERE ");
            ctx.sql(&where_)?;
        }
        let group_by = node.getattr(intern!(py, "_group_by"))?;
        if group_by.is_truthy()? {
            ctx.literal(" GROUP BY ");
            basic::comma_list(ctx, &group_by)?;
        }
        let having = node.getattr(intern!(py, "_having"))?;
        if !having.is_none() {
            ctx.literal(" HAVING ");
            ctx.sql(&having)?;
        }
        let windows = node.getattr(intern!(py, "_windows"))?;
        if !windows.is_none() {
            ctx.literal(" WINDOW ");
            basic::comma_list(ctx, &windows)?;
        }
        apply_ordering(ctx, node)?;

        let for_update = node.getattr(intern!(py, "_for_update"))?;
        if for_update.is_truthy()? {
            if !ctx.dialect.for_update {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "FOR UPDATE specified but not supported by database.",
                ));
            }
            ctx.literal(" ");
            let fu = ctx.c.for_update.bind(ctx.py).call1((
                &for_update,
                node.getattr(intern!(py, "_for_update_of"))?,
                node.getattr(intern!(py, "_for_update_nowait"))?,
            ))?;
            ctx.sql(&fu)?;
        }
        Ok(())
    })?;

    apply_subquery_alias(ctx, node)
}

/// `SelectBase._subquery_parens`.
fn subquery_parens(ctx: &Ctx<'_>) -> bool {
    // bare_subquery is set by Function for its subquery_functions.
    if ctx.state.flags.bare_subquery {
        return false;
    }
    ctx.state.flags.subquery || ctx.scope() == SCOPE_SOURCE
}

/// `SelectBase._apply_subquery_alias`: no alias inside a function, or when
/// unaliased in an expression or a SELECT list.
fn apply_subquery_alias<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let flags = &ctx.state.flags;
    if flags.in_function
        || ((flags.in_expr || flags.in_projection)
            && node.getattr(intern!(ctx.py, "_alias"))?.is_none())
    {
        return Ok(());
    }
    apply_alias(ctx, node)
}

/// `Select.__sql_selection__` / `ModelSelect.__sql_selection__`.
fn sql_selection<'py>(
    ctx: &mut Ctx<'py>,
    node: &Bound<'py, PyAny>,
    is_subquery: bool,
) -> PyResult<()> {
    let py = ctx.py;
    let returning = node.getattr(intern!(py, "_returning"))?;
    if isa(node, &ctx.c.model_select)? {
        let is_default = node.getattr(intern!(py, "_is_default"))?.is_truthy()?;
        if is_default && is_subquery && returning.len()? > 1 {
            let pk = node
                .getattr(intern!(py, "model"))?
                .getattr(intern!(py, "_meta"))?
                .getattr(intern!(py, "primary_key"))?;
            // `primary_key is not False`
            let is_false = pk.is_instance_of::<pyo3::types::PyBool>() && !pk.is_truthy()?;
            if !is_false {
                return ctx.sql(&pk);
            }
        }
    }
    basic::comma_list(ctx, &returning)
}

// NoopModelSelect.__sql__ → database.get_noop_select(ctx)
pub fn noop_model_select<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let database = node
        .getattr(intern!(py, "model"))?
        .getattr(intern!(py, "_meta"))?
        .getattr(intern!(py, "database"))?;
    ctx.via_python_ctx(|pctx| {
        database.call_method1("get_noop_select", (pctx,))?;
        Ok(())
    })
}

// CompoundSelectQuery.__sql__
pub fn compound_select<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    if ctx.scope() == SCOPE_COLUMN {
        return apply_column(ctx, node);
    }
    query_ctes(ctx, node)?;
    let outer_parens = ctx.state.flags.subquery || ctx.scope() == SCOPE_SOURCE;
    let lhs = node.getattr(intern!(py, "lhs"))?;
    let rhs = node.getattr(intern!(py, "rhs"))?;
    let op: String = node.getattr(intern!(py, "op"))?.extract()?;
    ctx.with(Over::default().parens(outer_parens), |ctx| {
        let lhs_parens = wrap_parens(ctx, &lhs)?;
        ctx.with(
            Over::default()
                .scope(SCOPE_NORMAL)
                .parens(lhs_parens)
                .subquery(false),
            |ctx| ctx.sql(&lhs),
        )?;
        ctx.literal(&format!(" {} ", op));
        ctx.with_alias_scope(|ctx| {
            let rhs_parens = wrap_parens(ctx, &rhs)?;
            ctx.with(
                Over::default()
                    .scope(SCOPE_NORMAL)
                    .parens(rhs_parens)
                    .subquery(false),
                |ctx| ctx.sql(&rhs),
            )
        })?;
        ctx.with(Over::default().scope(SCOPE_VALUES), |ctx| {
            apply_ordering(ctx, node)
        })
    })?;
    apply_alias(ctx, node)
}

fn wrap_parens<'py>(ctx: &Ctx<'py>, subq: &Bound<'py, PyAny>) -> PyResult<bool> {
    match ctx.dialect.compound_select_parentheses {
        None | Some(CSQ_PARENTHESES_NEVER) => Ok(false),
        Some(CSQ_PARENTHESES_ALWAYS) => Ok(true),
        Some(CSQ_PARENTHESES_UNNESTED) => {
            if ctx.state.flags.in_expr || ctx.state.flags.in_function {
                return Ok(false);
            }
            Ok(!isa(subq, &ctx.c.compound_select_query)?)
        }
        Some(_) => Ok(false),
    }
}

// RawQuery.__sql__
pub fn raw_query<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let sql: String = node.getattr(intern!(py, "_sql"))?.extract()?;
    ctx.literal(&sql);
    let params = node.getattr(intern!(py, "_params"))?;
    if params.is_truthy()? {
        for p in params.try_iter()? {
            ctx.value(&p?, Converter::Default, false)?;
        }
    }
    Ok(())
}

// ForUpdate.__sql__
pub fn for_update<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let expr: String = node.getattr(intern!(py, "_expr"))?.extract()?;
    ctx.literal(&expr);
    let of = node.getattr(intern!(py, "_of"))?;
    if !of.is_none() {
        ctx.literal(" OF ");
        basic::comma_list(ctx, &of)?;
    }
    if node.getattr(intern!(py, "_nowait"))?.is_truthy()? {
        ctx.literal(" NOWAIT");
    }
    Ok(())
}
