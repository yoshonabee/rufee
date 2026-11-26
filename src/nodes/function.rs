//! Function, Case, Window, WindowAlias, _InFunction.

use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::PyString;

use crate::classes::{isa, SCOPE_SOURCE};
use crate::ctx::{Converter, Ctx, Over};
use crate::nodes::basic;

// Function.__sql__
pub fn function<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let name: String = node.getattr(intern!(py, "name"))?.extract()?;
    ctx.literal(&name);

    let args: Vec<Bound<'py, PyAny>> = node
        .getattr(intern!(py, "arguments"))?
        .try_iter()?
        .collect::<PyResult<_>>()?;
    if args.is_empty() {
        ctx.literal("()");
    } else {
        let order_by = node.getattr(intern!(py, "_order_by"))?;
        let order_by = if order_by.is_truthy()? {
            Some(order_by)
        } else {
            None
        };
        let n = args.len();
        // peewee 4: a lone SelectBase argument of a subquery function (EXISTS,
        // ANY, ...) is rendered bare, the function's parentheses enclose it.
        // With an ORDER BY the last argument is wrapped in a NodeList.
        let bare = n == 1
            && order_by.is_none()
            && isa(&args[0], &ctx.c.select_base)?
            && node
                .getattr(intern!(py, "subquery_functions"))?
                .contains(name.to_lowercase())?;
        ctx.with(
            Over::default()
                .in_function(true)
                .function_arg_count(Some(n))
                .bare_subquery(bare),
            |ctx| function_args(ctx, &args, order_by.as_ref()),
        )?;
    }

    let filter = node.getattr(intern!(py, "_filter"))?;
    if filter.is_truthy()? {
        ctx.literal(" FILTER (WHERE ");
        ctx.sql(&filter)?;
        ctx.literal(")");
    }
    Ok(())
}

/// The argument list of a function call:
///
/// ```text
/// EnclosedNodeList([arg if isinstance(arg, Node) else Value(arg, False)
///                   for arg in args])
/// ```
///
/// where, for an ordered aggregate, the last argument has been replaced by
/// `NodeList((args[-1], SQL('ORDER BY'), CommaNodeList(order_by)))`.
///
/// `EnclosedNodeList` renders a single non-flat `Expression` flat (its
/// "avoid double parentheses" hack); the ordered-aggregate wrapper is a
/// `NodeList`, so the hack never applies when `order_by` is set.
fn function_args<'py>(
    ctx: &mut Ctx<'py>,
    args: &[Bound<'py, PyAny>],
    order_by: Option<&Bound<'py, PyAny>>,
) -> PyResult<()> {
    let n = args.len();
    if order_by.is_none() && n == 1 && isa(&args[0], &ctx.c.expression)? {
        // An Expression is a Node, so it is passed through unwrapped and
        // EnclosedNodeList's hack may apply to it.
        return basic::enclosed_items(ctx, args);
    }
    ctx.with(Over::default().parens(true), |ctx| {
        for (i, arg) in args.iter().enumerate() {
            if i + 1 < n {
                function_arg(ctx, arg)?;
                ctx.literal(", ");
            } else if let Some(order_by) = order_by {
                // NodeList((arg, SQL('ORDER BY'), CommaNodeList(order_by))):
                // the raw argument goes through `ctx.sql()` unwrapped, so a
                // plain value keeps the state's converter here.
                ctx.with(Over::default().parens(false), |ctx| {
                    ctx.sql(arg)?;
                    ctx.literal(" ORDER BY ");
                    basic::comma_list(ctx, order_by)
                })?;
            } else {
                function_arg(ctx, arg)?;
            }
        }
        Ok(())
    })
}

/// One function argument: `arg if isinstance(arg, Node) else Value(arg, False)`.
fn function_arg<'py>(ctx: &mut Ctx<'py>, arg: &Bound<'py, PyAny>) -> PyResult<()> {
    if isa(arg, &ctx.c.node)? {
        ctx.sql(arg)
    } else {
        basic::value_of(ctx, arg, Converter::NoConvert, true)
    }
}

// Case.__sql__
pub fn case<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let predicate = node.getattr(intern!(py, "predicate"))?;
    let tuples: Vec<Bound<'py, PyAny>> = node
        .getattr(intern!(py, "expression_tuples"))?
        .try_iter()?
        .collect::<PyResult<_>>()?;
    let default = node.getattr(intern!(py, "default"))?;
    ctx.with(Over::default().in_function(false), |ctx| {
        // NodeList(clauses) with glue ' ' and no parentheses.
        ctx.literal("CASE");
        if !predicate.is_none() {
            ctx.literal(" ");
            ctx.sql(&predicate)?;
        }
        for pair in &tuples {
            let (expr, value) = unpack_pair(pair)?;
            ctx.literal(" WHEN ");
            ctx.sql(&expr)?;
            ctx.literal(" THEN ");
            // _InFunction(value)
            ctx.with(Over::default().in_function(true), |ctx| ctx.sql(&value))?;
        }
        if !default.is_none() {
            ctx.literal(" ELSE ");
            ctx.with(Over::default().in_function(true), |ctx| ctx.sql(&default))?;
        }
        ctx.literal(" END");
        Ok(())
    })
}

