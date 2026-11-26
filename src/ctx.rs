//! A Rust mirror of `peewee.Context` / `State` / `AliasManager`.
//!
//! Semantics follow peewee 3.17.x exactly: scope and settings are inherited on
//! `with ctx(...)`, `parentheses` is not; entering a state with `parentheses`
//! writes `(` and leaving it writes `)`.

use pyo3::intern;
use std::collections::HashMap;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};

use crate::classes::{classes, isa, Classes, SCOPE_COLUMN, SCOPE_NORMAL};
use crate::nodes;
use crate::Unsupported;

/// The dialect part of `State.settings`: everything `get_context_options()`
/// produces. Fixed for the whole render, so it lives on the `Ctx`, not on the
/// per-scope state, and is never cloned.
pub struct Dialect {
    pub param: Option<String>,
    /// `quote` as (open, close) characters; peewee default `""`.
    pub quote: (char, char),
    pub operations: Option<Py<PyDict>>,
    pub nulls_ordering: bool,
    pub limit_max: Option<Py<PyAny>>,
    pub for_update: bool,
    pub compound_select_parentheses: Option<i64>,
    pub conflict_statement: Option<Py<PyAny>>,
    pub conflict_update: Option<Py<PyAny>>,
    /// The original kwargs dict, so a peewee `Context` with identical settings
    /// can be constructed when a Python hook has to be called with a context.
    pub raw: Py<PyDict>,
}

/// The per-scope part of `State.settings`: the flags nodes set with
/// `with ctx(...)`. Cloned on every scope change, so it is kept tiny.
#[derive(Clone, Default)]
pub struct Flags {
    pub converter: Option<Py<PyAny>>, // None ⇔ falsy (None / False)
    pub in_expr: bool,
    pub in_function: bool,
    /// peewee 4: set by `Function` for a lone sub-select argument of a
    /// `subquery_functions` member (EXISTS, ANY, ...), whose own parentheses
    /// are then dropped.
    pub bare_subquery: bool,
    /// peewee 4: set while rendering a SELECT list, which is SCOPE_SOURCE
    /// but holds operands, not FROM/JOIN sources.
    pub in_projection: bool,
    pub subquery: bool,
    pub is_fk_expr: bool,
    pub value_literals: bool,
}

