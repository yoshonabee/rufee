//! Expressions, values, entities and the small wrapper nodes.

use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyFrozenSet, PyList, PySet, PyTuple};

use crate::classes::{isa, SCOPE_COLUMN, SCOPE_SOURCE, SCOPE_VALUES};
use crate::ctx::{Converter, Ctx, Over};

/// `Value(value, converter, unpack=True)` rendered without building the node:
/// multi-part values become `EnclosedNodeList`, scalars go to `ctx.value()`.
pub fn value_of<'py>(
    ctx: &mut Ctx<'py>,
    v: &Bound<'py, PyAny>,
    conv: Converter<'_, 'py>,
    unpack: bool,
) -> PyResult<()> {
    if unpack && is_multi(ctx, v)? {
        let mut items: Vec<Bound<'py, PyAny>> = v.try_iter()?.collect::<PyResult<_>>()?;
        if items.is_empty() {
            ctx.literal("()");
            return Ok(());
        }
        // peewee reaches this through `EnclosedNodeList(self.values)`, so the
        // hack in its constructor applies here too. It shows up for a
        // one-element tuple holding an expression — `where((a | b,))`, usually
        // a stray trailing comma — which otherwise gains a paren level.
        if let Some(flat) = lone_flat_expression(ctx, &items)? {
            items[0] = flat;
        }
        return ctx.with(Over::default().parens(true), |ctx| {
            let n = items.len();
            for (i, item) in items.iter().enumerate() {
                if isa(item, &ctx.c.node)? {
                    ctx.sql(item)?;
                } else {
                    // Value(item, self.converter): nested Values are not unpacked again
                    // unless they are themselves multi (peewee recurses via Value()).
                    value_of(ctx, item, reborrow(&conv), true)?;
                }
                if i + 1 < n {
                    ctx.literal(", ");
                }
            }
            Ok(())
        });
    }
    ctx.value(v, conv, true)
}

fn reborrow<'a, 'py>(c: &Converter<'a, 'py>) -> Converter<'a, 'py> {
    match c {
        Converter::Default => Converter::Default,
        Converter::NoConvert => Converter::NoConvert,
        Converter::Fn(f) => Converter::Fn(f),
    }
}

/// peewee's `multi_types = (list, tuple, frozenset, set, range)`.
pub fn is_multi(ctx: &Ctx<'_>, v: &Bound<'_, PyAny>) -> PyResult<bool> {
    Ok(v.is_instance_of::<PyList>()
        || v.is_instance_of::<PyTuple>()
        || v.is_instance_of::<PySet>()
        || v.is_instance_of::<PyFrozenSet>()
        || isa(v, &ctx.c.range)?)
}

/// Turn a Python `Value.converter` attribute into a `Converter` argument.
pub fn converter_arg<'a, 'py>(conv: &'a Bound<'py, PyAny>) -> Converter<'a, 'py> {
    if conv.is_none() {
        Converter::Default
    } else if conv.is_truthy().unwrap_or(false) {
        Converter::Fn(conv)
    } else {
        Converter::NoConvert
    }
}

// Value.__sql__
pub fn value<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    if node.getattr(intern!(py, "multi"))?.is_truthy()? {
        let values = node.getattr(intern!(py, "values"))?;
        return enclosed_list(ctx, &values);
    }
    let v = node.getattr(intern!(py, "value"))?;
    let conv = node.getattr(intern!(py, "converter"))?;
    ctx.value(&v, converter_arg(&conv), true)
}

// ValueLiterals.__sql__
pub fn value_literals<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let inner = node.getattr(intern!(py, "node"))?;
    ctx.with(Over::default().value_literals(true), |ctx| ctx.sql(&inner))
}

// Entity.__sql__ — `_path` parts are stored raw; each is escaped (opening
// quote char doubled into the quote pair) and wrapped at render time.
pub fn entity<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    for (i, part) in node.getattr(intern!(py, "_path"))?.try_iter()?.enumerate() {
        if i > 0 {
            ctx.sql.push('.');
        }
        ctx.quoted_part(part?.cast::<pyo3::types::PyString>()?.to_str()?);
    }
    Ok(())
}

// SQL.__sql__
pub fn sql<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let s: String = node.getattr(intern!(py, "sql"))?.extract()?;
    ctx.literal(&s);
    let params = node.getattr(intern!(py, "params"))?;
    if params.is_truthy()? {
        for p in params.try_iter()? {
            ctx.value(&p?, Converter::NoConvert, false)?;
        }
    }
    Ok(())
}

// Column.__sql__
pub fn column<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let name: String = node.getattr(intern!(py, "name"))?.extract()?;
    if ctx.scope() == SCOPE_VALUES {
        return ctx.entity(&[&name]);
    }
    let source = node.getattr(intern!(py, "source"))?;
    ctx.with(Over::default().scope(SCOPE_COLUMN), |ctx| {
        ctx.sql(&source)?;
        ctx.literal(".");
        ctx.entity(&[&name])
    })
}

