//! `playhouse.postgres_ext` nodes: `JsonLookup`, `JsonPath`, `ObjectSlice`
//! and `ArrayValue`.
//!
//! The module is never imported by rufee: the classes are found through
//! `sys.modules`, since an instance can only exist once it is loaded.

use pyo3::intern;
use pyo3::prelude::*;

use crate::classes::isa;
use crate::ctx::Ctx;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PgKind {
    JsonLookup,
    JsonPath,
    ObjectSlice,
    ArrayValue,
}

/// `isinstance` against the `playhouse.postgres_ext` node classes.
pub fn kind(obj: &Bound<'_, PyAny>) -> PyResult<Option<PgKind>> {
    let py = obj.py();
    let modules = py
        .import(intern!(py, "sys"))?
        .getattr(intern!(py, "modules"))?;
    let module =
        modules.call_method1(intern!(py, "get"), (intern!(py, "playhouse.postgres_ext"),))?;
    if module.is_none() {
        return Ok(None);
    }
    for (name, kind) in [
        ("JsonLookup", PgKind::JsonLookup),
        ("JsonPath", PgKind::JsonPath),
        ("ObjectSlice", PgKind::ObjectSlice),
        ("ArrayValue", PgKind::ArrayValue),
    ] {
        if let Ok(cls) = module.getattr(name) {
            if obj.is_instance(&cls)? {
                return Ok(Some(kind));
            }
        }
    }
    Ok(None)
}

pub fn render<'py>(ctx: &mut Ctx<'py>, obj: &Bound<'py, PyAny>, kind: PgKind) -> PyResult<()> {
    match kind {
        PgKind::JsonLookup => json_lookup(ctx, obj),
        PgKind::JsonPath => json_path(ctx, obj),
        PgKind::ObjectSlice => object_slice(ctx, obj),
        PgKind::ArrayValue => array_value(ctx, obj),
    }
}

fn as_json(obj: &Bound<'_, PyAny>) -> PyResult<bool> {
    obj.getattr(intern!(obj.py(), "_as_json"))?.is_truthy()
}

/// `_key_literal(part)`: a node as is, anything else inlined through
/// `ValueLiterals` (psycopg3 would bind each key occurrence separately).
fn key<'py>(ctx: &mut Ctx<'py>, part: &Bound<'py, PyAny>) -> PyResult<()> {
    if isa(part, &ctx.c.node)? {
        return ctx.sql(part);
    }
    let lit = ctx.c.value_literals.bind(ctx.py).call1((part,))?;
    ctx.sql(&lit)
}

/// `JsonLookup.__sql__`: `node->'a'->'b'->>'c'` (`->` last when `_as_json`).
fn json_lookup<'py>(ctx: &mut Ctx<'py>, obj: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    ctx.sql(&obj.getattr(intern!(py, "node"))?)?;
    let parts: Vec<Bound<'py, PyAny>> = obj.getattr(intern!(py, "parts"))?.extract()?;
    if let Some((last, init)) = parts.split_last() {
        for part in init {
            ctx.literal("->");
            key(ctx, part)?;
        }
        ctx.literal(if as_json(obj)? { "->" } else { "->>" });
        key(ctx, last)?;
    }
    Ok(())
}

/// `JsonPath.__sql__`: `node #> '{a,b}'` (`#>>` unless `_as_json`).
fn json_path<'py>(ctx: &mut Ctx<'py>, obj: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    ctx.sql(&obj.getattr(intern!(py, "node"))?)?;
    ctx.literal(if as_json(obj)? { "#>" } else { "#>>" });
    let mut path = String::from("{");
    for (i, part) in obj.getattr(intern!(py, "parts"))?.try_iter()?.enumerate() {
        if i > 0 {
            path.push(',');
        }
        path.push_str(part?.str()?.to_str()?);
    }
    path.push('}');
    let lit = ctx.c.value_literals.bind(py).call1((path,))?;
    ctx.sql(&lit)
}

/// `ObjectSlice.__sql__`: `node[i]` / `node[i:j]`, 0-based Python indexes
/// shifted to Postgres' 1-based ones; a node index renders as `node[<expr>]`.
fn object_slice<'py>(ctx: &mut Ctx<'py>, obj: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    ctx.sql(&obj.getattr(intern!(py, "node"))?)?;
    let parts = obj.getattr(intern!(py, "parts"))?;
    if isa(&parts, &ctx.c.node)? {
        ctx.literal("[");
        ctx.sql(&parts)?;
        ctx.literal("]");
        return Ok(());
    }
    let mut s = String::from("[");
    for (i, part) in parts.try_iter()?.enumerate() {
        let part = part?;
        if i > 0 {
            s.push(':');
        }
        if !part.is_none() {
            s.push_str(part.add(1)?.str()?.to_str()?);
        }
    }
    s.push(']');
    ctx.literal(&s);
    Ok(())
}

/// `ArrayValue.__sql__`: `AsIs(value)` followed by `::<array type>`. The type
/// comes from the field's `ddl_datatype(ctx)`, which reads
/// `ctx.state.field_types`, so it is given a peewee context.
fn array_value<'py>(ctx: &mut Ctx<'py>, obj: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let value = obj.getattr(intern!(py, "value"))?;
    let as_is = ctx.c.value.bind(py).call1((value, py.None(), false))?;
    ctx.sql(&as_is)?;
    ctx.literal("::");
    let pctx = ctx.python_ctx()?;
    let datatype = obj
        .getattr(intern!(py, "field"))?
        .call_method1(intern!(py, "ddl_datatype"), (pctx,))?;
    ctx.sql(&datatype)
}
