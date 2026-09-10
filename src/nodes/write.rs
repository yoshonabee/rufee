//! Insert, Update, Delete.

use pyo3::prelude::*;

use crate::ctx::Ctx;

pub fn insert<'py>(ctx: &mut Ctx<'py>, _node: &Bound<'py, PyAny>) -> PyResult<()> {
    Err(ctx.unsupported("Insert"))
}

pub fn update<'py>(ctx: &mut Ctx<'py>, _node: &Bound<'py, PyAny>) -> PyResult<()> {
    Err(ctx.unsupported("Update"))
}

pub fn delete<'py>(ctx: &mut Ctx<'py>, _node: &Bound<'py, PyAny>) -> PyResult<()> {
    Err(ctx.unsupported("Delete"))
}
