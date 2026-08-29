# Tasks for the Rust workspace. The TS implementation keeps its package.json
# scripts; these targets drive only the Rust side.

CARGO ?= cargo

.PHONY: all fmt fmt-check lint test deny check parity e2e

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

# Migration acceptance gate: diff the TS and Rust CLIs over fixtures, a fresh
# dummy, and the error cases. Pass --full to add the cross-target matrix
# (downloads Bun releases). Builds the release binary if needed.
parity:
	$(CARGO) build --release -p bun-decompile-ag-cli
	bash src/scripts/parity-test.sh

# End-to-end against real Bun releases, driving the Rust CLI.
e2e:
	$(CARGO) build --release -p bun-decompile-ag-cli
	DECOMPILE_CMD="$(CURDIR)/target/release/bun-decompile" bash src/scripts/e2e-test.sh

check: fmt-check lint test deny