fn opt_obj(d: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<Py<PyAny>>> {
    match d.get_item(key)? {
        Some(v) if v.is_truthy()? => Ok(Some(v.unbind())),
        _ => Ok(None),
    }
}

fn opt_bool(d: &Bound<'_, PyDict>, key: &str) -> PyResult<bool> {
    match d.get_item(key)? {
        Some(v) => v.is_truthy(),
        None => Ok(false),
    }
}

fn opt_str(d: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<String>> {
    match d.get_item(key)? {
        Some(v) if !v.is_none() => Ok(Some(v.extract()?)),
        _ => Ok(None),
    }
}

impl Dialect {
    pub fn from_dict(d: &Bound<'_, PyDict>) -> PyResult<Self> {
        let compound_select_parentheses = match d.get_item("compound_select_parentheses")? {
            Some(v) if !v.is_none() => Some(v.extract::<i64>()?),
            _ => None,
        };
        let quote = match opt_str(d, "quote")? {
            Some(q) => {
                let mut it = q.chars();
                (it.next().unwrap_or('"'), it.next().unwrap_or('"'))
            }
            None => ('"', '"'),
        };
        let operations = match d.get_item("operations")? {
            Some(v) if v.is_truthy()? => Some(v.cast_into::<PyDict>()?.unbind()),
            _ => None,
        };
        Ok(Dialect {
            param: opt_str(d, "param")?,
            quote,
            operations,
            nulls_ordering: opt_bool(d, "nulls_ordering")?,
            limit_max: opt_obj(d, "limit_max")?,
            for_update: opt_bool(d, "for_update")?,
            compound_select_parentheses,
            conflict_statement: opt_obj(d, "conflict_statement")?,
            conflict_update: opt_obj(d, "conflict_update")?,
            raw: d.copy()?.unbind(),
        })
    }
}

impl Flags {
    pub fn from_dict(d: &Bound<'_, PyDict>) -> PyResult<Self> {
        Ok(Flags {
            converter: opt_obj(d, "converter")?,
            in_expr: opt_bool(d, "in_expr")?,
            in_function: opt_bool(d, "in_function")?,
            bare_subquery: opt_bool(d, "bare_subquery")?,
            in_projection: opt_bool(d, "in_projection")?,
            subquery: opt_bool(d, "subquery")?,
            is_fk_expr: opt_bool(d, "is_fk_expr")?,
            value_literals: opt_bool(d, "value_literals")?,
        })
    }
}

#[derive(Clone)]
pub struct State {
    pub scope: u8,
    pub parentheses: bool,
    pub flags: Flags,
}

/// A converter argument as peewee's `ctx.value()` sees it:
/// `None` → use the state's converter, `False` → no conversion, callable → use it.
pub enum Converter<'a, 'py> {
    Default,
    NoConvert,
    Fn(&'a Bound<'py, PyAny>),
}

/// The overrides accepted by `ctx(**overrides)`. Only the keys the renderer
/// ever passes are modelled.
#[derive(Default)]
pub struct Over {
    pub scope: Option<u8>,
    pub parentheses: Option<bool>,
    pub converter: Option<Option<Py<PyAny>>>,
    pub in_expr: Option<bool>,
    pub in_function: Option<bool>,
    pub bare_subquery: Option<bool>,
    pub in_projection: Option<bool>,
    pub subquery: Option<bool>,
    pub is_fk_expr: Option<bool>,
    pub value_literals: Option<bool>,
}

impl Over {
    pub fn scope(mut self, s: u8) -> Self {
        self.scope = Some(s);
        self
    }
    pub fn parens(mut self, p: bool) -> Self {
        self.parentheses = Some(p);
        self
    }
    pub fn converter(mut self, c: Option<Py<PyAny>>) -> Self {
        self.converter = Some(c);
        self
    }
    pub fn in_expr(mut self, v: bool) -> Self {
        self.in_expr = Some(v);
        self
    }
    pub fn in_function(mut self, v: bool) -> Self {
        self.in_function = Some(v);
        self
    }
    pub fn bare_subquery(mut self, v: bool) -> Self {
        self.bare_subquery = Some(v);
        self
    }
    pub fn in_projection(mut self, v: bool) -> Self {
        self.in_projection = Some(v);
        self
    }
    pub fn subquery(mut self, v: bool) -> Self {
        self.subquery = Some(v);
        self
    }
    pub fn is_fk_expr(mut self, v: bool) -> Self {
        self.is_fk_expr = Some(v);
        self
    }
    pub fn value_literals(mut self, v: bool) -> Self {
        self.value_literals = Some(v);
        self
    }
}

/// peewee's `AliasManager`: a stack of `{source: alias}` dicts keyed with
/// Python hashing (sources define `__hash__`/`__eq__`), one counter.
pub struct AliasManager {
    /// Source of truth: peewee semantics, keyed with Python hash/eq.
    mapping: Vec<Py<PyDict>>,
    /// Identity fast path per level: object address -> alias. Sources are
    /// looked up many times per query (every column reference) and each dict
    /// lookup costs a Python-level `__hash__` call; the same object always maps
    /// to the same alias within a level.
    by_id: Vec<HashMap<usize, String>>,
    current: usize, // 1-based like peewee's `_current_index`
    counter: usize,
}

impl AliasManager {
    fn new(py: Python<'_>) -> Self {
        AliasManager {
            mapping: vec![PyDict::new(py).unbind()],
            by_id: vec![HashMap::new()],
            current: 1,
            counter: 0,
        }
    }

    fn map<'py>(&self, py: Python<'py>) -> Bound<'py, PyDict> {
        self.mapping[self.current - 1].bind(py).clone()
    }

    /// `alias_manager.add(source)` / `alias_manager[source]`.
    pub fn get(&mut self, source: &Bound<'_, PyAny>) -> PyResult<String> {
        let key = source.as_ptr() as usize;
        if let Some(a) = self.by_id[self.current - 1].get(&key) {
            return Ok(a.clone());
        }
        let m = self.map(source.py());
        let alias = match m.get_item(source)? {
            Some(a) => a.extract::<String>()?,
            None => {
                self.counter += 1;
                let alias = format!("t{}", self.counter);
                m.set_item(source, &alias)?;
                alias
            }
        };
        self.by_id[self.current - 1].insert(key, alias.clone());
        Ok(alias)
    }

    /// `alias_manager[source] = alias`.
    pub fn set(&mut self, source: &Bound<'_, PyAny>, alias: &str) -> PyResult<()> {
        let m = self.map(source.py());
        // If an *equal* (not identical) source already had a different alias,
        // other objects' identity entries could now be stale; drop them.
        if let Some(prev) = m.get_item(source)? {
            if prev.extract::<String>()? != alias {
                self.by_id[self.current - 1].clear();
            }
        }
        m.set_item(source, alias)?;
        self.by_id[self.current - 1].insert(source.as_ptr() as usize, alias.to_string());
        Ok(())
    }

    pub fn push(&mut self, py: Python<'_>) {
        self.current += 1;
        if self.current > self.mapping.len() {
            self.mapping.push(PyDict::new(py).unbind());
            self.by_id.push(HashMap::new());
        }
    }

    pub fn pop(&mut self, py: Python<'_>) -> PyResult<()> {
        if self.current == 1 {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "Cannot pop() from empty alias manager.",
            ));
        }
        // peewee 4 clears the level being left, so a later push() at the same
        // depth starts empty (e.g. a second CTE numbers its sources afresh
        // from the shared counter instead of reusing the first CTE's).
        self.mapping[self.current - 1].bind(py).clear();
        self.by_id[self.current - 1].clear();
        self.current -= 1;
        Ok(())
    }
}

