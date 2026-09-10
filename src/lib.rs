//! rufee: peewee's SQL rendering `Context`, implemented in Rust.
//!
//! The Python side (`python/rufee/_context.py`) subclasses `peewee.Context`
//! and hands top-level query nodes to [`render`]. Anything this crate does not
//! understand raises [`Unsupported`], and the Python side falls back to
//! peewee's own renderer.

use std::panic::{catch_unwind, AssertUnwindSafe};

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyDict;

pub mod classes;
pub mod ctx;
pub mod nodes;

create_exception!(
    _native,
    Unsupported,
    PyException,
    "The Rust renderer does not support this node; fall back to peewee."
);

/// Render a top-level peewee query node.
///
/// * `query`       – a `peewee.BaseQuery` instance
/// * `scope`       – peewee `SCOPE_*` constant of the initial state
/// * `parentheses` – initial `State.parentheses`
/// * `settings`    – the `Context(**settings)` keyword arguments
///
/// Returns `(sql, params)` exactly as `peewee.Context.query()` would.
#[pyfunction]
fn render<'py>(
    py: Python<'py>,
    query: &Bound<'py, PyAny>,
    scope: u8,
    parentheses: bool,
    settings: &Bound<'py, PyDict>,
) -> PyResult<(String, Vec<Py<PyAny>>)> {
    let result = catch_unwind(AssertUnwindSafe(
        || -> PyResult<(String, Vec<Py<PyAny>>)> {
            let mut c = ctx::Ctx::new(py, scope, parentheses, settings)?;
            c.sql(query)?;
            Ok(c.finish())
        },
    ));
    match result {
        Ok(r) => r,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            Err(Unsupported::new_err(format!(
                "rust renderer panicked: {msg}"
            )))
        }
    }
}

#[pymodule]
fn _native(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("Unsupported", py.get_type::<Unsupported>())?;
    m.add_function(wrap_pyfunction!(render, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
