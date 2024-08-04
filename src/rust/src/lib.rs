use pyo3::prelude::*;
use pyo3::types::PyList;

// use std::collections::HashMap;
use std::vec::Vec;

enum Scope {
    NORMAL = 1,
    SOURCE = 2,
    VALUES = 4,
    CTE = 8,
    COLUMN = 16,
}

// #[pyclass]
// struct State {
//     scope: Scope,
//     parentheses: bool,
//     field_types: HashMap<String, String>,
//     operations: HashMap<String, String>,
//     param: String,
//     quote: String,
//     compound_select_parentheses: u8, // use enum
//     conflict_statement: PyFunction,
//     conflict_update: PyFunction,
//     for_update: bool,
//     index_schema_prefix: bool,
//     index_using_precedes_table: bool,
//     limit_max: Option<i32>,
//     nulls_ordering: bool,
// }

// #[pymethods]
// impl State {
//     #[new]
//     #[pyo3(signature = (scope=Scope::NORMAL, parentheses=false, **settings))]
//     fn new(
//         scope: Scope,
//         parentheses: bool,
//         field_types: HashMap<String, String>,
//         operations: HashMap<String, String>,
//         param: String,
//         quote: String,
//         compound_select_parentheses: u8, // use enum
//         conflict_statement: PyFunction,
//         conflict_update: PyFunction,
//         for_update: bool,
//         index_schema_prefix: bool,
//         index_using_precedes_table: bool,
//         limit_max: Option<i32>,
//         nulls_ordering: bool,
//     ) -> Self {
//         State {
//             scope,
//             parentheses,
//             field_types,
//             operations,
//             param,
//             quote,
//             compound_select_parentheses,
//             conflict_statement,
//             conflict_update,
//             for_update,
//             index_schema_prefix,
//             index_using_precedes_table,
//             limit_max,
//             nulls_ordering,
//         }
//     }
// }

// unsafe impl Send for State {} // TDOO fix unsafe

// #[pyclass]
// struct AliasManager; // TOOD: implement

// #[pymethods]
// impl AliasManager {
//     #[new]
//     fn new() -> Self {
//         AliasManager
//     }
// }

#[pyclass]
struct Context {
    // stack: Vec<State>,
    #[pyo3(get)]
    _sql: Vec<String>,

    #[pyo3(get, set)]
    _values: Py<PyList>,
    // alias_manager: AliasManager,
    // state: State,
    #[pyo3(get, set)]
    state: PyObject, // TODO implement as native rust type
    _proxy: PyObject, // ContextProxy obj in python
}

unsafe impl Send for Context {} // TDOO fix unsafe

#[pymethods]
impl Context {
    #[new]
    fn new(
        state: &Bound<'_, PyAny>,
        values: &Bound<'_, PyList>,
        context_proxy: &Bound<'_, PyAny>,
    ) -> Self {
        Context {
            // stack: Vec::new(),
            _sql: Vec::new(),

            _values: values.clone().unbind(),
            // alias_manager: AliasManager::new(),
            // state: State::new(settings),
            state: state.clone().unbind(),
            _proxy: context_proxy.clone().unbind(),
        }
    }

    fn literal<'py>(mut slf: PyRefMut<'py, Self>, keyword: &str) -> PyResult<PyRefMut<'py, Self>> {
        slf._sql.push(keyword.to_string());
        Ok(slf)
    }

    fn query(&self, py: Python) -> PyResult<(String, Py<PyList>)> {
        Ok((self._sql.join(""), self._values.clone_ref(py)))
    }

    fn __sql__<'py>(
        &self,
        mut ctx: PyRefMut<'py, Self>,
        py: Python,
    ) -> PyResult<PyRefMut<'py, Self>> {
        // extend sql
        ctx._sql.extend(self._sql.clone());

        // extend values
        let func: PyObject = ctx._values.getattr(py, "extend")?.into();
        let values: Py<PyList> = self._values.clone_ref(py);
        func.call1(py, (values,))?;
        Ok(ctx)
    }

    fn push_values(&mut self, value: &Bound<'_, PyAny>, py: Python) -> PyResult<()> {
        let func: PyObject = self._values.getattr(py, "append")?.into();
        func.call1(py, (value,))?;
        Ok(())
    }

    #[getter]
    fn get_scope(&self, py: Python) -> PyResult<i32> {
        let scope = self.state.getattr(py, "scope")?.extract(py)?;
        Ok(scope)
    }

    #[getter]
    fn get_parentheses(&self, py: Python) -> PyResult<PyObject> {
        let parentheses = self.state.getattr(py, "parentheses")?;
        Ok(parentheses)
    }

    #[getter]
    fn get_subquery(&self, py: Python) -> PyResult<PyObject> {
        let subquery = self.state.getattr(py, "subquery")?;
        Ok(subquery)
    }

    /*

    def parse(self, node):
        return self.sql(node).query()
     */

    // fn sql<'py>(slf: PyRef<'py, Self>, obj: &Bound<'_, PyAny>, py: Python) -> PyResult<PyRef<'py, Self>> {
    //     let func: PyObject = slf._proxy.getattr(py, "sql")?.into();
    //     func.call1(py, (obj,))?;
    //     Ok(slf)
    // }

    // python -> rust -> python may cause Already Borrowed error due to mutable borrow
    // fn parse(&self, node: &Bound<'_, PyAny>, py: Python) -> PyResult<(String, Py<PyList>)> {
    //     let func: PyObject = self._proxy.getattr(py, "sql")?.into();
    //     func.call1(py, (node,))?;
    //     self.query(py)
    // }
}

#[pymodule]
mod rufee {
    // use super::*;

    #[pymodule_export]
    use super::Context;
}
