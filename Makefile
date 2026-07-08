fmt:
	cargo fmt --all

check:
	cargo check --workspace --all-targets

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

purity:
	cargo test -p bt-agent --test purity

boundary:
	bash scripts/check_crate_boundaries.sh

file-size:
	bash scripts/file_size_lint.sh

openapi:
	cargo run -p bt-server --example openapi > docs/api/openapi.yaml

openapi-check:
	set -e; tmp=$$(mktemp); trap 'rm -f "$$tmp"' EXIT; cargo run -p bt-server --example openapi > "$$tmp"; diff -u docs/api/openapi.yaml "$$tmp"

acceptance:
	bash scripts/acceptance.sh

acceptance-exports:
	cargo test -p bt-server tests::session_export_returns_legacy_bundle_jsonl_html_sharegpt_and_otlp -- --exact
