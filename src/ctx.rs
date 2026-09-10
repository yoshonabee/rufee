//! A Rust mirror of `peewee.Context` / `State` / `AliasManager`.
//!
//! Semantics follow peewee 3.17.x exactly: scope and settings are inherited on
//! `with ctx(...)`, `parentheses` is not; entering a state with `parentheses`
//! writes `(` and leaving it writes `)`.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};

use crate::classes::{classes, isa, Classes, SCOPE_COLUMN, SCOPE_NORMAL};
use crate::nodes;
use crate::Unsupported;

/// The subset of `State.settings` the renderer reads. Everything peewee's
/// `get_context_options()` produces plus the per-node flags.
#[derive(Clone)]
pub struct Settings {
    pub converter: Option<Py<PyAny>>, // None ⇔ falsy (None / False)
    pub in_expr: bool,
    pub in_function: bool,
    pub function_arg_count: Option<usize>,
    pub subquery: bool,
    pub is_fk_expr: bool,
    pub value_literals: bool,
    pub param: Option<String>,
    pub quote: Option<String>,
    pub operations: Option<Py<PyAny>>,
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

impl Settings {
    pub fn from_dict(d: &Bound<'_, PyDict>) -> PyResult<Self> {
        let function_arg_count = match d.get_item("function_arg_count")? {
            Some(v) if !v.is_none() => Some(v.extract::<usize>()?),
            _ => None,
        };
        let compound_select_parentheses = match d.get_item("compound_select_parentheses")? {
            Some(v) if !v.is_none() => Some(v.extract::<i64>()?),
            _ => None,
        };
        Ok(Settings {
            converter: opt_obj(d, "converter")?,
            in_expr: opt_bool(d, "in_expr")?,
            in_function: opt_bool(d, "in_function")?,
            function_arg_count,
            subquery: opt_bool(d, "subquery")?,
            is_fk_expr: opt_bool(d, "is_fk_expr")?,
            value_literals: opt_bool(d, "value_literals")?,
            param: opt_str(d, "param")?,
            quote: opt_str(d, "quote")?,
            operations: opt_obj(d, "operations")?,
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

#[derive(Clone)]
pub struct State {
    pub scope: u8,
    pub parentheses: bool,
    pub settings: Settings,
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
    pub function_arg_count: Option<Option<usize>>,
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
    pub fn function_arg_count(mut self, v: Option<usize>) -> Self {
        self.function_arg_count = Some(v);
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
    mapping: Vec<Py<PyDict>>,
    current: usize, // 1-based like peewee's `_current_index`
    counter: usize,
}

impl AliasManager {
    fn new(py: Python<'_>) -> Self {
        AliasManager {
            mapping: vec![PyDict::new(py).unbind()],
            current: 1,
            counter: 0,
        }
    }

    fn map<'py>(&self, py: Python<'py>) -> Bound<'py, PyDict> {
        self.mapping[self.current - 1].bind(py).clone()
    }

    /// `alias_manager.add(source)` / `alias_manager[source]`.
    pub fn get(&mut self, source: &Bound<'_, PyAny>) -> PyResult<String> {
        let m = self.map(source.py());
        if let Some(a) = m.get_item(source)? {
            return a.extract();
        }
        self.counter += 1;
        let alias = format!("t{}", self.counter);
        m.set_item(source, &alias)?;
        Ok(alias)
    }

    /// `alias_manager[source] = alias`.
    pub fn set(&mut self, source: &Bound<'_, PyAny>, alias: &str) -> PyResult<()> {
        self.map(source.py()).set_item(source, alias)
    }

    pub fn push(&mut self, py: Python<'_>) {
        self.current += 1;
        if self.current > self.mapping.len() {
            self.mapping.push(PyDict::new(py).unbind());
        }
    }

    pub fn pop(&mut self) -> PyResult<()> {
        if self.current == 1 {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "Cannot pop() from empty alias manager.",
            ));
        }
        self.current -= 1;
        Ok(())
    }
}

pub struct Ctx<'py> {
    pub py: Python<'py>,
    pub c: &'static Classes,
    pub sql: String,
    pub values: Vec<Py<PyAny>>,
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
            state: State {
                scope,
                parentheses,
                settings: Settings::from_dict(settings)?,
            },
            stack: Vec::new(),
            aliases: AliasManager::new(py),
        })
    }

    /// `Context.as_new()`: same settings, fresh everything else.
    pub fn as_new(&self) -> Ctx<'py> {
        Ctx {
            py: self.py,
            c: self.c,
            sql: String::with_capacity(64),
            values: Vec::new(),
            state: State {
                scope: SCOPE_NORMAL,
                parentheses: false,
                settings: self.state.settings.clone(),
            },
            stack: Vec::new(),
            aliases: AliasManager::new(self.py),
        }
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
        let prev = self.state.clone();
        let mut st = State {
            scope: over.scope.unwrap_or(prev.scope),
            parentheses: over.parentheses.unwrap_or(false),
            settings: prev.settings.clone(),
        };
        if let Some(v) = over.converter {
            st.settings.converter = v;
        }
        if let Some(v) = over.in_expr {
            st.settings.in_expr = v;
        }
        if let Some(v) = over.in_function {
            st.settings.in_function = v;
        }
        if let Some(v) = over.function_arg_count {
            st.settings.function_arg_count = v;
        }
        if let Some(v) = over.subquery {
            st.settings.subquery = v;
        }
        if let Some(v) = over.is_fk_expr {
            st.settings.is_fk_expr = v;
        }
        if let Some(v) = over.value_literals {
            st.settings.value_literals = v;
        }
        self.stack.push(prev);
        self.state = st;
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
        self.aliases.pop()?;
        r
    }

    /// `ctx.sql(obj)`.
    pub fn sql(&mut self, obj: &Bound<'py, PyAny>) -> PyResult<()> {
        nodes::render(self, obj)
    }

    /// `ctx.sql(Entity(*path))` without building the Python node.
    pub fn entity(&mut self, path: &[&str]) -> PyResult<()> {
        let q = self
            .state
            .settings
            .quote
            .clone()
            .unwrap_or_else(|| "\"\"".to_string());
        let mut chars = q.chars();
        let (open, close) = (chars.next().unwrap_or('"'), chars.next().unwrap_or('"'));
        let mut first = true;
        for part in path.iter().filter(|p| !p.is_empty()) {
            if !first {
                self.sql.push('.');
            }
            first = false;
            self.sql.push(open);
            // Entity.__init__ escapes embedded double quotes.
            self.sql.push_str(&part.replace('"', "\"\""));
            self.sql.push(close);
        }
        Ok(())
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
            Converter::Default => match &self.state.settings.converter {
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
        if self.state.settings.value_literals {
            let lit = query_val_transform(&converted, c)?;
            self.sql.push_str(&lit);
            return Ok(());
        }
        self.values.push(converted.unbind());
        if add_param {
            match &self.state.settings.param {
                Some(p) => self.sql.push_str(p),
                None => self.sql.push('?'),
            }
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
        let pctx = self
            .c
            .context
            .bind(py)
            .call((), Some(self.state.settings.raw.bind(py)))?;
        let state_cls = py.import("peewee")?.getattr("State")?;
        let kwargs = self.state.settings.raw.bind(py);
        let st = state_cls.call((self.state.scope, self.state.parentheses), Some(kwargs))?;
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
        let i = py.import("builtins")?.getattr("int")?.call1((v,))?;
        return Ok(i.str()?.to_str()?.to_string());
    }
    if v.is_none() {
        return Ok("NULL".to_string());
    }
    Ok(v.str()?.to_str()?.to_string())
}