// Alias.__sql__
pub fn alias<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let a: String = node.getattr(intern!(py, "_alias"))?.extract()?;
    if ctx.scope() == SCOPE_SOURCE {
        let inner = node.getattr(intern!(py, "node"))?;
        ctx.sql(&inner)?;
        ctx.literal(" AS ");
    }
    ctx.entity(&[&a])
}

// BindTo.__sql__
pub fn bind_to<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let inner = node.getattr(intern!(py, "node"))?;
    ctx.sql(&inner)
}

// Negated.__sql__
pub fn negated<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    ctx.literal("NOT ");
    let inner = node.getattr(intern!(py, "node"))?;
    ctx.sql(&inner)
}

// BitwiseNegated.__sql__
pub fn bitwise_negated<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let op: String = node.getattr(intern!(py, "op"))?.extract()?;
    let op_sql = map_operation(ctx, &op)?;
    ctx.literal(&op_sql);
    let inner = node.getattr(intern!(py, "node"))?;
    ctx.sql(&inner)
}

/// `ctx.state.operations.get(op, op)` when the dialect maps operators.
pub fn map_operation(ctx: &Ctx<'_>, op: &str) -> PyResult<String> {
    if let Some(ops) = &ctx.dialect.operations {
        if let Some(v) = ops.bind(ctx.py).get_item(op)? {
            if !v.is_none() {
                return v.extract();
            }
        }
    }
    Ok(op.to_string())
}

// Cast.__sql__
pub fn cast<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    ctx.literal("CAST(");
    let inner = node.getattr(intern!(py, "node"))?;
    ctx.sql(&inner)?;
    let as_type: String = node.getattr(intern!(py, "_cast"))?.extract()?;
    ctx.literal(&format!(" AS {})", as_type));
    Ok(())
}

// Ordering.__sql__
pub fn ordering<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let nulls = node.getattr(intern!(py, "nulls"))?;
    let has_nulls = nulls.is_truthy()?;
    let nulls_ordering = ctx.dialect.nulls_ordering;
    if has_nulls && !nulls_ordering {
        // ctx.sql(self._null_ordering_case(self.nulls)).literal(', ')
        let case = node.call_method1("_null_ordering_case", (&nulls,))?;
        ctx.sql(&case)?;
        ctx.literal(", ");
    }
    let inner = node.getattr(intern!(py, "node"))?;
    ctx.sql(&inner)?;
    let collation = node.getattr(intern!(py, "collation"))?;
    if collation.is_truthy()? {
        ctx.literal(&format!(" COLLATE {}", collation.extract::<String>()?));
    }
    let direction: String = node.getattr(intern!(py, "direction"))?.extract()?;
    ctx.literal(&format!(" {}", direction));
    if has_nulls && nulls_ordering {
        ctx.literal(&format!(" NULLS {}", nulls.extract::<String>()?));
    }
    Ok(())
}

// Expression.__sql__
pub fn expression<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let c = ctx.c;
    let lhs = node.getattr(intern!(py, "lhs"))?;
    let op: String = node.getattr(intern!(py, "op"))?.extract()?;
    let rhs = node.getattr(intern!(py, "rhs"))?;
    let flat = node.getattr(intern!(py, "flat"))?.is_truthy()?;

    let mut over = Over::default().parens(!flat).in_expr(true);

    // Unwrap the lhs to find a Field and pick its converter.
    let mut inner = lhs.clone();
    if isa(&lhs, &c.wrapped_node)? {
        inner = lhs.call_method0("unwrap")?;
    }
    let raw_coerce = lhs
        .getattr(intern!(py, "_coerce"))
        .map(|v| v.is_truthy().unwrap_or(true))
        .unwrap_or(true);
    if isa(&inner, &c.field)? && raw_coerce {
        over = over
            .converter(Some(inner.getattr(intern!(py, "db_value"))?.unbind()))
            .is_fk_expr(isa(&inner, &c.foreign_key_field)?);
    } else {
        over = over.converter(None);
    }

    let op_sql = map_operation(ctx, &op)?;
    let op_in = op == "IN" || op == "NOT IN";

    ctx.with(over, |ctx| {
        if op_in && rhs_renders_empty(ctx, &rhs)? {
            ctx.literal(if op == "IN" { "0 = 1" } else { "1 = 1" });
            return Ok(());
        }
        ctx.sql(&lhs)?;
        ctx.literal(&format!(" {} ", op_sql));
        if rhs.is_none() && (op == "IS" || op == "IS NOT") {
            ctx.literal("NULL");
            return Ok(());
        }
        ctx.sql(&rhs)
    })
}

