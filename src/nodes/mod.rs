//! Node dispatch: the Rust equivalent of `Context.sql(obj)` → `obj.__sql__(ctx)`.
//!
//! Each family lives in its own module so node types can be added without
//! touching the others. Anything not matched raises `Unsupported`.

use pyo3::prelude::*;

use crate::classes::{is_model, isa};
use crate::ctx::{Converter, Ctx};

pub mod basic;
pub mod function;
pub mod query;
pub mod source;
pub mod write;

pub fn render<'py>(ctx: &mut Ctx<'py>, obj: &Bound<'py, PyAny>) -> PyResult<()> {
    let c = ctx.c;
    if isa(obj, &c.node)? {
        // Most frequent first. Subclasses must precede their bases.
        if isa(obj, &c.field)? {
            if isa(obj, &c.field_alias)? {
                return source::field_alias(ctx, obj);
            }
            if isa(obj, &c.composite_key)? {
                return source::composite_key(ctx, obj);
            }
            return source::field(ctx, obj);
        }
        if isa(obj, &c.expression)? {
            return basic::expression(ctx, obj);
        }
        if isa(obj, &c.entity)? {
            return basic::entity(ctx, obj);
        }
        if isa(obj, &c.sql)? {
            return basic::sql(ctx, obj);
        }
        if isa(obj, &c.value)? {
            return basic::value(ctx, obj);
        }
        if isa(obj, &c.column)? {
            return basic::column(ctx, obj);
        }
        if isa(obj, &c.node_list)? {
            return basic::node_list(ctx, obj);
        }
        if isa(obj, &c.wrapped_node)? {
            if isa(obj, &c.alias)? {
                return basic::alias(ctx, obj);
            }
            if isa(obj, &c.ordering)? {
                return basic::ordering(ctx, obj);
            }
            if isa(obj, &c.negated)? {
                return basic::negated(ctx, obj);
            }
            if isa(obj, &c.cast)? {
                return basic::cast(ctx, obj);
            }
            if isa(obj, &c.bind_to)? {
                return basic::bind_to(ctx, obj);
            }
            if isa(obj, &c.bitwise_negated)? {
                return basic::bitwise_negated(ctx, obj);
            }
            if isa(obj, &c.value_literals)? {
                return basic::value_literals(ctx, obj);
            }
            if isa(obj, &c.qualified_names)? {
                return basic::qualified_names(ctx, obj);
            }
        }
        if isa(obj, &c.function)? {
            return function::function(ctx, obj);
        }
        if isa(obj, &c.case)? {
            return function::case(ctx, obj);
        }
        if isa(obj, &c.select)? {
            if isa(obj, &c.noop_model_select)? {
                return query::noop_model_select(ctx, obj);
            }
            return query::select(ctx, obj);
        }
        if isa(obj, &c.compound_select_query)? {
            return query::compound_select(ctx, obj);
        }
        if isa(obj, &c.insert)? {
            return write::insert(ctx, obj);
        }
        if isa(obj, &c.update)? {
            return write::update(ctx, obj);
        }
        if isa(obj, &c.delete)? {
            return write::delete(ctx, obj);
        }
        if isa(obj, &c.raw_query)? {
            return query::raw_query(ctx, obj);
        }
        if isa(obj, &c.table)? {
            return source::table(ctx, obj);
        }
        if isa(obj, &c.join)? {
            return source::join(ctx, obj);
        }
        if isa(obj, &c.model_alias)? {
            return source::model_alias(ctx, obj);
        }
        if isa(obj, &c.model)? {
            return source::model_instance(ctx, obj);
        }
        if isa(obj, &c.values_list)? {
            return source::values_list(ctx, obj);
        }
        if isa(obj, &c.cte)? {
            return source::cte(ctx, obj);
        }
        if isa(obj, &c.window)? {
            return function::window(ctx, obj);
        }
        if isa(obj, &c.window_alias)? {
            return function::window_alias(ctx, obj);
        }
        if isa(obj, &c.in_function)? {
            return function::in_function(ctx, obj);
        }
        if isa(obj, &c.for_update)? {
            return query::for_update(ctx, obj);
        }
        if isa(obj, &c.star)? {
            return basic::star(ctx, obj);
        }
        if isa(obj, &c.namespace_attribute)? {
            return basic::namespace_attribute(ctx, obj);
        }
        return Err(ctx.unsupported(&format!("node type {}", obj.get_type().name()?)));
    }
    if is_model(obj, c)? {
        // Context.sql(): a Model class renders as its table.
        let table = obj.getattr("_meta")?.getattr("table")?;
        return ctx.sql(&table);
    }
    if isa(obj, &c.context)? {
        // Context.__sql__: splice a pre-rendered peewee context.
        let parts: Vec<String> = obj.getattr("_sql")?.extract()?;
        ctx.sql.push_str(&parts.concat());
        let values: Vec<Py<PyAny>> = obj.getattr("_values")?.extract()?;
        ctx.values.extend(values);
        return Ok(());
    }
    // Plain Python value: Context.sql(Value(obj)).
    basic::value_of(ctx, obj, Converter::Default, true)
}
