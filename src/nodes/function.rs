//! Function, Case, Window, WindowAlias, _InFunction.

use pyo3::prelude::*;

use crate::ctx::Ctx;

pub fn function<'py>(ctx: &mut Ctx<'py>, node: &Bound<'py, PyAny>) -> PyResult<()> {
    Err(ctx.unsupported(&format!("Function {}", node.getattr("name")?.str()?)))
}

pub fn case<'py>(ctx: &mut Ctx<'py>, _node: &Bound<'py, PyAny>) -> PyResult<()> {
    Err(ctx.unsupported("Case"))
}

pub fn window<'py>(ctx: &mut Ctx<'py>, _node: &Bound<'py, PyAny>) -> PyResult<()> {
    Err(ctx.unsupported("Window"))
}

pub fn window_alias<'py>(ctx: &mut Ctx<'py>, _node: &Bound<'py, PyAny>) -> PyResult<()> {
    Err(ctx.unsupported("WindowAlias"))
}

pub fn in_function<'py>(ctx: &mut Ctx<'py>, _node: &Bound<'py, PyAny>) -> PyResult<()> {
    Err(ctx.unsupported("_InFunction"))
}