/// `expr, value = pair` for any two-element iterable.
fn unpack_pair<'py>(pair: &Bound<'py, PyAny>) -> PyResult<(Bound<'py, PyAny>, Bound<'py, PyAny>)> {
    let items: Vec<Bound<'py, PyAny>> = pair.try_iter()?.collect::<PyResult<_>>()?;
    if items.len() != 2 {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "expected 2 values to unpack, got {}",
            items.len()
        )));
    }
    let mut it = items.into_iter();
    Ok((it.next().unwrap(), it.next().unwrap()))
}

// Window.__sql__
pub fn window<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let inline = node.getattr(intern!(py, "_inline"))?.is_truthy()?;
    if ctx.scope() != SCOPE_SOURCE && !inline {
        let alias: String = node.getattr(intern!(py, "_alias"))?.extract()?;
        ctx.literal(&alias);
        ctx.literal(" AS ");
    }

    let extends = node.getattr(intern!(py, "_extends"))?;
    let partition_by = node.getattr(intern!(py, "partition_by"))?;
    let order_by = node.getattr(intern!(py, "order_by"))?;
    let start = node.getattr(intern!(py, "start"))?;
    let end = node.getattr(intern!(py, "end"))?;
    let frame_type = node.getattr(intern!(py, "frame_type"))?;
    let exclude = node.getattr(intern!(py, "_exclude"))?;

    ctx.with(Over::default().parens(true), |ctx| {
        // The parts are rendered as NodeList(parts): glue ' ', no parentheses.
        let mut first = true;
        let mut sep = |ctx: &mut Ctx<'py>| {
            if !first {
                ctx.literal(" ");
            }
            first = false;
        };

        if !extends.is_none() {
            sep(ctx);
            if isa(&extends, &ctx.c.window)? {
                let alias: String = extends.getattr(intern!(py, "_alias"))?.extract()?;
                ctx.literal(&alias);
            } else if extends.is_instance_of::<PyString>() {
                let s: String = extends.extract()?;
                ctx.literal(&s);
            } else {
                ctx.sql(&extends)?;
            }
        }
        if partition_by.is_truthy()? {
            sep(ctx);
            ctx.literal("PARTITION BY ");
            basic::comma_list(ctx, &partition_by)?;
        }
        if order_by.is_truthy()? {
            sep(ctx);
            ctx.literal("ORDER BY ");
            basic::comma_list(ctx, &order_by)?;
        }
        let frame: Option<String> = if frame_type.is_none() {
            None
        } else {
            Some(frame_type.extract()?)
        };
        if !start.is_none() && !end.is_none() {
            sep(ctx);
            let frame = frame.clone().unwrap_or_else(|| "ROWS".to_string());
            ctx.literal(&format!("{} BETWEEN ", frame));
            ctx.sql(&start)?;
            ctx.literal(" AND ");
            ctx.sql(&end)?;
        } else if !start.is_none() {
            sep(ctx);
            let frame = frame.clone().unwrap_or_else(|| "ROWS".to_string());
            ctx.literal(&frame);
            ctx.literal(" ");
            ctx.sql(&start)?;
        } else if let Some(frame) = &frame {
            sep(ctx);
            ctx.literal(&format!("{} UNBOUNDED PRECEDING", frame));
        }
        if !exclude.is_none() {
            sep(ctx);
            ctx.literal("EXCLUDE ");
            ctx.sql(&exclude)?;
        }
        Ok(())
    })
}

// WindowAlias.__sql__
pub fn window_alias<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let alias = node
        .getattr(intern!(py, "window"))?
        .getattr(intern!(py, "_alias"))?;
    if alias.is_truthy()? {
        let a: String = alias.extract()?;
        ctx.literal(&a);
    } else {
        ctx.literal("w");
    }
    Ok(())
}

// _InFunction.__sql__
pub fn in_function<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let in_function = node.getattr(intern!(py, "in_function"))?.is_truthy()?;
    let inner = node.getattr(intern!(py, "node"))?;
    ctx.with(Over::default().in_function(in_function), |ctx| {
        ctx.sql(&inner)
    })
}
