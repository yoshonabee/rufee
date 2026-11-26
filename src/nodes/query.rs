//! SELECT-side queries: Select, CompoundSelectQuery, RawQuery, ForUpdate.

use pyo3::intern;
use pyo3::prelude::*;

use crate::classes::{
    isa, CSQ_PARENTHESES_ALWAYS, CSQ_PARENTHESES_GROUPED, CSQ_PARENTHESES_NEVER,
    CSQ_PARENTHESES_UNNESTED, SCOPE_COLUMN, SCOPE_CTE, SCOPE_NORMAL, SCOPE_SOURCE, SCOPE_VALUES,
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
    let mut parens = is_subquery || ctx.scope() == SCOPE_SOURCE;
    if ctx.state.flags.in_function && ctx.state.flags.function_arg_count == Some(1) {
        parens = false;
    }
    let over = Over::default()
        .scope(SCOPE_NORMAL)
        .converter(None)
        .in_function(false)
        .parens(parens)
        .bare_subquery(false)
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
        ctx.with(Over::default().scope(SCOPE_SOURCE), |ctx| {
            sql_selection(ctx, node, is_subquery)
        })?;

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

        // peewee 4.0.3+: `_for_update` holds a ready-made `ForUpdate` node
        // (or None); the `of` / `nowait` / `skip_locked` parts live on it.
        let for_update = node.getattr(intern!(py, "_for_update"))?;
        if !for_update.is_none() {
            if !ctx.dialect.for_update {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "FOR UPDATE specified but not supported by database.",
                ));
            }
            ctx.literal(" ");
            ctx.sql(&for_update)?;
        }
        Ok(())
    })?;

    if ctx.state.flags.in_function
        || (ctx.state.flags.in_expr && node.getattr(intern!(py, "_alias"))?.is_none())
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
    // SelectBase._subquery_parens: no parens of our own when a function's
    // parentheses already enclose us.
    let parens =
        !ctx.state.flags.bare_subquery && (ctx.state.flags.subquery || ctx.scope() == SCOPE_SOURCE);
    let lhs = node.getattr(intern!(py, "lhs"))?;
    let rhs = node.getattr(intern!(py, "rhs"))?;
    let op: String = node.getattr(intern!(py, "op"))?.extract()?;
    ctx.with(Over::default().parens(parens).bare_subquery(false), |ctx| {
        // Correlated rhs refs must resolve to the enclosing aliases: the rhs
        // renders in a fresh alias scope seeded with the aliases known before
        // the lhs was rendered, so its own sources still get fresh aliases.
        let outer = ctx.aliases.snapshot(py)?;
        member_sql(ctx, &op, &lhs, true)?;
        ctx.literal(" ");
        ctx.literal(&op);
        ctx.literal(" ");
        ctx.with_alias_scope(|ctx| {
            ctx.aliases.seed(py, &outer)?;
            member_sql(ctx, &op, &rhs, false)
        })?;
        ctx.with(Over::default().scope(SCOPE_VALUES), |ctx| {
            apply_ordering(ctx, node)
        })
    })?;
    // SelectBase._apply_subquery_alias.
    if ctx.state.flags.in_function
        || (ctx.state.flags.in_expr && node.getattr(intern!(py, "_alias"))?.is_none())
    {
        return Ok(());
    }
    apply_alias(ctx, node)
}

/// How a compound member is written (`CSQ_FLAT` / `CSQ_PARENS` / `CSQ_WRAP`).
#[derive(PartialEq, Eq, Clone, Copy)]
enum MemberStyle {
    Flat,
    Parens,
    /// `SELECT * FROM (...)`, for dialects without member parentheses.
    Wrap,
}

/// `CompoundSelectQuery._member_sql`.
fn member_sql<'py>(
    ctx: &mut Ctx<'py>,
    op: &str,
    subq: &Bound<'py, PyAny>,
    lhs: bool,
) -> PyResult<()> {
    let style = member_style(ctx, op, subq, lhs)?;
    if style == MemberStyle::Wrap {
        ctx.literal("SELECT * FROM ");
    }
    ctx.with(
        Over::default()
            .scope(SCOPE_NORMAL)
            .parens(style != MemberStyle::Flat)
            .subquery(false),
        |ctx| ctx.sql(subq),
    )
}