pub struct Ctx<'py> {
    pub py: Python<'py>,
    pub c: &'static Classes,
    pub sql: String,
    pub values: Vec<Py<PyAny>>,
    pub dialect: Dialect,
    pub state: State,
    stack: Vec<State>,
    pub aliases: AliasManager,
}

impl<'py> Ctx<'py> {
    pub fn new(
        py: Python<'py>,
        scope: u8,
        parentheses: bool,
        settings: &Bound<'py, PyDict>,
    ) -> PyResult<Self> {
        Ok(Ctx {
            py,
            c: classes(py)?,
            sql: String::with_capacity(256),
            values: Vec::new(),
            dialect: Dialect::from_dict(settings)?,
            state: State {
                scope,
                parentheses,
                flags: Flags::from_dict(settings)?,
            },
            stack: Vec::new(),
            aliases: AliasManager::new(py),
        })
    }

    /// `Context.as_new()`: same settings, fresh everything else. peewee passes
    /// the *current* settings dict, so the flags carry over too.
    pub fn as_new(&self) -> PyResult<Ctx<'py>> {
        Ok(Ctx {
            py: self.py,
            c: self.c,
            sql: String::with_capacity(64),
            values: Vec::new(),
            dialect: Dialect::from_dict(self.dialect.raw.bind(self.py))?,
            state: State {
                scope: SCOPE_NORMAL,
                parentheses: false,
                flags: self.state.flags.clone(),
            },
            stack: Vec::new(),
            aliases: AliasManager::new(self.py),
        })
    }

    #[inline]
    pub fn scope(&self) -> u8 {
        self.state.scope
    }

    #[inline]
    pub fn literal(&mut self, s: &str) -> &mut Self {
        self.sql.push_str(s);
        self
    }

    /// `with ctx(**over): body(ctx)` — push, `(`, body, `)`, pop.
    pub fn with<F>(&mut self, over: Over, body: F) -> PyResult<()>
    where
        F: FnOnce(&mut Ctx<'py>) -> PyResult<()>,
    {
        let mut st = State {
            scope: over.scope.unwrap_or(self.state.scope),
            parentheses: over.parentheses.unwrap_or(false),
            flags: self.state.flags.clone(),
        };
        if let Some(v) = over.converter {
            st.flags.converter = v;
        }
        if let Some(v) = over.in_expr {
            st.flags.in_expr = v;
        }
        if let Some(v) = over.in_function {
            st.flags.in_function = v;
        }
        if let Some(v) = over.bare_subquery {
            st.flags.bare_subquery = v;
        }
        if let Some(v) = over.in_projection {
            st.flags.in_projection = v;
        }
        if let Some(v) = over.subquery {
            st.flags.subquery = v;
        }
        if let Some(v) = over.is_fk_expr {
            st.flags.is_fk_expr = v;
        }
        if let Some(v) = over.value_literals {
            st.flags.value_literals = v;
        }
        let prev = std::mem::replace(&mut self.state, st);
        self.stack.push(prev);
        if self.state.parentheses {
            self.sql.push('(');
        }
        let r = body(self);
        // __exit__ runs even when the body raised.
        if self.state.parentheses {
            self.sql.push(')');
        }
        self.state = self.stack.pop().expect("state stack underflow");
        r
    }

    /// `with ctx.push_alias(): body(ctx)`.
    pub fn with_alias_scope<F>(&mut self, body: F) -> PyResult<()>
    where
        F: FnOnce(&mut Ctx<'py>) -> PyResult<()>,
    {
        self.aliases.push(self.py);
        let r = body(self);
        self.aliases.pop(self.py)?;
        r
    }

    /// `ctx.sql(obj)`.
    pub fn sql(&mut self, obj: &Bound<'py, PyAny>) -> PyResult<()> {
        nodes::render(self, obj)
    }

    /// `ctx.sql(Entity(*path))` without building the Python node.
    pub fn entity(&mut self, path: &[&str]) -> PyResult<()> {
        let (open, close) = self.dialect.quote;
        let mut first = true;
        for part in path.iter().filter(|p| !p.is_empty()) {
            if !first {
                self.sql.push('.');
            }
            first = false;
            self.sql.push(open);
            // Entity.__init__ escapes embedded double quotes.
            if part.contains('"') {
                self.sql.push_str(&part.replace('"', "\"\""));
            } else {
                self.sql.push_str(part);
            }
            self.sql.push(close);
        }
        Ok(())
    }

    /// Write the parameter placeholder (`ctx.state.param or '?'`).
    #[inline]
    pub fn placeholder(&mut self) {
        match &self.dialect.param {
            Some(p) => self.sql.push_str(p),
            None => self.sql.push('?'),
        }
    }

    /// `ctx.value(value, converter, add_param)`.
    pub fn value(
        &mut self,
        value: &Bound<'py, PyAny>,
        converter: Converter<'_, 'py>,
        add_param: bool,
    ) -> PyResult<()> {
        let py = self.py;
        let converted: Bound<'py, PyAny> = match converter {
            Converter::Fn(f) => f.call1((value,))?,
            Converter::Default => match &self.state.flags.converter {
                Some(f) => f.bind(py).call1((value,))?,
                None => value.clone(),
            },
            Converter::NoConvert => value.clone(),
        };
        let c = self.c;
        if isa(&converted, &c.node)? {
            return self.with(Over::default().converter(None), |ctx| ctx.sql(&converted));
        }
        if crate::classes::is_model(&converted, c)? {
            return self.with(Over::default().scope(SCOPE_COLUMN), |ctx| {
                ctx.sql(&converted)
            });
        }
        if self.state.flags.value_literals {
            let lit = query_val_transform(&converted, c)?;
            self.sql.push_str(&lit);
            return Ok(());
        }
        self.values.push(converted.unbind());
        if add_param {
            self.placeholder();
        }
        Ok(())
    }

    /// Run a Python hook that expects a peewee `Context` (e.g.
    /// `Database.default_values_insert(ctx)`) on a throw-away peewee context
    /// carrying our current state, then splice its output into ours.
    pub fn via_python_ctx<F>(&mut self, f: F) -> PyResult<()>
    where
        F: FnOnce(&Bound<'py, PyAny>) -> PyResult<()>,
    {
        let py = self.py;
        // Rebuild the settings dict peewee would hold at this point: dialect
        // options plus the current per-scope flags.
        let kwargs = self.dialect.raw.bind(py).copy()?;
        let fl = &self.state.flags;
        match &fl.converter {
            Some(c) => kwargs.set_item("converter", c.bind(py))?,
            None => kwargs.set_item("converter", py.None())?,
        }
        kwargs.set_item("in_expr", fl.in_expr)?;
        kwargs.set_item("in_function", fl.in_function)?;
        kwargs.set_item("bare_subquery", fl.bare_subquery)?;
        kwargs.set_item("in_projection", fl.in_projection)?;
        kwargs.set_item("subquery", fl.subquery)?;
        kwargs.set_item("is_fk_expr", fl.is_fk_expr)?;
        kwargs.set_item("value_literals", fl.value_literals)?;
        let pctx = self.c.context.bind(py).call((), Some(&kwargs))?;
        let state_cls = py.import("peewee")?.getattr(intern!(py, "State"))?;
        // peewee 4: State(scope, parentheses, settings), settings as a dict.
        let st = state_cls.call1((self.state.scope, self.state.parentheses, kwargs.copy()?))?;
        pctx.setattr("state", st)?;
        f(&pctx)?;
        let (sql, params): (String, Vec<Py<PyAny>>) = pctx.call_method0("query")?.extract()?;
        self.sql.push_str(&sql);
        self.values.extend(params);
        Ok(())
    }

    pub fn unsupported(&self, what: &str) -> PyErr {
        Unsupported::new_err(what.to_string())
    }

    pub fn finish(self) -> (String, Vec<Py<PyAny>>) {
        (self.sql, self.values)
    }
}

/// peewee's `_query_val_transform` (used when `value_literals` is set).
fn query_val_transform(v: &Bound<'_, PyAny>, c: &Classes) -> PyResult<String> {
    let py = v.py();
    if v.is_instance_of::<PyString>()
        || isa(v, &c.datetime)?
        || isa(v, &c.date)?
        || isa(v, &c.time)?
    {
        return Ok(format!("'{}'", v.str()?.to_str()?));
    }
    if v.is_instance_of::<pyo3::types::PyBytes>() {
        let decoded = match v.call_method1("decode", ("utf8",)) {
            Ok(s) => s,
            Err(_) => v.call_method1("decode", ("raw_unicode_escape",))?,
        };
        return Ok(format!("'{}'", decoded.str()?.to_str()?));
    }
    if v.is_instance_of::<pyo3::types::PyInt>() {
        // int('%s' % int(v)) also maps bool -> 0/1
        let i = py
            .import("builtins")?
            .getattr(intern!(py, "int"))?
            .call1((v,))?;
        return Ok(i.str()?.to_str()?.to_string());
    }
    if v.is_none() {
        return Ok("NULL".to_string());
    }
    Ok(v.str()?.to_str()?.to_string())
}
