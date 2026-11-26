//! Node dispatch: the Rust equivalent of `Context.sql(obj)` → `obj.__sql__(ctx)`.
//!
//! Each family lives in its own module so node types can be added without
//! touching the others. Anything not matched raises `Unsupported`.

use std::cell::RefCell;
use std::collections::HashMap;

use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::PyType;

use crate::classes::{is_model, isa};
use crate::ctx::{Converter, Ctx};

pub mod basic;
pub mod function;
pub mod query;
pub mod source;
pub mod write;

/// What `Context.sql()` would do with an object, decided once per exact
/// Python type and cached. peewee never overrides `__instancecheck__`, so the
/// exact type fully determines the `isinstance` chain below.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    FieldAlias,
    StringExpression,
    CompositeKey,
    Field,
    Expression,
    Entity,
    Sql,
    Value,
    Column,
    NodeList,
    Alias,
    Ordering,
    Negated,
    Cast,
    BindTo,
    BitwiseNegated,
    ValueLiterals,
    QualifiedNames,
    Function,
    Case,
    NoopModelSelect,
    Select,
    CompoundSelect,
    Insert,
    Update,
    Delete,
    RawQuery,
    Table,
    Join,
    ModelAlias,
    ModelInstance,
    ValuesList,
    Cte,
    Window,
    WindowAlias,
    InFunction,
    ForUpdate,
    Star,
    NamespaceAttribute,
    /// `BitField.flag()`'s descriptor: a class defined inside a method, so it
    /// cannot be imported; recognised by name and shape.
    FlagDescriptor,
    UnknownNode,
    ModelClass,
    Context,
    Plain,
}

thread_local! {
    // type pointer -> kind; the Py<PyType> keeps the type object alive so the
    // pointer can never be reused by a different class.
    static KINDS: RefCell<HashMap<usize, (Kind, Py<PyType>)>> = RefCell::new(HashMap::new());
}

fn kind_of<'py>(ctx: &Ctx<'py>, obj: &Bound<'py, PyAny>) -> PyResult<Kind> {
    let tp = obj.get_type();
    let key = tp.as_ptr() as usize;
    if let Some(k) = KINDS.with(|m| m.borrow().get(&key).map(|(k, _)| *k)) {
        return Ok(k);
    }
    let k = classify(ctx, obj)?;
    KINDS.with(|m| m.borrow_mut().insert(key, (k, tp.unbind())));
    Ok(k)
}

/// The `isinstance` chain, run once per type. Subclasses precede their bases.
fn classify<'py>(ctx: &Ctx<'py>, obj: &Bound<'py, PyAny>) -> PyResult<Kind> {
    let c = ctx.c;
    if isa(obj, &c.node)? {
        let table: &[(&Py<PyAny>, Kind)] = &[
            (&c.field_alias, Kind::FieldAlias),
            (&c.composite_key, Kind::CompositeKey),
            (&c.field, Kind::Field),
            (&c.string_expression, Kind::StringExpression),
            (&c.expression, Kind::Expression),
            (&c.entity, Kind::Entity),
            (&c.sql, Kind::Sql),
            (&c.value, Kind::Value),
            (&c.column, Kind::Column),
            (&c.node_list, Kind::NodeList),
            (&c.alias, Kind::Alias),
            (&c.ordering, Kind::Ordering),
            (&c.negated, Kind::Negated),
            (&c.cast, Kind::Cast),
            (&c.bind_to, Kind::BindTo),
            (&c.bitwise_negated, Kind::BitwiseNegated),
            (&c.value_literals, Kind::ValueLiterals),
            (&c.qualified_names, Kind::QualifiedNames),
            (&c.function, Kind::Function),
            (&c.case, Kind::Case),
            (&c.noop_model_select, Kind::NoopModelSelect),
            (&c.select, Kind::Select),
            (&c.compound_select_query, Kind::CompoundSelect),
            (&c.insert, Kind::Insert),
            (&c.update, Kind::Update),
            (&c.delete, Kind::Delete),
            (&c.raw_query, Kind::RawQuery),
            (&c.table, Kind::Table),
            (&c.join, Kind::Join),
            (&c.model_alias, Kind::ModelAlias),
            (&c.model, Kind::ModelInstance),
            (&c.values_list, Kind::ValuesList),
            (&c.cte, Kind::Cte),
            (&c.window, Kind::Window),
            (&c.window_alias, Kind::WindowAlias),
            (&c.in_function, Kind::InFunction),
            (&c.for_update, Kind::ForUpdate),
            (&c.star, Kind::Star),
            (&c.namespace_attribute, Kind::NamespaceAttribute),
        ];
        for (cls, kind) in table {
            if isa(obj, cls)? {
                return Ok(*kind);
            }
        }
        if obj.get_type().name()? == "FlagDescriptor"
            && obj.hasattr(intern!(obj.py(), "_field"))?
            && obj.hasattr(intern!(obj.py(), "_value"))?
        {
            return Ok(Kind::FlagDescriptor);
        }
        return Ok(Kind::UnknownNode);
    }
    if is_model(obj, c)? {
        return Ok(Kind::ModelClass);
    }
    if isa(obj, &c.context)? {
        return Ok(Kind::Context);
    }
    Ok(Kind::Plain)
}

