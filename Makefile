.PHONY: all build check test lint fmt run validate

all: check test

build:
	cargo build --workspace

check:
	cargo check --workspace --all-targets --all-features

test:
	cargo test --workspace --all-features

lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

fmt:
	cargo fmt --all --check

run:
	cargo run -p agentmesh -- serve --config config/agentmesh.yaml

validate:
	cargo run -p agentmesh -- validate config/agentmesh.yaml