/// `CompoundSelectQuery._member_style`.
fn member_style<'py>(
    ctx: &Ctx<'py>,
    op: &str,
    subq: &Bound<'py, PyAny>,
    lhs: bool,
) -> PyResult<MemberStyle> {
    let is_compound = isa(subq, &ctx.c.compound_select_query)?;
    let in_expr = ctx.state.flags.in_expr || ctx.state.flags.in_function;
    match ctx.dialect.compound_select_parentheses {
        None | Some(CSQ_PARENTHESES_NEVER) => {
            // Parens are not allowed (sqlite): wrap a member that needs
            // grouping as a subquery instead.
            if is_compound {
                if needs_group(ctx, op, subq, lhs)? {
                    return Ok(MemberStyle::Wrap);
                }
            } else if self_wraps(subq)? {
                return Ok(MemberStyle::Wrap);
            }
        }
        Some(CSQ_PARENTHESES_ALWAYS) => return Ok(MemberStyle::Parens),
        Some(CSQ_PARENTHESES_UNNESTED) => {
            if in_expr {
                return Ok(MemberStyle::Flat);
            }
            if !is_compound {
                return Ok(MemberStyle::Parens);
            }
        }
        Some(CSQ_PARENTHESES_GROUPED) => {
            if is_compound {
                if in_expr && (op == "UNION" || op == "UNION ALL") && union_only(ctx, subq)? {
                    // IN / EXISTS only see the distinct set; stay flat.
                    return Ok(MemberStyle::Flat);
                }
                if needs_group(ctx, op, subq, lhs)? {
                    return Ok(MemberStyle::Parens);
                }
            } else if !in_expr {
                return Ok(MemberStyle::Parens);
            }
        }
        Some(_) => {}
    }
    Ok(MemberStyle::Flat)
}

fn compound_op(subq: &Bound<'_, PyAny>) -> PyResult<String> {
    subq.getattr(intern!(subq.py(), "op"))?.extract()
}

/// `CompoundSelectQuery._self_wraps`: an inner ORDER BY / LIMIT / OFFSET.
fn self_wraps(subq: &Bound<'_, PyAny>) -> PyResult<bool> {
    let py = subq.py();
    Ok(subq.getattr(intern!(py, "_order_by"))?.is_truthy()?
        || !subq.getattr(intern!(py, "_limit"))?.is_none()
        || !subq.getattr(intern!(py, "_offset"))?.is_none())
}

/// `CompoundSelectQuery._needs_group`: whether writing the member flat
/// would change the result.
fn needs_group<'py>(
    ctx: &Ctx<'py>,
    op: &str,
    subq: &Bound<'py, PyAny>,
    lhs: bool,
) -> PyResult<bool> {
    if self_wraps(subq)? {
        return Ok(true);
    }
    if lhs {
        // A flat lhs folds left to right before our op applies.
        return Ok(op == "INTERSECT" && compound_op(subq)? != op);
    }
    if op == "EXCEPT" {
        return Ok(true);
    }
    // A flat rhs joins the outer chain through its left spine, so every op
    // along it must match ours.
    let csq = &ctx.c.compound_select_query;
    let mut node = subq.clone();
    while isa(&node, csq)? {
        if compound_op(&node)? != op {
            return Ok(true);
        }
        node = node.getattr(intern!(ctx.py, "lhs"))?;
        if isa(&node, csq)? && self_wraps(&node)? {
            break;
        }
    }
    Ok(false)
}

/// `CompoundSelectQuery._union_only`: the whole subtree is UNION / UNION ALL
/// with no inner ordering.
fn union_only<'py>(ctx: &Ctx<'py>, subq: &Bound<'py, PyAny>) -> PyResult<bool> {
    if !isa(subq, &ctx.c.compound_select_query)? {
        return Ok(true);
    }
    let op = compound_op(subq)?;
    if (op != "UNION" && op != "UNION ALL") || self_wraps(subq)? {
        return Ok(false);
    }
    Ok(union_only(ctx, &subq.getattr(intern!(ctx.py, "lhs"))?)?
        && union_only(ctx, &subq.getattr(intern!(ctx.py, "rhs"))?)?)
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
    } else if node.getattr(intern!(py, "_skip_locked"))?.is_truthy()? {
        ctx.literal(" SKIP LOCKED");
    }
    Ok(())
}