pub fn render<'py>(ctx: &mut Ctx<'py>, obj: &Bound<'py, PyAny>) -> PyResult<()> {
    let py = ctx.py;
    match kind_of(ctx, obj)? {
        Kind::FieldAlias => source::field_alias(ctx, obj),
        Kind::CompositeKey => source::composite_key(ctx, obj),
        Kind::Field => source::field(ctx, obj),
        Kind::StringExpression => basic::string_expression(ctx, obj),
        Kind::Expression => basic::expression(ctx, obj),
        Kind::Entity => basic::entity(ctx, obj),
        Kind::Sql => basic::sql(ctx, obj),
        Kind::Value => basic::value(ctx, obj),
        Kind::Column => basic::column(ctx, obj),
        Kind::NodeList => basic::node_list(ctx, obj),
        Kind::Alias => basic::alias(ctx, obj),
        Kind::Ordering => basic::ordering(ctx, obj),
        Kind::Negated => basic::negated(ctx, obj),
        Kind::Cast => basic::cast(ctx, obj),
        Kind::BindTo => basic::bind_to(ctx, obj),
        Kind::BitwiseNegated => basic::bitwise_negated(ctx, obj),
        Kind::ValueLiterals => basic::value_literals(ctx, obj),
        Kind::QualifiedNames => basic::qualified_names(ctx, obj),
        Kind::Function => function::function(ctx, obj),
        Kind::Case => function::case(ctx, obj),
        Kind::NoopModelSelect => query::noop_model_select(ctx, obj),
        Kind::Select => query::select(ctx, obj),
        Kind::CompoundSelect => query::compound_select(ctx, obj),
        Kind::Insert => write::insert(ctx, obj),
        Kind::Update => write::update(ctx, obj),
        Kind::Delete => write::delete(ctx, obj),
        Kind::RawQuery => query::raw_query(ctx, obj),
        Kind::Table => source::table(ctx, obj),
        Kind::Join => source::join(ctx, obj),
        Kind::ModelAlias => source::model_alias(ctx, obj),
        Kind::ModelInstance => source::model_instance(ctx, obj),
        Kind::ValuesList => source::values_list(ctx, obj),
        Kind::Cte => source::cte(ctx, obj),
        Kind::Window => function::window(ctx, obj),
        Kind::WindowAlias => function::window_alias(ctx, obj),
        Kind::InFunction => function::in_function(ctx, obj),
        Kind::ForUpdate => query::for_update(ctx, obj),
        Kind::Star => basic::star(ctx, obj),
        Kind::NamespaceAttribute => basic::namespace_attribute(ctx, obj),
        Kind::FlagDescriptor => {
            // FlagDescriptor.__sql__: ctx.sql(self._field.bin_and(self._value) != 0)
            let field = obj.getattr(intern!(py, "_field"))?;
            let value = obj.getattr(intern!(py, "_value"))?;
            let expr = field
                .call_method1(intern!(py, "bin_and"), (value,))?
                .call_method1(intern!(py, "__ne__"), (0,))?;
            ctx.sql(&expr)
        }
        Kind::UnknownNode => Err(ctx.unsupported(&format!("node type {}", obj.get_type().name()?))),
        Kind::ModelClass => {
            // Context.sql(): a Model class renders as its table.
            let table = obj
                .getattr(intern!(py, "_meta"))?
                .getattr(intern!(py, "table"))?;
            ctx.sql(&table)
        }
        Kind::Context => {
            // Context.__sql__: splice a pre-rendered peewee context.
            let parts: Vec<String> = obj.getattr(intern!(py, "_sql"))?.extract()?;
            ctx.sql.push_str(&parts.concat());
            let values: Vec<Py<PyAny>> = obj.getattr(intern!(py, "_values"))?.extract()?;
            ctx.values.extend(values);
            Ok(())
        }
        // Plain Python value: Context.sql(Value(obj)).
        Kind::Plain => basic::value_of(ctx, obj, Converter::Default, true),
    }
}
