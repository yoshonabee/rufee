use pyo3::prelude::*;
use pyo3::types::{PyFrozenSet, PyList, PySet, PyTuple};
use rufee_derive::{HelloWorld, Node};


// #[pyclass]
// struct PyWrappedMethod {
//     _wrapped: PyObject,
//     // _slf: PyObject,
// }

// #[pymethods]
// impl PyWrappedMethod {
//     #[new]
//     fn new(_wrapped: PyObject) -> Self {
//         println!("new pywrapped");
//         PyWrappedMethod { _wrapped }
//     }

//     #[getter("__wrapped__")]
//     fn get_wrapped(&self, py: Python) -> PyResult<PyObject> {
//         Ok(self._wrapped.clone_ref(py))
//     }

//     #[pyo3(signature = (*args, **kwargs))]
//     fn __call__(
//         &mut self,
//         py: Python<'_>,
//         args: &Bound<'_, PyTuple>,
//         kwargs: Option<&Bound<'_, PyDict>>,
//     ) -> PyResult<PyObject> {
//         println!("args: {:?}", args);
//         println!("kwargs: {:?}", kwargs);

//         // let args_list = args.to_list();
//         // args_list.insert(0, self._slf.clone_ref(py))?;

//         // self._wrapped.call_bound(py, args_list.to_tuple(), kwargs)
//         self._wrapped.call_bound(py, args, kwargs)
//     }
// }

// #[pyfunction]
// fn copy(py: Python, func: PyObject) -> PyResult<Bound<PyCFunction>> {
//     let f = move |args: &Bound<'_, PyTuple>,
//                   kwargs: Option<&Bound<'_, PyDict>>|
//           -> PyResult<PyObject> {
//         Python::with_gil(|py| func.call_bound(py, args, kwargs))
//     };
//     PyCFunction::new_closure_bound(py, None, None, f)
// }

// #[pyclass(subclass)]
// #[derive(Copy, Clone)]
// pub struct Node {
//     _coerce: bool,
// }

// #[pymethods]
// impl Node {
//     #[new]
//     #[pyo3(signature = (_coerce=true))]
//     fn new(_coerce: bool) -> Self {
//         Node { _coerce }
//     }

//     // TODO: refine
//     fn coerce(self_: PyRef<'_, Self>, _coerce: bool) -> Self {
//         if _coerce != self._coerce {
//             let mut clone = self.clone();
//             clone._coerce = _coerce;
//             return clone;
//         }
//         self.clone()
//     }

//     fn is_alias(&self) -> bool {
//         false
//     }

//     fn unwrap(&self) -> Self {
//         self.clone()
//     }
// }

#[pyclass]
#[derive(Clone)]
enum ValueType {
    Single(PyObject),
    Multi(PyObject),
}

unsafe impl Send for ValueType {}

#[pyclass]
#[derive(Node)]
pub struct Value {
    value: ValueType,
    converter: PyObject,
    // multi: bool,
    // values: PyObject,
}

#[pymethods]
impl Value {
    #[new]
    #[pyo3(signature = (value, converter, unpack=true))]
    fn new(value: &Bound<'_, PyAny>, converter: PyObject, unpack: bool) -> Self {
        let multi = unpack
            && (value.is_instance_of::<PyList>()
                || value.is_instance_of::<PyTuple>()
                || value.is_instance_of::<PyFrozenSet>()
                || value.is_instance_of::<PySet>());
        if multi {
            Value {
                value: ValueType::Multi(value.clone().unbind()),
                converter,
                // multi,
            }
        } else {
            Value {
                value: ValueType::Single(value.clone().unbind()),
                converter,
                // multi,
            }
        }
    }

    #[getter("value")]
    fn get_value(&self, py: Python) -> PyObject {
        match &self.value {
            ValueType::Single(value) => value.clone_ref(py),
            ValueType::Multi(value) => value.clone_ref(py),
        }
    }

    fn __sql__(&self, ctx: PyObject, py: Python) -> PyResult<PyObject> {
        match &self.value {
            ValueType::Single(value) => ctx
                .getattr(py, "value")?
                .call1(py, (value, self.converter.clone_ref(py))),

            ValueType::Multi(value) => {
                let peewee: Bound<PyModule> = PyModule::import_bound(py, "peewee")?;
                let values: PyObject = peewee
                    .getattr("EnclosedNodeList")?
                    .call1((value,))?
                    .extract()?;
                
                ctx.getattr(py, "sql")?
                    .call1(py, (values, self.converter.clone_ref(py)))
            }
        }
    }
}
