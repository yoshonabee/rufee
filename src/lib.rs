//! rufee: peewee's SQL rendering `Context`, implemented in Rust.
//!
//! The Python side (`python/rufee/__init__.py`) subclasses `peewee.Context`
//! and hands top-level query nodes to [`render`]. Anything this crate does not
//! understand raises [`Unsupported`], and the Python side falls back to
//! peewee's own renderer.

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyDict;

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
fn render(
    _py: Python<'_>,
    query: &Bound<'_, PyAny>,
    _scope: u8,
    _parentheses: bool,
    _settings: &Bound<'_, PyDict>,
) -> PyResult<(String, Vec<Py<PyAny>>)> {
    // Scaffold: nothing is supported yet.
    Err(Unsupported::new_err(format!(
        "rufee has no renderer yet ({})",
        query.get_type().name()?
    )))
}

#[pymodule]
fn _native(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("Unsupported", py.get_type::<Unsupported>())?;
    m.add_function(wrap_pyfunction!(render, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
