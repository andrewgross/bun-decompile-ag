# Tasks for the Rust workspace.

CARGO ?= cargo

.PHONY: all fmt fmt-check lint test deny check e2e dummy fixtures

all: check

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all --check

lint:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

# Prefers cargo-nextest when installed. Doctests always run via cargo test
# (nextest does not run them).
test:
	@if command -v cargo-nextest >/dev/null 2>&1; then \
		$(CARGO) nextest run --workspace --all-features; \
	else \
		$(CARGO) test --workspace --all-features; \
	fi
	$(CARGO) test --workspace --all-features --doc

deny:
	$(CARGO) deny check

# End-to-end against real Bun releases (downloads and caches them under
# ~/.cache/bun-decompile), driving the release CLI.
e2e:
	$(CARGO) build --release -p bun-decompile-ag-cli
	DECOMPILE_CMD="$(CURDIR)/target/release/bun-decompile" bash scripts/e2e-test.sh

# Compile the test dummy with the Bun on PATH. The real-binary unit tests
# soft-skip without it.
dummy:
	bun build --compile --sourcemap=inline fixtures/dummy/index.ts --outfile fixtures/dummy/dummy

# Regenerate the committed fixtures from the milestone Bun releases.
fixtures:
	$(CARGO) build --release -p bun-decompile-ag-cli
	bash scripts/build-fixtures.sh

check: fmt-check lint test deny
