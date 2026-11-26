//! JSON path nodes: `peewee.JSONPath` and `playhouse.sqlite_ext`'s
//! `JSONPath` / `JSONBPath`.
//!
//! Neither renders SQL of its own: each `__sql__` builds an ordinary node
//! tree (`Expression`, `Function`, `Cast`, `Value`) and hands it to
//! `ctx.sql()`. The tree is dialect-specific and built by Python code that
//! databases and fields may override, so it is built by calling that same
//! Python code and only the result is rendered here.

use pyo3::intern;
use pyo3::prelude::*;

use crate::ctx::Ctx;

/// `peewee.JSONPath.__sql__`: the field's backend helper
/// (`database.json_methods`, bound in `JSONField._db_hook`) builds the
/// extraction, e.g. `"data" -> '$."k"'` / `->>` on sqlite,
/// `CAST(json_extract(...) AS JSON)` / `json_unquote(json_extract(...))` on
/// MySQL. The path value (`$."k"[0]`, `[#-1]` vs `[last]`) is part of it.
pub fn json_path<'py>(ctx: &mut Ctx<'py>, obj: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    let field = obj.getattr(intern!(py, "_field"))?;
    let keys = obj.getattr(intern!(py, "_keys"))?;
    let helper = field.getattr(intern!(py, "_helper"))?;
    let method = if obj.getattr(intern!(py, "_as_text"))?.is_truthy()? {
        intern!(py, "extract_text")
    } else {
        intern!(py, "extract")
    };
    let expr = helper.call_method1(method, (field, keys))?;
    ctx.sql(&expr)
}

/// `playhouse.sqlite_ext.JSONPath.__sql__` (and `JSONBPath`, which only swaps
/// the function): `fn.json_extract(field, self.path)` when the path has
/// elements, the bare field otherwise.
pub fn sqlite_json_path<'py>(
    ctx: &mut Ctx<'py>,
    obj: &Bound<'py, PyAny>,
    jsonb: bool,
) -> PyResult<()> {
    let py = ctx.py;
    let field = obj.getattr(intern!(py, "_field"))?;
    if !obj.getattr(intern!(py, "_path"))?.is_truthy()? {
        return ctx.sql(&field);
    }
    let path = obj.getattr(intern!(py, "path"))?;
    let name = if jsonb {
        intern!(py, "jsonb_extract")
    } else {
        intern!(py, "json_extract")
    };
    let expr = ctx.c.fn_.bind(py).getattr(name)?.call1((field, path))?;
    ctx.sql(&expr)
}

/// `isinstance` against the `playhouse.sqlite_ext` path classes, without
/// importing the module: an instance can only exist once it is loaded.
/// Returns `Some(jsonb)` on a match.
pub fn sqlite_json_path_kind(obj: &Bound<'_, PyAny>) -> PyResult<Option<bool>> {
    let py = obj.py();
    let modules = py
        .import(intern!(py, "sys"))?
        .getattr(intern!(py, "modules"))?;
    let module =
        modules.call_method1(intern!(py, "get"), (intern!(py, "playhouse.sqlite_ext"),))?;
    if module.is_none() {
        return Ok(None);
    }
    // JSONBPath subclasses JSONPath, so it is checked first.
    for (name, jsonb) in [("JSONBPath", true), ("JSONPath", false)] {
        if let Ok(cls) = module.getattr(name) {
            if obj.is_instance(&cls)? {
                return Ok(Some(jsonb));
            }
        }
    }
    Ok(None)
}
