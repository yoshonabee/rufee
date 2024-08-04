from __future__ import annotations

from contextlib import contextmanager
import peewee
from peewee import _query_val_transform, __scope_context__
from rufee import Context as RufeeContext


SCOPE_NORMAL = 1
SCOPE_SOURCE = 2
SCOPE_VALUES = 4
SCOPE_CTE = 8
SCOPE_COLUMN = 16

_PeeweeContext = peewee.Context


class ContextProxy:
    __rust_readable_attr_names__: str[str] = {
        "_values",
        "_sql",
        "state",
        "scope",
        "parentheses",
        "subquery",
    }

    __rust_writable_attr_names__: str[str] = {
        "state",
    }

    __rust_method_names__: str[str] = {
        "query",
        "sql",
        "__sql__",
    }

    def __init__(self, **settings) -> None:
        self._py_context = _PeeweeContext()
        state = peewee.State(**settings)
        self._rust_context = RufeeContext(state, [], self)
        self.stack = []

    def __getattr__(self, attr_name):
        if (
            attr_name in self.__rust_readable_attr_names__
            or attr_name in self.__rust_method_names__
        ):
            return getattr(self._rust_context, attr_name)

        return getattr(self._py_context, attr_name)

    def __setattr__(self, attr_name, value):
        if attr_name.startswith("_"):
            return super().__setattr__(attr_name, value)
        elif attr_name in self.__rust_writable_attr_names__:
            return setattr(self._rust_context, attr_name, value)
        elif (
            attr_name in self.__rust_readable_attr_names__
            or attr_name in self.__rust_method_names__
        ):
            raise AttributeError(f"can't set attribute {attr_name}")

        return setattr(self._py_context, attr_name, value)

    def as_new(self) -> ContextProxy:
        return ContextProxy(**self.state.settings)

    @contextmanager
    def __call__(self, **overrides):
        if overrides and overrides.get("scope") == self.scope:
            del overrides["scope"]

        self.stack.append(self.state)
        self.state = self.state(**overrides)

        if self.parentheses:
            self.literal("(")

        try:
            yield self
        finally:
            if self.parentheses:
                self.literal(")")
            self.state = self.stack.pop()

        return self

    # TODO: Implement the following context managers
    scope_normal = __scope_context__(SCOPE_NORMAL)
    scope_source = __scope_context__(SCOPE_SOURCE)
    scope_values = __scope_context__(SCOPE_VALUES)
    scope_cte = __scope_context__(SCOPE_CTE)
    scope_column = __scope_context__(SCOPE_COLUMN)

    @contextmanager
    def push_alias(self):
        with self._py_context.push_alias():
            yield

    def value(self, value, converter=None, add_param=True):
        if converter:
            value = converter(value)
        elif converter is None and self.state.converter:
            # Explicitly check for None so that "False" can be used to signify
            # that no conversion should be applied.
            value = self.state.converter(value)

        if isinstance(value, peewee.Node):
            with self(converter=None):
                return self.sql(value)
        elif peewee.is_model(value):
            # Under certain circumstances, we could end-up treating a model-
            # class itself as a value. This check ensures that we drop the
            # table alias into the query instead of trying to parameterize a
            # model (for instance, passing a model as a function argument).
            with self.scope_column():
                return self.sql(value)

        if self.state.value_literals:
            return self.literal(_query_val_transform(value))

        self._values.append(value)
        return self.literal(self.state.param or "?") if add_param else self

    def parse(self, node):
        return self.sql(node).query()

    def sql(self, obj):
        if isinstance(obj, peewee.Node):
            obj.__sql__(self)
        elif isinstance(obj, Context):
            obj.__sql__(self._rust_context)
        elif peewee.is_model(obj):
            obj._meta.table.__sql__(self)
        else:
            peewee.Value(obj).__sql__(self)

        return self

    def literal(self, value):
        self._rust_context.literal(value)
        return self


Context = ContextProxy
