from peewee import *
from .context_proxy import ContextProxy as Context

import peewee
peewee.Context = Context

peewee.Database.context_class = Context
