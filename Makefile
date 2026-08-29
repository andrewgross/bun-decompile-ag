# Tasks for the Rust workspace. The TS implementation keeps its package.json
# scripts; these targets drive only the Rust side.

CARGO ?= cargo

.PHONY: all fmt fmt-check lint test deny check

all: check

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all --check

lint:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

# Prefers cargo-nextest when installed. Doctests always run via cargo test
# (nextest does not run them). --no-tests=pass is scaffolding-only: it comes
# out in the commit that adds the first tests.
test:
	@if command -v cargo-nextest >/dev/null 2>&1; then \
		$(CARGO) nextest run --workspace --all-features --no-tests=pass; \
	else \
		$(CARGO) test --workspace --all-features; \
	fi
	$(CARGO) test --workspace --all-features --doc

deny:
	$(CARGO) deny check

check: fmt-check lint test deny
