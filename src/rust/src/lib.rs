use pyo3::types::{PyDict, PyList, PyTuple};
use pyo3::{prelude::*, AsPyPointer};

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
}

unsafe impl Send for Context {} // TDOO fix unsafe

#[pymethods]
impl Context {
    #[new]
    fn new(state: &Bound<'_, PyAny>, values: &Bound<'_, PyList>) -> Self {
        Context {
            // stack: Vec::new(),
            _sql: Vec::new(),

            _values: values.clone().unbind(),
            // alias_manager: AliasManager::new(),
            // state: State::new(settings),
            state: state.clone().unbind(),
        }
    }

    fn literal<'py>(mut slf: PyRefMut<'py, Self>, keyword: &str) -> PyResult<PyRefMut<'py, Self>> {
        slf._sql.push(keyword.to_string());
        Ok(slf)
    }

    fn query(&self, py: Python) -> PyResult<(String, Py<PyList>)> {
        Ok((self._sql.join(""), self._values.clone_ref(py)))
    }

    fn __sql__<'py>(&self, mut ctx: PyRefMut<'py, Self>, py: Python) -> PyResult<PyRefMut<'py, Self>> {
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
}

fn print_var_type<T>(_: &T) {
    println!("T is {:?}", std::any::type_name::<T>());
}

#[pyfunction]
fn call_python_func<'py>(
    func: &Bound<'py, PyAny>,
    args: &Bound<'py, PyTuple>,
    kwargs: &Bound<'py, PyDict>,
) -> PyResult<Bound<'py, PyAny>> {
    print_var_type(&func);
    func.call(args, Some(kwargs))
}

#[pymodule]
mod rufee {
    // use super::*;

    #[pymodule_export]
    use super::Context;

    #[pymodule_export]
    use super::call_python_func;
}
