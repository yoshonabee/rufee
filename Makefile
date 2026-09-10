PY ?= .venv/bin/python
PEEWEE_TAG ?= 3.17.9

.PHONY: venv dev test cargo-test lint peewee-src peewee-tests mysql-up mysql-down clean

venv:            ## create .venv (Python 3.11) with dev deps; peewee without C extensions
	uv venv -p 3.11 .venv
	NO_SQLITE=1 uv pip install -p $(PY) --no-binary peewee -e ".[dev]"
	rm -f .venv/lib/python3.11/site-packages/playhouse/_sqlite_*.so

dev:             ## build the extension into .venv (release)
	VIRTUAL_ENV=$(CURDIR)/.venv .venv/bin/maturin develop --release

test: cargo-test ## rust + python tests
	$(PY) -m pytest -q

cargo-test:      ## needs a Python to link against; PYO3_PYTHON points at the venv interpreter
	PYO3_PYTHON=$(CURDIR)/$(PY) cargo test --quiet

lint:
	cargo fmt --check
	cargo clippy --quiet -- -D warnings

peewee-src:      ## check out peewee's test-suite at PEEWEE_TAG into peewee-src/
	@test -d peewee-src/.git || git clone -q --no-checkout https://github.com/coleifer/peewee.git peewee-src
	@git -C peewee-src fetch -q --depth 1 origin tag $(PEEWEE_TAG)
	@git -C peewee-src checkout -q $(PEEWEE_TAG)

peewee-tests: peewee-src ## run peewee's own suite through rufee (ARGS: -e sqlite|mysql, RUFEE_FALLBACK=0|1)
	$(PY) scripts/run_peewee_tests.py $(ARGS)

mysql-up:
	docker compose up -d --wait mysql

mysql-down:
	docker compose down -v

clean:
	rm -rf target dist build .pytest_cache