/// `ctx.as_new().parse(rhs)[0] == '()'`
///
/// Fast path: a plain Python collection whose items are all non-Node values
/// renders as one placeholder per item, so it is empty iff it has no items.
/// That avoids rendering (and converting) every IN value twice.
fn rhs_renders_empty<'py>(ctx: &Ctx<'py>, rhs: &Bound<'py, PyAny>) -> PyResult<bool> {
    if is_multi(ctx, rhs)? {
        let mut n = 0usize;
        let mut plain = true;
        for item in rhs.try_iter()? {
            n += 1;
            if isa(&item?, &ctx.c.node)? {
                plain = false;
                break;
            }
        }
        if plain {
            return Ok(n == 0);
        }
    }
    let mut probe = ctx.as_new()?;
    probe.sql(rhs)?;
    Ok(probe.sql == "()")
}

// NodeList.__sql__
pub fn node_list<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let nodes = node.getattr(intern!(py, "nodes"))?;
    let glue: String = node.getattr(intern!(py, "glue"))?.extract()?;
    let parens = node.getattr(intern!(py, "parens"))?.is_truthy()?;
    let items: Vec<Bound<'py, PyAny>> = nodes.try_iter()?.collect::<PyResult<_>>()?;
    render_list(ctx, &items, &glue, parens)
}

pub fn render_list<'py>(
    ctx: &mut Ctx<'py>,
    items: &[Bound<'py, PyAny>],
    glue: &str,
    parens: bool,
) -> PyResult<()> {
    if items.is_empty() {
        if parens {
            ctx.literal("()");
        }
        return Ok(());
    }
    ctx.with(Over::default().parens(parens), |ctx| {
        let n = items.len();
        for (i, item) in items.iter().enumerate() {
            ctx.sql(item)?;
            if i + 1 < n {
                ctx.literal(glue);
            }
        }
        Ok(())
    })
}

/// `CommaNodeList(nodes)` from a Python iterable.
pub fn comma_list<'py>(ctx: &mut Ctx<'py>, nodes: &Bound<'py, PyAny>) -> PyResult<()> {
    let items: Vec<Bound<'py, PyAny>> = nodes.try_iter()?.collect::<PyResult<_>>()?;
    render_list(ctx, &items, ", ", false)
}

/// `EnclosedNodeList(nodes)` from a Python iterable.
pub fn enclosed_list<'py>(ctx: &mut Ctx<'py>, nodes: &Bound<'py, PyAny>) -> PyResult<()> {
    let items: Vec<Bound<'py, PyAny>> = nodes.try_iter()?.collect::<PyResult<_>>()?;
    enclosed_items(ctx, &items)
}

/// peewee's `NodeList.__init__` "avoid double parentheses" hack: inside a
/// parenthesised list, a lone non-flat `Expression` is cloned with
/// `flat = True` so it does not add a second pair of parentheses. Returns the
/// clone when the hack applies, `None` when the list renders as-is.
pub fn lone_flat_expression<'py>(
    ctx: &Ctx<'py>,
    items: &[Bound<'py, PyAny>],
) -> PyResult<Option<Bound<'py, PyAny>>> {
    let [item] = items else { return Ok(None) };
    if !isa(item, &ctx.c.expression)? || item.getattr(intern!(ctx.py, "flat"))?.is_truthy()? {
        return Ok(None);
    }
    let clone = item.call_method0(intern!(ctx.py, "clone"))?;
    clone.setattr(intern!(ctx.py, "flat"), true)?;
    Ok(Some(clone))
}

/// `EnclosedNodeList` including the hack above.
pub fn enclosed_items<'py>(ctx: &mut Ctx<'py>, items: &[Bound<'py, PyAny>]) -> PyResult<()> {
    if let Some(flat) = lone_flat_expression(ctx, items)? {
        return render_list(ctx, &[flat], ", ", true);
    }
    render_list(ctx, items, ", ", true)
}

// Star.__sql__
pub fn star<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let source = node.getattr(intern!(py, "source"))?;
    ctx.with(Over::default().scope(SCOPE_COLUMN), |ctx| ctx.sql(&source))?;
    ctx.literal(".*");
    Ok(())
}

// QualifiedNames.__sql__
pub fn qualified_names<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let inner = node.getattr(intern!(py, "node"))?;
    ctx.with(Over::default().scope(SCOPE_COLUMN), |ctx| ctx.sql(&inner))
}

// NamespaceAttribute.__sql__
pub fn namespace_attribute<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let ns: String = node
        .getattr(intern!(py, "_namespace"))?
        .getattr(intern!(py, "_name"))?
        .extract()?;
    let attr: String = node.getattr(intern!(py, "_attribute"))?.extract()?;
    ctx.literal(&format!("{}.", ns));
    ctx.entity(&[&attr])
}
