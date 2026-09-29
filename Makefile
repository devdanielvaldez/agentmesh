.PHONY: all build check test lint fmt docs ci run validate schema doctor

all: check test

build:
	cargo build --workspace --locked

check:
	cargo check --workspace --all-targets --all-features --locked

test:
	cargo test --workspace --all-features --locked

lint:
	cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

fmt:
	cargo fmt --all -- --check

docs:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked

ci: fmt lint test docs

run:
	cargo run -p agentmesh -- serve --config config/agentmesh.yaml

validate:
	cargo run -p agentmesh -- validate config/agentmesh.yaml

schema:
	cargo run -q -p agentmesh -- schema

doctor:
	cargo run -p agentmesh -- doctor --config config/agentmesh.yaml
